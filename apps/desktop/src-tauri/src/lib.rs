use reqwest::Method;
use serde::Serialize;
use serde_json::Value;

const DEFAULT_TEA_SERVER_URL: &str = "http://127.0.0.1:48910";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeaRuntimeConfig {
    pub server_url: String,
    pub auth_configured: bool,
}

#[tauri::command]
fn resolve_tea_runtime_config() -> TeaRuntimeConfig {
    TeaRuntimeConfig {
        server_url: configured_server_url(),
        auth_configured: configured_auth_token().is_some(),
    }
}

#[tauri::command]
async fn tea_request(
    method: String,
    path: String,
    body: Option<Value>,
    base_url: Option<String>,
    auth_token: Option<String>,
) -> Result<Value, String> {
    let method = Method::from_bytes(method.as_bytes())
        .map_err(|error| format!("invalid HTTP method '{method}': {error}"))?;
    let base_url = normalize_base_url(
        base_url
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(configured_server_url),
    );
    let url = join_url(&base_url, &path)?;
    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| format!("failed to build Tea HTTP client: {error}"))?;
    let mut request = client.request(method, url);

    if let Some(token) = auth_token
        .filter(|value| !value.trim().is_empty())
        .or_else(configured_auth_token)
        .or_else(|| Some("dev-token".to_string()))
    {
        request = request.bearer_auth(token);
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
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("failed to read Tea response body: {error}"))?;

    if !status.is_success() {
        return Err(format!("Tea returned HTTP {status}: {text}"));
    }

    if text.trim().is_empty() {
        return Ok(Value::Null);
    }

    serde_json::from_str::<Value>(&text).or(Ok(Value::String(text)))
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
    let path = dir.join(&safe_name);
    std::fs::write(&path, content)
        .map_err(|error| format!("failed to write export file: {error}"))?;
    Ok(path.to_string_lossy().to_string())
}

pub fn run() {
    // Make tea.exe fully self-contained: start our own tea-daemon.exe if one is
    // not already answering, so a user only ever needs to launch a single exe.
    // Failures here are non-fatal; the UI still renders and surfaces the
    // connection state, and the launcher .bat path continues to work.
    if let Err(error) = ensure_daemon_running() {
        eprintln!("Tea: could not auto-start daemon: {error}");
    }

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            resolve_tea_runtime_config,
            tea_request,
            save_tea_export
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tea desktop");
}

/// Ensure a Tea daemon is reachable, spawning a bundled `tea-daemon.exe` if not.
///
/// Single-exe UX: launching `tea.exe` alone must bring up the whole stack. We
/// resolve (or create) a shared auth token, check whether a daemon already
/// answers `/health`, and if not spawn the sibling `tea-daemon.exe` with the
/// same token + a package-local data directory, then wait for it to become
/// healthy. The token is written to `data/auth-token.txt` so `tea_request`'s
/// `configured_auth_token()` resolves the identical value.
fn ensure_daemon_running() -> Result<(), String> {
    let base_url = configured_server_url();
    let token = resolve_or_create_token()?;

    // Make the token visible to this process's own token resolution and to the
    // spawned child, keeping desktop and daemon in agreement.
    std::env::set_var("TEA_AUTH_TOKEN", &token);

    // Already up? Nothing to do.
    if daemon_health_ok(&base_url, &token) {
        return Ok(());
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .ok_or_else(|| "cannot locate the tea.exe directory".to_string())?;

    let daemon_path = exe_dir.join("tea-daemon.exe");
    if !daemon_path.exists() {
        return Err(format!(
            "tea-daemon.exe not found next to tea.exe ({})",
            daemon_path.display()
        ));
    }

    let data_dir = package_data_dir();
    std::fs::create_dir_all(&data_dir)
        .map_err(|error| format!("failed to create data directory: {error}"))?;
    let store_path = data_dir.join("tea.sqlite");
    let config_path = data_dir.join("config.json");
    let bind_addr =
        std::env::var("TEA_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:48910".to_string());

    let mut command = std::process::Command::new(&daemon_path);
    command
        .arg("--bind-addr")
        .arg(&bind_addr)
        .arg("--auth-token")
        .arg(&token)
        .arg("--store-path")
        .arg(&store_path)
        .arg("--config-path")
        .arg(&config_path)
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

    command
        .spawn()
        .map_err(|error| format!("failed to spawn tea-daemon.exe: {error}"))?;

    // Wait up to ~10s for the daemon to answer /health.
    for _ in 0..40 {
        if daemon_health_ok(&base_url, &token) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Err("tea-daemon.exe did not become healthy in time".to_string())
}

/// Blocking `/health` probe using only std (no reqwest `blocking` feature, which
/// the build environment does not enable). Opens a short-timeout TCP connection
/// to the daemon's host:port and issues a minimal HTTP/1.0 GET /health, treating
/// a `2xx` status line as healthy. Health needs no auth.
fn daemon_health_ok(base_url: &str, _token: &str) -> bool {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let (host, port) = match host_port_from_base_url(base_url) {
        Some(pair) => pair,
        None => return false,
    };

    let addr = format!("{host}:{port}");
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
    let without_scheme = base_url
        .trim()
        .trim_end_matches('/')
        .strip_prefix("http://")
        .or_else(|| {
            base_url
                .trim()
                .trim_end_matches('/')
                .strip_prefix("https://")
        })
        .unwrap_or(base_url.trim().trim_end_matches('/'));
    // Drop any path segment.
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    let mut parts = authority.rsplitn(2, ':');
    let port_str = parts.next()?;
    let host = parts.next().unwrap_or("127.0.0.1");
    let (host, port) = if host.is_empty() {
        ("127.0.0.1", port_str.parse::<u16>().ok()?)
    } else {
        (host, port_str.parse::<u16>().unwrap_or(48910))
    };
    Some((host.to_string(), port))
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
fn resolve_or_create_token() -> Result<String, String> {
    if let Some(token) = configured_auth_token() {
        return Ok(token);
    }
    let data_dir = package_data_dir();
    std::fs::create_dir_all(&data_dir)
        .map_err(|error| format!("failed to create data directory: {error}"))?;
    let token_file = data_dir.join("auth-token.txt");
    if let Ok(contents) = std::fs::read_to_string(&token_file) {
        let existing = contents.trim().to_string();
        if !existing.is_empty() {
            return Ok(existing);
        }
    }
    let token = generate_token();
    std::fs::write(&token_file, &token)
        .map_err(|error| format!("failed to write auth token file: {error}"))?;
    Ok(token)
}

/// Package-local data directory: `TEA_DATA_DIR` if set (launcher path), else a
/// `data` directory beside the executable (single-exe path).
fn package_data_dir() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("TEA_DATA_DIR") {
        return std::path::PathBuf::from(dir);
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("data")))
        .unwrap_or_else(|| std::env::temp_dir().join("tea-data"))
}

/// Generate a local auth token without extra dependencies. This only guards a
/// loopback daemon, so a high-entropy-ish value derived from the clock and pid
/// is sufficient to keep casual collisions away.
fn generate_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("{nanos:x}{pid:x}")
}

fn sanitize_export_file_name(value: &str) -> String {
    let base = value
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or("")
        .trim();
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

#[cfg(test)]
mod tests {
    use super::*;

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

fn configured_auth_token() -> Option<String> {
    // 1. Explicit env var (set by start-tea.bat before launching tea.exe).
    if let Some(token) = std::env::var("TEA_AUTH_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Some(token);
    }
    // 2. Shared token file written by start-tea.bat. Windows `start` does not
    //    always propagate the parent shell's environment to tea.exe, so the env
    //    var can be missing even though the daemon was launched with a random
    //    token. Reading the same file the launcher wrote keeps the desktop and
    //    daemon tokens in agreement regardless of env propagation.
    read_token_file()
}

// Locate and read the launcher's `data/auth-token.txt`. The launcher sets
// TEA_DATA_DIR to `<package>\data`; when that is absent we fall back to a
// `data` directory beside the running executable (the package layout).
fn read_token_file() -> Option<String> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("TEA_DATA_DIR") {
        candidates.push(std::path::Path::new(&dir).join("auth-token.txt"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join("data").join("auth-token.txt"));
        }
    }
    for path in candidates {
        if let Ok(contents) = std::fs::read_to_string(&path) {
            let token = contents.trim().to_string();
            if !token.is_empty() {
                return Some(token);
            }
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
