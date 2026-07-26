#![forbid(unsafe_code)]

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};

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
    List,
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
}

impl CreateTicketArgs {
    fn into_request_body(self) -> serde_json::Value {
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
        body
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
    },
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
                CommandOutput::Json(client.get(&format!("/v1/runs/{run_id}")).await?)
            }
            RunCommand::Stop { run_id } => CommandOutput::Json(
                client
                    .post(&format!("/v1/runs/{run_id}/stop"), json!({}))
                    .await?,
            ),
            RunCommand::Retry { run_id } => CommandOutput::Json(
                client
                    .post(&format!("/v1/runs/{run_id}/retry"), json!({}))
                    .await?,
            ),
        },
        Command::Hook { command } => match command {
            HookCommand::Intake { file } => {
                let payload: Value = serde_json::from_str(&std::fs::read_to_string(file)?)?;
                CommandOutput::Json(client.post("/v1/intake/hook", payload).await?)
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
        } => {
            let current = client.get("/v1/configuration").await?;
            let config = current
                .get("config")
                .cloned()
                .unwrap_or_else(default_config_payload);
            Ok(CommandOutput::Json(
                client
                    .put(
                        "/v1/configuration",
                        update_config_fields(
                            config,
                            notifications_enabled,
                            human_ticket_default_approval_policy,
                            hook_ticket_default_approval_policy,
                        ),
                    )
                    .await?,
            ))
        }
        ConfigCommand::SetNotifications { enabled } => {
            let current = client.get("/v1/configuration").await?;
            let config = current
                .get("config")
                .cloned()
                .unwrap_or_else(default_config_payload);
            Ok(CommandOutput::Json(
                client
                    .put(
                        "/v1/configuration",
                        update_config_notifications(config, enabled),
                    )
                    .await?,
            ))
        }
    }
}

fn default_config_payload() -> Value {
    json!({
        "notifications_enabled": true,
        "human_ticket_default_approval_policy": "human_before_execute",
        "hook_ticket_default_approval_policy": "plan_only"
    })
}

fn update_config_notifications(mut config: Value, enabled: bool) -> Value {
    if !config.is_object() {
        config = default_config_payload();
    }
    if let Some(object) = config.as_object_mut() {
        object.insert("notifications_enabled".to_string(), Value::Bool(enabled));
    }
    config
}

fn update_config_fields(
    mut config: Value,
    notifications_enabled: Option<bool>,
    human_ticket_default_approval_policy: Option<String>,
    hook_ticket_default_approval_policy: Option<String>,
) -> Value {
    if !config.is_object() {
        config = default_config_payload();
    }
    if let Some(object) = config.as_object_mut() {
        if let Some(enabled) = notifications_enabled {
            object.insert("notifications_enabled".to_string(), Value::Bool(enabled));
        }
        if let Some(policy) = human_ticket_default_approval_policy {
            object.insert(
                "human_ticket_default_approval_policy".to_string(),
                Value::String(policy),
            );
        }
        if let Some(policy) = hook_ticket_default_approval_policy {
            object.insert(
                "hook_ticket_default_approval_policy".to_string(),
                Value::String(policy),
            );
        }
    }
    config
}

enum CommandOutput {
    Json(Value),
    Text(String),
}

fn format_status(status: &Value) -> String {
    let service = scalar_to_string(status.get("service")).unwrap_or_else(|| "tea".to_string());
    let status_text =
        scalar_to_string(status.get("status")).unwrap_or_else(|| "unknown".to_string());
    let mut lines = vec![
        format!("Service: {service}"),
        format!("Status: {status_text}"),
    ];

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

async fn run_ticket_command(
    client: &ApiClient,
    command: TicketCommand,
) -> anyhow::Result<CommandOutput> {
    match command {
        TicketCommand::Create(args) => Ok(CommandOutput::Json(
            client.post("/v1/tickets", args.into_request_body()).await?,
        )),
        TicketCommand::Edit(args) => {
            let (ticket_id, body) = args.into_id_and_request_body();
            Ok(CommandOutput::Json(
                client
                    .patch(&format!("/v1/tickets/{ticket_id}"), body)
                    .await?,
            ))
        }
        TicketCommand::List => Ok(CommandOutput::Json(client.get("/v1/tickets").await?)),
        TicketCommand::Show { ticket_id } => Ok(CommandOutput::Json(
            client.get(&format!("/v1/tickets/{ticket_id}")).await?,
        )),
        TicketCommand::Comment { ticket_id, body } => Ok(CommandOutput::Json(
            client
                .post(
                    &format!("/v1/tickets/{ticket_id}/comments"),
                    json!({ "body": body }),
                )
                .await?,
        )),
        TicketCommand::Events { ticket_id } => Ok(CommandOutput::Json(
            client
                .get(&format!("/v1/tickets/{ticket_id}/events"))
                .await?,
        )),
        TicketCommand::Export { ticket_id, format } => match format {
            ExportFormat::Json => Ok(CommandOutput::Json(
                client
                    .get(&format!("/v1/tickets/{ticket_id}/export/json"))
                    .await?,
            )),
            ExportFormat::Markdown => Ok(CommandOutput::Text(
                client
                    .get_text(&format!("/v1/tickets/{ticket_id}/export/markdown"))
                    .await?,
            )),
        },
        TicketCommand::Analyze { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&format!("/v1/tickets/{ticket_id}/analyze"), json!({}))
                .await?,
        )),
        TicketCommand::Analysis { ticket_id } => Ok(CommandOutput::Json(
            client
                .get(&format!("/v1/tickets/{ticket_id}/analysis"))
                .await?,
        )),
        TicketCommand::Decompose { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&format!("/v1/tickets/{ticket_id}/decompose"), json!({}))
                .await?,
        )),
        TicketCommand::Plan { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&format!("/v1/tickets/{ticket_id}/plan"), json!({}))
                .await?,
        )),
        TicketCommand::PlanShow { ticket_id } => Ok(CommandOutput::Json(
            client.get(&format!("/v1/tickets/{ticket_id}/plan")).await?,
        )),
        TicketCommand::Policy { ticket_id, mode } => Ok(CommandOutput::Json(
            client
                .post(
                    &format!("/v1/tickets/{ticket_id}/policy"),
                    json!({ "mode": mode }),
                )
                .await?,
        )),
        TicketCommand::Approve { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&format!("/v1/tickets/{ticket_id}/approve"), json!({}))
                .await?,
        )),
        TicketCommand::Reject { ticket_id, reason } => Ok(CommandOutput::Json(
            client
                .post(
                    &format!("/v1/tickets/{ticket_id}/reject"),
                    json!({ "reason": reason }),
                )
                .await?,
        )),
        TicketCommand::Run { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&format!("/v1/tickets/{ticket_id}/run"), json!({}))
                .await?,
        )),
        TicketCommand::Stop { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&format!("/v1/tickets/{ticket_id}/stop"), json!({}))
                .await?,
        )),
        TicketCommand::Retry { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&format!("/v1/tickets/{ticket_id}/retry"), json!({}))
                .await?,
        )),
        TicketCommand::Accept { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&format!("/v1/tickets/{ticket_id}/accept"), json!({}))
                .await?,
        )),
        TicketCommand::Close { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&format!("/v1/tickets/{ticket_id}/close"), json!({}))
                .await?,
        )),
        TicketCommand::Cancel { ticket_id } => Ok(CommandOutput::Json(
            client
                .post(&format!("/v1/tickets/{ticket_id}/cancel"), json!({}))
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
            http: reqwest::Client::new(),
        }
    }

    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        self.send(self.http.get(self.url(path))).await
    }

    async fn post(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        self.send(self.http.post(self.url(path)).json(&body)).await
    }

    async fn put(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        self.send(self.http.put(self.url(path)).json(&body)).await
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
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            anyhow::bail!("Tea API returned {status}: {body}");
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
    fn format_status_includes_sqlite_schema_metadata() {
        let output = format_status(&json!({
            "service": "tea",
            "status": "ok",
            "store": {
                "backend": "sqlite",
                "schema_version": 1,
                "supported_schema_version": 1
            }
        }));

        assert!(output.contains("Service: tea"));
        assert!(output.contains("Status: ok"));
        assert!(output.contains("Store: sqlite"));
        assert!(output.contains("SQLite schema: 1 (supported: 1)"));
    }

    #[test]
    fn format_status_handles_memory_store_metadata() {
        let output = format_status(&json!({
            "service": "tea",
            "status": "ok",
            "store": {
                "backend": "memory",
                "schema_version": null,
                "supported_schema_version": null
            }
        }));

        assert!(output.contains("Service: tea"));
        assert!(output.contains("Status: ok"));
        assert!(output.contains("Store: memory"));
        assert!(output.contains("SQLite schema: n/a"));
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
    fn update_config_notifications_preserves_policy_fields() {
        let updated = update_config_notifications(
            json!({
                "notifications_enabled": true,
                "human_ticket_default_approval_policy": "human_before_execute",
                "hook_ticket_default_approval_policy": "plan_only"
            }),
            false,
        );

        assert_eq!(updated["notifications_enabled"], false);
        assert_eq!(
            updated["human_ticket_default_approval_policy"],
            "human_before_execute"
        );
        assert_eq!(updated["hook_ticket_default_approval_policy"], "plan_only");
    }

    #[test]
    fn update_config_fields_can_patch_all_local_config_fields() {
        let updated = update_config_fields(
            json!({
                "notifications_enabled": true,
                "human_ticket_default_approval_policy": "human_before_execute",
                "hook_ticket_default_approval_policy": "plan_only"
            }),
            Some(false),
            Some("human_before_completion".to_string()),
            Some("manual_only".to_string()),
        );

        assert_eq!(updated["notifications_enabled"], false);
        assert_eq!(
            updated["human_ticket_default_approval_policy"],
            "human_before_completion"
        );
        assert_eq!(
            updated["hook_ticket_default_approval_policy"],
            "manual_only"
        );
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
