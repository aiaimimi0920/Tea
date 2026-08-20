use reqwest::Method;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::sync::OnceLock;
use tea_http_read::error_body_preview;

const DEFAULT_TEA_SERVER_URL: &str = "http://127.0.0.1:48910";
const MAX_TEA_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeaRuntimeConfig {
    pub server_url: String,
    pub auth_configured: bool,
}

#[tauri::command]
fn resolve_tea_runtime_config() -> TeaRuntimeConfig {
    let server_url = configured_server_url();
    TeaRuntimeConfig {
        auth_configured: configured_auth_token_for(&server_url).is_some(),
        server_url,
    }
}

#[tauri::command]
async fn tea_request(
    method: String,
    path: String,
    body: Option<Value>,
    base_url: Option<String>,
    auth_token: Option<String>,
    idempotency_key: Option<String>,
    timeout_ms: Option<u64>,
) -> Result<Value, String> {
    let method = Method::from_bytes(method.as_bytes())
        .map_err(|error| format!("invalid HTTP method '{method}': {error}"))?;
    let configured_base_url = configured_server_url();
    let base_url = normalize_base_url(
        base_url
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| configured_base_url.clone()),
    );
    validate_tea_base_url(&base_url)?;
    let url = join_url(&base_url, &path)?;
    let client = shared_tea_http_client()?;
    let mut request = client.request(method, url);
    if let Some(timeout_ms) = timeout_ms {
        request = request.timeout(std::time::Duration::from_millis(timeout_ms));
    }

    let token = request_auth_token(auth_token, &base_url, &configured_base_url);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    } else if path.trim_start_matches('/').starts_with("v1/") {
        return Err("remote Tea API requests require an explicit auth token".to_string());
    }
    if let Some(key) = idempotency_key {
        request = request.header("Idempotency-Key", key);
    }

    if let Some(value) = body {
        if !value.is_null() {
            request = request.json(&value);
        }
    }

    let response = request
        .send()
        .await
        .map_err(|error| format!("Tea request failed: {error}"))?;
    let (status, text) = read_response_text_limited(response, MAX_TEA_RESPONSE_BYTES).await?;

    if !status.is_success() {
        return Err(format!(
            "Tea returned HTTP {status}: {}",
            error_body_preview(&text)
        ));
    }

    if text.trim().is_empty() {
        return Ok(Value::Null);
    }

    serde_json::from_str::<Value>(&text).or(Ok(Value::String(text)))
}

/// Process-wide HTTP client so every `tea_request` reuses one connection pool
/// instead of building a fresh `reqwest::Client` (and pool) per invocation.
fn shared_tea_http_client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let client = build_tea_http_client()?;
    Ok(CLIENT.get_or_init(|| client))
}

fn build_tea_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("failed to build Tea HTTP client: {error}"))
}

/// Bounded response read via the shared `tea_http_read` helper, mapped to this
/// binary's `String` error boundary. Context `"Tea"` keeps the existing
/// `"Tea response body exceeded the {n}-byte limit"` contract.
async fn read_response_text_limited(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<(reqwest::StatusCode, String), String> {
    tea_http_read::read_response_text_limited(response, max_bytes, "Tea")
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn save_tea_export(file_name: String, content: String) -> Result<String, String> {
    let safe_name = sanitize_export_file_name(&file_name);
    if safe_name.is_empty() {
        return Err("export file name is empty".to_string());
    }
    let dir = downloads_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("failed to create export directory: {error}"))?;
    let path = write_unique_export(&dir, &safe_name, content.as_bytes())?;
    Ok(path.to_string_lossy().to_string())
}

fn write_unique_export(
    directory: &std::path::Path,
    file_name: &str,
    content: &[u8],
) -> Result<std::path::PathBuf, String> {
    for index in 0..10_000 {
        let candidate_name = if index == 0 {
            file_name.to_string()
        } else {
            suffixed_file_name(file_name, index)
        };
        let path = directory.join(candidate_name);
        let mut file = match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("failed to create export file: {error}")),
        };
        if let Err(error) = file.write_all(content).and_then(|_| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(format!("failed to write export file: {error}"));
        }
        return Ok(path);
    }
    Err("failed to allocate a unique export file name".to_string())
}

fn suffixed_file_name(file_name: &str, index: usize) -> String {
    let path = std::path::Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("tea-export");
    match path.extension().and_then(|value| value.to_str()) {
        Some(extension) if !extension.is_empty() => format!("{stem}-{index}.{extension}"),
        _ => format!("{stem}-{index}"),
    }
}

pub fn run() {
    // Make tea.exe fully self-contained: start our own tea-daemon.exe if one is
    // not already answering, so a user only ever needs to launch a single exe.
    // Failures here are non-fatal; the UI still renders and surfaces the
    // connection state, and the launcher .bat path continues to work.
    // Run the probe/spawn off the window-creation path: a slow daemon (mutex
    // wait + health retries) must not delay first paint. The frontend already
    // shows a connecting state and retries with backoff.
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            resolve_tea_runtime_config,
            tea_request,
            save_tea_export
        ])
        .setup(|_app| {
            std::thread::spawn(|| {
                if let Err(error) = ensure_daemon_running() {
                    eprintln!("Tea: could not auto-start daemon: {error}");
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Tea desktop");
}

/// Ensure a Tea daemon is reachable, spawning a bundled `tea-daemon.exe` if not.
///
/// Single-exe UX: launching `tea.exe` alone must bring up the whole stack. We
/// resolve (or create) a shared auth token, check whether a daemon already
/// answers authenticated `/v1/status`, and if not spawn the sibling `tea-daemon.exe` with the
/// same token + a writable data directory, then wait for it to become healthy.
/// The token is written to `auth-token.txt` in that data directory so `tea_request`'s
/// `configured_auth_token()` resolves the identical value.
fn ensure_daemon_running() -> Result<(), String> {
    let base_url = configured_server_url();
    if !is_loopback_base_url(&base_url) {
        return Ok(());
    }
    let data_dir = package_data_dir();
    std::fs::create_dir_all(&data_dir)
        .map_err(|error| format!("failed to create data directory: {error}"))?;
    let _startup_mutex =
        DaemonStartupMutex::acquire(&data_dir, std::time::Duration::from_secs(15))?;
    let token = resolve_or_create_token(&data_dir)?;

    // A health response alone is not enough: another Tea instance or unrelated
    // process may own this port. Verify the authenticated API before reusing it.
    if daemon_health_ok(&base_url) {
        return daemon_status_ok(&base_url, &token);
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .ok_or_else(|| "cannot locate the tea.exe directory".to_string())?;

    let daemon_path = exe_dir.join("tea-daemon.exe");
    validate_daemon_executable_path(&daemon_path)?;

    let store_path = configured_runtime_path("TEA_STORE_PATH", data_dir.join("tea.sqlite"));
    let config_path = configured_runtime_path("TEA_CONFIG_PATH", data_dir.join("config.json"));
    let (server_host, server_port) = host_port_from_base_url(&base_url)
        .ok_or_else(|| format!("invalid local Tea server URL: {base_url}"))?;
    let bind_addr = std::env::var("TEA_BIND_ADDR")
        .unwrap_or_else(|_| format_host_port(&server_host, server_port));

    let mut command = std::process::Command::new(&daemon_path);
    command
        .arg("--bind-addr")
        .arg(&bind_addr)
        .arg("--store-path")
        .arg(&store_path)
        .arg("--config-path")
        .arg(&config_path)
        .env("TEA_AUTH_TOKEN", &token)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    // On Windows, avoid flashing a console window for the background daemon.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn tea-daemon.exe: {error}"))?;
    let pid_path = data_dir.join("tea-daemon.pid");
    if let Err(error) = std::fs::write(&pid_path, child.id().to_string()) {
        stop_spawned_daemon(&mut child, &pid_path);
        return Err(format!("failed to write daemon PID file: {error}"));
    }

    // Wait up to ~10s for the daemon's authenticated API, and do not leave a
    // failed or late-starting child behind when startup cannot complete.
    for _ in 0..40 {
        let child_status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                stop_spawned_daemon(&mut child, &pid_path);
                return Err(format!("failed to inspect tea-daemon.exe: {error}"));
            }
        };
        if let Some(status) = child_status {
            let _ = remove_pid_file_if_matches(&pid_path, child.id());
            return Err(format!(
                "tea-daemon.exe exited during startup with {status}"
            ));
        }
        if daemon_health_ok(&base_url) {
            match daemon_status_ok(&base_url, &token) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    stop_spawned_daemon(&mut child, &pid_path);
                    return Err(error);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    stop_spawned_daemon(&mut child, &pid_path);
    Err("tea-daemon.exe did not become healthy in time".to_string())
}

fn daemon_status_ok(base_url: &str, token: &str) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_millis(800))
        .timeout(std::time::Duration::from_millis(800))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("failed to build daemon status probe: {error}"))?;
    let response = client
        .get(format!("{}/v1/status", base_url.trim_end_matches('/')))
        .bearer_auth(token)
        .send()
        .map_err(|error| {
            format!("a process answers Tea's port but status probing failed: {error}")
        })?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "a process answers Tea's port but rejected the configured daemon identity (HTTP {})",
            response.status()
        ))
    }
}

fn stop_spawned_daemon(child: &mut std::process::Child, pid_path: &std::path::Path) {
    let child_id = child.id();
    let _ = child.kill();
    let _ = child.wait();
    let _ = remove_pid_file_if_matches(pid_path, child_id);
}

fn configured_runtime_path(name: &str, fallback: std::path::PathBuf) -> std::path::PathBuf {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or(fallback)
}

fn daemon_mutex_name(data_dir: &std::path::Path) -> Result<String, String> {
    let absolute = std::path::absolute(data_dir)
        .map_err(|error| format!("failed to resolve Tea data directory: {error}"))?;
    let normalized = absolute
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_uppercase();
    let digest = Sha256::digest(normalized.as_bytes());
    let suffix = digest
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    Ok(format!("Local\\Neuro.Tea.Daemon.{suffix}"))
}

#[cfg(windows)]
struct DaemonStartupMutex {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl DaemonStartupMutex {
    fn acquire(data_dir: &std::path::Path, timeout: std::time::Duration) -> Result<Self, String> {
        use windows_sys::Win32::Foundation::{
            CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
        };
        use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};

        let name = daemon_mutex_name(data_dir)?;
        let wide_name = name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, wide_name.as_ptr()) };
        if handle.is_null() {
            return Err(format!(
                "failed to create Tea daemon startup mutex: {}",
                std::io::Error::last_os_error()
            ));
        }

        let timeout_ms = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
        let wait_result = unsafe { WaitForSingleObject(handle, timeout_ms) };
        match wait_result {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Self { handle }),
            WAIT_TIMEOUT => {
                unsafe {
                    CloseHandle(handle);
                }
                Err(format!(
                    "timed out waiting for another Tea launcher after {} seconds",
                    timeout.as_secs()
                ))
            }
            _ => {
                let error = std::io::Error::last_os_error();
                unsafe {
                    CloseHandle(handle);
                }
                Err(format!(
                    "failed to wait for Tea daemon startup mutex: {error}"
                ))
            }
        }
    }
}

#[cfg(windows)]
impl Drop for DaemonStartupMutex {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::ReleaseMutex;

        unsafe {
            ReleaseMutex(self.handle);
            CloseHandle(self.handle);
        }
    }
}

#[cfg(not(windows))]
type DaemonStartupMutex = tea_startup_lock::StartupMutex;

#[cfg(test)]
use tea_startup_lock::StartupMutex as FileStartupMutex;

fn remove_pid_file_if_matches(pid_path: &std::path::Path, expected_pid: u32) -> bool {
    let Ok(contents) = std::fs::read_to_string(pid_path) else {
        return false;
    };
    if contents.trim() != expected_pid.to_string() {
        return false;
    }
    std::fs::remove_file(pid_path).is_ok()
}

/// Blocking `/health` probe using only std (no reqwest `blocking` feature, which
/// the build environment does not enable). Opens a short-timeout TCP connection
/// to the daemon's host:port and issues a minimal HTTP/1.0 GET /health, treating
/// a `2xx` status line as healthy. Health needs no auth.
fn daemon_health_ok(base_url: &str) -> bool {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let (host, port) = match host_port_from_base_url(base_url) {
        Some(pair) => pair,
        None => return false,
    };

    let addr = format_host_port(&host, port);
    let socket_addr = match addr.to_socket_addrs_first() {
        Some(addr) => addr,
        None => return false,
    };

    let mut stream =
        match TcpStream::connect_timeout(&socket_addr, std::time::Duration::from_millis(800)) {
            Ok(stream) => stream,
            Err(_) => return false,
        };
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(800)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_millis(800)));

    let request = format!("GET /health HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }

    let mut buf = Vec::with_capacity(256);
    let mut chunk = [0u8; 256];
    // Read just enough to capture the status line.
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() >= 64 || buf.windows(2).any(|w| w == b"\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let head = String::from_utf8_lossy(&buf);
    let status_line = head.lines().next().unwrap_or("");
    // e.g. "HTTP/1.1 200 OK"
    status_line.contains(" 200") || status_line.contains(" 204")
}

/// Extract `(host, port)` from a base URL like `http://127.0.0.1:48910`.
fn host_port_from_base_url(base_url: &str) -> Option<(String, u16)> {
    let parsed = reqwest::Url::parse(base_url.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed
        .host_str()?
        .trim_start_matches('[')
        .trim_end_matches(']');
    Some((host.to_string(), parsed.port_or_known_default()?))
}

fn format_host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn is_loopback_base_url(base_url: &str) -> bool {
    let Some((host, _)) = host_port_from_base_url(base_url) else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn validate_tea_base_url(base_url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(base_url)
        .map_err(|error| format!("invalid Tea server URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Tea server URL must use http or https".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("Tea server URL must include a host".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Tea server URL must not include credentials".to_string());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("Tea server URL must not include a query or fragment".to_string());
    }
    if parsed.scheme() == "http" && !is_loopback_base_url(base_url) {
        return Err("non-loopback Tea server URLs must use https".to_string());
    }
    Ok(())
}

fn request_auth_token(
    provided: Option<String>,
    base_url: &str,
    configured_base_url: &str,
) -> Option<String> {
    provided
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            (base_url == configured_base_url)
                .then(|| configured_auth_token_for(base_url))
                .flatten()
        })
}

/// Minimal helper: resolve the first socket address for a `host:port` string.
trait ToSocketAddrsFirst {
    fn to_socket_addrs_first(&self) -> Option<std::net::SocketAddr>;
}

impl ToSocketAddrsFirst for str {
    fn to_socket_addrs_first(&self) -> Option<std::net::SocketAddr> {
        use std::net::ToSocketAddrs;
        self.to_socket_addrs().ok().and_then(|mut it| it.next())
    }
}

/// Resolve the shared auth token, creating and persisting one when absent so the
/// daemon we spawn and this process agree. Order: env var, then token file, then
/// a freshly generated token written to `data/auth-token.txt`.
fn resolve_or_create_token(data_dir: &std::path::Path) -> Result<String, String> {
    if let Some(token) = configured_auth_token() {
        return Ok(token);
    }
    let token_file = data_dir.join("auth-token.txt");
    resolve_or_create_token_file(&token_file)
}

fn resolve_or_create_token_file(token_file: &std::path::Path) -> Result<String, String> {
    for _ in 0..50 {
        ensure_safe_token_path(token_file)?;
        match std::fs::read_to_string(token_file) {
            Ok(contents) => {
                if let Some(existing) = parse_token_file_contents(&contents) {
                    return Ok(existing);
                }
                if !contents.trim().is_empty() {
                    return Err(format!(
                        "Tea auth token file has an invalid format: {}",
                        token_file.display()
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "failed to read auth token file {}: {error}",
                    token_file.display()
                ));
            }
        }

        let token = generate_token();
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(token_file)
        {
            Ok(mut file) => {
                file.write_all(token.as_bytes())
                    .and_then(|_| file.sync_all())
                    .map_err(|error| format!("failed to write auth token file: {error}"))?;
                return Ok(token);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(error) => return Err(format!("failed to create auth token file: {error}")),
        }
    }
    Err(format!(
        "Tea auth token file stayed empty or unreadable: {}",
        token_file.display()
    ))
}

fn parse_token_file_contents(contents: &str) -> Option<String> {
    let token = contents.trim();
    (token.len() == 32 && token.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| token.to_string())
}

fn ensure_safe_token_path(token_file: &std::path::Path) -> Result<(), String> {
    if let Some(parent) = token_file.parent() {
        validate_token_path_component(parent, true, "directory")?;
    }
    validate_token_path_component(token_file, false, "file")
}

fn validate_token_path_component(
    path: &std::path::Path,
    expect_directory: bool,
    description: &str,
) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "failed to inspect Tea auth token {description} {}: {error}",
                path.display()
            ));
        }
    };
    if metadata_is_reparse_or_symlink(&metadata) {
        return Err(format!(
            "Tea auth token {description} must not be a symlink or reparse point: {}",
            path.display()
        ));
    }
    let expected_type_matches = if expect_directory {
        metadata.is_dir()
    } else {
        metadata.is_file()
    };
    if !expected_type_matches {
        return Err(format!(
            "Tea auth token {description} has the wrong filesystem type: {}",
            path.display()
        ));
    }
    Ok(())
}

fn validate_daemon_executable_path(path: &std::path::Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!(
                "tea-daemon.exe not found next to tea.exe ({})",
                path.display()
            )
        } else {
            format!(
                "failed to inspect tea-daemon.exe {}: {error}",
                path.display()
            )
        }
    })?;
    if metadata_is_reparse_or_symlink(&metadata) {
        return Err(format!(
            "tea-daemon.exe must not be a symlink or reparse point: {}",
            path.display()
        ));
    }
    if !metadata.is_file() {
        return Err(format!(
            "tea-daemon.exe must be a regular file: {}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn metadata_is_reparse_or_symlink(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse_or_symlink(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

/// Writable data directory: `TEA_DATA_DIR` if set (portable launcher path), then
/// the current user's local application data, with executable-local data only as
/// a final compatibility fallback.
fn package_data_dir() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("TEA_DATA_DIR") {
        return std::path::PathBuf::from(dir);
    }
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        return std::path::PathBuf::from(dir).join("Neuro").join("tea");
    }
    if let Some(dir) = std::env::var_os("APPDATA") {
        return std::path::PathBuf::from(dir).join("Neuro").join("tea");
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("data")))
        .unwrap_or_else(|| std::env::temp_dir().join("tea-data"))
}

/// Generate an unpredictable local bearer token for the loopback daemon.
fn generate_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn sanitize_export_file_name(value: &str) -> String {
    let base = value.rsplit(['/', '\\']).next().unwrap_or("").trim();
    base.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' '))
        .collect::<String>()
        .trim()
        .to_string()
}

fn downloads_dir() -> std::path::PathBuf {
    #[cfg(target_os = "windows")]
    let home = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from);
    #[cfg(not(target_os = "windows"))]
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);

    home.map(|base| base.join("Downloads"))
        .unwrap_or_else(std::env::temp_dir)
}

fn configured_server_url() -> String {
    std::env::var("TEA_SERVER_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("TEA_DAEMON_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .map(normalize_base_url)
        .unwrap_or_else(|| DEFAULT_TEA_SERVER_URL.to_string())
}

fn explicit_auth_token() -> Option<String> {
    // 1. Explicit env var (set by start-tea.bat before launching tea.exe).
    std::env::var("TEA_AUTH_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn configured_auth_token() -> Option<String> {
    if let Some(token) = explicit_auth_token() {
        return Some(token);
    }
    // 2. Shared token file written by start-tea.bat. Windows `start` does not
    //    always propagate the parent shell's environment to tea.exe, so the env
    //    var can be missing even though the daemon was launched with a random
    //    token. Reading the same file the launcher wrote keeps the desktop and
    //    daemon tokens in agreement regardless of env propagation.
    read_token_file()
}

fn configured_auth_token_for(base_url: &str) -> Option<String> {
    explicit_auth_token().or_else(|| {
        is_loopback_base_url(base_url)
            .then(read_token_file)
            .flatten()
    })
}

// Locate and read the shared `auth-token.txt`. The launcher sets TEA_DATA_DIR
// to `<package>\data`; direct launches use the writable per-user data directory.
// Executable-local data remains a compatibility fallback for older packages.
fn read_token_file() -> Option<String> {
    let mut candidates = vec![package_data_dir().join("auth-token.txt")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join("data").join("auth-token.txt"));
        }
    }
    for path in candidates {
        if ensure_safe_token_path(&path).is_err() {
            return None;
        }
        match std::fs::read_to_string(&path) {
            Ok(contents) => return parse_token_file_contents(&contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        }
    }
    None
}

fn normalize_base_url(value: String) -> String {
    value.trim().trim_end_matches('/').to_string()
}

fn join_url(base_url: &str, path: &str) -> Result<String, String> {
    if path.starts_with("http://") || path.starts_with("https://") {
        return Err("absolute Tea request URLs are not allowed".to_string());
    }

    let normalized_path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    Ok(format!("{base_url}{normalized_path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_raw_http_server(response: String) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};

            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
        });
        (format!("http://{address}"), server)
    }

    fn spawn_capturing_http_server(
        response: String,
    ) -> (
        String,
        std::sync::mpsc::Receiver<String>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};

            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).unwrap();
            sender
                .send(String::from_utf8_lossy(&request[..read]).to_string())
                .unwrap();
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
        });
        (format!("http://{address}"), receiver, server)
    }

    #[test]
    fn sanitize_strips_path_traversal_and_separators() {
        assert_eq!(
            sanitize_export_file_name("../../etc/passwd"),
            "passwd".to_string()
        );
        assert_eq!(
            sanitize_export_file_name("C:\\Windows\\System32\\tea.json"),
            "tea.json".to_string()
        );
        assert_eq!(
            sanitize_export_file_name("tea-abc123-20260710-120000.md"),
            "tea-abc123-20260710-120000.md".to_string()
        );
    }

    #[test]
    fn sanitize_drops_unsafe_characters() {
        assert_eq!(
            sanitize_export_file_name("re;port\"<>|.json"),
            "report.json".to_string()
        );
    }

    #[test]
    fn sanitize_rejects_empty_after_cleaning() {
        assert_eq!(sanitize_export_file_name("///"), "".to_string());
        assert_eq!(sanitize_export_file_name("  "), "".to_string());
    }

    #[test]
    fn server_url_parsing_handles_defaults_and_ipv6() {
        assert_eq!(
            host_port_from_base_url("http://127.0.0.1:48910"),
            Some(("127.0.0.1".to_string(), 48910))
        );
        assert_eq!(
            host_port_from_base_url("https://tea.example.test/api"),
            Some(("tea.example.test".to_string(), 443))
        );
        assert_eq!(
            host_port_from_base_url("http://[::1]:48911"),
            Some(("::1".to_string(), 48911))
        );
    }

    #[test]
    fn daemon_auto_start_is_limited_to_loopback_servers() {
        assert!(is_loopback_base_url("http://localhost:48910"));
        assert!(is_loopback_base_url("http://127.0.0.2:48910"));
        assert!(is_loopback_base_url("http://[::1]:48910"));
        assert!(!is_loopback_base_url("https://tea.example.test"));
    }

    #[test]
    fn tea_server_url_validation_requires_confidential_remote_transport() {
        for accepted in [
            "http://127.0.0.1:48910",
            "http://localhost:48910/api",
            "http://[::1]:48910",
            "https://tea.example.test/api",
        ] {
            assert!(validate_tea_base_url(accepted).is_ok(), "{accepted}");
        }

        for rejected in [
            "http://tea.example.test",
            "ftp://127.0.0.1:48910",
            "https://user:secret@tea.example.test",
            "https://tea.example.test?token=secret",
            "https://tea.example.test/#fragment",
        ] {
            assert!(validate_tea_base_url(rejected).is_err(), "{rejected}");
        }
    }

    #[test]
    fn remote_endpoint_override_does_not_inherit_local_auth() {
        assert_eq!(
            request_auth_token(None, "https://tea.example.test", "http://127.0.0.1:48910"),
            None
        );
        assert_eq!(
            request_auth_token(
                Some(" remote-token ".to_string()),
                "https://tea.example.test",
                "http://127.0.0.1:48910"
            ),
            Some("remote-token".to_string())
        );
        assert_eq!(
            request_auth_token(None, "http://127.0.0.1:48911", "http://127.0.0.1:48910"),
            None
        );
    }

    #[test]
    fn response_reader_rejects_oversized_content_length() {
        let response = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Length: 9\r\n",
            "Connection: close\r\n\r\n",
            "123456789"
        )
        .to_string();
        let (url, server) = spawn_raw_http_server(response);

        let error = tauri::async_runtime::block_on(async {
            let response = reqwest::Client::new().get(url).send().await.unwrap();
            read_response_text_limited(response, 8).await.unwrap_err()
        });

        server.join().unwrap();
        assert!(error.contains("8-byte limit"));
    }

    #[test]
    fn response_reader_rejects_oversized_chunked_body() {
        let response = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Transfer-Encoding: chunked\r\n",
            "Connection: close\r\n\r\n",
            "5\r\n12345\r\n",
            "5\r\n67890\r\n",
            "0\r\n\r\n"
        )
        .to_string();
        let (url, server) = spawn_raw_http_server(response);

        let error = tauri::async_runtime::block_on(async {
            let response = reqwest::Client::new().get(url).send().await.unwrap();
            read_response_text_limited(response, 8).await.unwrap_err()
        });

        server.join().unwrap();
        assert!(error.contains("8-byte limit"));
    }

    #[test]
    fn desktop_http_client_does_not_follow_redirects() {
        let response = concat!(
            "HTTP/1.1 307 Temporary Redirect\r\n",
            "Location: http://127.0.0.1:1/redirected\r\n",
            "Content-Length: 0\r\n",
            "Connection: close\r\n\r\n"
        )
        .to_string();
        let (url, server) = spawn_raw_http_server(response);

        let status = tauri::async_runtime::block_on(async {
            build_tea_http_client()
                .unwrap()
                .get(url)
                .send()
                .await
                .unwrap()
                .status()
        });

        server.join().unwrap();
        assert_eq!(status, reqwest::StatusCode::TEMPORARY_REDIRECT);
    }

    #[test]
    fn desktop_tea_request_sends_idempotency_key_header() {
        let response =
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_string();
        let (url, request, server) = spawn_capturing_http_server(response);

        tauri::async_runtime::block_on(tea_request(
            "POST".to_string(),
            "/v1/tickets".to_string(),
            Some(serde_json::json!({
                "title": "Desktop header test",
                "description": "Verify desktop idempotency header propagation."
            })),
            Some(url),
            Some("test-token".to_string()),
            Some("desktop-idempotency-1".to_string()),
            None,
        ))
        .unwrap();

        server.join().unwrap();
        let request = request.recv().unwrap().to_ascii_lowercase();
        assert!(request.contains("idempotency-key: desktop-idempotency-1\r\n"));
        assert!(request.contains("authorization: bearer test-token\r\n"));
    }

    #[cfg(windows)]
    #[test]
    fn daemon_mutex_name_matches_the_launcher_contract() {
        assert_eq!(
            daemon_mutex_name(std::path::Path::new("C:\\Tea\\data")).unwrap(),
            "Local\\Neuro.Tea.Daemon.A17D1CBC9082BFD453BA126427C5F78A4AB2241E507D7020C23BF3A0A022746F"
        );
    }

    #[cfg(windows)]
    #[test]
    fn daemon_startup_mutex_serializes_other_threads() {
        let data_dir = std::env::temp_dir().join(format!("tea-mutex-test-{}", generate_token()));
        let guard =
            DaemonStartupMutex::acquire(&data_dir, std::time::Duration::from_secs(1)).unwrap();
        let contender_path = data_dir.clone();
        let contender = std::thread::spawn(move || {
            DaemonStartupMutex::acquire(&contender_path, std::time::Duration::from_millis(100))
                .is_err()
        });
        assert!(contender.join().unwrap());
        drop(guard);
        assert!(DaemonStartupMutex::acquire(&data_dir, std::time::Duration::from_secs(1)).is_ok());
    }

    #[test]
    fn file_startup_mutex_serializes_competing_guards() {
        let data_dir = std::env::temp_dir().join(format!(
            "tea-file-mutex-test-{}-{}",
            std::process::id(),
            generate_token()
        ));
        let guard =
            FileStartupMutex::acquire(&data_dir, std::time::Duration::from_secs(1)).unwrap();
        assert!(data_dir.join("tea-daemon-startup.lock").is_file());

        let contender_path = data_dir.clone();
        let contender = std::thread::spawn(move || {
            FileStartupMutex::acquire(&contender_path, std::time::Duration::from_millis(100))
                .is_err()
        });
        assert!(contender.join().unwrap());

        drop(guard);
        assert!(FileStartupMutex::acquire(&data_dir, std::time::Duration::from_secs(1)).is_ok());
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[test]
    fn export_writes_unique_files_without_overwriting() {
        let directory = std::env::temp_dir().join(format!(
            "tea-export-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();

        let first = write_unique_export(&directory, "tea-ticket.json", b"first").unwrap();
        let second = write_unique_export(&directory, "tea-ticket.json", b"second").unwrap();

        assert_eq!(first.file_name().unwrap(), "tea-ticket.json");
        assert_eq!(second.file_name().unwrap(), "tea-ticket-1.json");
        assert_eq!(std::fs::read(first).unwrap(), b"first");
        assert_eq!(std::fs::read(second).unwrap(), b"second");

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn generated_auth_tokens_are_random_uuid_values() {
        let first = generate_token();
        let second = generate_token();
        assert_eq!(first.len(), 32);
        assert_eq!(second.len(), 32);
        assert_ne!(first, second);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn token_file_parser_requires_generated_token_format() {
        assert_eq!(
            parse_token_file_contents(" 0123456789abcdefABCDEF0123456789\r\n"),
            Some("0123456789abcdefABCDEF0123456789".to_string())
        );
        assert_eq!(parse_token_file_contents("dev-token"), None);
        assert_eq!(
            parse_token_file_contents("0123456789abcdefABCDEF012345678g"),
            None
        );
        assert_eq!(
            parse_token_file_contents("\u{feff}0123456789abcdefABCDEF0123456789"),
            None
        );
    }

    #[test]
    fn malformed_token_file_is_rejected_without_replacement() {
        let directory = std::env::temp_dir().join(format!(
            "tea-token-invalid-test-{}-{}",
            std::process::id(),
            generate_token()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let token_file = directory.join("auth-token.txt");
        std::fs::write(&token_file, "not-a-generated-token").unwrap();

        let error = resolve_or_create_token_file(&token_file).unwrap_err();

        assert!(error.contains("invalid format"));
        assert_eq!(
            std::fs::read_to_string(&token_file).unwrap(),
            "not-a-generated-token"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn token_path_must_be_a_regular_file() {
        let directory = std::env::temp_dir().join(format!(
            "tea-token-type-test-{}-{}",
            std::process::id(),
            generate_token()
        ));
        let token_file = directory.join("auth-token.txt");
        std::fs::create_dir_all(&token_file).unwrap();

        let error = resolve_or_create_token_file(&token_file).unwrap_err();

        assert!(error.contains("wrong filesystem type"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn daemon_executable_path_must_be_a_regular_file() {
        let directory = std::env::temp_dir().join(format!(
            "tea-daemon-type-test-{}-{}",
            std::process::id(),
            generate_token()
        ));
        let daemon_path = directory.join("tea-daemon.exe");
        std::fs::create_dir_all(&daemon_path).unwrap();

        let error = validate_daemon_executable_path(&daemon_path).unwrap_err();

        assert!(error.contains("must be a regular file"));
        std::fs::remove_dir_all(&daemon_path).unwrap();
        std::fs::write(&daemon_path, b"test daemon executable").unwrap();
        validate_daemon_executable_path(&daemon_path).unwrap();
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn missing_daemon_executable_path_is_rejected() {
        let daemon_path = std::env::temp_dir()
            .join(generate_token())
            .join("tea-daemon.exe");

        let error = validate_daemon_executable_path(&daemon_path).unwrap_err();

        assert!(error.contains("not found next to tea.exe"));
    }

    #[cfg(unix)]
    #[test]
    fn daemon_executable_path_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let directory = std::env::temp_dir().join(format!(
            "tea-daemon-symlink-test-{}-{}",
            std::process::id(),
            generate_token()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let target = directory.join("target");
        let daemon_path = directory.join("tea-daemon.exe");
        std::fs::write(&target, b"test daemon executable").unwrap();
        symlink(&target, &daemon_path).unwrap();

        let error = validate_daemon_executable_path(&daemon_path).unwrap_err();

        assert!(error.contains("must not be a symlink or reparse point"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn concurrent_token_creation_returns_one_shared_value() {
        let directory = std::env::temp_dir().join(format!(
            "tea-token-test-{}-{}",
            std::process::id(),
            generate_token()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let token_file = directory.join("auth-token.txt");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles = (0..8)
            .map(|_| {
                let token_file = token_file.clone();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    resolve_or_create_token_file(&token_file).unwrap()
                })
            })
            .collect::<Vec<_>>();
        let tokens = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens.into_iter().next().unwrap(),
            std::fs::read_to_string(&token_file).unwrap()
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn pid_cleanup_preserves_a_new_owner() {
        let directory = std::env::temp_dir().join(format!(
            "tea-pid-test-{}-{}",
            std::process::id(),
            generate_token()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let pid_file = directory.join("tea-daemon.pid");
        std::fs::write(&pid_file, "222").unwrap();

        assert!(!remove_pid_file_if_matches(&pid_file, 111));
        assert_eq!(std::fs::read_to_string(&pid_file).unwrap(), "222");
        assert!(remove_pid_file_if_matches(&pid_file, 222));
        assert!(!pid_file.exists());

        let _ = std::fs::remove_dir_all(directory);
    }
}
