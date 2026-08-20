#![forbid(unsafe_code)]

use std::io::Read as _;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};
#[cfg(test)]
use tea_http_read::MAX_ERROR_PREVIEW_CHARS;
use tea_http_read::{error_body_preview, percent_encode_component, read_response_text_limited};

const MAX_HOOK_INTAKE_FILE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Parser)]
#[command(name = "tea-cli", about = "Tea AI work-order CLI")]
struct Cli {
    #[arg(long, env = "TEA_SERVER_URL", default_value = "http://127.0.0.1:48910")]
    server_url: String,
    #[arg(long, env = "TEA_AUTH_TOKEN", default_value = "dev-token")]
    auth_token: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Status,
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Ticket {
        #[command(subcommand)]
        command: TicketCommand,
    },
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
    Hook {
        #[command(subcommand)]
        command: HookCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    Show,
    Set {
        #[arg(long)]
        notifications_enabled: Option<bool>,
        #[arg(long)]
        human_ticket_default_approval_policy: Option<String>,
        #[arg(long)]
        hook_ticket_default_approval_policy: Option<String>,
    },
    SetNotifications {
        #[arg(long)]
        enabled: bool,
    },
}

#[derive(Debug, Subcommand)]
enum TicketCommand {
    Create(CreateTicketArgs),
    /// Edit mutable fields on an existing work order. System-derived labels
    /// (source:, policy:, context:) are always preserved.
    Edit(EditTicketArgs),
    List(ListTicketArgs),
    Show {
        ticket_id: String,
    },
    Comment {
        ticket_id: String,
        body: String,
    },
    Events {
        ticket_id: String,
    },
    Export {
        ticket_id: String,
        #[arg(long, value_enum, default_value_t = ExportFormat::Json)]
        format: ExportFormat,
    },
    Analyze {
        ticket_id: String,
    },
    /// Show the stored AI analysis record for a work order.
    Analysis {
        ticket_id: String,
    },
    Decompose {
        ticket_id: String,
    },
    Plan {
        ticket_id: String,
    },
    /// Show the stored AI plan record for a work order.
    PlanShow {
        ticket_id: String,
    },
    Policy {
        ticket_id: String,
        #[arg(long)]
        mode: String,
    },
    Approve {
        ticket_id: String,
    },
    Reject {
        ticket_id: String,
        #[arg(long)]
        reason: String,
    },
    Run {
        ticket_id: String,
    },
    Stop {
        ticket_id: String,
    },
    Retry {
        ticket_id: String,
    },
    Accept {
        ticket_id: String,
    },
    Close {
        ticket_id: String,
    },
    Cancel {
        ticket_id: String,
    },
}

#[derive(Debug, Args)]
struct CreateTicketArgs {
    #[arg(long)]
    title: String,
    #[arg(long, alias = "body")]
    description: String,
    /// Optional priority for the new work order (e.g. high, normal, low).
    #[arg(long)]
    priority: Option<String>,
    /// Optional initial label; repeat the flag to add several.
    #[arg(long = "label", value_name = "LABEL")]
    labels: Vec<String>,
    /// Optional approval policy override applied at creation.
    #[arg(long = "approval-policy")]
    approval_policy: Option<String>,
    /// Stable key to replay the same logical create safely after an uncertain failure.
    #[arg(long = "idempotency-key")]
    idempotency_key: Option<String>,
}

impl CreateTicketArgs {
    fn into_request(self) -> (serde_json::Value, Option<String>) {
        let mut body = json!({ "title": self.title, "description": self.description });
        let map = body
            .as_object_mut()
            .expect("create ticket body is a JSON object");
        if let Some(priority) = self
            .priority
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            map.insert("priority".to_string(), json!(priority));
        }
        let labels: Vec<String> = self
            .labels
            .into_iter()
            .map(|label| label.trim().to_string())
            .filter(|label| !label.is_empty())
            .collect();
        if !labels.is_empty() {
            map.insert("labels".to_string(), json!(labels));
        }
        if let Some(policy) = self
            .approval_policy
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            map.insert("approval_policy".to_string(), json!(policy));
        }
        (body, self.idempotency_key)
    }
}

#[derive(Debug, Args)]
struct EditTicketArgs {
    ticket_id: String,
    /// New title for the work order.
    #[arg(long)]
    title: Option<String>,
    /// New description/body for the work order.
    #[arg(long, alias = "body")]
    description: Option<String>,
    /// New priority for the work order (e.g. high, normal, low).
    #[arg(long)]
    priority: Option<String>,
    /// Replace operator labels; repeat the flag to set several. System-derived
    /// labels (source:, policy:, context:) are always preserved by the daemon.
    #[arg(long = "label", value_name = "LABEL")]
    labels: Option<Vec<String>>,
}

#[derive(Debug, Args, Default)]
struct ListTicketArgs {
    /// Optional exact ticket status filter.
    #[arg(long)]
    status: Option<String>,
    /// Optional exact ticket source filter.
    #[arg(long)]
    source: Option<String>,
    /// Opt into a paged response with at most this many tickets (1-200).
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=200))]
    limit: Option<u16>,
    /// Opaque continuation cursor returned by a previous paged response.
    #[arg(long)]
    cursor: Option<String>,
}

impl ListTicketArgs {
    fn request_path(&self) -> String {
        let mut query = Vec::new();
        for (name, value) in [
            ("status", self.status.as_deref()),
            ("source", self.source.as_deref()),
        ] {
            if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
                query.push(format!("{name}={}", percent_encode_component(value)));
            }
        }
        if let Some(limit) = self.limit {
            query.push(format!("limit={limit}"));
        }
        if let Some(cursor) = self
            .cursor
            .as_deref()
            .map(str::trim)
            .filter(|cursor| !cursor.is_empty())
        {
            query.push(format!("cursor={}", percent_encode_component(cursor)));
        }
        if query.is_empty() {
            "/v1/tickets".to_string()
        } else {
            format!("/v1/tickets?{}", query.join("&"))
        }
    }
}

impl EditTicketArgs {
    fn into_id_and_request_body(self) -> (String, serde_json::Value) {
        let mut body = json!({});
        let map = body
            .as_object_mut()
            .expect("edit ticket body is a JSON object");
        if let Some(title) = self
            .title
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            map.insert("title".to_string(), json!(title));
        }
        if let Some(description) = self.description {
            map.insert("description".to_string(), json!(description));
        }
        if let Some(priority) = self
            .priority
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            map.insert("priority".to_string(), json!(priority));
        }
        if let Some(labels) = self.labels {
            let labels: Vec<String> = labels
                .into_iter()
                .map(|label| label.trim().to_string())
                .filter(|label| !label.is_empty())
                .collect();
            map.insert("labels".to_string(), json!(labels));
        }
        (self.ticket_id, body)
    }
}

#[derive(Debug, Clone, ValueEnum)]
enum ExportFormat {
    Json,
    Markdown,
}

#[derive(Debug, Subcommand)]
enum RunCommand {
    Show { run_id: String },
    Stop { run_id: String },
    Retry { run_id: String },
}

#[derive(Debug, Subcommand)]
enum HookCommand {
    Intake {
        #[arg(long)]
        file: PathBuf,
        /// Stable key to replay the same Hook intake safely after an uncertain failure.
        #[arg(long = "idempotency-key")]
        idempotency_key: Option<String>,
    },
}

fn parse_hook_intake_payload(reader: impl std::io::Read, source: &str) -> anyhow::Result<Value> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_HOOK_INTAKE_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_HOOK_INTAKE_FILE_BYTES {
        anyhow::bail!(
            "Hook intake file {source} exceeds the {MAX_HOOK_INTAKE_FILE_BYTES}-byte limit"
        );
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| anyhow::anyhow!("invalid Hook intake JSON in {source}: {error}"))
}

fn read_hook_intake_payload(path: &Path) -> anyhow::Result<Value> {
    let file = std::fs::File::open(path)?;
    parse_hook_intake_payload(file, &path.display().to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let client = ApiClient::new(cli.server_url, cli.auth_token);
    let output = match cli.command {
        Command::Status => {
            let status = client.get("/v1/status").await?;
            CommandOutput::Text(format_status(&status))
        }
        Command::Config { command } => run_config_command(&client, command).await?,
        Command::Ticket { command } => run_ticket_command(&client, command).await?,
        Command::Run { command } => match command {
            RunCommand::Show { run_id } => {
                CommandOutput::Json(client.get(&run_path(&run_id, "")).await?)
            }
            RunCommand::Stop { run_id } => {
                CommandOutput::Json(client.post(&run_path(&run_id, "/stop"), json!({})).await?)
            }
            RunCommand::Retry { run_id } => {
                CommandOutput::Json(client.post(&run_path(&run_id, "/retry"), json!({})).await?)
            }
        },
        Command::Hook { command } => match command {
            HookCommand::Intake {
                file,
                idempotency_key,
            } => {
                let payload = read_hook_intake_payload(&file)?;
                CommandOutput::Json(
                    client
                        .post_idempotent("/v1/intake/hook", payload, idempotency_key.as_deref())
                        .await?,
                )
            }
        },
    };
    match output {
        CommandOutput::Json(value) => println!("{}", serde_json::to_string_pretty(&value)?),
        CommandOutput::Text(text) => println!("{text}"),
    }
    Ok(())
}

async fn run_config_command(
    client: &ApiClient,
    command: ConfigCommand,
) -> anyhow::Result<CommandOutput> {
    match command {
        ConfigCommand::Show => Ok(CommandOutput::Json(client.get("/v1/configuration").await?)),
        ConfigCommand::Set {
            notifications_enabled,
            human_ticket_default_approval_policy,
            hook_ticket_default_approval_policy,
        } => Ok(CommandOutput::Json(
            client
                .patch(
                    "/v1/configuration",
                    config_patch_payload(
                        notifications_enabled,
                        human_ticket_default_approval_policy,
                        hook_ticket_default_approval_policy,
                    ),
                )
                .await?,
        )),
        ConfigCommand::SetNotifications { enabled } => Ok(CommandOutput::Json(
            client
                .patch(
                    "/v1/configuration",
                    json!({ "notifications_enabled": enabled }),
                )
                .await?,
        )),
    }
}

fn config_patch_payload(
    notifications_enabled: Option<bool>,
    human_ticket_default_approval_policy: Option<String>,
    hook_ticket_default_approval_policy: Option<String>,
) -> Value {
    let mut patch = serde_json::Map::new();
    if let Some(enabled) = notifications_enabled {
        patch.insert("notifications_enabled".to_string(), Value::Bool(enabled));
    }
    if let Some(policy) = human_ticket_default_approval_policy {
        patch.insert(
            "human_ticket_default_approval_policy".to_string(),
            Value::String(policy),
        );
    }
    if let Some(policy) = hook_ticket_default_approval_policy {
        patch.insert(
            "hook_ticket_default_approval_policy".to_string(),
            Value::String(policy),
        );
    }
    Value::Object(patch)
}

enum CommandOutput {
    Json(Value),
    Text(String),
}

const MAX_TEA_API_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

fn format_status(status: &Value) -> String {
    let service = scalar_to_string(status.get("service")).unwrap_or_else(|| "tea".to_string());
    let status_text =
        scalar_to_string(status.get("status")).unwrap_or_else(|| "unknown".to_string());
    let mut lines = vec![
        format!("Service: {service}"),
        format!("Status: {status_text}"),
    ];

    if let Some(provider) = scalar_to_string(status.get("execution_provider")) {
        lines.push(format!("Execution provider: {provider}"));
    }

    if let Some(store) = status.get("store") {
        let backend =
            scalar_to_string(store.get("backend")).unwrap_or_else(|| "unknown".to_string());
        lines.push(format!("Store: {backend}"));

        let schema_version = scalar_to_string(store.get("schema_version"));
        let supported_schema_version = scalar_to_string(store.get("supported_schema_version"));
        match (schema_version, supported_schema_version) {
            (Some(schema_version), Some(supported_schema_version)) => {
                lines.push(format!(
                    "SQLite schema: {schema_version} (supported: {supported_schema_version})"
                ));
            }
            _ => lines.push("SQLite schema: n/a".to_string()),
        }

        if let Some(key_count) = scalar_to_string(store.get("idempotency_key_count")) {
            lines.push(format!("Idempotency keys: {key_count}"));
        }
        if let (Some(page_count), Some(freelist_count)) = (
            scalar_to_string(store.get("sqlite_page_count")),
            scalar_to_string(store.get("sqlite_freelist_count")),
        ) {
            lines.push(format!(
                "SQLite pages: {page_count} (free: {freelist_count})"
            ));
        }
    }

    if let Some(source) = scalar_to_string(status.get("configuration_source")) {
        lines.push(format!("Configuration source: {source}"));
    }
    if let Some(configuration) = status.get("configuration") {
        if let Some(owner) = scalar_to_string(configuration.get("owner")) {
            lines.push(format!("Configuration owner: {owner}"));
        }
        if let Some(panel_url) = scalar_to_string(configuration.get("loom_panel_url")) {
            lines.push(format!("Loom settings: {panel_url}"));
        }
        if let Some(reason) = scalar_to_string(configuration.get("reason")) {
            lines.push(format!("Configuration note: {reason}"));
        }
    }

    lines.join("\n")
}

fn scalar_to_string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Number(value)) => Some(value.to_string()),
        Some(Value::Bool(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn ticket_path(ticket_id: &str, suffix: &str) -> String {
    format!(
        "/v1/tickets/{}{suffix}",
        percent_encode_component(ticket_id)
    )
}

fn run_path(run_id: &str, suffix: &str) -> String {
    format!("/v1/runs/{}{suffix}", percent_encode_component(run_id))
}

async fn run_ticket_command(
    client: &ApiClient,
    command: TicketCommand,
) -> anyhow::Result<CommandOutput> {
    match command {
        TicketCommand::Create(args) => {
            let (body, idempotency_key) = args.into_request();
            Ok(CommandOutput::Json(
                client
                    .post_idempotent("/v1/tickets", body, idempotency_key.as_deref())
                    .await?,
            ))
        }
        TicketCommand::Edit(args) => {
            let (ticket_id, body) = args.into_id_and_request_body();
            Ok(CommandOutput::Json(
                client.patch(&ticket_path(&ticket_id, ""), body).await?,
            ))
        }
        TicketCommand::List(args) => {
            Ok(CommandOutput::Json(client.get(&args.request_path()).await?))
        }
        TicketCommand::Show { ticket_id } => Ok(CommandOutput::Json(
            client.get(&ticket_path(&ticket_id, "")).await?,
        )),
        TicketCommand::Comment { ticket_id, body } => Ok(CommandOutput::Json(
            client
                .post(
                    &ticket_path(&ticket_id, "/comments"),
                    json!({ "body": body }),
                )
                .await?,
        )),
        TicketCommand::Events { ticket_id } => Ok(CommandOutput::Json(
            client.get(&ticket_path(&ticket_id, "/events")).await?,
        )),
        TicketCommand::Export { ticket_id, format } => match format {
            ExportFormat::Json => Ok(CommandOutput::Json(
                client.get(&ticket_path(&ticket_id, "/export/json")).await?,
            )),
            ExportFormat::Markdown => Ok(CommandOutput::Text(
                client
                    .get_text(&ticket_path(&ticket_id, "/export/markdown"))
                    .await?,
            )),
        },
        TicketCommand::Analyze { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&ticket_path(&ticket_id, "/analyze"), json!({}))
                .await?,
        )),
        TicketCommand::Analysis { ticket_id } => Ok(CommandOutput::Json(
            client.get(&ticket_path(&ticket_id, "/analysis")).await?,
        )),
        TicketCommand::Decompose { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&ticket_path(&ticket_id, "/decompose"), json!({}))
                .await?,
        )),
        TicketCommand::Plan { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&ticket_path(&ticket_id, "/plan"), json!({}))
                .await?,
        )),
        TicketCommand::PlanShow { ticket_id } => Ok(CommandOutput::Json(
            client.get(&ticket_path(&ticket_id, "/plan")).await?,
        )),
        TicketCommand::Policy { ticket_id, mode } => Ok(CommandOutput::Json(
            client
                .post(&ticket_path(&ticket_id, "/policy"), json!({ "mode": mode }))
                .await?,
        )),
        TicketCommand::Approve { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&ticket_path(&ticket_id, "/approve"), json!({}))
                .await?,
        )),
        TicketCommand::Reject { ticket_id, reason } => Ok(CommandOutput::Json(
            client
                .post(
                    &ticket_path(&ticket_id, "/reject"),
                    json!({ "reason": reason }),
                )
                .await?,
        )),
        TicketCommand::Run { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&ticket_path(&ticket_id, "/run"), json!({}))
                .await?,
        )),
        TicketCommand::Stop { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&ticket_path(&ticket_id, "/stop"), json!({}))
                .await?,
        )),
        TicketCommand::Retry { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&ticket_path(&ticket_id, "/retry"), json!({}))
                .await?,
        )),
        TicketCommand::Accept { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&ticket_path(&ticket_id, "/accept"), json!({}))
                .await?,
        )),
        TicketCommand::Close { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&ticket_path(&ticket_id, "/close"), json!({}))
                .await?,
        )),
        TicketCommand::Cancel { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&ticket_path(&ticket_id, "/cancel"), json!({}))
                .await?,
        )),
    }
}

struct ApiClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl ApiClient {
    fn new(base_url: String, token: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(300))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("build Tea CLI HTTP client"),
        }
    }

    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        self.send(self.http.get(self.url(path))).await
    }

    async fn post(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        self.send(self.http.post(self.url(path)).json(&body)).await
    }

    async fn post_idempotent(
        &self,
        path: &str,
        body: Value,
        idempotency_key: Option<&str>,
    ) -> anyhow::Result<Value> {
        let mut request = self.http.post(self.url(path)).json(&body);
        if let Some(key) = idempotency_key {
            request = request.header("Idempotency-Key", key);
        }
        self.send(request).await
    }

    async fn patch(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        self.send(self.http.patch(self.url(path)).json(&body)).await
    }

    async fn get_text(&self, path: &str) -> anyhow::Result<String> {
        self.send_text(self.http.get(self.url(path))).await
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> anyhow::Result<Value> {
        let body = self.send_text(request).await?;
        Ok(serde_json::from_str(&body)?)
    }

    async fn send_text(&self, request: reqwest::RequestBuilder) -> anyhow::Result<String> {
        let response = request.bearer_auth(&self.token).send().await?;
        let (status, body) =
            read_response_text_limited(response, MAX_TEA_API_RESPONSE_BYTES, "Tea API").await?;
        if !status.is_success() {
            anyhow::bail!("Tea API returned {status}: {}", error_body_preview(&body));
        }
        Ok(body)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_intake_file_reader_rejects_oversized_input_before_json_parsing() {
        let oversized = vec![b' '; MAX_HOOK_INTAKE_FILE_BYTES + 1];
        let error = parse_hook_intake_payload(std::io::Cursor::new(oversized), "oversized.json")
            .unwrap_err();
        assert!(error.to_string().contains("exceeds the"));
        assert!(error.to_string().contains("byte limit"));
    }

    #[test]
    fn hook_intake_file_reader_parses_bounded_json() {
        let payload = parse_hook_intake_payload(
            std::io::Cursor::new(br#"{"source":"hook","text":"bounded"}"#),
            "bounded.json",
        )
        .unwrap();
        assert_eq!(payload["source"], "hook");
        assert_eq!(payload["text"], "bounded");
    }

    fn spawn_raw_http_server(response: String) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};

            let (mut stream, _) = listener.accept().unwrap();
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

    #[tokio::test]
    async fn cli_response_reader_rejects_oversized_chunked_body() {
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
        let response = reqwest::Client::new().get(url).send().await.unwrap();

        let error = read_response_text_limited(response, 8, "Tea API")
            .await
            .unwrap_err();

        server.join().unwrap();
        assert!(error.to_string().contains("8-byte limit"));
    }

    #[test]
    fn cli_error_preview_is_bounded_and_single_line() {
        let body = format!("first\nsecond\t{}", "x".repeat(600));
        let preview = error_body_preview(&body);

        assert!(!preview
            .chars()
            .any(|character| matches!(character, '\n' | '\r' | '\t')));
        assert!(preview.ends_with("..."));
        assert!(preview.chars().count() <= MAX_ERROR_PREVIEW_CHARS + 3);
    }

    #[tokio::test]
    async fn cli_http_errors_do_not_echo_unbounded_multiline_bodies() {
        let body = format!("first\nsecond\t{}", "x".repeat(600));
        let response = format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (url, server) = spawn_raw_http_server(response);
        let client = ApiClient::new(url, "test-token".to_string());

        let message = client.get("/v1/status").await.unwrap_err().to_string();

        server.join().unwrap();
        assert!(message.contains("500 Internal Server Error"));
        assert!(!message
            .chars()
            .any(|character| matches!(character, '\n' | '\r' | '\t')));
        assert!(message.ends_with("..."));
        assert!(message.chars().count() < 600);
    }

    #[test]
    fn api_paths_encode_utf8_and_reserved_characters() {
        assert_eq!(
            ticket_path("中文/é", "/events"),
            "/v1/tickets/%E4%B8%AD%E6%96%87%2F%C3%A9/events"
        );
        assert_eq!(run_path("a/b c", "/stop"), "/v1/runs/a%2Fb%20c/stop");
    }

    #[test]
    fn format_status_includes_sqlite_schema_metadata() {
        let output = format_status(&json!({
            "service": "tea",
            "status": "ok",
            "execution_provider": "loom",
            "store": {
                "backend": "sqlite",
                "schema_version": 1,
                "supported_schema_version": 1,
                "idempotency_key_count": 12,
                "sqlite_page_count": 48,
                "sqlite_freelist_count": 3
            }
        }));

        assert!(output.contains("Service: tea"));
        assert!(output.contains("Status: ok"));
        assert!(output.contains("Execution provider: loom"));
        assert!(output.contains("Store: sqlite"));
        assert!(output.contains("SQLite schema: 1 (supported: 1)"));
        assert!(output.contains("Idempotency keys: 12"));
        assert!(output.contains("SQLite pages: 48 (free: 3)"));
    }

    #[test]
    fn format_status_handles_memory_store_metadata() {
        let output = format_status(&json!({
            "service": "tea",
            "status": "ok",
            "store": {
                "backend": "memory",
                "schema_version": null,
                "supported_schema_version": null,
                "idempotency_key_count": null,
                "sqlite_page_count": null,
                "sqlite_freelist_count": null
            }
        }));

        assert!(output.contains("Service: tea"));
        assert!(output.contains("Status: ok"));
        assert!(output.contains("Store: memory"));
        assert!(output.contains("SQLite schema: n/a"));
        assert!(!output.contains("Idempotency keys:"));
        assert!(!output.contains("SQLite pages:"));
    }

    #[test]
    fn format_status_includes_configuration_source() {
        let output = format_status(&json!({
            "service": "tea",
            "status": "ok",
            "configuration_source": "loom-managed",
            "configuration": {
                "owner": "loom",
                "loom_panel_url": "loom://settings/tea"
            }
        }));

        assert!(output.contains("Configuration source: loom-managed"));
        assert!(output.contains("Configuration owner: loom"));
        assert!(output.contains("Loom settings: loom://settings/tea"));
    }

    #[test]
    fn config_notification_patch_contains_only_notification_field() {
        let patch = config_patch_payload(Some(false), None, None);

        assert_eq!(patch, json!({ "notifications_enabled": false }));
    }

    #[test]
    fn config_patch_can_include_all_local_config_fields() {
        let patch = config_patch_payload(
            Some(false),
            Some("human_before_completion".to_string()),
            Some("manual_only".to_string()),
        );

        assert_eq!(patch["notifications_enabled"], false);
        assert_eq!(
            patch["human_ticket_default_approval_policy"],
            "human_before_completion"
        );
        assert_eq!(patch["hook_ticket_default_approval_policy"], "manual_only");
    }

    #[test]
    fn run_command_parses_stop_and_retry_actions() {
        let stop = Cli::try_parse_from(["tea", "run", "stop", "run-123"]).unwrap();
        match stop.command {
            Command::Run {
                command: RunCommand::Stop { run_id },
            } => assert_eq!(run_id, "run-123"),
            other => panic!("expected run stop command, got {other:?}"),
        }

        let retry = Cli::try_parse_from(["tea", "run", "retry", "run-456"]).unwrap();
        match retry.command {
            Command::Run {
                command: RunCommand::Retry { run_id },
            } => assert_eq!(run_id, "run-456"),
            other => panic!("expected run retry command, got {other:?}"),
        }
    }

    #[test]
    fn create_commands_parse_idempotency_keys() {
        let ticket = Cli::try_parse_from([
            "tea",
            "ticket",
            "create",
            "--title",
            "Retry-safe create",
            "--description",
            "Create this logical request only once.",
            "--idempotency-key",
            "ticket-request-1",
        ])
        .unwrap();
        let Command::Ticket {
            command: TicketCommand::Create(args),
        } = ticket.command
        else {
            panic!("expected ticket create command");
        };
        assert_eq!(args.idempotency_key.as_deref(), Some("ticket-request-1"));

        let hook = Cli::try_parse_from([
            "tea",
            "hook",
            "intake",
            "--file",
            "hook.json",
            "--idempotency-key",
            "hook-request-1",
        ])
        .unwrap();
        let Command::Hook {
            command: HookCommand::Intake {
                idempotency_key, ..
            },
        } = hook.command
        else {
            panic!("expected Hook intake command");
        };
        assert_eq!(idempotency_key.as_deref(), Some("hook-request-1"));
    }

    #[tokio::test]
    async fn idempotent_post_sends_the_requested_header() {
        let response =
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_string();
        let (url, request, server) = spawn_capturing_http_server(response);
        let client = ApiClient::new(url, "test-token".to_string());

        client
            .post_idempotent(
                "/v1/tickets",
                json!({ "title": "Test", "description": "Test description" }),
                Some("cli-idempotency-1"),
            )
            .await
            .unwrap();

        server.join().unwrap();
        let request = request.recv().unwrap().to_ascii_lowercase();
        assert!(request.contains("idempotency-key: cli-idempotency-1\r\n"));
        assert!(request.contains("authorization: bearer test-token\r\n"));
    }

    #[test]
    fn ticket_command_parses_policy_override() {
        let parsed = Cli::try_parse_from([
            "tea",
            "ticket",
            "policy",
            "ticket-123",
            "--mode",
            "manual_only",
        ])
        .unwrap();

        match parsed.command {
            Command::Ticket {
                command: TicketCommand::Policy { ticket_id, mode },
            } => {
                assert_eq!(ticket_id, "ticket-123");
                assert_eq!(mode, "manual_only");
            }
            other => panic!("expected ticket policy command, got {other:?}"),
        }
    }

    #[test]
    fn ticket_list_parses_optional_pagination_and_filters() {
        let parsed = Cli::try_parse_from([
            "tea",
            "ticket",
            "list",
            "--status",
            "open",
            "--source",
            "hook",
            "--limit",
            "25",
            "--cursor",
            "v1-000000000000000a-open-hook",
        ])
        .unwrap();
        let Command::Ticket {
            command: TicketCommand::List(args),
        } = parsed.command
        else {
            panic!("expected ticket list command");
        };
        assert_eq!(
            args.request_path(),
            "/v1/tickets?status=open&source=hook&limit=25&cursor=v1-000000000000000a-open-hook"
        );

        let legacy = Cli::try_parse_from(["tea", "ticket", "list"]).unwrap();
        let Command::Ticket {
            command: TicketCommand::List(args),
        } = legacy.command
        else {
            panic!("expected legacy ticket list command");
        };
        assert_eq!(args.request_path(), "/v1/tickets");
        assert!(Cli::try_parse_from(["tea", "ticket", "list", "--limit", "0"]).is_err());
        assert!(Cli::try_parse_from(["tea", "ticket", "list", "--limit", "201"]).is_err());
    }

    #[test]
    fn ticket_command_parses_cancel() {
        let parsed = Cli::try_parse_from(["tea", "ticket", "cancel", "ticket-123"]).unwrap();

        match parsed.command {
            Command::Ticket {
                command: TicketCommand::Cancel { ticket_id },
            } => assert_eq!(ticket_id, "ticket-123"),
            other => panic!("expected ticket cancel command, got {other:?}"),
        }
    }

    #[test]
    fn ticket_command_parses_edit_with_all_fields() {
        let parsed = Cli::try_parse_from([
            "tea",
            "ticket",
            "edit",
            "ticket-123",
            "--title",
            "New title",
            "--description",
            "New body",
            "--priority",
            "high",
            "--label",
            "area:auth",
            "--label",
            "needs-review",
        ])
        .unwrap();

        match parsed.command {
            Command::Ticket {
                command: TicketCommand::Edit(args),
            } => {
                let (ticket_id, body) = args.into_id_and_request_body();
                assert_eq!(ticket_id, "ticket-123");
                assert_eq!(body["title"], "New title");
                assert_eq!(body["description"], "New body");
                assert_eq!(body["priority"], "high");
                assert_eq!(body["labels"], json!(["area:auth", "needs-review"]));
            }
            other => panic!("expected ticket edit command, got {other:?}"),
        }
    }

    #[test]
    fn edit_request_body_omits_untouched_fields() {
        let parsed = Cli::try_parse_from([
            "tea",
            "ticket",
            "edit",
            "ticket-123",
            "--title",
            "Only title",
        ])
        .unwrap();

        match parsed.command {
            Command::Ticket {
                command: TicketCommand::Edit(args),
            } => {
                let (ticket_id, body) = args.into_id_and_request_body();
                assert_eq!(ticket_id, "ticket-123");
                assert_eq!(body["title"], "Only title");
                let map = body.as_object().expect("edit body is an object");
                assert!(!map.contains_key("description"));
                assert!(!map.contains_key("priority"));
                assert!(!map.contains_key("labels"));
            }
            other => panic!("expected ticket edit command, got {other:?}"),
        }
    }

    #[test]
    fn edit_request_body_sends_empty_labels_to_clear_them() {
        // `--label ""` (or no operator labels while the flag is present) yields an
        // explicit empty array so the daemon clears operator labels while still
        // preserving its own system-derived labels.
        let parsed =
            Cli::try_parse_from(["tea", "ticket", "edit", "ticket-123", "--label", ""]).unwrap();

        match parsed.command {
            Command::Ticket {
                command: TicketCommand::Edit(args),
            } => {
                let (_ticket_id, body) = args.into_id_and_request_body();
                assert_eq!(body["labels"], json!([]));
            }
            other => panic!("expected ticket edit command, got {other:?}"),
        }
    }

    #[test]
    fn ticket_command_parses_decompose() {
        let parsed = Cli::try_parse_from(["tea", "ticket", "decompose", "ticket-123"]).unwrap();

        match parsed.command {
            Command::Ticket {
                command: TicketCommand::Decompose { ticket_id },
            } => assert_eq!(ticket_id, "ticket-123"),
            other => panic!("expected ticket decompose command, got {other:?}"),
        }
    }
}
