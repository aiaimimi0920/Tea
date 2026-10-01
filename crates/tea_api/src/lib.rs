#![forbid(unsafe_code)]

mod run_authorization;
mod settings_page;

use settings_page::render_settings_page;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex, Weak};

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tea_audit::{export_json, render_export_markdown};
use tea_brain::{
    BrainError, DecomposeContext, DecomposeTicketProposal, DecomposeTicketRequest, TeaBrainProvider,
};
use tea_config::{
    read_local_config_file, update_local_config_atomic, write_local_config_atomic, ConfigError,
    ConfigurationDetails, ConfigurationOwner, ConfigurationOwnership, ConfigurationSource,
    TeaConfiguration,
};
use tea_core::{
    ActorRef, ApprovalPolicy, Plan, Run, RunId, RunStatus, Ticket, TicketAnalysis, TicketComment,
    TicketCreateOptions, TicketEdits, TicketEvent, TicketId, TicketSource, TicketStatus,
};
use tea_hook::{normalize_hook_intake, HookIntakeRequest};
use tea_loom::LoomClient;
use tea_policy::{
    evaluate_close, evaluate_run, weakens_approval_policy, PolicyDecision, PolicyInput,
};
use tea_store::{
    IdempotencyRequest, InMemoryTicketStore, StoreError, TicketBundle, TicketPageRequest,
    TicketStore, MAX_TICKET_PAGE_SIZE,
};

const MAX_HTTP_JSON_REQUEST_BYTES: usize = 2 * 1024 * 1024;
const MAX_TICKET_TITLE_BYTES: usize = 1024;
const MAX_TICKET_DESCRIPTION_BYTES: usize = 512 * 1024;
const MAX_TICKET_PRIORITY_BYTES: usize = 128;
const MAX_TICKET_LABELS: usize = 64;
const MAX_TICKET_LABEL_BYTES: usize = 256;
const MAX_COMMENT_BODY_BYTES: usize = 256 * 1024;
const MAX_REJECTION_REASON_BYTES: usize = 64 * 1024;
const MAX_HOOK_SOURCE_BYTES: usize = 256;
const MAX_HOOK_TEXT_BYTES: usize = 256 * 1024;
const MAX_HOOK_CONTEXT_FIELD_BYTES: usize = 128 * 1024;
const MAX_HOOK_ATTACHMENTS: usize = 64;
const MAX_HOOK_ATTACHMENT_KIND_BYTES: usize = 128;
const MAX_HOOK_ATTACHMENT_REFERENCE_BYTES: usize = 4 * 1024;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 255;
const HUMAN_CREATE_IDEMPOTENCY_SCOPE: &str = "human-ticket-create";
const HOOK_CREATE_IDEMPOTENCY_SCOPE: &str = "hook-ticket-create";
const DEFAULT_TICKET_PAGE_SIZE: usize = 50;
const TICKET_CURSOR_PREFIX: &str = "v1-";

#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Precomputed `Bearer {token}` header value so per-request auth checks
    /// need no allocation.
    expected_header: String,
}

impl AuthConfig {
    pub fn new(token: String) -> Self {
        Self {
            expected_header: format!("Bearer {token}"),
        }
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
    run_actions: RunActionCoordinator,
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
            run_actions: RunActionCoordinator::default(),
        }
    }
}

#[derive(Clone, Default)]
struct RunActionCoordinator {
    locks: Arc<Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>>,
}

impl RunActionCoordinator {
    fn try_lock(&self, ticket_id: &TicketId) -> Result<tokio::sync::OwnedMutexGuard<()>, ApiError> {
        let lock = {
            let mut locks = self
                .locks
                .lock()
                .map_err(|_| ApiError::internal("run action lock registry poisoned"))?;
            locks.retain(|_, lock| lock.strong_count() > 0);
            let key = ticket_id.to_string();
            if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(tokio::sync::Mutex::new(()));
                locks.insert(key, Arc::downgrade(&lock));
                lock
            }
        };
        lock.try_lock_owned().map_err(|_| {
            ApiError::conflict(format!(
                "another run action is already in progress for ticket {ticket_id}"
            ))
        })
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

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigurationPatchRequest {
    #[serde(default)]
    notifications_enabled: Option<bool>,
    #[serde(default)]
    human_ticket_default_approval_policy: Option<String>,
    #[serde(default)]
    hook_ticket_default_approval_policy: Option<String>,
}

impl ConfigurationPatchRequest {
    fn is_empty(&self) -> bool {
        self.notifications_enabled.is_none()
            && self.human_ticket_default_approval_policy.is_none()
            && self.hook_ticket_default_approval_policy.is_none()
    }

    fn apply_to(self, mut config: TeaConfiguration) -> TeaConfiguration {
        if let Some(enabled) = self.notifications_enabled {
            config.notifications_enabled = enabled;
        }
        if let Some(policy) = self.human_ticket_default_approval_policy {
            config.human_ticket_default_approval_policy = policy;
        }
        if let Some(policy) = self.hook_ticket_default_approval_policy {
            config.hook_ticket_default_approval_policy = policy;
        }
        config
    }
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
        let mut state = self
            .inner
            .lock()
            .map_err(|_| ApiError::internal("configuration lock poisoned"))?;
        Self::refresh_local_config_locked(&mut state)?;
        Ok(ConfigurationResponse {
            configuration_source: state.ownership.source,
            configuration: state.ownership.configuration.clone(),
            config: state.config.clone(),
        })
    }

    fn patch_local_config(
        &self,
        request: ConfigurationPatchRequest,
    ) -> Result<ConfigurationResponse, ApiError> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| ApiError::internal("configuration lock poisoned"))?;
        if state.ownership.source == ConfigurationSource::LoomManaged {
            return Err(ApiError::conflict(
                "configuration_managed_by_loom".to_string(),
            ));
        }

        let fallback = state.config.clone();
        let config = if let Some(path) = &state.local_config_path {
            update_local_config_atomic(path, fallback, |current| {
                let updated = request.apply_to(current);
                validate_tea_configuration(&updated)
                    .map_err(|error| ConfigError::InvalidUpdate(error.message))?;
                Ok(updated)
            })
            .map_err(|error| match error {
                ConfigError::InvalidUpdate(message) => ApiError::bad_request(message),
                error => ApiError::internal(error.to_string()),
            })?
        } else {
            let updated = request.apply_to(state.config.clone());
            validate_tea_configuration(&updated)?;
            updated
        };
        state.config = config;
        Ok(ConfigurationResponse {
            configuration_source: state.ownership.source,
            configuration: state.ownership.configuration.clone(),
            config: state.config.clone(),
        })
    }

    fn refresh_local_config_locked(state: &mut ConfigurationRuntimeState) -> Result<(), ApiError> {
        if state.ownership.source == ConfigurationSource::LoomManaged {
            return Ok(());
        }
        let Some(path) = &state.local_config_path else {
            return Ok(());
        };
        let Some(config) =
            read_local_config_file(path).map_err(|error| ApiError::internal(error.to_string()))?
        else {
            return Ok(());
        };
        validate_tea_configuration(&config).map_err(|error| {
            ApiError::internal(format!("invalid Tea local config: {}", error.message))
        })?;
        state.config = config;
        Ok(())
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
            write_local_config_atomic(path, &config)
                .map_err(|error| ApiError::internal(error.to_string()))?;
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
        let mut state = self
            .inner
            .lock()
            .map_err(|_| ApiError::internal("configuration lock poisoned"))?;
        Self::refresh_local_config_locked(&mut state)?;
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
            get(get_configuration::<S, B, L>)
                .put(put_configuration::<S, B, L>)
                .patch(patch_configuration::<S, B, L>),
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
        .layer(DefaultBodyLimit::max(MAX_HTTP_JSON_REQUEST_BYTES))
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

async fn status<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError>
where
    S: TicketStore,
    A: TeaBrainProvider,
    L: LoomClient,
{
    require_auth(&state.auth, &headers)?;
    let store = state.store.store_status().await?;
    let configuration = state.configuration.response()?;
    let brain_provider = state.brain.metadata();
    let execution_provider = state.loom.execution_provider();
    Ok(Json(json!({
        "service": "tea",
        "status": "ok",
        "store": store,
        "brain_provider": brain_provider,
        "execution_provider": execution_provider,
        "configuration_source": configuration.configuration_source,
        "configuration": configuration.configuration,
    })))
}

async fn get_configuration<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
) -> Result<Json<ConfigurationResponse>, ApiError> {
    require_auth(&state.auth, &headers)?;
    Ok(Json(state.configuration.response()?))
}

async fn put_configuration<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Json(request): Json<TeaConfiguration>,
) -> Result<Json<ConfigurationResponse>, ApiError> {
    require_auth(&state.auth, &headers)?;
    // `replace_local_config` takes tea_config's cross-process file lock, which
    // can sleep for seconds and fsync; keep that off the async worker threads.
    let configuration = state.configuration.clone();
    let response = run_blocking(move || configuration.replace_local_config(request)).await?;
    Ok(Json(response))
}

async fn patch_configuration<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Json(request): Json<ConfigurationPatchRequest>,
) -> Result<Json<ConfigurationResponse>, ApiError> {
    require_auth(&state.auth, &headers)?;
    if request.is_empty() {
        return Err(ApiError::bad_request(
            "configuration patch must update at least one field".to_string(),
        ));
    }
    // Same blocking file-lock concern as `put_configuration`.
    let configuration = state.configuration.clone();
    let response = run_blocking(move || configuration.patch_local_config(request)).await?;
    Ok(Json(response))
}

/// Runs a blocking closure on tokio's blocking pool, flattening the join error.
async fn run_blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|error| ApiError::internal(format!("blocking task failed: {error}")))?
}

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Default, Deserialize)]
struct TicketListQuery {
    status: Option<String>,
    source: Option<String>,
    limit: Option<String>,
    cursor: Option<String>,
}

impl TicketListQuery {
    fn pagination_requested(&self) -> bool {
        self.status.is_some()
            || self.source.is_some()
            || self.limit.is_some()
            || self.cursor.is_some()
    }
}

#[derive(Debug, Serialize)]
struct TicketPageResponse<T> {
    items: Vec<T>,
    next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TicketCursor {
    ordinal: i64,
    status: Option<TicketStatus>,
    source: Option<TicketSource>,
}

async fn create_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Json(request): Json<CreateTicketRequest>,
) -> Result<Json<Ticket>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    validate_create_ticket_request(&request)?;
    let idempotency = idempotency_request(&headers, HUMAN_CREATE_IDEMPOTENCY_SCOPE, &request)?;
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
        .create_ticket_idempotent(
            request.title,
            request.description,
            TicketSource::Human,
            ActorRef::human("local-user"),
            policy,
            options,
            idempotency,
        )
        .await?;
    Ok(Json(ticket))
}

async fn list_tickets<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Query(query): Query<TicketListQuery>,
) -> Result<Response, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    if !query.pagination_requested() {
        return Ok(Json(state.store.list_tickets().await?).into_response());
    }
    let request = parse_ticket_page_request(&query)?;
    let page = state.store.list_tickets_page(request).await?;
    Ok(Json(TicketPageResponse {
        items: page.items,
        next_cursor: page
            .next_ordinal
            .map(|ordinal| encode_ticket_cursor(ordinal, request.status, request.source)),
    })
    .into_response())
}

async fn ticket_metrics<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Query(query): Query<TicketListQuery>,
) -> Result<Response, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    if !query.pagination_requested() {
        return Ok(Json(state.store.ticket_metrics().await?).into_response());
    }
    let request = parse_ticket_page_request(&query)?;
    let page = state.store.ticket_metrics_page(request).await?;
    Ok(Json(TicketPageResponse {
        items: page.items,
        next_cursor: page
            .next_ordinal
            .map(|ordinal| encode_ticket_cursor(ordinal, request.status, request.source)),
    })
    .into_response())
}

async fn get_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Ticket>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    Ok(Json(
        state
            .store
            .get_ticket(&parse_ticket_id(&ticket_id)?)
            .await?,
    ))
}

async fn ticket_bundle<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<TicketBundle>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    Ok(Json(
        state
            .store
            .ticket_bundle(&parse_ticket_id(&ticket_id)?)
            .await?,
    ))
}

async fn edit_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
    Json(request): Json<EditTicketRequest>,
) -> Result<Json<Ticket>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    validate_edit_ticket_request(&request)?;
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
    Ok(Json(ticket))
}

async fn add_comment<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
    Json(request): Json<CommentRequest>,
) -> Result<Json<TicketComment>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    validate_max_bytes("comment body", &request.body, MAX_COMMENT_BODY_BYTES)?;
    let comment = state
        .store
        .add_comment(
            &parse_ticket_id(&ticket_id)?,
            ActorRef::human("local-user"),
            request.body,
        )
        .await?;
    Ok(Json(comment))
}

async fn ticket_comments<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Vec<TicketComment>>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    Ok(Json(
        state
            .store
            .ticket_comments(&parse_ticket_id(&ticket_id)?)
            .await?,
    ))
}

async fn ticket_events<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Vec<TicketEvent>>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    Ok(Json(
        state
            .store
            .ticket_events(&parse_ticket_id(&ticket_id)?)
            .await?,
    ))
}

async fn ticket_analysis_record<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Option<TicketAnalysis>>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    // Confirm the ticket exists so unknown ids return 404, then return the
    // stored analysis or `null` when no analysis has been generated yet.
    state.store.get_ticket(&ticket_id).await?;
    Ok(Json(state.store.ticket_analysis(&ticket_id).await?))
}

async fn ticket_plan_record<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Option<Plan>>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    state.store.get_ticket(&ticket_id).await?;
    Ok(Json(state.store.ticket_plan(&ticket_id).await?))
}

async fn analyze_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<TicketAnalysis>, ApiError>
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
    Ok(Json(analysis))
}

async fn plan_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Plan>, ApiError>
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
    Ok(Json(plan))
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
    let current_policy = ticket.approval_policy;
    let ticket_source = ticket.source;
    let request = DecomposeTicketRequest::new(ticket, comments, decomposition_context());
    let provider = state.brain.metadata();
    let proposal = state.brain.decompose_ticket(request).await?;
    validate_decomposition_proposal(&proposal, current_policy, ticket_source)?;
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

fn validate_decomposition_proposal(
    proposal: &DecomposeTicketProposal,
    current_policy: ApprovalPolicy,
    ticket_source: TicketSource,
) -> Result<(), ApiError> {
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
    if weakens_approval_policy(current_policy, proposal.analysis.recommended_policy) {
        return Err(ApiError::bad_gateway(format!(
            "invalid BrainProvider proposal: recommended policy {:?} weakens the current {:?} approval gates",
            proposal.analysis.recommended_policy, current_policy
        )));
    }
    let proposed_run_decision = evaluate_run(&PolicyInput {
        source: ticket_source,
        risk_level: proposal.analysis.risk_assessment,
        approval_policy: proposal.analysis.recommended_policy,
        has_approval: false,
        has_evidence: false,
        validation_passed: false,
    });
    if !matches!(proposed_run_decision, PolicyDecision::Allow)
        && !proposal.plan.requires_approval_before_execute
    {
        return Err(ApiError::bad_gateway(
            "invalid BrainProvider proposal: plan omits the approval gate required by its recommended policy",
        ));
    }
    Ok(())
}

async fn update_ticket_policy<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
    Json(request): Json<PolicyRequest>,
) -> Result<Json<Ticket>, ApiError>
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
    Ok(Json(ticket))
}

async fn approve_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Ticket>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket = state
        .store
        .grant_approval(&parse_ticket_id(&ticket_id)?, ActorRef::human("local-user"))
        .await?;
    Ok(Json(ticket))
}

async fn reject_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
    Json(request): Json<RejectRequest>,
) -> Result<Json<Ticket>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    validate_max_bytes(
        "approval rejection reason",
        &request.reason,
        MAX_REJECTION_REASON_BYTES,
    )?;
    let ticket = state
        .store
        .reject_approval(
            &parse_ticket_id(&ticket_id)?,
            ActorRef::human("local-user"),
            request.reason,
        )
        .await?;
    Ok(Json(ticket))
}

async fn run_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Run>, ApiError>
where
    S: TicketStore,
    L: LoomClient,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    let _run_action = state.run_actions.try_lock(&ticket_id)?;
    let ticket = state.store.get_ticket(&ticket_id).await?;
    run_authorization::ensure_authorized(&state.store, &ticket).await?;
    let run = state.loom.start_run(&ticket).await?;
    let run = state
        .store
        .add_run(&ticket_id, ActorRef::loom("tea-loom"), run)
        .await?;
    Ok(Json(run))
}

async fn stop_latest_run<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Run>, ApiError>
where
    S: TicketStore,
    L: LoomClient,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    let _run_action = state.run_actions.try_lock(&ticket_id)?;
    let ticket = state.store.get_ticket(&ticket_id).await?;
    ensure_ticket_mutable_for_api(&ticket, "stop latest run for")?;
    let latest = require_latest_run(&state.store, &ticket_id).await?;
    ensure_run_can_stop_for_api(&latest)?;
    let stopped = state.loom.stop_run(&latest).await?;
    ensure_loom_run_action_response_matches(&latest, &stopped, RunStatus::Stopped)?;
    let updated = state
        .store
        .update_latest_run_if_unchanged(&ticket_id, ActorRef::loom("tea-loom"), latest, stopped)
        .await?;
    Ok(Json(updated))
}

async fn retry_latest_run<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Run>, ApiError>
where
    S: TicketStore,
    L: LoomClient,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    let _run_action = state.run_actions.try_lock(&ticket_id)?;
    let ticket = state.store.get_ticket(&ticket_id).await?;
    ensure_ticket_mutable_for_api(&ticket, "retry latest run for")?;
    let latest = require_latest_run(&state.store, &ticket_id).await?;
    ensure_run_can_retry_for_api(&latest)?;
    run_authorization::ensure_authorized(&state.store, &ticket).await?;
    let retrying = state.loom.retry_run(&latest).await?;
    ensure_loom_run_action_response_matches(&latest, &retrying, RunStatus::Retrying)?;
    let updated = state
        .store
        .update_latest_run_if_unchanged(&ticket_id, ActorRef::loom("tea-loom"), latest, retrying)
        .await?;
    Ok(Json(updated))
}

async fn stop_run<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Json<Run>, ApiError>
where
    S: TicketStore,
    L: LoomClient,
{
    require_auth(&state.auth, &headers)?;
    let run_id = parse_run_id(&run_id)?;
    let initial_run = state.store.get_run(&run_id).await?;
    let _run_action = state.run_actions.try_lock(&initial_run.ticket_id)?;
    let run = state.store.get_run(&run_id).await?;
    let ticket = state.store.get_ticket(&run.ticket_id).await?;
    ensure_ticket_mutable_for_api(&ticket, "stop run for")?;
    ensure_run_can_stop_for_api(&run)?;
    let stopped = state.loom.stop_run(&run).await?;
    ensure_loom_run_action_response_matches(&run, &stopped, RunStatus::Stopped)?;
    let updated = state
        .store
        .update_run_if_unchanged(
            &run.ticket_id,
            ActorRef::loom("tea-loom"),
            run.clone(),
            stopped,
        )
        .await?;
    Ok(Json(updated))
}

async fn retry_run<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Json<Run>, ApiError>
where
    S: TicketStore,
    L: LoomClient,
{
    require_auth(&state.auth, &headers)?;
    let run_id = parse_run_id(&run_id)?;
    let initial_run = state.store.get_run(&run_id).await?;
    let _run_action = state.run_actions.try_lock(&initial_run.ticket_id)?;
    let run = state.store.get_run(&run_id).await?;
    let ticket = state.store.get_ticket(&run.ticket_id).await?;
    ensure_ticket_mutable_for_api(&ticket, "retry run for")?;
    ensure_run_can_retry_for_api(&run)?;
    run_authorization::ensure_authorized(&state.store, &ticket).await?;
    let retrying = state.loom.retry_run(&run).await?;
    ensure_loom_run_action_response_matches(&run, &retrying, RunStatus::Retrying)?;
    let updated = state
        .store
        .update_run_if_unchanged(
            &run.ticket_id,
            ActorRef::loom("tea-loom"),
            run.clone(),
            retrying,
        )
        .await?;
    Ok(Json(updated))
}

async fn list_runs<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Vec<Run>>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    Ok(Json(
        state.store.list_runs(&parse_ticket_id(&ticket_id)?).await?,
    ))
}

async fn get_run<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Json<Run>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    Ok(Json(state.store.get_run(&parse_run_id(&run_id)?).await?))
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
    let bundle = state.store.ticket_bundle(&ticket_id).await?;
    Ok(Json(export_json(
        &bundle.ticket,
        &bundle.events,
        &bundle.runs,
        &bundle.comments,
        bundle.analysis.as_ref(),
        bundle.plan.as_ref(),
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
    let bundle = state.store.ticket_bundle(&ticket_id).await?;
    Ok((
        [("content-type", "text/markdown; charset=utf-8")],
        render_export_markdown(
            &bundle.ticket,
            &bundle.events,
            &bundle.runs,
            &bundle.comments,
            bundle.analysis.as_ref(),
            bundle.plan.as_ref(),
        ),
    )
        .into_response())
}

async fn accept_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Ticket>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket = state
        .store
        .accept_ticket(&parse_ticket_id(&ticket_id)?, ActorRef::human("local-user"))
        .await?;
    Ok(Json(ticket))
}

async fn close_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Ticket>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket_id = parse_ticket_id(&ticket_id)?;
    let ticket = state.store.get_ticket(&ticket_id).await?;
    ensure_ticket_mutable_for_api(&ticket, "close")?;
    let has_approval = state.store.has_approval(&ticket_id).await?;
    let has_evidence = state.store.has_run_evidence(&ticket_id).await?;
    match evaluate_close(&PolicyInput {
        source: ticket.source,
        risk_level: ticket.risk_level,
        approval_policy: ticket.approval_policy,
        has_approval,
        has_evidence,
        validation_passed: false,
    }) {
        PolicyDecision::Allow => {}
        PolicyDecision::RequestApproval { reason } => return Err(ApiError::forbidden(reason)),
        PolicyDecision::Deny { reason } => return Err(ApiError::forbidden(reason)),
    }
    let ticket = state
        .store
        .close_ticket(&ticket_id, ActorRef::human("local-user"))
        .await?;
    Ok(Json(ticket))
}

async fn cancel_ticket<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Path(ticket_id): Path<String>,
) -> Result<Json<Ticket>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    let ticket = state
        .store
        .cancel_ticket(&parse_ticket_id(&ticket_id)?, ActorRef::human("local-user"))
        .await?;
    Ok(Json(ticket))
}

async fn hook_intake<S, A, L>(
    State(state): State<AppState<S, A, L>>,
    headers: HeaderMap,
    Json(request): Json<HookIntakeRequest>,
) -> Result<Json<Ticket>, ApiError>
where
    S: TicketStore,
{
    require_auth(&state.auth, &headers)?;
    validate_hook_intake_request(&request)?;
    let idempotency = idempotency_request(&headers, HOOK_CREATE_IDEMPOTENCY_SCOPE, &request)?;
    let normalized = normalize_hook_intake(&request);
    validate_max_bytes(
        "normalized Hook description",
        &normalized.description,
        MAX_TICKET_DESCRIPTION_BYTES,
    )?;
    let policy = state
        .configuration
        .default_approval_policy_for_source(normalized.source)?;
    let ticket = state
        .store
        .create_ticket_idempotent(
            normalized.title,
            normalized.description,
            normalized.source,
            normalized.actor,
            policy,
            TicketCreateOptions::default(),
            idempotency,
        )
        .await?;
    Ok(Json(ticket))
}

async fn require_latest_run<S>(store: &S, ticket_id: &TicketId) -> Result<Run, ApiError>
where
    S: TicketStore,
{
    store
        .latest_run(ticket_id)
        .await?
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
    if matches!(
        ticket.status,
        TicketStatus::Blocked | TicketStatus::NeedsInfo
    ) {
        return Err(ApiError::conflict(format!(
            "invalid ticket transition: cannot run ticket {} in {:?} status",
            ticket.id, ticket.status
        )));
    }
    Ok(())
}

fn ensure_run_can_stop_for_api(run: &tea_core::Run) -> Result<(), ApiError> {
    if run.status.can_stop() {
        return Ok(());
    }

    Err(ApiError::conflict(format!(
        "invalid run transition: cannot stop run {} in {:?} status",
        run.id, run.status
    )))
}

fn ensure_run_can_retry_for_api(run: &tea_core::Run) -> Result<(), ApiError> {
    if run.status.can_retry() {
        return Ok(());
    }

    Err(ApiError::conflict(format!(
        "invalid run transition: cannot retry run {} in {:?} status",
        run.id, run.status
    )))
}

fn ensure_loom_run_action_response_matches(
    expected: &tea_core::Run,
    actual: &tea_core::Run,
    expected_status: RunStatus,
) -> Result<(), ApiError> {
    if actual.id != expected.id || actual.ticket_id != expected.ticket_id {
        return Err(ApiError::conflict(format!(
            "Loom run action returned mismatched run: expected run {} for ticket {}, got run {} for ticket {}",
            expected.id, expected.ticket_id, actual.id, actual.ticket_id
        )));
    }
    if actual.status != expected_status {
        return Err(ApiError::conflict(format!(
            "Loom run action returned invalid status for run {}: expected {:?}, got {:?}",
            expected.id, expected_status, actual.status
        )));
    }
    Ok(())
}

fn require_auth(auth: &AuthConfig, headers: &HeaderMap) -> Result<(), ApiError> {
    match headers.get("authorization") {
        Some(actual) if constant_time_eq(actual.as_bytes(), auth.expected_header.as_bytes()) => {
            Ok(())
        }
        _ => Err(ApiError::unauthorized("missing or invalid bearer token")),
    }
}

/// Constant-time byte equality for secret comparison: content differences
/// never short-circuit, so timing cannot leak how many leading bytes of the
/// presented token matched. Returning early on a length mismatch is fine
/// because the expected header length is not secret.
fn constant_time_eq(actual: &[u8], expected: &[u8]) -> bool {
    if actual.len() != expected.len() {
        return false;
    }
    actual
        .iter()
        .zip(expected)
        .fold(0u8, |diff, (a, b)| std::hint::black_box(diff | (a ^ b)))
        == 0
}

fn idempotency_request(
    headers: &HeaderMap,
    scope: &str,
    request: &impl Serialize,
) -> Result<Option<IdempotencyRequest>, ApiError> {
    let mut values = headers.get_all("idempotency-key").iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(ApiError::bad_request(
            "Idempotency-Key must be supplied exactly once".to_string(),
        ));
    }
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_IDEMPOTENCY_KEY_BYTES {
        return Err(ApiError::bad_request(format!(
            "Idempotency-Key must contain between 1 and {MAX_IDEMPOTENCY_KEY_BYTES} bytes"
        )));
    }
    if !bytes.iter().all(|byte| (0x21..=0x7e).contains(byte)) {
        return Err(ApiError::bad_request(
            "Idempotency-Key must contain visible ASCII characters without whitespace".to_string(),
        ));
    }
    let request_json = serde_json::to_vec(request)
        .map_err(|error| ApiError::internal(format!("failed to fingerprint request: {error}")))?;
    let request_hash = format!("{:x}", Sha256::digest(request_json));
    let key = std::str::from_utf8(bytes)
        .map_err(|_| ApiError::bad_request("Idempotency-Key must be valid ASCII".to_string()))?;
    Ok(Some(IdempotencyRequest::new(scope, key, request_hash)))
}

fn parse_ticket_id(value: &str) -> Result<TicketId, ApiError> {
    TicketId::from_str(value).map_err(|error| ApiError::bad_request(error.to_string()))
}

fn parse_run_id(value: &str) -> Result<RunId, ApiError> {
    RunId::from_str(value).map_err(|error| ApiError::bad_request(error.to_string()))
}

fn parse_ticket_page_request(query: &TicketListQuery) -> Result<TicketPageRequest, ApiError> {
    let limit = match query.limit.as_deref() {
        Some(value) => value.parse::<usize>().map_err(|_| {
            ApiError::bad_request(format!(
                "ticket page limit must be an integer between 1 and {MAX_TICKET_PAGE_SIZE}"
            ))
        })?,
        None => DEFAULT_TICKET_PAGE_SIZE,
    };
    if !(1..=MAX_TICKET_PAGE_SIZE).contains(&limit) {
        return Err(ApiError::bad_request(format!(
            "ticket page limit must be between 1 and {MAX_TICKET_PAGE_SIZE}"
        )));
    }

    let status = query
        .status
        .as_deref()
        .map(parse_ticket_status_filter)
        .transpose()?;
    let source = query
        .source
        .as_deref()
        .map(parse_ticket_source_filter)
        .transpose()?;
    let cursor = query
        .cursor
        .as_deref()
        .map(decode_ticket_cursor)
        .transpose()?;
    if cursor.is_some_and(|cursor| cursor.status != status || cursor.source != source) {
        return Err(ApiError::bad_request(
            "ticket page cursor does not match the requested filters".to_string(),
        ));
    }

    Ok(TicketPageRequest {
        after_ordinal: cursor.map(|cursor| cursor.ordinal),
        limit,
        status,
        source,
    })
}

fn parse_ticket_status_filter(value: &str) -> Result<TicketStatus, ApiError> {
    serde_json::from_value(Value::String(value.to_string()))
        .map_err(|_| ApiError::bad_request(format!("invalid ticket status filter: {value}")))
}

fn parse_ticket_source_filter(value: &str) -> Result<TicketSource, ApiError> {
    serde_json::from_value(Value::String(value.to_string()))
        .map_err(|_| ApiError::bad_request(format!("invalid ticket source filter: {value}")))
}

fn encode_ticket_cursor(
    ordinal: i64,
    status: Option<TicketStatus>,
    source: Option<TicketSource>,
) -> String {
    format!(
        "{TICKET_CURSOR_PREFIX}{ordinal:016x}-{}-{}",
        cursor_filter_name(status),
        cursor_filter_name(source)
    )
}

fn cursor_filter_name<T: Serialize>(value: Option<T>) -> String {
    value.map_or_else(
        || "0".to_string(),
        |value| {
            serde_json::to_value(value)
                .expect("ticket cursor filters serialize")
                .as_str()
                .expect("ticket cursor filters serialize as strings")
                .to_string()
        },
    )
}

fn decode_ticket_cursor(cursor: &str) -> Result<TicketCursor, ApiError> {
    let Some(encoded) = cursor.strip_prefix(TICKET_CURSOR_PREFIX) else {
        return Err(ApiError::bad_request(
            "invalid ticket page cursor".to_string(),
        ));
    };
    let mut parts = encoded.split('-');
    let ordinal = parts.next().unwrap_or_default();
    let status = parts.next().unwrap_or_default();
    let source = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || ordinal.len() != 16
        || !ordinal
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ApiError::bad_request(
            "invalid ticket page cursor".to_string(),
        ));
    }
    let ordinal = i64::from_str_radix(ordinal, 16)
        .map_err(|_| ApiError::bad_request("invalid ticket page cursor".to_string()))?;
    if ordinal < 0 {
        return Err(ApiError::bad_request(
            "invalid ticket page cursor".to_string(),
        ));
    }
    let status = if status == "0" {
        None
    } else {
        Some(parse_ticket_status_filter(status)?)
    };
    let source = if source == "0" {
        None
    } else {
        Some(parse_ticket_source_filter(source)?)
    };
    Ok(TicketCursor {
        ordinal,
        status,
        source,
    })
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

fn validate_max_bytes(field: &str, value: &str, max_bytes: usize) -> Result<(), ApiError> {
    if value.len() > max_bytes {
        return Err(ApiError::bad_request(format!(
            "{field} must be at most {max_bytes} UTF-8 bytes"
        )));
    }
    Ok(())
}

fn validate_optional_max_bytes(
    field: &str,
    value: Option<&String>,
    max_bytes: usize,
) -> Result<(), ApiError> {
    if let Some(value) = value {
        validate_max_bytes(field, value, max_bytes)?;
    }
    Ok(())
}

fn validate_ticket_labels(labels: &[String]) -> Result<(), ApiError> {
    if labels.len() > MAX_TICKET_LABELS {
        return Err(ApiError::bad_request(format!(
            "ticket labels must contain at most {MAX_TICKET_LABELS} items"
        )));
    }
    for label in labels {
        validate_max_bytes("ticket label", label, MAX_TICKET_LABEL_BYTES)?;
    }
    Ok(())
}

fn validate_create_ticket_request(request: &CreateTicketRequest) -> Result<(), ApiError> {
    validate_max_bytes("ticket title", &request.title, MAX_TICKET_TITLE_BYTES)?;
    validate_max_bytes(
        "ticket description",
        &request.description,
        MAX_TICKET_DESCRIPTION_BYTES,
    )?;
    validate_optional_max_bytes(
        "ticket priority",
        request.priority.as_ref(),
        MAX_TICKET_PRIORITY_BYTES,
    )?;
    validate_ticket_labels(&request.labels)
}

fn validate_edit_ticket_request(request: &EditTicketRequest) -> Result<(), ApiError> {
    validate_optional_max_bytes(
        "ticket title",
        request.title.as_ref(),
        MAX_TICKET_TITLE_BYTES,
    )?;
    validate_optional_max_bytes(
        "ticket description",
        request.description.as_ref(),
        MAX_TICKET_DESCRIPTION_BYTES,
    )?;
    validate_optional_max_bytes(
        "ticket priority",
        request.priority.as_ref(),
        MAX_TICKET_PRIORITY_BYTES,
    )?;
    if let Some(labels) = &request.labels {
        validate_ticket_labels(labels)?;
    }
    Ok(())
}

fn validate_hook_intake_request(request: &HookIntakeRequest) -> Result<(), ApiError> {
    validate_max_bytes("Hook source", &request.source, MAX_HOOK_SOURCE_BYTES)?;
    validate_max_bytes("Hook text", &request.text, MAX_HOOK_TEXT_BYTES)?;

    for (field, value) in [
        ("Hook active_window", request.context.active_window.as_ref()),
        (
            "Hook selection_text",
            request.context.selection_text.as_ref(),
        ),
        ("Hook ocr_text", request.context.ocr_text.as_ref()),
        (
            "Hook screenshot_ref",
            request.context.screenshot_ref.as_ref(),
        ),
        ("Hook cwd", request.context.cwd.as_ref()),
        ("Hook app", request.context.app.as_ref()),
    ] {
        validate_optional_max_bytes(field, value, MAX_HOOK_CONTEXT_FIELD_BYTES)?;
    }

    if request.attachments.len() > MAX_HOOK_ATTACHMENTS {
        return Err(ApiError::bad_request(format!(
            "Hook attachments must contain at most {MAX_HOOK_ATTACHMENTS} items"
        )));
    }
    for attachment in &request.attachments {
        validate_max_bytes(
            "Hook attachment kind",
            &attachment.kind,
            MAX_HOOK_ATTACHMENT_KIND_BYTES,
        )?;
        validate_max_bytes(
            "Hook attachment reference",
            &attachment.reference,
            MAX_HOOK_ATTACHMENT_REFERENCE_BYTES,
        )?;
    }
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

    fn internal(diagnostic: impl std::fmt::Display) -> Self {
        eprintln!("Tea API internal error: {diagnostic}");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal server error".to_string(),
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::TicketNotFound | StoreError::RunNotFound => {
                Self::not_found(error.to_string())
            }
            StoreError::EvidenceRequired | StoreError::ApprovalRequired => {
                Self::forbidden(error.to_string())
            }
            StoreError::InvalidTransition(_)
            | StoreError::InvalidRunTransition(_)
            | StoreError::RunConflict(_)
            | StoreError::IdempotencyConflict => Self::conflict(error.to_string()),
            StoreError::InvalidPageRequest(_) => Self::bad_request(error.to_string()),
            StoreError::LockPoisoned
            | StoreError::Database(_)
            | StoreError::Codec(_)
            | StoreError::Io(_)
            | StoreError::UnsupportedSchemaVersion { .. } => Self::internal(error),
        }
    }
}

impl From<BrainError> for ApiError {
    fn from(error: BrainError) -> Self {
        eprintln!("Tea API BrainProvider error: {error}");
        Self::bad_gateway("BrainProvider unavailable")
    }
}

impl From<tea_loom::LoomError> for ApiError {
    fn from(error: tea_loom::LoomError) -> Self {
        eprintln!("Tea API Loom error: {error}");
        Self::bad_gateway("Loom service unavailable")
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
mod tests;
