#![forbid(unsafe_code)]

use async_trait::async_trait;
use serde::Serialize;
use std::path::Path;
use tea_core::{
    ActorRef, ApprovalPolicy, Plan, Run, RunId, Ticket, TicketAnalysis, TicketComment,
    TicketCreateOptions, TicketEdits, TicketEvent, TicketId, TicketSource, TicketStatus,
};
use thiserror::Error;

mod helpers;
mod memory;
mod retry_authorization;
mod sqlite;

pub use memory::InMemoryTicketStore;
pub use sqlite::SqliteTicketStore;

#[cfg(test)]
pub(crate) use sqlite::{
    applied_sqlite_schema_version, create_sqlite_v1_schema, create_sqlite_v2_schema,
    create_sqlite_v3_schema, ensure_schema_migrations_table, init_sqlite, insert_ticket,
    push_sqlite_event, record_sqlite_schema_version, sqlite_list_tickets_page_sql,
    sqlite_ticket_metrics_page_sql, ticket_page_binds, CURRENT_SQLITE_SCHEMA_VERSION,
};

pub const MAX_TICKET_PAGE_SIZE: usize = 200;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("ticket not found")]
    TicketNotFound,
    #[error("run not found")]
    RunNotFound,
    #[error("ticket transition requires evidence")]
    EvidenceRequired,
    #[error("ticket transition requires approval")]
    ApprovalRequired,
    #[error("invalid ticket transition: {0}")]
    InvalidTransition(String),
    #[error("invalid run transition: {0}")]
    InvalidRunTransition(String),
    #[error("run {0} changed while the action was in flight")]
    RunConflict(RunId),
    #[error("idempotency key was already used with a different request")]
    IdempotencyConflict,
    #[error("store lock poisoned")]
    LockPoisoned,
    #[error("store database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("store serialization error: {0}")]
    Codec(#[from] serde_json::Error),
    #[error("store io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid ticket page request: {0}")]
    InvalidPageRequest(String),
    #[error("unsupported sqlite schema version {found}; this binary supports up to {supported}")]
    UnsupportedSchemaVersion { found: i64, supported: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoreStatus {
    pub backend: StoreBackend,
    pub schema_version: Option<i64>,
    pub supported_schema_version: Option<i64>,
    pub idempotency_key_count: Option<u64>,
    pub sqlite_page_count: Option<u64>,
    pub sqlite_freelist_count: Option<u64>,
}

impl StoreStatus {
    pub(crate) fn memory() -> Self {
        Self {
            backend: StoreBackend::Memory,
            schema_version: None,
            supported_schema_version: None,
            idempotency_key_count: None,
            sqlite_page_count: None,
            sqlite_freelist_count: None,
        }
    }

    pub(crate) fn sqlite(
        schema_version: i64,
        supported_schema_version: i64,
        idempotency_key_count: u64,
        sqlite_page_count: u64,
        sqlite_freelist_count: u64,
    ) -> Self {
        Self {
            backend: StoreBackend::Sqlite,
            schema_version: Some(schema_version),
            supported_schema_version: Some(supported_schema_version),
            idempotency_key_count: Some(idempotency_key_count),
            sqlite_page_count: Some(sqlite_page_count),
            sqlite_freelist_count: Some(sqlite_freelist_count),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreBackend {
    Memory,
    Sqlite,
}

/// Aggregated per-ticket counts and latest activity, computed server-side so the
/// UI can render its issue list without fetching every ticket's comments/events/runs
/// individually (avoids a 3N request fan-out on each poll).
#[derive(Debug, Clone, Serialize)]
pub struct TicketMetrics {
    pub ticket_id: TicketId,
    pub comments_count: usize,
    pub runs_count: usize,
    pub latest_comment: Option<TicketComment>,
    pub latest_event: Option<TicketEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TicketPageRequest {
    pub after_ordinal: Option<i64>,
    pub limit: usize,
    pub status: Option<TicketStatus>,
    pub source: Option<TicketSource>,
}

#[derive(Debug, Clone)]
pub struct TicketListPage {
    pub items: Vec<Ticket>,
    pub next_ordinal: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TicketMetricsPage {
    pub items: Vec<TicketMetrics>,
    pub next_ordinal: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyRequest {
    pub(crate) scope: String,
    pub(crate) key: String,
    pub(crate) request_hash: String,
}

impl IdempotencyRequest {
    pub fn new(
        scope: impl Into<String>,
        key: impl Into<String>,
        request_hash: impl Into<String>,
    ) -> Self {
        Self {
            scope: scope.into(),
            key: key.into(),
            request_hash: request_hash.into(),
        }
    }
}

/// Everything needed to render a ticket detail view, fetched in a single request
/// so the UI does not fan out into one call each for comments/events/runs/analysis/plan.
#[derive(Debug, Clone, Serialize)]
pub struct TicketBundle {
    pub ticket: Ticket,
    pub comments: Vec<TicketComment>,
    pub events: Vec<TicketEvent>,
    pub runs: Vec<Run>,
    pub analysis: Option<TicketAnalysis>,
    pub plan: Option<Plan>,
}

#[async_trait]
pub trait TicketStore: Send + Sync {
    async fn store_status(&self) -> Result<StoreStatus, StoreError>;

    async fn create_ticket(
        &self,
        title: String,
        description: String,
        source: TicketSource,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        self.create_ticket_with_policy(
            title,
            description,
            source,
            actor,
            Ticket::default_approval_policy_for_source(source),
        )
        .await
    }

    async fn create_ticket_with_policy(
        &self,
        title: String,
        description: String,
        source: TicketSource,
        actor: ActorRef,
        approval_policy: ApprovalPolicy,
    ) -> Result<Ticket, StoreError> {
        self.create_ticket_with_options(
            title,
            description,
            source,
            actor,
            approval_policy,
            TicketCreateOptions::default(),
        )
        .await
    }

    async fn create_ticket_with_options(
        &self,
        title: String,
        description: String,
        source: TicketSource,
        actor: ActorRef,
        approval_policy: ApprovalPolicy,
        options: TicketCreateOptions,
    ) -> Result<Ticket, StoreError> {
        self.create_ticket_idempotent(
            title,
            description,
            source,
            actor,
            approval_policy,
            options,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_ticket_idempotent(
        &self,
        title: String,
        description: String,
        source: TicketSource,
        actor: ActorRef,
        approval_policy: ApprovalPolicy,
        options: TicketCreateOptions,
        idempotency: Option<IdempotencyRequest>,
    ) -> Result<Ticket, StoreError>;

    async fn list_tickets(&self) -> Result<Vec<Ticket>, StoreError>;
    async fn list_tickets_page(
        &self,
        request: TicketPageRequest,
    ) -> Result<TicketListPage, StoreError>;
    /// Per-ticket aggregated metrics for every ticket, in `list_tickets` order.
    async fn ticket_metrics(&self) -> Result<Vec<TicketMetrics>, StoreError>;
    async fn ticket_metrics_page(
        &self,
        request: TicketPageRequest,
    ) -> Result<TicketMetricsPage, StoreError>;
    /// Full detail view for one ticket, read under a single lock.
    async fn ticket_bundle(&self, id: &TicketId) -> Result<TicketBundle, StoreError>;
    async fn get_ticket(&self, id: &TicketId) -> Result<Ticket, StoreError>;
    async fn ticket_events(&self, id: &TicketId) -> Result<Vec<TicketEvent>, StoreError>;
    async fn ticket_comments(&self, id: &TicketId) -> Result<Vec<TicketComment>, StoreError>;
    async fn add_comment(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        body: String,
    ) -> Result<TicketComment, StoreError>;
    async fn set_analysis(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        analysis: TicketAnalysis,
    ) -> Result<TicketAnalysis, StoreError>;
    async fn set_plan(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        plan: Plan,
    ) -> Result<Plan, StoreError>;
    async fn ticket_analysis(
        &self,
        ticket_id: &TicketId,
    ) -> Result<Option<TicketAnalysis>, StoreError>;
    async fn ticket_plan(&self, ticket_id: &TicketId) -> Result<Option<Plan>, StoreError>;
    async fn set_approval_policy(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        policy: ApprovalPolicy,
    ) -> Result<Ticket, StoreError>;
    async fn update_ticket_fields(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        edits: TicketEdits,
    ) -> Result<Ticket, StoreError>;
    async fn grant_approval(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError>;
    async fn reject_approval(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        reason: String,
    ) -> Result<Ticket, StoreError>;
    async fn has_approval(&self, ticket_id: &TicketId) -> Result<bool, StoreError>;
    async fn add_run(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        run: Run,
    ) -> Result<Run, StoreError>;
    async fn update_run(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        run: Run,
    ) -> Result<Run, StoreError>;
    async fn update_run_if_unchanged(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        expected: Run,
        run: Run,
    ) -> Result<Run, StoreError>;
    async fn update_latest_run_if_unchanged(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        expected: Run,
        run: Run,
    ) -> Result<Run, StoreError>;
    async fn list_runs(&self, ticket_id: &TicketId) -> Result<Vec<Run>, StoreError>;
    async fn get_run(&self, run_id: &RunId) -> Result<Run, StoreError>;
    /// True when any run for the ticket carries evidence. Must observably match
    /// `list_runs(..).iter().any(|run| run.evidence.is_some())` on every backend.
    async fn has_run_evidence(&self, ticket_id: &TicketId) -> Result<bool, StoreError>;
    /// The most recently added run for the ticket, if any. Must observably match
    /// `list_runs(..).last()` on every backend.
    async fn latest_run(&self, ticket_id: &TicketId) -> Result<Option<Run>, StoreError>;
    async fn accept_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError>;
    async fn close_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError>;
    async fn cancel_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError>;
}

#[derive(Clone)]
pub enum RuntimeTicketStore {
    Memory(InMemoryTicketStore),
    Sqlite(SqliteTicketStore),
}

impl RuntimeTicketStore {
    pub fn memory() -> Self {
        Self::Memory(InMemoryTicketStore::default())
    }

    pub fn sqlite(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Ok(Self::Sqlite(SqliteTicketStore::open(path)?))
    }
}

/// Forwards each `TicketStore` method to the Memory or Sqlite backend.
/// The impl is generated here so `#[async_trait]` sees the expanded methods.
/// New trait methods only need a signature plus call-args, not another match.
macro_rules! delegate_runtime_ticket_store {
    (
        $(
            async fn $name:ident(&self $(, $arg:ident: $ty:ty)* $(,)?) -> $ret:ty { $($call:tt)* }
        )*
    ) => {
        #[async_trait]
        impl TicketStore for RuntimeTicketStore {
            $(
                async fn $name(&self $(, $arg: $ty)*) -> $ret {
                    match self {
                        Self::Memory(store) => store.$name($($call)*).await,
                        Self::Sqlite(store) => store.$name($($call)*).await,
                    }
                }
            )*
        }
    };
}

delegate_runtime_ticket_store! {
    async fn store_status(&self) -> Result<StoreStatus, StoreError> {}
    async fn list_tickets_page(
        &self,
        request: TicketPageRequest,
    ) -> Result<TicketListPage, StoreError> {
        request
    }
    async fn create_ticket(
        &self,
        title: String,
        description: String,
        source: TicketSource,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        title, description, source, actor
    }
    async fn create_ticket_idempotent(
        &self,
        title: String,
        description: String,
        source: TicketSource,
        actor: ActorRef,
        approval_policy: ApprovalPolicy,
        options: TicketCreateOptions,
        idempotency: Option<IdempotencyRequest>,
    ) -> Result<Ticket, StoreError> {
        title, description, source, actor, approval_policy, options, idempotency
    }
    async fn list_tickets(&self) -> Result<Vec<Ticket>, StoreError> {}
    async fn ticket_metrics(&self) -> Result<Vec<TicketMetrics>, StoreError> {}
    async fn ticket_metrics_page(
        &self,
        request: TicketPageRequest,
    ) -> Result<TicketMetricsPage, StoreError> {
        request
    }
    async fn ticket_bundle(&self, id: &TicketId) -> Result<TicketBundle, StoreError> {
        id
    }
    async fn get_ticket(&self, id: &TicketId) -> Result<Ticket, StoreError> {
        id
    }
    async fn ticket_events(&self, id: &TicketId) -> Result<Vec<TicketEvent>, StoreError> {
        id
    }
    async fn ticket_comments(&self, id: &TicketId) -> Result<Vec<TicketComment>, StoreError> {
        id
    }
    async fn add_comment(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        body: String,
    ) -> Result<TicketComment, StoreError> {
        ticket_id, actor, body
    }
    async fn set_analysis(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        analysis: TicketAnalysis,
    ) -> Result<TicketAnalysis, StoreError> {
        ticket_id, actor, analysis
    }
    async fn set_plan(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        plan: Plan,
    ) -> Result<Plan, StoreError> {
        ticket_id, actor, plan
    }
    async fn ticket_analysis(
        &self,
        ticket_id: &TicketId,
    ) -> Result<Option<TicketAnalysis>, StoreError> {
        ticket_id
    }
    async fn ticket_plan(&self, ticket_id: &TicketId) -> Result<Option<Plan>, StoreError> {
        ticket_id
    }
    async fn set_approval_policy(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        policy: ApprovalPolicy,
    ) -> Result<Ticket, StoreError> {
        ticket_id, actor, policy
    }
    async fn update_ticket_fields(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        edits: TicketEdits,
    ) -> Result<Ticket, StoreError> {
        ticket_id, actor, edits
    }
    async fn grant_approval(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        ticket_id, actor
    }
    async fn reject_approval(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        reason: String,
    ) -> Result<Ticket, StoreError> {
        ticket_id, actor, reason
    }
    async fn has_approval(&self, ticket_id: &TicketId) -> Result<bool, StoreError> {
        ticket_id
    }
    async fn add_run(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        run: Run,
    ) -> Result<Run, StoreError> {
        ticket_id, actor, run
    }
    async fn update_run(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        run: Run,
    ) -> Result<Run, StoreError> {
        ticket_id, actor, run
    }
    async fn update_run_if_unchanged(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        expected: Run,
        run: Run,
    ) -> Result<Run, StoreError> {
        ticket_id, actor, expected, run
    }
    async fn update_latest_run_if_unchanged(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        expected: Run,
        run: Run,
    ) -> Result<Run, StoreError> {
        ticket_id, actor, expected, run
    }
    async fn list_runs(&self, ticket_id: &TicketId) -> Result<Vec<Run>, StoreError> {
        ticket_id
    }
    async fn get_run(&self, run_id: &RunId) -> Result<Run, StoreError> {
        run_id
    }
    async fn has_run_evidence(&self, ticket_id: &TicketId) -> Result<bool, StoreError> {
        ticket_id
    }
    async fn latest_run(&self, ticket_id: &TicketId) -> Result<Option<Run>, StoreError> {
        ticket_id
    }
    async fn accept_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        ticket_id, actor
    }
    async fn close_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        ticket_id, actor
    }
    async fn cancel_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        ticket_id, actor
    }
}

#[cfg(test)]
mod tests;
