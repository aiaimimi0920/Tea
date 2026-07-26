#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tea_audit::{export_json, render_export_markdown};
use tea_brain::{
    BrainError, DecomposeContext, DecomposeTicketProposal, DecomposeTicketRequest, TeaBrainProvider,
};
use tea_config::{
    encode_local_config, ConfigurationDetails, ConfigurationOwner, ConfigurationOwnership,
    ConfigurationSource, TeaConfiguration,
};
use tea_core::{
    ActorRef, ApprovalPolicy, RunId, Ticket, TicketCreateOptions, TicketEdits, TicketId,
    TicketSource, TicketStatus,
};
use tea_hook::{normalize_hook_intake, HookIntakeRequest};
use tea_loom::LoomClient;
use tea_policy::{evaluate_close, evaluate_run, PolicyDecision, PolicyInput};
use tea_store::{InMemoryTicketStore, StoreError, TicketStore};

#[derive(Debug, Clone)]
pub struct AuthConfig {
    token: String,
}

impl AuthConfig {
    pub fn new(token: String) -> Self {
        Self { token }
    }
}

#[derive(Clone)]
pub struct AppState<
    S = InMemoryTicketStore,
    B = tea_brain::TemplateBrainProvider,
    L = tea_loom::MockLoomClient,
> {
    store: S,
    brain: B,
    loom: L,
    auth: AuthConfig,
    configuration: ConfigurationRuntime,
}

impl<S, B, L> AppState<S, B, L> {
    pub fn new(store: S, brain: B, loom: L, auth: AuthConfig) -> Self {
        Self::new_with_configuration(
            store,
            brain,
            loom,
            auth,
            ConfigurationRuntime::local_for_tests(),
        )
    }

    pub fn new_with_configuration(
        store: S,
        brain: B,
        loom: L,
        auth: AuthConfig,
        configuration: ConfigurationRuntime,
    ) -> Self {
        Self {
            store,
            brain,
            loom,
            auth,
            configuration,
        }
    }
}

#[derive(Clone)]
pub struct ConfigurationRuntime {
    inner: Arc<Mutex<ConfigurationRuntimeState>>,
}

#[derive(Clone)]
struct ConfigurationRuntimeState {
    ownership: ConfigurationOwnership,
    config: TeaConfiguration,
    local_config_path: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct ConfigurationResponse {
    configuration_source: ConfigurationSource,
    configuration: ConfigurationDetails,
    config: TeaConfiguration,
}

impl ConfigurationRuntime {
    pub fn new(ownership: ConfigurationOwnership, config: TeaConfiguration) -> Self {
        Self::new_with_local_path(ownership, config, None)
    }

    pub fn new_with_local_path(
        ownership: ConfigurationOwnership,
        config: TeaConfiguration,
        local_config_path: Option<PathBuf>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ConfigurationRuntimeState {
                ownership,
                config,
                local_config_path,
            })),
        }
    }

    pub fn local_for_tests() -> Self {
        Self::new(
            ConfigurationOwnership {
                source: ConfigurationSource::Local,
                configuration: ConfigurationDetails {
                    owner: ConfigurationOwner::Tea,
                    local_config_path: None,
                    loom_base_url: None,
                    loom_panel_url: None,
                    reason: None,
                },
            },
            TeaConfiguration::default(),
        )
    }

    #[cfg(test)]
    pub fn loom_managed_for_tests(panel_url: impl Into<String>) -> Self {
        Self::new(
            ConfigurationOwnership {
                source: ConfigurationSource::LoomManaged,
                configuration: ConfigurationDetails {
                    owner: ConfigurationOwner::Loom,
                    local_config_path: None,
                    loom_base_url: Some("http://127.0.0.1:8765".to_string()),
                    loom_panel_url: Some(panel_url.into()),
                    reason: None,
                },
            },
            TeaConfiguration::default(),
        )
    }

    fn response(&self) -> Result<ConfigurationResponse, ApiError> {
        let state = self
            .inner
            .lock()
            .map_err(|_| ApiError::internal("configuration lock poisoned"))?;
        Ok(ConfigurationResponse {
            configuration_source: state.ownership.source,
            configuration: state.ownership.configuration.clone(),
            config: state.config.clone(),
        })
    }

    fn replace_local_config(
        &self,
        config: TeaConfiguration,
    ) -> Result<ConfigurationResponse, ApiError> {
        validate_tea_configuration(&config)?;
        let mut state = self
            .inner
            .lock()
            .map_err(|_| ApiError::internal("configuration lock poisoned"))?;
        if state.ownership.source == ConfigurationSource::LoomManaged {
            return Err(ApiError::conflict(
                "configuration_managed_by_loom".to_string(),
            ));
        }
        if let Some(path) = &state.local_config_path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    ApiError::internal(format!("create Tea config directory: {error}"))
                })?;
            }
            let encoded = encode_local_config(&config)
                .map_err(|error| ApiError::internal(error.to_string()))?;
            std::fs::write(path, encoded)
                .map_err(|error| ApiError::internal(format!("write Tea config file: {error}")))?;
        }
        state.config = config;
        Ok(ConfigurationResponse {
            configuration_source: state.ownership.source,
            configuration: state.ownership.configuration.clone(),
            config: state.config.clone(),
        })
    }

    pub fn replace_runtime_config_from_loom(
        &self,
        config: TeaConfiguration,
    ) -> Result<ConfigurationResponse, ApiError> {
        validate_tea_configuration(&config)?;
        let mut state = self
            .inner
            .lock()
            .map_err(|_| ApiError::internal("configuration lock poisoned"))?;
        state.config = config;
        Ok(ConfigurationResponse {
            configuration_source: state.ownership.source,
            configuration: state.ownership.configuration.clone(),
            config: state.config.clone(),
        })
    }

    fn default_approval_policy_for_source(
        &self,
        source: TicketSource,
    ) -> Result<ApprovalPolicy, ApiError> {
        let state = self
            .inner
            .lock()
            .map_err(|_| ApiError::internal("configuration lock poisoned"))?;
        let configured = match source {
            TicketSource::Hook => &state.config.hook_ticket_default_approval_policy,
            _ => &state.config.human_ticket_default_approval_policy,
        };
        parse_configured_approval_policy(configured)
    }
}

pub fn router<S, B, L>(state: AppState<S, B, L>) -> Router
where
    S: TicketStore + Clone + Send + Sync + 'static,
    B: TeaBrainProvider + Clone + Send + Sync + 'static,
    L: LoomClient + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/health", get(health))
        .route("/settings", get(settings_page::<S, B, L>))
        .route("/v1/status", get(status::<S, B, L>))
        .route(
            "/v1/configuration",
            get(get_configuration::<S, B, L>).put(put_configuration::<S, B, L>),
        )
        .route(
            "/v1/tickets",
            post(create_ticket::<S, B, L>).get(list_tickets::<S, B, L>),
        )
        // Static segment; registered before the `/:ticket_id` param route (matchit
        // gives static paths priority regardless, so "metrics" is never a ticket id).
        .route("/v1/tickets/metrics", get(ticket_metrics::<S, B, L>))
        .route(
            "/v1/tickets/:ticket_id",
            get(get_ticket::<S, B, L>).patch(edit_ticket::<S, B, L>),
        )
        .route(
            "/v1/tickets/:ticket_id/bundle",
            get(ticket_bundle::<S, B, L>),
        )
        .route(
            "/v1/tickets/:ticket_id/comments",
            get(ticket_comments::<S, B, L>).post(add_comment::<S, B, L>),
        )
        .route(
            "/v1/tickets/:ticket_id/events",
            get(ticket_events::<S, B, L>),
        )
        .route(
            "/v1/tickets/:ticket_id/analyze",
            post(analyze_ticket::<S, B, L>),
        )
        .route(
            "/v1/tickets/:ticket_id/plan",
            get(ticket_plan_record::<S, B, L>).post(plan_ticket::<S, B, L>),
        )
        .route(
            "/v1/tickets/:ticket_id/analysis",
            get(ticket_analysis_record::<S, B, L>),
        )
        .route(
            "/v1/tickets/:ticket_id/decompose",
            post(decompose_ticket::<S, B, L>),
        )
        .route(
            "/v1/tickets/:ticket_id/policy",
            post(update_ticket_policy::<S, B, L>),
        )
        .route(
            "/v1/tickets/:ticket_id/approve",
            post(approve_ticket::<S, B, L>),
        )
        .route(
            "/v1/tickets/:ticket_id/reject",
            post(reject_ticket::<S, B, L>),
        )
        .route("/v1/tickets/:ticket_id/run", post(run_ticket::<S, B, L>))
        .route(
            "/v1/tickets/:ticket_id/stop",
            post(stop_latest_run::<S, B, L>),
        )
        .route(
            "/v1/tickets/:ticket_id/retry",
            post(retry_latest_run::<S, B, L>),
        )
        .route("/v1/tickets/:ticket_id/runs", get(list_runs::<S, B, L>))
        .route("/v1/runs/:run_id/stop", post(stop_run::<S, B, L>))
        .route("/v1/runs/:run_id/retry", post(retry_run::<S, B, L>))
        .route(
            "/v1/tickets/:ticket_id/export/json",
            get(export_ticket_json::<S, B, L>),
        )
        .route(
            "/v1/tickets/:ticket_id/export/markdown",
            get(export_ticket_markdown::<S, B, L>),
        )
        .route("/v1/runs/:run_id", get(get_run::<S, B, L>))
        .route(
            "/v1/tickets/:ticket_id/accept",
            post(accept_ticket::<S, B, L>),
        )
        .route(
            "/v1/tickets/:ticket_id/close",
            post(close_ticket::<S, B, L>),
        )
        .route(
            "/v1/tickets/:ticket_id/cancel",
            post(cancel_ticket::<S, B, L>),
        )
        .route("/v1/intake/hook", post(hook_intake::<S, B, L>))
        .with_state(state)
}

pub fn test_router() -> Router {
    router(AppState::new(
        InMemoryTicketStore::default(),
        tea_brain::TemplateBrainProvider,
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
    ))
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn settings_page<S, A, L>(
    State(state): State<AppState<S, A, L>>,
) -> Result<Html<String>, ApiError> {
    let configuration = state.configuration.response()?;
    Ok(Html(render_settings_page(&configuration)))
}

fn render_settings_page(configuration: &ConfigurationResponse) -> String {
    let source = configuration_source_label(configuration.configuration_source);
    let owner = configuration_owner_label(configuration.configuration.owner);
    let disabled = configuration.configuration_source == ConfigurationSource::LoomManaged;
    let disabled_attr = if disabled { " disabled" } else { "" };
    let loom_panel_url = configuration
        .configuration
        .loom_panel_url
        .as_deref()
        .unwrap_or("");
    let loom_panel = if disabled {
        format!(
            r#"<section class="loom-callout">
                <h2>This Tea configuration is managed by Loom</h2>
                <p>Tea is running as an independent app, but Loom owns this configuration. Use Loom's Tea configuration panel for changes.</p>
                <a class="primary-link" href="{loom_panel_url}">Open Loom Tea settings</a>
            </section>"#,
            loom_panel_url = escape_html(loom_panel_url)
        )
    } else {
        r#"<section class="local-callout">
                <h2>Tea local settings</h2>
                <p>Loom is not managing Tea configuration, so this standalone Tea daemon can edit its local settings.</p>
            </section>"#
            .to_string()
    };
    let reason = configuration
        .configuration
        .reason
        .as_deref()
        .map(|value| {
            format!(
                r#"<p class="muted"><strong>Reason:</strong> {}</p>"#,
                escape_html(value)
            )
        })
        .unwrap_or_default();
    let local_path = configuration
        .configuration
        .local_config_path
        .as_deref()
        .unwrap_or("not configured");
    let loom_base_url = configuration
        .configuration
        .loom_base_url
        .as_deref()
        .unwrap_or("not configured");
    let notifications_checked = if configuration.config.notifications_enabled {
        " checked"
    } else {
        ""
    };

    format!(
        r#"<!doctype html>
<html lang="en" data-configuration-source="{source}">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Tea Settings</title>
  <style>
    :root {{
      color-scheme: dark;
      font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      background: #090d1a;
      color: #edf3ff;
    }}
    body {{
      margin: 0;
      min-height: 100vh;
      background:
        radial-gradient(circle at 20% 20%, rgba(113, 96, 255, 0.30), transparent 34rem),
        radial-gradient(circle at 80% 10%, rgba(45, 212, 191, 0.18), transparent 26rem),
        linear-gradient(135deg, #070b16 0%, #101827 100%);
    }}
    main {{
      box-sizing: border-box;
      width: min(960px, calc(100% - 32px));
      margin: 0 auto;
      padding: 48px 0 64px;
    }}
    .panel, .loom-callout, .local-callout {{
      border: 1px solid rgba(148, 163, 184, 0.22);
      border-radius: 24px;
      background: rgba(15, 23, 42, 0.72);
      box-shadow: 0 24px 80px rgba(0, 0, 0, 0.28);
      backdrop-filter: blur(18px);
      padding: 24px;
      margin-top: 20px;
    }}
    h1 {{
      font-size: clamp(2.1rem, 6vw, 4rem);
      margin: 0 0 10px;
      letter-spacing: -0.05em;
    }}
    h2 {{
      margin-top: 0;
    }}
    .muted {{
      color: #aab8d4;
    }}
    .grid {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
      gap: 14px;
    }}
    label {{
      display: grid;
      gap: 8px;
      margin: 16px 0;
      color: #cbd5e1;
    }}
    input, select {{
      border: 1px solid rgba(148, 163, 184, 0.28);
      border-radius: 14px;
      background: rgba(2, 6, 23, 0.72);
      color: #f8fafc;
      padding: 12px 14px;
      font: inherit;
    }}
    input[disabled], select[disabled] {{
      color: #94a3b8;
      cursor: not-allowed;
      opacity: 0.65;
    }}
    button, .primary-link {{
      border: 0;
      border-radius: 999px;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      background: linear-gradient(135deg, #7c3aed, #06b6d4);
      color: white;
      cursor: pointer;
      font-weight: 700;
      min-height: 44px;
      padding: 0 20px;
      text-decoration: none;
    }}
    button[disabled] {{
      cursor: not-allowed;
      filter: grayscale(0.7);
      opacity: 0.55;
    }}
    code {{
      color: #bfdbfe;
      overflow-wrap: anywhere;
    }}
    #message {{
      min-height: 1.4em;
    }}
  </style>
</head>
<body>
  <main>
    <p class="muted">Standalone Tea configuration</p>
    <h1>Tea Settings</h1>
    <p class="muted">Tea can run as an independent local app. When Loom claims Tea configuration, this page becomes a read-only jump surface.</p>

    <section class="panel">
      <div class="grid">
        <p><strong>Configuration source:</strong><br><code>{source}</code></p>
        <p><strong>Owner:</strong><br><code>{owner}</code></p>
        <p><strong>Local config:</strong><br><code>{local_path}</code></p>
        <p><strong>Loom base URL:</strong><br><code>{loom_base_url}</code></p>
      </div>
      {reason}
    </section>

    {loom_panel}

    <form class="panel" id="settings-form">
      <h2>Editable local config</h2>
      <label>
        <span>Auth token for saving</span>
        <input name="auth_token" type="password" autocomplete="current-password" placeholder="Bearer token required for PUT /v1/configuration"{disabled_attr}>
      </label>
      <label>
        <span>Notifications enabled</span>
        <input name="notifications_enabled" type="checkbox"{notifications_checked}{disabled_attr}>
      </label>
      <label>
        <span>Human ticket default approval policy</span>
        <select name="human_ticket_default_approval_policy"{disabled_attr}>
          {human_options}
        </select>
      </label>
      <label>
        <span>Hook ticket default approval policy</span>
        <select name="hook_ticket_default_approval_policy"{disabled_attr}>
          {hook_options}
        </select>
      </label>
      <button type="submit"{disabled_attr}>Save Tea local settings</button>
      <p id="message" class="muted"></p>
    </form>
  </main>
  <script>
    const form = document.getElementById('settings-form');
    const message = document.getElementById('message');
    form.addEventListener('submit', async (event) => {{
      event.preventDefault();
      if ({disabled_js}) {{
        message.textContent = 'Tea configuration is managed by Loom. Open Loom Tea settings instead.';
        return;
      }}
      const data = new FormData(form);
      const token = String(data.get('auth_token') || '').trim();
      const response = await fetch('/v1/configuration', {{
        method: 'PUT',
        headers: {{
          'content-type': 'application/json',
          ...(token ? {{ authorization: `Bearer ${{token}}` }} : {{}})
        }},
        body: JSON.stringify({{
          notifications_enabled: data.get('notifications_enabled') === 'on',
          human_ticket_default_approval_policy: data.get('human_ticket_default_approval_policy'),
          hook_ticket_default_approval_policy: data.get('hook_ticket_default_approval_policy')
        }})
      }});
      message.textContent = response.ok ? 'Saved Tea local settings.' : `Save failed: ${{await response.text()}}`;
    }});
  </script>
</body>
</html>"#,
        source = source,
        owner = owner,
        local_path = escape_html(local_path),
        loom_base_url = escape_html(loom_base_url),
        reason = reason,
        loom_panel = loom_panel,
        disabled_attr = disabled_attr,
        disabled_js = if disabled { "true" } else { "false" },
        notifications_checked = notifications_checked,
        human_options =
            approval_policy_options(&configuration.config.human_ticket_default_approval_policy),
        hook_options =
            approval_policy_options(&configuration.config.hook_ticket_default_approval_policy),
    )
}

fn approval_policy_options(selected: &str) -> String {
    [
        ("human_before_execute", "Human before execute"),
        ("human_before_completion", "Human before completion"),
        ("manual_only", "Manual only"),
        ("plan_only", "Plan only"),
    ]
    .into_iter()
    .map(|(value, label)| {
        let selected_attr = if selected == value { " selected" } else { "" };
        format!(
            r#"<option value="{}"{}>{}</option>"#,
            escape_html(value),
            selected_attr,
            escape_html(label)
        )
    })
    .collect::<Vec<_>>()
    .join("\n")
}

fn configuration_source_label(source: ConfigurationSource) -> &'static str {
    match source {
        ConfigurationSource::Local => "local",
        ConfigurationSource::LoomManaged => "loom-managed",
        ConfigurationSource::Fallback => "fallback",
    }
}

fn configuration_owner_label(owner: ConfigurationOwner) -> &'static str {
    match owner {
        ConfigurationOwner::Tea => "tea",
        ConfigurationOwner::Loom => "loom",
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn status<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
    A: TeaBrainProvider,
{
    require_auth(&state.auth, &headers)?;
    let store = state.store.store_status().await?;
    let configuration = state.configuration.response()?;
    let brain_provider = state.brain.metadata();
    Ok(Json(json!({
        "service": "tea",
        "status": "ok",
        "store": store,
        "brain_provider": brain_provider,
        "configuration_source": configuration.configuration_source,
        "configuration": configuration.configuration,
    })))
}

async fn get_configuration<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state.auth, &headers)?;
    Ok(Json(json!(state.configuration.response()?)))
}

async fn put_configuration<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Json(request): Json<TeaConfiguration>,
) -> Result<Json<Value>, ApiError> {
    require_auth(&state.auth, &headers)?;
    Ok(Json(json!(state
        .configuration
        .replace_local_config(request)?)))
}

#[derive(Debug, Deserialize)]
pub struct CreateTicketRequest {
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub approval_policy: Option<ApprovalPolicy>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
}

/// Operator edits to a ticket's mutable fields. Absent fields are left
/// unchanged; a present `labels` array replaces operator labels while Tea
/// preserves system-derived labels (`source:`/`policy:`/`context:`).
#[derive(Debug, Deserialize)]
pub struct EditTicketRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct CommentRequest {
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct RejectRequest {
    pub reason: String,
}

#[derive(Debug, Deserialize)]
pub struct PolicyRequest {
    pub mode: ApprovalPolicy,
}

async fn create_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Json(request): Json<CreateTicketRequest>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let policy = match request.approval_policy {
        Some(policy) => policy,
        None => state
            .configuration
            .default_approval_policy_for_source(TicketSource::Human)?,
    };
    let options = TicketCreateOptions {
        priority: request.priority,
        labels: request.labels,
    };
    let ticket = state
        .store
        .create_ticket_with_options(
            request.title,
            request.description,
            TicketSource::Human,
            ActorRef::human("local-user"),
            policy,
            options,
        )
        .await?;
    Ok(Json(json!(ticket)))
}

async fn list_tickets<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    Ok(Json(json!(state.store.list_tickets().await?)))
}

async fn ticket_metrics<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    Ok(Json(json!(state.store.ticket_metrics().await?)))
}

async fn get_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    Ok(Json(json!(
        state
            .store
            .get_ticket(&parse_ticket_id(&ticket_id)?)
            .await?
    )))
}

async fn ticket_bundle<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    Ok(Json(json!(
        state
            .store
            .ticket_bundle(&parse_ticket_id(&ticket_id)?)
            .await?
    )))
}

async fn edit_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
    Json(request): Json<EditTicketRequest>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let edits = TicketEdits {
        title: request.title,
        description: request.description,
        priority: request.priority,
        labels: request.labels,
    };
    let ticket = state
        .store
        .update_ticket_fields(
            &parse_ticket_id(&ticket_id)?,
            ActorRef::human("local-user"),
            edits,
        )
        .await?;
    Ok(Json(json!(ticket)))
}

async fn add_comment<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
    Json(request): Json<CommentRequest>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let comment = state
        .store
        .add_comment(
            &parse_ticket_id(&ticket_id)?,
            ActorRef::human("local-user"),
            request.body,
        )
        .await?;
    Ok(Json(json!(comment)))
}

async fn ticket_comments<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    Ok(Json(json!(
        state
            .store
            .ticket_comments(&parse_ticket_id(&ticket_id)?)
            .await?
    )))
}

async fn ticket_events<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    Ok(Json(json!(
        state
            .store
            .ticket_events(&parse_ticket_id(&ticket_id)?)
            .await?
    )))
}

async fn ticket_analysis_record<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    // Confirm the ticket exists so unknown ids return 404, then return the
    // stored analysis or `null` when no analysis has been generated yet.
    state.store.get_ticket(&ticket_id).await?;
    Ok(Json(json!(state.store.ticket_analysis(&ticket_id).await?)))
}

async fn ticket_plan_record<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    state.store.get_ticket(&ticket_id).await?;
    Ok(Json(json!(state.store.ticket_plan(&ticket_id).await?)))
}

async fn analyze_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
    A: TeaBrainProvider,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    let ticket = state.store.get_ticket(&ticket_id).await?;
    ensure_ticket_mutable_for_api(&ticket, "analyze")?;
    let (_provider, proposal) = request_decomposition_proposal(&state, ticket).await?;
    let analysis = state
        .store
        .set_analysis(
            &ticket_id,
            ActorRef::agent("tea-brain-provider"),
            proposal.analysis,
        )
        .await?;
    Ok(Json(json!(analysis)))
}

async fn plan_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
    A: TeaBrainProvider,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    let ticket = state.store.get_ticket(&ticket_id).await?;
    ensure_ticket_mutable_for_api(&ticket, "plan")?;
    let (_provider, proposal) = request_decomposition_proposal(&state, ticket).await?;
    state
        .store
        .set_analysis(
            &ticket_id,
            ActorRef::agent("tea-brain-provider"),
            proposal.analysis,
        )
        .await?;
    let plan = state
        .store
        .set_plan(
            &ticket_id,
            ActorRef::agent("tea-brain-provider"),
            proposal.plan,
        )
        .await?;
    Ok(Json(json!(plan)))
}

async fn decompose_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
    A: TeaBrainProvider,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    let ticket = state.store.get_ticket(&ticket_id).await?;
    ensure_ticket_mutable_for_api(&ticket, "decompose")?;
    let (provider, proposal) = request_decomposition_proposal(&state, ticket).await?;
    let analysis = state
        .store
        .set_analysis(
            &ticket_id,
            ActorRef::agent("tea-brain-provider"),
            proposal.analysis.clone(),
        )
        .await?;
    let plan = state
        .store
        .set_plan(
            &ticket_id,
            ActorRef::agent("tea-brain-provider"),
            proposal.plan.clone(),
        )
        .await?;
    Ok(Json(json!({
        "provider": provider,
        "proposal_id": proposal.proposal_id,
        "analysis": analysis,
        "plan": plan,
        "requires_human_review": proposal.requires_human_review,
        "notes": proposal.notes
    })))
}

async fn request_decomposition_proposal<S, A, L>(
    state: &AppState<S, A, L>,
    ticket: Ticket,
) -> Result<(tea_brain::BrainProviderMetadata, DecomposeTicketProposal), ApiError>
where
    S: TicketStore,
    A: TeaBrainProvider,
{
    let comments = state.store.ticket_comments(&ticket.id).await?;
    let request = DecomposeTicketRequest::new(ticket, comments, decomposition_context());
    let provider = state.brain.metadata();
    let proposal = state.brain.decompose_ticket(request).await?;
    validate_decomposition_proposal(&proposal)?;
    Ok((provider, proposal))
}

fn decomposition_context() -> DecomposeContext {
    DecomposeContext {
        workspace_root: std::env::current_dir()
            .ok()
            .map(|path| path.display().to_string()),
        platform_mode: "standalone".to_string(),
        requested_by: "tea-api".to_string(),
    }
}

fn validate_decomposition_proposal(proposal: &DecomposeTicketProposal) -> Result<(), ApiError> {
    if proposal.schema_version != 1 {
        return Err(ApiError::bad_gateway(format!(
            "invalid BrainProvider proposal schema_version: {}",
            proposal.schema_version
        )));
    }
    if proposal.proposal_id.trim().is_empty() {
        return Err(ApiError::bad_gateway(
            "invalid BrainProvider proposal: proposal_id is required",
        ));
    }
    if proposal.analysis.intent.trim().is_empty() {
        return Err(ApiError::bad_gateway(
            "invalid BrainProvider proposal: analysis.intent is required",
        ));
    }
    if proposal.analysis.recommended_workflow.trim().is_empty() {
        return Err(ApiError::bad_gateway(
            "invalid BrainProvider proposal: analysis.recommended_workflow is required",
        ));
    }
    if proposal.plan.summary.trim().is_empty() {
        return Err(ApiError::bad_gateway(
            "invalid BrainProvider proposal: plan.summary is required",
        ));
    }
    if proposal.plan.steps.is_empty() {
        return Err(ApiError::bad_gateway(
            "invalid BrainProvider proposal: plan.steps is required",
        ));
    }
    Ok(())
}

async fn update_ticket_policy<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
    Json(request): Json<PolicyRequest>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket = state
        .store
        .set_approval_policy(
            &parse_ticket_id(&ticket_id)?,
            ActorRef::human("local-user"),
            request.mode,
        )
        .await?;
    Ok(Json(json!(ticket)))
}

async fn approve_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket = state
        .store
        .grant_approval(&parse_ticket_id(&ticket_id)?, ActorRef::human("local-user"))
        .await?;
    Ok(Json(json!(ticket)))
}

async fn reject_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
    Json(request): Json<RejectRequest>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket = state
        .store
        .reject_approval(
            &parse_ticket_id(&ticket_id)?,
            ActorRef::human("local-user"),
            request.reason,
        )
        .await?;
    Ok(Json(json!(ticket)))
}

async fn run_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
    L: LoomClient,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    let ticket = state.store.get_ticket(&ticket_id).await?;
    ensure_ticket_can_run_for_api(&ticket)?;
    let has_approval = state.store.has_approval(&ticket_id).await?;
    match evaluate_run(&PolicyInput {
        source: ticket.source,
        risk_level: ticket.risk_level,
        approval_policy: ticket.approval_policy,
        has_approval,
        has_evidence: false,
    }) {
        PolicyDecision::Allow => {
            let run = state.loom.start_run(&ticket).await?;
            let run = state
                .store
                .add_run(&ticket_id, ActorRef::loom("tea-loom"), run)
                .await?;
            Ok(Json(json!(run)))
        }
        PolicyDecision::RequestApproval { reason } => Err(ApiError::forbidden(reason)),
        PolicyDecision::Deny { reason } => Err(ApiError::forbidden(reason)),
    }
}

async fn stop_latest_run<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
    L: LoomClient,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    let ticket = state.store.get_ticket(&ticket_id).await?;
    ensure_ticket_mutable_for_api(&ticket, "stop latest run for")?;
    let latest = latest_run(&state.store, &ticket_id).await?;
    let stopped = state.loom.stop_run(&latest).await?;
    ensure_loom_run_action_response_matches(&latest, &stopped)?;
    let updated = state
        .store
        .update_run_status(
            &stopped.id,
            &ticket_id,
            ActorRef::loom("tea-loom"),
            stopped.status,
        )
        .await?;
    Ok(Json(json!(updated)))
}

async fn retry_latest_run<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
    L: LoomClient,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    let ticket = state.store.get_ticket(&ticket_id).await?;
    ensure_ticket_mutable_for_api(&ticket, "retry latest run for")?;
    let latest = latest_run(&state.store, &ticket_id).await?;
    let retrying = state.loom.retry_run(&latest).await?;
    ensure_loom_run_action_response_matches(&latest, &retrying)?;
    let updated = state
        .store
        .update_run_status(
            &retrying.id,
            &ticket_id,
            ActorRef::loom("tea-loom"),
            retrying.status,
        )
        .await?;
    Ok(Json(json!(updated)))
}

async fn stop_run<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
    L: LoomClient,
{
    require_auth(&state.auth, &headers)?;
    let run = state.store.get_run(&parse_run_id(&run_id)?).await?;
    let ticket = state.store.get_ticket(&run.ticket_id).await?;
    ensure_ticket_mutable_for_api(&ticket, "stop run for")?;
    let stopped = state.loom.stop_run(&run).await?;
    ensure_loom_run_action_response_matches(&run, &stopped)?;
    let updated = state
        .store
        .update_run_status(
            &stopped.id,
            &run.ticket_id,
            ActorRef::loom("tea-loom"),
            stopped.status,
        )
        .await?;
    Ok(Json(json!(updated)))
}

async fn retry_run<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
    L: LoomClient,
{
    require_auth(&state.auth, &headers)?;
    let run = state.store.get_run(&parse_run_id(&run_id)?).await?;
    let ticket = state.store.get_ticket(&run.ticket_id).await?;
    ensure_ticket_mutable_for_api(&ticket, "retry run for")?;
    let retrying = state.loom.retry_run(&run).await?;
    ensure_loom_run_action_response_matches(&run, &retrying)?;
    let updated = state
        .store
        .update_run_status(
            &retrying.id,
            &run.ticket_id,
            ActorRef::loom("tea-loom"),
            retrying.status,
        )
        .await?;
    Ok(Json(json!(updated)))
}

async fn list_runs<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    Ok(Json(json!(
        state.store.list_runs(&parse_ticket_id(&ticket_id)?).await?
    )))
}

async fn get_run<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    Ok(Json(json!(
        state.store.get_run(&parse_run_id(&run_id)?).await?
    )))
}

async fn export_ticket_json<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    let ticket = state.store.get_ticket(&ticket_id).await?;
    let events = state.store.ticket_events(&ticket_id).await?;
    let runs = state.store.list_runs(&ticket_id).await?;
    let comments = state.store.ticket_comments(&ticket_id).await?;
    let analysis = state.store.ticket_analysis(&ticket_id).await?;
    let plan = state.store.ticket_plan(&ticket_id).await?;
    Ok(Json(export_json(
        &ticket,
        &events,
        &runs,
        &comments,
        analysis.as_ref(),
        plan.as_ref(),
    )))
}

async fn export_ticket_markdown<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Response, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    let ticket = state.store.get_ticket(&ticket_id).await?;
    let events = state.store.ticket_events(&ticket_id).await?;
    let runs = state.store.list_runs(&ticket_id).await?;
    let comments = state.store.ticket_comments(&ticket_id).await?;
    let analysis = state.store.ticket_analysis(&ticket_id).await?;
    let plan = state.store.ticket_plan(&ticket_id).await?;
    Ok((
        [("content-type", "text/markdown; charset=utf-8")],
        render_export_markdown(
            &ticket,
            &events,
            &runs,
            &comments,
            analysis.as_ref(),
            plan.as_ref(),
        ),
    )
        .into_response())
}

async fn accept_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket = state
        .store
        .accept_ticket(&parse_ticket_id(&ticket_id)?, ActorRef::human("local-user"))
        .await?;
    Ok(Json(json!(ticket)))
}

async fn close_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    let ticket = state.store.get_ticket(&ticket_id).await?;
    ensure_ticket_mutable_for_api(&ticket, "close")?;
    let has_approval = state.store.has_approval(&ticket_id).await?;
    let has_evidence = state
        .store
        .list_runs(&ticket_id)
        .await?
        .into_iter()
        .any(|run| run.evidence.is_some());
    match evaluate_close(&PolicyInput {
        source: ticket.source,
        risk_level: ticket.risk_level,
        approval_policy: ticket.approval_policy,
        has_approval,
        has_evidence,
    }) {
        PolicyDecision::Allow => {}
        PolicyDecision::RequestApproval { reason } => return Err(ApiError::forbidden(reason)),
        PolicyDecision::Deny { reason } => return Err(ApiError::forbidden(reason)),
    }
    let ticket = state
        .store
        .close_ticket(&ticket_id, ActorRef::human("local-user"))
        .await?;
    Ok(Json(json!(ticket)))
}

async fn cancel_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket = state
        .store
        .cancel_ticket(&parse_ticket_id(&ticket_id)?, ActorRef::human("local-user"))
        .await?;
    Ok(Json(json!(ticket)))
}

async fn hook_intake<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Json(request): Json<HookIntakeRequest>,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let normalized = normalize_hook_intake(&request);
    let policy = state
        .configuration
        .default_approval_policy_for_source(normalized.source)?;
    let ticket = state
        .store
        .create_ticket_with_policy(
            normalized.title,
            normalized.description,
            normalized.source,
            normalized.actor,
            policy,
        )
        .await?;
    Ok(Json(json!(ticket)))
}

async fn latest_run<S>(store: &S, ticket_id: &TicketId) -> Result<tea_core::Run, ApiError>
where
    S: TicketStore,
{
    store
        .list_runs(ticket_id)
        .await?
        .into_iter()
        .last()
        .ok_or_else(|| ApiError::not_found("run not found".to_string()))
}

fn ensure_ticket_mutable_for_api(ticket: &Ticket, action: &str) -> Result<(), ApiError> {
    if matches!(
        ticket.status,
        TicketStatus::Closed | TicketStatus::Cancelled
    ) {
        return Err(ApiError::conflict(format!(
            "invalid ticket transition: cannot {action} ticket {} in {:?} status",
            ticket.id, ticket.status
        )));
    }
    Ok(())
}

fn ensure_ticket_can_run_for_api(ticket: &Ticket) -> Result<(), ApiError> {
    ensure_ticket_mutable_for_api(ticket, "run")?;
    if ticket.status == TicketStatus::Blocked {
        return Err(ApiError::conflict(format!(
            "invalid ticket transition: cannot run ticket {} in {:?} status",
            ticket.id, ticket.status
        )));
    }
    Ok(())
}

fn ensure_loom_run_action_response_matches(
    expected: &tea_core::Run,
    actual: &tea_core::Run,
) -> Result<(), ApiError> {
    if actual.id != expected.id || actual.ticket_id != expected.ticket_id {
        return Err(ApiError::conflict(format!(
            "Loom run action returned mismatched run: expected run {} for ticket {}, got run {} for ticket {}",
            expected.id, expected.ticket_id, actual.id, actual.ticket_id
        )));
    }
    Ok(())
}

fn require_auth(auth: &AuthConfig, headers: &HeaderMap) -> Result<(), ApiError> {
    let expected = format!("Bearer {}", auth.token);
    match headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
    {
        Some(actual) if actual == expected => Ok(()),
        _ => Err(ApiError::unauthorized("missing or invalid bearer token")),
    }
}

fn parse_ticket_id(value: &str) -> Result<TicketId, ApiError> {
    TicketId::from_str(value).map_err(|error| ApiError::bad_request(error.to_string()))
}

fn parse_run_id(value: &str) -> Result<RunId, ApiError> {
    RunId::from_str(value).map_err(|error| ApiError::bad_request(error.to_string()))
}

fn parse_configured_approval_policy(value: &str) -> Result<ApprovalPolicy, ApiError> {
    serde_json::from_value(Value::String(value.to_string())).map_err(|_| {
        ApiError::bad_request(format!(
            "invalid approval policy in Tea configuration: {value}"
        ))
    })
}

fn validate_tea_configuration(config: &TeaConfiguration) -> Result<(), ApiError> {
    parse_configured_approval_policy(&config.human_ticket_default_approval_policy)?;
    parse_configured_approval_policy(&config.hook_ticket_default_approval_policy)?;
    Ok(())
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: String) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message,
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    fn forbidden(message: String) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message,
        }
    }

    fn conflict(message: String) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message,
        }
    }

    fn not_found(message: String) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message,
        }
    }

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::TicketNotFound | StoreError::RunNotFound => {
                Self::not_found(error.to_string())
            }
            StoreError::EvidenceRequired => Self::forbidden(error.to_string()),
            StoreError::InvalidTransition(_) => Self::conflict(error.to_string()),
            StoreError::LockPoisoned
            | StoreError::Database(_)
            | StoreError::Codec(_)
            | StoreError::Io(_)
            | StoreError::UnsupportedSchemaVersion { .. } => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: error.to_string(),
            },
        }
    }
}

impl From<BrainError> for ApiError {
    fn from(error: BrainError) -> Self {
        Self::bad_gateway(error.to_string())
    }
}

impl From<tea_loom::LoomError> for ApiError {
    fn from(error: tea_loom::LoomError) -> Self {
        Self::bad_gateway(error.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: self.message,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tea_store::SqliteTicketStore;
    use tower::Service;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_ok() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn v1_ticket_metrics_aggregates_in_one_response() {
        let store = InMemoryTicketStore::default();
        let ticket = store
            .create_ticket(
                "Metrics".to_string(),
                "body".to_string(),
                tea_core::TicketSource::Human,
                tea_core::ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        store
            .add_comment(
                &ticket.id,
                tea_core::ActorRef::human("vmjcv"),
                "note".to_string(),
            )
            .await
            .unwrap();

        let app = router(AppState::new(
            store.clone(),
            tea_brain::TemplateBrainProvider,
            tea_loom::MockLoomClient,
            AuthConfig::new("dev-token".to_string()),
        ));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/tickets/metrics")
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let entries = body.as_array().expect("metrics is an array");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["ticket_id"], json!(ticket.id));
        assert_eq!(entries[0]["comments_count"], 1);
        assert_eq!(entries[0]["runs_count"], 0);
        assert_eq!(entries[0]["latest_comment"]["body"], "note");
    }

    #[tokio::test]
    async fn v1_ticket_bundle_returns_detail_in_one_response() {
        let store = InMemoryTicketStore::default();
        let ticket = store
            .create_ticket(
                "Bundle".to_string(),
                "body".to_string(),
                tea_core::TicketSource::Human,
                tea_core::ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        store
            .add_comment(
                &ticket.id,
                tea_core::ActorRef::human("vmjcv"),
                "note".to_string(),
            )
            .await
            .unwrap();

        let app = router(AppState::new(
            store.clone(),
            tea_brain::TemplateBrainProvider,
            tea_loom::MockLoomClient,
            AuthConfig::new("dev-token".to_string()),
        ));
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/tickets/{}/bundle", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["ticket"]["id"], json!(ticket.id));
        assert_eq!(
            body["comments"].as_array().expect("comments array").len(),
            1
        );
        assert_eq!(body["comments"][0]["body"], "note");
        assert!(!body["events"].as_array().expect("events array").is_empty());
        assert!(body["analysis"].is_null());
        assert!(body["plan"].is_null());
    }

    #[tokio::test]
    async fn status_reports_memory_store_metadata() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/status")
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["service"], "tea");
        assert_eq!(body["status"], "ok");
        assert_eq!(
            body["store"],
            json!({
                "backend": "memory",
                "schema_version": null,
                "supported_schema_version": null
            })
        );
    }

    #[tokio::test]
    async fn v1_status_requires_auth() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn v1_read_endpoints_require_auth() {
        let mut app = test_router();
        let ticket_response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "title": "Auth read smoke",
                            "description": "Verify read endpoints require bearer auth."
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ticket_response.status(), StatusCode::OK);
        let ticket: tea_core::Ticket =
            serde_json::from_slice(&body_bytes(ticket_response).await).unwrap();

        for uri in [
            "/v1/configuration".to_string(),
            "/v1/tickets".to_string(),
            format!("/v1/tickets/{}", ticket.id),
            format!("/v1/tickets/{}/comments", ticket.id),
            format!("/v1/tickets/{}/events", ticket.id),
            format!("/v1/tickets/{}/runs", ticket.id),
            format!("/v1/tickets/{}/export/json", ticket.id),
            format!("/v1/tickets/{}/export/markdown", ticket.id),
        ] {
            let response = app
                .call(Request::builder().uri(&uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "uri={uri}");
        }
    }

    #[tokio::test]
    async fn authenticated_v1_read_endpoints_still_work() {
        let mut app = test_router();
        let ticket_response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "title": "Authenticated read smoke",
                            "description": "Verify read endpoints still work with bearer auth."
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ticket_response.status(), StatusCode::OK);
        let ticket: tea_core::Ticket =
            serde_json::from_slice(&body_bytes(ticket_response).await).unwrap();

        for uri in [
            "/v1/status".to_string(),
            "/v1/configuration".to_string(),
            "/v1/tickets".to_string(),
            format!("/v1/tickets/{}", ticket.id),
            format!("/v1/tickets/{}/comments", ticket.id),
            format!("/v1/tickets/{}/events", ticket.id),
            format!("/v1/tickets/{}/runs", ticket.id),
            format!("/v1/tickets/{}/export/json", ticket.id),
            format!("/v1/tickets/{}/export/markdown", ticket.id),
        ] {
            let response = app
                .call(
                    Request::builder()
                        .uri(&uri)
                        .header("authorization", "Bearer dev-token")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "uri={uri}");
        }
    }

    #[tokio::test]
    async fn status_reports_sqlite_schema_metadata() {
        let path = temp_store_path("tea-api-status-sqlite");
        let store = SqliteTicketStore::open(&path).unwrap();
        let app = router(AppState::new(
            store,
            tea_brain::TemplateBrainProvider,
            tea_loom::MockLoomClient,
            AuthConfig::new("dev-token".to_string()),
        ));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/status")
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body["store"],
            json!({
                "backend": "sqlite",
                "schema_version": 1,
                "supported_schema_version": 1
            })
        );

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn status_reports_configuration_source() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/status")
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["configuration_source"], "local");
        assert_eq!(body["configuration"]["owner"], "tea");
    }

    #[tokio::test]
    async fn configuration_put_updates_local_config() {
        let mut app = test_router();
        let response = app
            .call(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/configuration")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "notifications_enabled": false,
                            "human_ticket_default_approval_policy": "human_before_completion",
                            "hook_ticket_default_approval_policy": "plan_only"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["configuration_source"], "local");
        assert_eq!(body["config"]["notifications_enabled"], false);
        assert_eq!(
            body["config"]["human_ticket_default_approval_policy"],
            "human_before_completion"
        );
    }

    #[tokio::test]
    async fn configuration_put_rejects_loom_managed_config() {
        let state = AppState::new_with_configuration(
            InMemoryTicketStore::default(),
            tea_brain::TemplateBrainProvider,
            tea_loom::MockLoomClient,
            AuthConfig::new("dev-token".to_string()),
            ConfigurationRuntime::loom_managed_for_tests("loom://settings/tea"),
        );
        let mut app = router(state);

        let response = app
            .call(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/configuration")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "notifications_enabled": false,
                            "human_ticket_default_approval_policy": "human_before_completion",
                            "hook_ticket_default_approval_policy": "plan_only"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"], "configuration_managed_by_loom");
    }

    #[test]
    fn loom_runtime_config_can_replace_startup_snapshot() {
        let runtime = ConfigurationRuntime::loom_managed_for_tests("loom://settings/tea");
        let response = runtime
            .replace_runtime_config_from_loom(TeaConfiguration {
                notifications_enabled: false,
                human_ticket_default_approval_policy: "manual_only".to_string(),
                hook_ticket_default_approval_policy: "plan_only".to_string(),
            })
            .expect("replace Loom runtime config");

        assert_eq!(
            response.configuration_source,
            ConfigurationSource::LoomManaged
        );
        assert!(!response.config.notifications_enabled);
        assert_eq!(
            response.config.human_ticket_default_approval_policy,
            "manual_only"
        );
    }

    #[tokio::test]
    async fn configuration_put_rejects_unknown_approval_policy() {
        let mut app = test_router();
        let response = app
            .call(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/configuration")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "notifications_enabled": true,
                            "human_ticket_default_approval_policy": "not_a_policy",
                            "hook_ticket_default_approval_policy": "plan_only"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body["error"],
            "invalid approval policy in Tea configuration: not_a_policy"
        );
    }

    #[tokio::test]
    async fn settings_page_exposes_local_configuration_ui() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/settings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_text(response).await;
        assert!(body.contains("Tea Settings"));
        assert!(body.contains("data-configuration-source=\"local\""));
        assert!(body.contains("notifications_enabled"));
        assert!(body.contains("human_ticket_default_approval_policy"));
        assert!(body.contains("hook_ticket_default_approval_policy"));
        assert!(body.contains("Save Tea local settings"));
    }

    #[tokio::test]
    async fn settings_page_links_to_loom_when_configuration_is_loom_managed() {
        let state = AppState::new_with_configuration(
            InMemoryTicketStore::default(),
            tea_brain::TemplateBrainProvider,
            tea_loom::MockLoomClient,
            AuthConfig::new("dev-token".to_string()),
            ConfigurationRuntime::loom_managed_for_tests("loom://settings/tea"),
        );
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/settings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_text(response).await;
        assert!(body.contains("data-configuration-source=\"loom-managed\""));
        assert!(body.contains("This Tea configuration is managed by Loom"));
        assert!(body.contains("href=\"loom://settings/tea\""));
        assert!(body.contains("Open Loom Tea settings"));
        assert!(body.contains("disabled"));
    }

    #[tokio::test]
    async fn create_ticket_uses_configured_human_default_policy() {
        let mut app = test_router();
        let config_response = app
            .call(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/configuration")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "notifications_enabled": true,
                            "human_ticket_default_approval_policy": "manual_only",
                            "hook_ticket_default_approval_policy": "plan_only"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(config_response.status(), StatusCode::OK);

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title":"Configured","description":"Use configured policy default"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(ticket.approval_policy, tea_core::ApprovalPolicy::ManualOnly);
        assert!(ticket.labels.contains(&"policy:manual-only".to_string()));
    }

    #[tokio::test]
    async fn create_ticket_honors_requested_approval_policy() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "title": "Explicit policy",
                            "description": "Operator picked a policy on create",
                            "approval_policy": "manual_only"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(ticket.approval_policy, tea_core::ApprovalPolicy::ManualOnly);
    }

    #[tokio::test]
    async fn create_ticket_honors_requested_priority_and_labels() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "title": "Prioritized",
                            "description": "Operator set priority and labels on create",
                            "priority": "high",
                            "labels": ["area:desktop", "  needs-triage  ", "area:desktop", ""]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(ticket.priority, "high");
        assert!(ticket.labels.contains(&"area:desktop".to_string()));
        assert!(ticket.labels.contains(&"needs-triage".to_string()));
        // Trimmed duplicates and blank labels are dropped.
        assert_eq!(
            ticket
                .labels
                .iter()
                .filter(|label| label.as_str() == "area:desktop")
                .count(),
            1
        );
        assert!(!ticket.labels.iter().any(|label| label.is_empty()));
        // Source and policy labels are still present.
        assert!(ticket.labels.iter().any(|label| label == "source:human"));
    }

    #[tokio::test]
    async fn patch_ticket_edits_fields_and_preserves_system_labels() {
        let mut app = test_router();
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "title": "Original title",
                            "description": "Original body",
                            "labels": ["area:auth"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

        let response = app
            .call(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/v1/tickets/{}", created.id))
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "title": "Edited title",
                            "priority": "high",
                            "labels": ["area:desktop", "needs-review"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let edited: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(edited.title, "Edited title");
        // Description was not provided, so it is unchanged.
        assert_eq!(edited.description, "Original body");
        assert_eq!(edited.priority, "high");
        // New operator labels replaced the old ones.
        assert!(edited.labels.iter().any(|label| label == "area:desktop"));
        assert!(edited.labels.iter().any(|label| label == "needs-review"));
        assert!(!edited.labels.iter().any(|label| label == "area:auth"));
        // System labels are preserved.
        assert!(edited.labels.iter().any(|label| label == "source:human"));
        assert!(edited
            .labels
            .iter()
            .any(|label| label.starts_with("policy:")));
    }

    #[tokio::test]
    async fn patch_ticket_rejects_terminal_ticket() {
        let mut app = test_router();
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title": "To cancel", "description": "Will be cancelled"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/cancel", created.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .call(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/v1/tickets/{}", created.id))
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"title": "too late"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn create_ticket_rejects_invalid_approval_policy() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "title": "Bad policy",
                            "description": "Invalid approval policy value",
                            "approval_policy": "not_a_real_policy"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn hook_intake_uses_configured_hook_default_policy_label() {
        let mut app = test_router();
        let config_response = app
            .call(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/configuration")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "notifications_enabled": true,
                            "human_ticket_default_approval_policy": "human_before_execute",
                            "hook_ticket_default_approval_policy": "human_before_execute"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(config_response.status(), StatusCode::OK);

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/intake/hook")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "source":"hook",
                            "text":"Please analyze current failure",
                            "context":{
                                "active_window":"PowerShell",
                                "selection_text":"cargo test failed",
                                "ocr_text":null,
                                "screenshot_ref":null,
                                "cwd":"C:\\repo",
                                "app":"terminal"
                            },
                            "attachments":[]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            ticket.approval_policy,
            tea_core::ApprovalPolicy::HumanBeforeExecute
        );
        assert!(ticket
            .labels
            .contains(&"policy:human-before-execute".to_string()));
        assert!(!ticket.labels.contains(&"policy:plan-only".to_string()));
        assert!(ticket.labels.contains(&"context:untrusted".to_string()));
    }

    #[tokio::test]
    async fn create_ticket_requires_auth() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title":"Smoke","description":"Body"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn create_and_list_ticket() {
        let mut app = test_router();
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title":"Smoke","description":"Create a safe plan"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .call(
                Request::builder()
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn decompose_ticket_stores_analysis_and_plan_from_one_provider_proposal() {
        let observed_store = InMemoryTicketStore::default();
        let state = AppState::new(
            observed_store.clone(),
            tea_brain::TemplateBrainProvider,
            tea_loom::MockLoomClient,
            AuthConfig::new("dev-token".to_string()),
        );
        let mut app = router(state);
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "title": "Decompose",
                            "description": "Use one BrainProvider proposal for analysis and plan."
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/decompose", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["provider"]["capability"], "tea.ticket.decompose.v1");
        assert_eq!(body["analysis"]["intent"], "engineering_work_order");
        assert_eq!(
            body["analysis"]["recommended_workflow"],
            "loom.tea_ticket_decompose.v1"
        );
        assert!(body["plan"]["steps"].as_array().unwrap().len() >= 3);
        assert_eq!(body["plan"]["requires_approval_before_execute"], true);

        let stored_ticket = observed_store.get_ticket(&ticket.id).await.unwrap();
        assert_eq!(stored_ticket.status, TicketStatus::AwaitingApproval);
        let events = observed_store.ticket_events(&ticket.id).await.unwrap();
        assert!(events
            .iter()
            .any(|event| event.kind == tea_core::TicketEventKind::TicketAnalyzed));
        assert!(events
            .iter()
            .any(|event| event.kind == tea_core::TicketEventKind::PlanProposed));
    }

    #[tokio::test]
    async fn analysis_and_plan_records_are_readable_after_decompose() {
        let mut app = test_router();
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "title": "Readable records",
                            "description": "Analysis and plan must be readable after decompose."
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

        // Before decompose: records read back as JSON null, not 404.
        let response = app
            .call(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/tickets/{}/analysis", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body.is_null());

        let response = app
            .call(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/tickets/{}/plan", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body.is_null());

        // Generate the records.
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/decompose", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // After decompose: GET returns the stored analysis and plan.
        let response = app
            .call(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/tickets/{}/analysis", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["intent"], "engineering_work_order");

        let response = app
            .call(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/tickets/{}/plan", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body["steps"].as_array().unwrap().len() >= 3);
    }

    #[tokio::test]
    async fn analysis_and_plan_records_require_auth() {
        let app = test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/tickets/{}/analysis", TicketId::new()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn run_requires_approval() {
        let mut app = test_router();
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title":"Smoke","description":"Create a safe plan"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/run", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn accept_requires_run_evidence() {
        let mut app = test_router();
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title":"Review","description":"Accept only after evidence exists"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/accept", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"], "ticket transition requires evidence");
    }

    #[tokio::test]
    async fn accept_after_run_evidence_succeeds() {
        let mut app = test_router();
        let run = create_approved_ticket_and_run(&mut app).await;

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/accept", run.ticket_id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(ticket.status, TicketStatus::Accepted);
    }

    #[tokio::test]
    async fn close_honors_completion_approval_policy() {
        let store = InMemoryTicketStore::default();
        let ticket = store
            .create_ticket(
                "Completion approval".to_string(),
                "Closing should require a final human decision.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        store
            .set_analysis(
                &ticket.id,
                ActorRef::system(),
                tea_core::TicketAnalysis {
                    intent: "verify completion policy".to_string(),
                    target_components: vec!["tea_api".to_string()],
                    target_paths: vec!["Tea/crates/tea_api/src/lib.rs".to_string()],
                    constraints: vec![],
                    acceptance_criteria: vec!["close requires approval".to_string()],
                    missing_context: vec![],
                    risk_assessment: tea_core::RiskLevel::Low,
                    confidence: 0.9,
                    recommended_policy: tea_core::ApprovalPolicy::HumanBeforeCompletion,
                    recommended_workflow: "manual close".to_string(),
                },
            )
            .await
            .unwrap();
        store
            .add_run(
                &ticket.id,
                ActorRef::loom("test-loom"),
                tea_core::Run {
                    id: RunId::new(),
                    ticket_id: ticket.id.clone(),
                    loom_session_id: Some("test".to_string()),
                    status: tea_core::RunStatus::Succeeded,
                    evidence: Some(tea_core::RunEvidence {
                        summary: "done".to_string(),
                        commands: vec![],
                        artifacts: vec![],
                        risks: vec![],
                    }),
                },
            )
            .await
            .unwrap();

        let observed_store = store.clone();
        let state = AppState::new(
            store,
            tea_brain::TemplateBrainProvider,
            tea_loom::MockLoomClient,
            AuthConfig::new("dev-token".to_string()),
        );
        let mut app = router(state);
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/close", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let after_close_attempt = observed_store.get_ticket(&ticket.id).await.unwrap();
        assert_ne!(after_close_attempt.status, TicketStatus::Closed);
    }

    #[tokio::test]
    async fn hook_intake_creates_plan_only_ticket() {
        let mut app = test_router();
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/intake/hook")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "source":"hook",
                            "text":"Please analyze current failure",
                            "context":{
                                "active_window":"PowerShell",
                                "selection_text":"cargo test failed",
                                "ocr_text":null,
                                "screenshot_ref":null,
                                "cwd":"C:\\repo",
                                "app":"terminal"
                            },
                            "attachments":[]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
        assert!(ticket.labels.contains(&"source:hook".to_string()));
        assert!(ticket.labels.contains(&"policy:plan-only".to_string()));
        assert!(ticket.labels.contains(&"context:untrusted".to_string()));
    }

    #[tokio::test]
    async fn ticket_policy_endpoint_updates_policy_and_appends_event() {
        let mut app = test_router();
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title":"Policy","description":"Override approval policy"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/policy", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"mode":"manual_only"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let updated: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            updated.approval_policy,
            tea_core::ApprovalPolicy::ManualOnly
        );

        let events_response = app
            .call(
                Request::builder()
                    .uri(format!("/v1/tickets/{}/events", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(events_response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(events_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let events: Vec<tea_core::TicketEvent> = serde_json::from_slice(&bytes).unwrap();
        assert!(events
            .iter()
            .any(|event| event.kind == tea_core::TicketEventKind::PolicyUpdated));
    }

    #[tokio::test]
    async fn approve_run_and_close_ticket_with_evidence() {
        let mut app = test_router();
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title":"Smoke","description":"Create a safe plan"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

        let approve_response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/approve", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(approve_response.status(), StatusCode::OK);

        let run_response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/run", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(run_response.status(), StatusCode::OK);

        let close_response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/close", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(close_response.status(), StatusCode::OK);

        let events_response = app
            .call(
                Request::builder()
                    .uri(format!("/v1/tickets/{}/events", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(events_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let events: Vec<tea_core::TicketEvent> = serde_json::from_slice(&bytes).unwrap();
        assert!(events
            .iter()
            .any(|event| event.kind == tea_core::TicketEventKind::ApprovalGranted));
        assert!(events
            .iter()
            .any(|event| event.kind == tea_core::TicketEventKind::RunSucceeded));
        assert!(events
            .iter()
            .any(|event| event.kind == tea_core::TicketEventKind::EvidenceAttached));
        assert!(events
            .iter()
            .any(|event| event.kind == tea_core::TicketEventKind::TicketClosed));
    }

    #[tokio::test]
    async fn run_stop_endpoint_stops_the_addressed_run() {
        let mut app = test_router();
        let run = create_approved_ticket_and_run(&mut app).await;

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/runs/{}/stop", run.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let stopped: tea_core::Run = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(stopped.id, run.id);
        assert_eq!(stopped.ticket_id, run.ticket_id);
        assert_eq!(stopped.status, tea_core::RunStatus::Stopped);
    }

    #[tokio::test]
    async fn run_stop_rejects_mismatched_loom_response_id() {
        use axum::{
            extract::State as AxumState, routing::post, Json as AxumJson, Router as AxumRouter,
        };

        async fn stop_handler(
            AxumState(wrong_run): AxumState<tea_core::Run>,
        ) -> AxumJson<tea_core::Run> {
            let mut run = wrong_run;
            run.status = tea_core::RunStatus::Stopped;
            AxumJson(run)
        }

        let store = InMemoryTicketStore::default();
        let ticket = store
            .create_ticket(
                "Stop mismatch".to_string(),
                "Loom must not redirect run actions to another run.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        let addressed_run = tea_core::Run {
            id: RunId::new(),
            ticket_id: ticket.id.clone(),
            loom_session_id: Some("addressed".to_string()),
            status: tea_core::RunStatus::Running,
            evidence: None,
        };
        let wrong_run = tea_core::Run {
            id: RunId::new(),
            ticket_id: ticket.id.clone(),
            loom_session_id: Some("wrong".to_string()),
            status: tea_core::RunStatus::Running,
            evidence: None,
        };
        store
            .add_run(
                &ticket.id,
                ActorRef::loom("test-loom"),
                addressed_run.clone(),
            )
            .await
            .unwrap();
        store
            .add_run(&ticket.id, ActorRef::loom("test-loom"), wrong_run.clone())
            .await
            .unwrap();

        let loom_app = AxumRouter::new()
            .route(
                &format!("/v1/runs/{}/stop", addressed_run.id),
                post(stop_handler),
            )
            .with_state(wrong_run.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, loom_app).await.unwrap();
        });

        let observed_store = store.clone();
        let state = AppState::new(
            store,
            tea_brain::TemplateBrainProvider,
            tea_loom::HttpLoomClient::new(format!("http://{address}"), None),
            AuthConfig::new("dev-token".to_string()),
        );
        let mut app = router(state);

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/runs/{}/stop", addressed_run.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            observed_store.get_run(&wrong_run.id).await.unwrap().status,
            tea_core::RunStatus::Running
        );
    }

    #[tokio::test]
    async fn run_retry_endpoint_retries_the_addressed_run() {
        let mut app = test_router();
        let run = create_approved_ticket_and_run(&mut app).await;

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/runs/{}/retry", run.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let retrying: tea_core::Run = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(retrying.id, run.id);
        assert_eq!(retrying.ticket_id, run.ticket_id);
        assert_eq!(retrying.status, tea_core::RunStatus::Retrying);
    }

    #[tokio::test]
    async fn run_retry_rejects_mismatched_loom_response_id() {
        use axum::{
            extract::State as AxumState, routing::post, Json as AxumJson, Router as AxumRouter,
        };

        async fn retry_handler(
            AxumState(wrong_run): AxumState<tea_core::Run>,
        ) -> AxumJson<tea_core::Run> {
            let mut run = wrong_run;
            run.status = tea_core::RunStatus::Retrying;
            AxumJson(run)
        }

        let store = InMemoryTicketStore::default();
        let ticket = store
            .create_ticket(
                "Retry mismatch".to_string(),
                "Loom must not redirect run actions to another run.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        let addressed_run = tea_core::Run {
            id: RunId::new(),
            ticket_id: ticket.id.clone(),
            loom_session_id: Some("addressed".to_string()),
            status: tea_core::RunStatus::Running,
            evidence: None,
        };
        let wrong_run = tea_core::Run {
            id: RunId::new(),
            ticket_id: ticket.id.clone(),
            loom_session_id: Some("wrong".to_string()),
            status: tea_core::RunStatus::Running,
            evidence: None,
        };
        store
            .add_run(
                &ticket.id,
                ActorRef::loom("test-loom"),
                addressed_run.clone(),
            )
            .await
            .unwrap();
        store
            .add_run(&ticket.id, ActorRef::loom("test-loom"), wrong_run.clone())
            .await
            .unwrap();

        let loom_app = AxumRouter::new()
            .route(
                &format!("/v1/runs/{}/retry", addressed_run.id),
                post(retry_handler),
            )
            .with_state(wrong_run.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, loom_app).await.unwrap();
        });

        let observed_store = store.clone();
        let state = AppState::new(
            store,
            tea_brain::TemplateBrainProvider,
            tea_loom::HttpLoomClient::new(format!("http://{address}"), None),
            AuthConfig::new("dev-token".to_string()),
        );
        let mut app = router(state);

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/runs/{}/retry", addressed_run.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            observed_store.get_run(&wrong_run.id).await.unwrap().status,
            tea_core::RunStatus::Running
        );
    }

    #[tokio::test]
    async fn closed_ticket_rejects_mutation_with_conflict() {
        let mut app = test_router();
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title":"Closed","description":"Finish and freeze this work order"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

        app.call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/approve", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        app.call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/run", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        app.call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/close", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/comments", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"body":"late mutation"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn cancel_ticket_endpoint_sets_cancelled_and_blocks_mutations() {
        let mut app = test_router();
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title":"Cancel","description":"Cancel this work order"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/cancel", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let cancelled: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(cancelled.status, TicketStatus::Cancelled);

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/comments", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"body":"late mutation"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let events_response = app
            .call(
                Request::builder()
                    .uri(format!("/v1/tickets/{}/events", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(events_response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(events_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let events: Vec<tea_core::TicketEvent> = serde_json::from_slice(&bytes).unwrap();
        assert!(events
            .iter()
            .any(|event| event.kind == tea_core::TicketEventKind::TicketCancelled));
    }

    #[tokio::test]
    async fn closed_ticket_analyze_rejects_before_remote_ai_call() {
        let store = InMemoryTicketStore::default();
        let ticket = create_closed_ticket_in_store(&store, false).await;
        let state = AppState::new(
            store,
            tea_brain::LoomCapabilityBrainProvider::new(
                "http://127.0.0.1:9".to_string(),
                Some("brain-token".to_string()),
            ),
            tea_loom::MockLoomClient,
            AuthConfig::new("dev-token".to_string()),
        );
        let mut app = router(state);

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/analyze", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn closed_ticket_run_rejects_before_remote_loom_call() {
        let store = InMemoryTicketStore::default();
        let ticket = create_closed_ticket_in_store(&store, true).await;
        let state = AppState::new(
            store,
            tea_brain::TemplateBrainProvider,
            tea_loom::HttpLoomClient::new(
                "http://127.0.0.1:9".to_string(),
                Some("loom-token".to_string()),
            ),
            AuthConfig::new("dev-token".to_string()),
        );
        let mut app = router(state);

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/run", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn blocked_ticket_run_rejects_before_remote_loom_call() {
        let store = InMemoryTicketStore::default();
        let ticket = store
            .create_ticket(
                "Blocked".to_string(),
                "Rejected tickets must not start remote Loom runs.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        store
            .set_approval_policy(
                &ticket.id,
                ActorRef::human("vmjcv"),
                ApprovalPolicy::AlwaysAuto,
            )
            .await
            .unwrap();
        store
            .reject_approval(
                &ticket.id,
                ActorRef::human("vmjcv"),
                "Rejected by reviewer".to_string(),
            )
            .await
            .unwrap();
        let state = AppState::new(
            store,
            tea_brain::TemplateBrainProvider,
            tea_loom::HttpLoomClient::new(
                "http://127.0.0.1:9".to_string(),
                Some("loom-token".to_string()),
            ),
            AuthConfig::new("dev-token".to_string()),
        );
        let mut app = router(state);

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/run", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn export_markdown_returns_run_evidence() {
        let mut app = test_router();
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title":"Smoke","description":"Create a safe plan"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

        app.call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/approve", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        app.call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/run", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        let response = app
            .call(
                Request::builder()
                    .uri(format!("/v1/tickets/{}/export/markdown", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("mock loom run completed"));
    }

    #[tokio::test]
    async fn comments_endpoint_and_exports_return_review_comment_bodies() {
        let mut app = test_router();
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title":"Commented","description":"Ticket with review comments"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: Ticket = serde_json::from_slice(&bytes).unwrap();

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/comments", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"body":"Manual review comment must be exportable"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let comments_response = app
            .call(
                Request::builder()
                    .uri(format!("/v1/tickets/{}/comments", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(comments_response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(comments_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let comments: Vec<tea_core::TicketComment> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].body, "Manual review comment must be exportable");

        let export_response = app
            .call(
                Request::builder()
                    .uri(format!("/v1/tickets/{}/export/json", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(export_response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(export_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let exported: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            exported["comments"][0]["body"],
            "Manual review comment must be exportable"
        );

        let markdown_response = app
            .call(
                Request::builder()
                    .uri(format!("/v1/tickets/{}/export/markdown", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(markdown_response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(markdown_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let markdown = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(markdown.contains("## Comments"));
        assert!(markdown.contains("Manual review comment must be exportable"));
    }

    #[tokio::test]
    async fn remote_ai_failure_returns_bad_gateway() {
        let state = AppState::new(
            InMemoryTicketStore::default(),
            tea_brain::LoomCapabilityBrainProvider::new(
                "http://127.0.0.1:9".to_string(),
                Some("brain-token".to_string()),
            ),
            tea_loom::MockLoomClient,
            AuthConfig::new("dev-token".to_string()),
        );
        let mut app = router(state);
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title":"Remote AI","description":"Analyze through remote brain"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/analyze", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn remote_loom_failure_returns_bad_gateway() {
        let state = AppState::new(
            InMemoryTicketStore::default(),
            tea_brain::TemplateBrainProvider,
            tea_loom::HttpLoomClient::new(
                "http://127.0.0.1:9".to_string(),
                Some("loom-token".to_string()),
            ),
            AuthConfig::new("dev-token".to_string()),
        );
        let mut app = router(state);
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title":"Remote Loom","description":"Execute through remote loom"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

        app.call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/approve", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/run", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    async fn create_closed_ticket_in_store(
        store: &InMemoryTicketStore,
        approved: bool,
    ) -> tea_core::Ticket {
        let ticket = store
            .create_ticket(
                "Closed".to_string(),
                "Closed before remote side effects.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        if approved {
            store
                .grant_approval(&ticket.id, ActorRef::human("vmjcv"))
                .await
                .unwrap();
        }
        store
            .add_run(
                &ticket.id,
                ActorRef::loom("test-loom"),
                tea_core::Run {
                    id: RunId::new(),
                    ticket_id: ticket.id.clone(),
                    loom_session_id: Some("test".to_string()),
                    status: tea_core::RunStatus::Succeeded,
                    evidence: Some(tea_core::RunEvidence {
                        summary: "done".to_string(),
                        commands: vec![],
                        artifacts: vec![],
                        risks: vec![],
                    }),
                },
            )
            .await
            .unwrap();
        store
            .close_ticket(&ticket.id, ActorRef::human("vmjcv"))
            .await
            .unwrap()
    }

    async fn create_approved_ticket_and_run(app: &mut Router) -> tea_core::Run {
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title":"Run action","description":"Exercise run-level action endpoint"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

        let approve_response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/approve", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(approve_response.status(), StatusCode::OK);

        let run_response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{}/run", ticket.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(run_response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(run_response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    async fn body_bytes(response: Response) -> axum::body::Bytes {
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
    }

    fn temp_store_path(prefix: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("current time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{nanos}.sqlite", std::process::id()))
    }
}
