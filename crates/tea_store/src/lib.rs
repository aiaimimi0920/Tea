#![forbid(unsafe_code)]

use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tea_core::{
    ActorRef, ApprovalPolicy, Plan, Run, RunId, RunStatus, Ticket, TicketAnalysis, TicketComment,
    TicketCreateOptions, TicketEdits, TicketEvent, TicketEventId, TicketEventKind, TicketId,
    TicketSource, TicketStatus,
};
use thiserror::Error;

const CURRENT_SQLITE_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("ticket not found")]
    TicketNotFound,
    #[error("run not found")]
    RunNotFound,
    #[error("ticket transition requires evidence")]
    EvidenceRequired,
    #[error("invalid ticket transition: {0}")]
    InvalidTransition(String),
    #[error("store lock poisoned")]
    LockPoisoned,
    #[error("store database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("store serialization error: {0}")]
    Codec(#[from] serde_json::Error),
    #[error("store io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported sqlite schema version {found}; this binary supports up to {supported}")]
    UnsupportedSchemaVersion { found: i64, supported: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoreStatus {
    pub backend: StoreBackend,
    pub schema_version: Option<i64>,
    pub supported_schema_version: Option<i64>,
}

impl StoreStatus {
    fn memory() -> Self {
        Self {
            backend: StoreBackend::Memory,
            schema_version: None,
            supported_schema_version: None,
        }
    }

    fn sqlite(schema_version: i64, supported_schema_version: i64) -> Self {
        Self {
            backend: StoreBackend::Sqlite,
            schema_version: Some(schema_version),
            supported_schema_version: Some(supported_schema_version),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreBackend {
    Memory,
    Sqlite,
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
    ) -> Result<Ticket, StoreError>;

    async fn list_tickets(&self) -> Result<Vec<Ticket>, StoreError>;
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
    async fn update_run_status(
        &self,
        run_id: &RunId,
        ticket_id: &TicketId,
        actor: ActorRef,
        status: RunStatus,
    ) -> Result<Run, StoreError>;
    async fn list_runs(&self, ticket_id: &TicketId) -> Result<Vec<Run>, StoreError>;
    async fn get_run(&self, run_id: &RunId) -> Result<Run, StoreError>;
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

#[derive(Default, Clone)]
pub struct InMemoryTicketStore {
    inner: Arc<Mutex<InnerStore>>,
}

#[derive(Clone)]
pub struct SqliteTicketStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteTicketStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut conn = Connection::open(path)?;
        init_sqlite(&mut conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }
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

#[derive(Default)]
struct InnerStore {
    ticket_order: Vec<TicketId>,
    tickets: BTreeMap<TicketId, Ticket>,
    comments: BTreeMap<TicketId, Vec<TicketComment>>,
    events: BTreeMap<TicketId, Vec<TicketEvent>>,
    analyses: BTreeMap<TicketId, TicketAnalysis>,
    plans: BTreeMap<TicketId, Plan>,
    approvals: BTreeMap<TicketId, bool>,
    approval_rejections: BTreeMap<TicketId, String>,
    runs_by_ticket: BTreeMap<TicketId, Vec<RunId>>,
    runs: BTreeMap<RunId, Run>,
}

#[async_trait]
impl TicketStore for InMemoryTicketStore {
    async fn store_status(&self) -> Result<StoreStatus, StoreError> {
        Ok(StoreStatus::memory())
    }

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

    async fn create_ticket_with_options(
        &self,
        title: String,
        description: String,
        source: TicketSource,
        actor: ActorRef,
        approval_policy: ApprovalPolicy,
        options: TicketCreateOptions,
    ) -> Result<Ticket, StoreError> {
        let ticket = Ticket::new_with_options(
            TicketId::new(),
            title,
            description,
            source,
            actor.clone(),
            approval_policy,
            options,
        );
        let event = TicketEvent::new(
            TicketEventId::new(),
            ticket.id.clone(),
            actor,
            TicketEventKind::TicketCreated,
        );
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        inner.events.insert(ticket.id.clone(), vec![event]);
        inner.ticket_order.push(ticket.id.clone());
        inner.tickets.insert(ticket.id.clone(), ticket.clone());
        Ok(ticket)
    }

    async fn list_tickets(&self) -> Result<Vec<Ticket>, StoreError> {
        let inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        Ok(inner
            .ticket_order
            .iter()
            .filter_map(|id| inner.tickets.get(id).cloned())
            .collect())
    }

    async fn get_ticket(&self, id: &TicketId) -> Result<Ticket, StoreError> {
        let inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        inner
            .tickets
            .get(id)
            .cloned()
            .ok_or(StoreError::TicketNotFound)
    }

    async fn ticket_events(&self, id: &TicketId) -> Result<Vec<TicketEvent>, StoreError> {
        let inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_ticket(&inner, id)?;
        Ok(inner.events.get(id).cloned().unwrap_or_default())
    }

    async fn ticket_comments(&self, id: &TicketId) -> Result<Vec<TicketComment>, StoreError> {
        let inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_ticket(&inner, id)?;
        Ok(inner.comments.get(id).cloned().unwrap_or_default())
    }

    async fn add_comment(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        body: String,
    ) -> Result<TicketComment, StoreError> {
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        let ticket = ticket_mut(&mut inner, ticket_id)?;
        ensure_ticket_mutable_for(ticket, "add comment to")?;
        ticket.touch();
        let comment = TicketComment::new(ticket_id.clone(), actor.clone(), body);
        inner
            .comments
            .entry(ticket_id.clone())
            .or_default()
            .push(comment.clone());
        push_event(&mut inner, ticket_id, actor, TicketEventKind::CommentAdded);
        Ok(comment)
    }

    async fn set_analysis(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        analysis: TicketAnalysis,
    ) -> Result<TicketAnalysis, StoreError> {
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        let ticket = ticket_mut(&mut inner, ticket_id)?;
        ensure_ticket_mutable_for(ticket, "analyze")?;
        ticket.status = if analysis.missing_context.is_empty() {
            TicketStatus::AnalysisReady
        } else {
            TicketStatus::NeedsInfo
        };
        ticket.risk_level = analysis.risk_assessment;
        ticket.set_approval_policy(analysis.recommended_policy);
        inner.analyses.insert(ticket_id.clone(), analysis.clone());
        push_event(
            &mut inner,
            ticket_id,
            actor,
            TicketEventKind::TicketAnalyzed,
        );
        Ok(analysis)
    }

    async fn set_plan(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        plan: Plan,
    ) -> Result<Plan, StoreError> {
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        let ticket = ticket_mut(&mut inner, ticket_id)?;
        ensure_ticket_mutable_for(ticket, "plan")?;
        ticket.status = if plan.requires_approval_before_execute {
            TicketStatus::AwaitingApproval
        } else {
            TicketStatus::PlanReady
        };
        ticket.touch();
        inner.plans.insert(ticket_id.clone(), plan.clone());
        push_event(&mut inner, ticket_id, actor, TicketEventKind::PlanProposed);
        Ok(plan)
    }

    async fn ticket_analysis(
        &self,
        ticket_id: &TicketId,
    ) -> Result<Option<TicketAnalysis>, StoreError> {
        let inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_ticket(&inner, ticket_id)?;
        Ok(inner.analyses.get(ticket_id).cloned())
    }

    async fn ticket_plan(&self, ticket_id: &TicketId) -> Result<Option<Plan>, StoreError> {
        let inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_ticket(&inner, ticket_id)?;
        Ok(inner.plans.get(ticket_id).cloned())
    }

    async fn set_approval_policy(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        policy: ApprovalPolicy,
    ) -> Result<Ticket, StoreError> {
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        let ticket = ticket_mut(&mut inner, ticket_id)?;
        ensure_ticket_mutable_for(ticket, "update policy for")?;
        ticket.set_approval_policy(policy);
        push_event(&mut inner, ticket_id, actor, TicketEventKind::PolicyUpdated);
        inner
            .tickets
            .get(ticket_id)
            .cloned()
            .ok_or(StoreError::TicketNotFound)
    }

    async fn update_ticket_fields(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        edits: TicketEdits,
    ) -> Result<Ticket, StoreError> {
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        let ticket = ticket_mut(&mut inner, ticket_id)?;
        ensure_ticket_mutable_for(ticket, "edit")?;
        let changed = ticket.apply_edits(edits);
        if changed {
            push_event(&mut inner, ticket_id, actor, TicketEventKind::TicketEdited);
        }
        inner
            .tickets
            .get(ticket_id)
            .cloned()
            .ok_or(StoreError::TicketNotFound)
    }

    async fn grant_approval(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        let ticket = ticket_mut(&mut inner, ticket_id)?;
        ensure_ticket_mutable_for(ticket, "approve")?;
        ticket.status = TicketStatus::Approved;
        ticket.touch();
        inner.approvals.insert(ticket_id.clone(), true);
        push_event(
            &mut inner,
            ticket_id,
            actor,
            TicketEventKind::ApprovalGranted,
        );
        inner
            .tickets
            .get(ticket_id)
            .cloned()
            .ok_or(StoreError::TicketNotFound)
    }

    async fn reject_approval(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        reason: String,
    ) -> Result<Ticket, StoreError> {
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        let ticket = ticket_mut(&mut inner, ticket_id)?;
        ensure_ticket_mutable_for(ticket, "reject approval for")?;
        ticket.status = TicketStatus::Blocked;
        ticket.touch();
        inner.approvals.insert(ticket_id.clone(), false);
        inner.approval_rejections.insert(ticket_id.clone(), reason);
        push_event(
            &mut inner,
            ticket_id,
            actor,
            TicketEventKind::ApprovalRejected,
        );
        inner
            .tickets
            .get(ticket_id)
            .cloned()
            .ok_or(StoreError::TicketNotFound)
    }

    async fn has_approval(&self, ticket_id: &TicketId) -> Result<bool, StoreError> {
        let inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_ticket(&inner, ticket_id)?;
        Ok(inner.approvals.get(ticket_id).copied().unwrap_or(false))
    }

    async fn add_run(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        run: Run,
    ) -> Result<Run, StoreError> {
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        let ticket = ticket_mut(&mut inner, ticket_id)?;
        ensure_ticket_can_run(ticket)?;
        ensure_run_belongs_to_ticket(&run, ticket_id)?;
        ticket.status = TicketStatus::Running;
        ticket.touch();
        inner
            .runs_by_ticket
            .entry(ticket_id.clone())
            .or_default()
            .push(run.id.clone());
        inner.runs.insert(run.id.clone(), run.clone());
        push_event(
            &mut inner,
            ticket_id,
            actor.clone(),
            TicketEventKind::RunQueued,
        );
        push_event(
            &mut inner,
            ticket_id,
            actor.clone(),
            TicketEventKind::RunStarted,
        );
        match run.status {
            RunStatus::Succeeded => {
                let ticket = ticket_mut(&mut inner, ticket_id)?;
                ticket.status = TicketStatus::Completed;
                ticket.touch();
                push_event(
                    &mut inner,
                    ticket_id,
                    actor.clone(),
                    TicketEventKind::RunSucceeded,
                );
                if run.evidence.is_some() {
                    push_event(
                        &mut inner,
                        ticket_id,
                        actor,
                        TicketEventKind::EvidenceAttached,
                    );
                }
            }
            RunStatus::Failed => {
                let ticket = ticket_mut(&mut inner, ticket_id)?;
                ticket.status = TicketStatus::Failed;
                ticket.touch();
                push_event(&mut inner, ticket_id, actor, TicketEventKind::RunFailed);
            }
            _ => {}
        }
        Ok(run)
    }

    async fn update_run_status(
        &self,
        run_id: &RunId,
        ticket_id: &TicketId,
        actor: ActorRef,
        status: RunStatus,
    ) -> Result<Run, StoreError> {
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_ticket_mutable_for(
            inner
                .tickets
                .get(ticket_id)
                .ok_or(StoreError::TicketNotFound)?,
            "update run status for",
        )?;
        {
            let run = inner.runs.get_mut(run_id).ok_or(StoreError::RunNotFound)?;
            if run.ticket_id != *ticket_id {
                return Err(StoreError::InvalidTransition(format!(
                    "run {run_id} does not belong to ticket {ticket_id}"
                )));
            }
            run.status = status;
        }
        ticket_mut(&mut inner, ticket_id)?.touch();
        push_event(
            &mut inner,
            ticket_id,
            actor,
            TicketEventKind::RunEventReceived,
        );
        inner
            .runs
            .get(run_id)
            .cloned()
            .ok_or(StoreError::RunNotFound)
    }

    async fn list_runs(&self, ticket_id: &TicketId) -> Result<Vec<Run>, StoreError> {
        let inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_ticket(&inner, ticket_id)?;
        let runs = inner
            .runs_by_ticket
            .get(ticket_id)
            .into_iter()
            .flatten()
            .filter_map(|id| inner.runs.get(id).cloned())
            .collect();
        Ok(runs)
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Run, StoreError> {
        let inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        inner
            .runs
            .get(run_id)
            .cloned()
            .ok_or(StoreError::RunNotFound)
    }

    async fn accept_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_ticket_mutable_for(
            inner
                .tickets
                .get(ticket_id)
                .ok_or(StoreError::TicketNotFound)?,
            "accept",
        )?;
        let has_evidence = inner
            .runs_by_ticket
            .get(ticket_id)
            .into_iter()
            .flatten()
            .filter_map(|id| inner.runs.get(id))
            .any(|run| run.evidence.is_some());
        if !has_evidence {
            return Err(StoreError::EvidenceRequired);
        }
        let ticket = ticket_mut(&mut inner, ticket_id)?;
        ticket.status = TicketStatus::Accepted;
        ticket.touch();
        push_event(&mut inner, ticket_id, actor, TicketEventKind::HumanAccepted);
        inner
            .tickets
            .get(ticket_id)
            .cloned()
            .ok_or(StoreError::TicketNotFound)
    }

    async fn close_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_ticket_mutable_for(
            inner
                .tickets
                .get(ticket_id)
                .ok_or(StoreError::TicketNotFound)?,
            "close",
        )?;
        let has_evidence = inner
            .runs_by_ticket
            .get(ticket_id)
            .into_iter()
            .flatten()
            .filter_map(|id| inner.runs.get(id))
            .any(|run| run.evidence.is_some());
        if !has_evidence {
            return Err(StoreError::EvidenceRequired);
        }
        let ticket = ticket_mut(&mut inner, ticket_id)?;
        ticket.status = TicketStatus::Closed;
        ticket.touch();
        push_event(&mut inner, ticket_id, actor, TicketEventKind::TicketClosed);
        inner
            .tickets
            .get(ticket_id)
            .cloned()
            .ok_or(StoreError::TicketNotFound)
    }

    async fn cancel_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        let ticket = ticket_mut(&mut inner, ticket_id)?;
        ensure_ticket_mutable_for(ticket, "cancel")?;
        ticket.status = TicketStatus::Cancelled;
        ticket.touch();
        push_event(
            &mut inner,
            ticket_id,
            actor,
            TicketEventKind::TicketCancelled,
        );
        inner
            .tickets
            .get(ticket_id)
            .cloned()
            .ok_or(StoreError::TicketNotFound)
    }
}

#[async_trait]
impl TicketStore for SqliteTicketStore {
    async fn store_status(&self) -> Result<StoreStatus, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        Ok(StoreStatus::sqlite(
            applied_sqlite_schema_version(&conn)?,
            CURRENT_SQLITE_SCHEMA_VERSION,
        ))
    }

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

    async fn create_ticket_with_options(
        &self,
        title: String,
        description: String,
        source: TicketSource,
        actor: ActorRef,
        approval_policy: ApprovalPolicy,
        options: TicketCreateOptions,
    ) -> Result<Ticket, StoreError> {
        let ticket = Ticket::new_with_options(
            TicketId::new(),
            title,
            description,
            source,
            actor.clone(),
            approval_policy,
            options,
        );
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        insert_ticket(&conn, &ticket)?;
        push_sqlite_event(&conn, &ticket.id, actor, TicketEventKind::TicketCreated)?;
        Ok(ticket)
    }

    async fn list_tickets(&self) -> Result<Vec<Ticket>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut statement = conn.prepare("SELECT json FROM tickets ORDER BY ordinal ASC")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        decode_rows(rows)
    }

    async fn get_ticket(&self, id: &TicketId) -> Result<Ticket, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        get_sqlite_ticket(&conn, id)
    }

    async fn ticket_events(&self, id: &TicketId) -> Result<Vec<TicketEvent>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_sqlite_ticket(&conn, id)?;
        let mut statement =
            conn.prepare("SELECT json FROM events WHERE ticket_id = ?1 ORDER BY ordinal ASC")?;
        let rows = statement.query_map(params![id.to_string()], |row| row.get::<_, String>(0))?;
        decode_rows(rows)
    }

    async fn ticket_comments(&self, id: &TicketId) -> Result<Vec<TicketComment>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_sqlite_ticket(&conn, id)?;
        let mut statement =
            conn.prepare("SELECT json FROM comments WHERE ticket_id = ?1 ORDER BY ordinal ASC")?;
        let rows = statement.query_map(params![id.to_string()], |row| row.get::<_, String>(0))?;
        decode_rows(rows)
    }

    async fn add_comment(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        body: String,
    ) -> Result<TicketComment, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut ticket = get_sqlite_ticket(&conn, ticket_id)?;
        ensure_ticket_mutable_for(&ticket, "add comment to")?;
        ticket.touch();
        update_ticket(&conn, &ticket)?;
        let comment = TicketComment::new(ticket_id.clone(), actor.clone(), body);
        let ordinal = next_scoped_ordinal(&conn, "comments", ticket_id)?;
        conn.execute(
            "INSERT INTO comments (id, ticket_id, ordinal, json) VALUES (?1, ?2, ?3, ?4)",
            params![
                comment.id.0.to_string(),
                ticket_id.to_string(),
                ordinal,
                encode(&comment)?
            ],
        )?;
        push_sqlite_event(&conn, ticket_id, actor, TicketEventKind::CommentAdded)?;
        Ok(comment)
    }

    async fn set_analysis(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        analysis: TicketAnalysis,
    ) -> Result<TicketAnalysis, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut ticket = get_sqlite_ticket(&conn, ticket_id)?;
        ensure_ticket_mutable_for(&ticket, "analyze")?;
        ticket.status = if analysis.missing_context.is_empty() {
            TicketStatus::AnalysisReady
        } else {
            TicketStatus::NeedsInfo
        };
        ticket.risk_level = analysis.risk_assessment;
        ticket.set_approval_policy(analysis.recommended_policy);
        update_ticket(&conn, &ticket)?;
        conn.execute(
            "INSERT INTO analyses (ticket_id, json) VALUES (?1, ?2)
             ON CONFLICT(ticket_id) DO UPDATE SET json = excluded.json",
            params![ticket_id.to_string(), encode(&analysis)?],
        )?;
        push_sqlite_event(&conn, ticket_id, actor, TicketEventKind::TicketAnalyzed)?;
        Ok(analysis)
    }

    async fn set_plan(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        plan: Plan,
    ) -> Result<Plan, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut ticket = get_sqlite_ticket(&conn, ticket_id)?;
        ensure_ticket_mutable_for(&ticket, "plan")?;
        ticket.status = if plan.requires_approval_before_execute {
            TicketStatus::AwaitingApproval
        } else {
            TicketStatus::PlanReady
        };
        ticket.touch();
        update_ticket(&conn, &ticket)?;
        conn.execute(
            "INSERT INTO plans (ticket_id, json) VALUES (?1, ?2)
             ON CONFLICT(ticket_id) DO UPDATE SET json = excluded.json",
            params![ticket_id.to_string(), encode(&plan)?],
        )?;
        push_sqlite_event(&conn, ticket_id, actor, TicketEventKind::PlanProposed)?;
        Ok(plan)
    }

    async fn ticket_analysis(
        &self,
        ticket_id: &TicketId,
    ) -> Result<Option<TicketAnalysis>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_sqlite_ticket(&conn, ticket_id)?;
        let json = conn
            .query_row(
                "SELECT json FROM analyses WHERE ticket_id = ?1",
                params![ticket_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        json.map(decode).transpose()
    }

    async fn ticket_plan(&self, ticket_id: &TicketId) -> Result<Option<Plan>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_sqlite_ticket(&conn, ticket_id)?;
        let json = conn
            .query_row(
                "SELECT json FROM plans WHERE ticket_id = ?1",
                params![ticket_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        json.map(decode).transpose()
    }

    async fn set_approval_policy(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        policy: ApprovalPolicy,
    ) -> Result<Ticket, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut ticket = get_sqlite_ticket(&conn, ticket_id)?;
        ensure_ticket_mutable_for(&ticket, "update policy for")?;
        ticket.set_approval_policy(policy);
        update_ticket(&conn, &ticket)?;
        push_sqlite_event(&conn, ticket_id, actor, TicketEventKind::PolicyUpdated)?;
        Ok(ticket)
    }

    async fn update_ticket_fields(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        edits: TicketEdits,
    ) -> Result<Ticket, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut ticket = get_sqlite_ticket(&conn, ticket_id)?;
        ensure_ticket_mutable_for(&ticket, "edit")?;
        let changed = ticket.apply_edits(edits);
        if changed {
            update_ticket(&conn, &ticket)?;
            push_sqlite_event(&conn, ticket_id, actor, TicketEventKind::TicketEdited)?;
        }
        Ok(ticket)
    }

    async fn grant_approval(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut ticket = get_sqlite_ticket(&conn, ticket_id)?;
        ensure_ticket_mutable_for(&ticket, "approve")?;
        ticket.status = TicketStatus::Approved;
        ticket.touch();
        update_ticket(&conn, &ticket)?;
        conn.execute(
            "INSERT INTO approvals (ticket_id, approved, rejection_reason) VALUES (?1, 1, NULL)
             ON CONFLICT(ticket_id) DO UPDATE SET approved = excluded.approved, rejection_reason = NULL",
            params![ticket_id.to_string()],
        )?;
        push_sqlite_event(&conn, ticket_id, actor, TicketEventKind::ApprovalGranted)?;
        Ok(ticket)
    }

    async fn reject_approval(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        reason: String,
    ) -> Result<Ticket, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut ticket = get_sqlite_ticket(&conn, ticket_id)?;
        ensure_ticket_mutable_for(&ticket, "reject approval for")?;
        ticket.status = TicketStatus::Blocked;
        ticket.touch();
        update_ticket(&conn, &ticket)?;
        conn.execute(
            "INSERT INTO approvals (ticket_id, approved, rejection_reason) VALUES (?1, 0, ?2)
             ON CONFLICT(ticket_id) DO UPDATE SET approved = excluded.approved, rejection_reason = excluded.rejection_reason",
            params![ticket_id.to_string(), reason],
        )?;
        push_sqlite_event(&conn, ticket_id, actor, TicketEventKind::ApprovalRejected)?;
        Ok(ticket)
    }

    async fn has_approval(&self, ticket_id: &TicketId) -> Result<bool, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_sqlite_ticket(&conn, ticket_id)?;
        let approved = conn
            .query_row(
                "SELECT approved FROM approvals WHERE ticket_id = ?1",
                params![ticket_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        Ok(approved.unwrap_or(0) == 1)
    }

    async fn add_run(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        run: Run,
    ) -> Result<Run, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut ticket = get_sqlite_ticket(&conn, ticket_id)?;
        ensure_ticket_can_run(&ticket)?;
        ensure_run_belongs_to_ticket(&run, ticket_id)?;
        ticket.status = TicketStatus::Running;
        ticket.touch();
        update_ticket(&conn, &ticket)?;
        insert_run(&conn, ticket_id, &run)?;
        push_sqlite_event(&conn, ticket_id, actor.clone(), TicketEventKind::RunQueued)?;
        push_sqlite_event(&conn, ticket_id, actor.clone(), TicketEventKind::RunStarted)?;
        match run.status {
            RunStatus::Succeeded => {
                ticket.status = TicketStatus::Completed;
                ticket.touch();
                update_ticket(&conn, &ticket)?;
                push_sqlite_event(
                    &conn,
                    ticket_id,
                    actor.clone(),
                    TicketEventKind::RunSucceeded,
                )?;
                if run.evidence.is_some() {
                    push_sqlite_event(&conn, ticket_id, actor, TicketEventKind::EvidenceAttached)?;
                }
            }
            RunStatus::Failed => {
                ticket.status = TicketStatus::Failed;
                ticket.touch();
                update_ticket(&conn, &ticket)?;
                push_sqlite_event(&conn, ticket_id, actor, TicketEventKind::RunFailed)?;
            }
            _ => {}
        }
        Ok(run)
    }

    async fn update_run_status(
        &self,
        run_id: &RunId,
        ticket_id: &TicketId,
        actor: ActorRef,
        status: RunStatus,
    ) -> Result<Run, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut ticket = get_sqlite_ticket(&conn, ticket_id)?;
        ensure_ticket_mutable_for(&ticket, "update run status for")?;
        let mut run = get_sqlite_run(&conn, run_id)?;
        if run.ticket_id != *ticket_id {
            return Err(StoreError::InvalidTransition(format!(
                "run {run_id} does not belong to ticket {ticket_id}"
            )));
        }
        run.status = status;
        conn.execute(
            "UPDATE runs SET json = ?1 WHERE id = ?2",
            params![encode(&run)?, run_id.to_string()],
        )?;
        ticket.touch();
        update_ticket(&conn, &ticket)?;
        push_sqlite_event(&conn, ticket_id, actor, TicketEventKind::RunEventReceived)?;
        Ok(run)
    }

    async fn list_runs(&self, ticket_id: &TicketId) -> Result<Vec<Run>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_sqlite_ticket(&conn, ticket_id)?;
        list_sqlite_runs(&conn, ticket_id)
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Run, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        get_sqlite_run(&conn, run_id)
    }

    async fn accept_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut ticket = get_sqlite_ticket(&conn, ticket_id)?;
        ensure_ticket_mutable_for(&ticket, "accept")?;
        let has_evidence = list_sqlite_runs(&conn, ticket_id)?
            .into_iter()
            .any(|run| run.evidence.is_some());
        if !has_evidence {
            return Err(StoreError::EvidenceRequired);
        }
        ticket.status = TicketStatus::Accepted;
        ticket.touch();
        update_ticket(&conn, &ticket)?;
        push_sqlite_event(&conn, ticket_id, actor, TicketEventKind::HumanAccepted)?;
        Ok(ticket)
    }

    async fn close_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut ticket = get_sqlite_ticket(&conn, ticket_id)?;
        ensure_ticket_mutable_for(&ticket, "close")?;
        let has_evidence = list_sqlite_runs(&conn, ticket_id)?
            .into_iter()
            .any(|run| run.evidence.is_some());
        if !has_evidence {
            return Err(StoreError::EvidenceRequired);
        }
        ticket.status = TicketStatus::Closed;
        ticket.touch();
        update_ticket(&conn, &ticket)?;
        push_sqlite_event(&conn, ticket_id, actor, TicketEventKind::TicketClosed)?;
        Ok(ticket)
    }

    async fn cancel_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::LockPoisoned)?;
        let mut ticket = get_sqlite_ticket(&conn, ticket_id)?;
        ensure_ticket_mutable_for(&ticket, "cancel")?;
        ticket.status = TicketStatus::Cancelled;
        ticket.touch();
        update_ticket(&conn, &ticket)?;
        push_sqlite_event(&conn, ticket_id, actor, TicketEventKind::TicketCancelled)?;
        Ok(ticket)
    }
}

#[async_trait]
impl TicketStore for RuntimeTicketStore {
    async fn store_status(&self) -> Result<StoreStatus, StoreError> {
        match self {
            Self::Memory(store) => store.store_status().await,
            Self::Sqlite(store) => store.store_status().await,
        }
    }

    async fn create_ticket(
        &self,
        title: String,
        description: String,
        source: TicketSource,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        match self {
            Self::Memory(store) => store.create_ticket(title, description, source, actor).await,
            Self::Sqlite(store) => store.create_ticket(title, description, source, actor).await,
        }
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
        match self {
            Self::Memory(store) => {
                store
                    .create_ticket_with_options(
                        title,
                        description,
                        source,
                        actor,
                        approval_policy,
                        options,
                    )
                    .await
            }
            Self::Sqlite(store) => {
                store
                    .create_ticket_with_options(
                        title,
                        description,
                        source,
                        actor,
                        approval_policy,
                        options,
                    )
                    .await
            }
        }
    }

    async fn list_tickets(&self) -> Result<Vec<Ticket>, StoreError> {
        match self {
            Self::Memory(store) => store.list_tickets().await,
            Self::Sqlite(store) => store.list_tickets().await,
        }
    }

    async fn get_ticket(&self, id: &TicketId) -> Result<Ticket, StoreError> {
        match self {
            Self::Memory(store) => store.get_ticket(id).await,
            Self::Sqlite(store) => store.get_ticket(id).await,
        }
    }

    async fn ticket_events(&self, id: &TicketId) -> Result<Vec<TicketEvent>, StoreError> {
        match self {
            Self::Memory(store) => store.ticket_events(id).await,
            Self::Sqlite(store) => store.ticket_events(id).await,
        }
    }

    async fn ticket_comments(&self, id: &TicketId) -> Result<Vec<TicketComment>, StoreError> {
        match self {
            Self::Memory(store) => store.ticket_comments(id).await,
            Self::Sqlite(store) => store.ticket_comments(id).await,
        }
    }

    async fn add_comment(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        body: String,
    ) -> Result<TicketComment, StoreError> {
        match self {
            Self::Memory(store) => store.add_comment(ticket_id, actor, body).await,
            Self::Sqlite(store) => store.add_comment(ticket_id, actor, body).await,
        }
    }

    async fn set_analysis(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        analysis: TicketAnalysis,
    ) -> Result<TicketAnalysis, StoreError> {
        match self {
            Self::Memory(store) => store.set_analysis(ticket_id, actor, analysis).await,
            Self::Sqlite(store) => store.set_analysis(ticket_id, actor, analysis).await,
        }
    }

    async fn set_plan(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        plan: Plan,
    ) -> Result<Plan, StoreError> {
        match self {
            Self::Memory(store) => store.set_plan(ticket_id, actor, plan).await,
            Self::Sqlite(store) => store.set_plan(ticket_id, actor, plan).await,
        }
    }

    async fn ticket_analysis(
        &self,
        ticket_id: &TicketId,
    ) -> Result<Option<TicketAnalysis>, StoreError> {
        match self {
            Self::Memory(store) => store.ticket_analysis(ticket_id).await,
            Self::Sqlite(store) => store.ticket_analysis(ticket_id).await,
        }
    }

    async fn ticket_plan(&self, ticket_id: &TicketId) -> Result<Option<Plan>, StoreError> {
        match self {
            Self::Memory(store) => store.ticket_plan(ticket_id).await,
            Self::Sqlite(store) => store.ticket_plan(ticket_id).await,
        }
    }

    async fn set_approval_policy(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        policy: ApprovalPolicy,
    ) -> Result<Ticket, StoreError> {
        match self {
            Self::Memory(store) => store.set_approval_policy(ticket_id, actor, policy).await,
            Self::Sqlite(store) => store.set_approval_policy(ticket_id, actor, policy).await,
        }
    }

    async fn update_ticket_fields(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        edits: TicketEdits,
    ) -> Result<Ticket, StoreError> {
        match self {
            Self::Memory(store) => store.update_ticket_fields(ticket_id, actor, edits).await,
            Self::Sqlite(store) => store.update_ticket_fields(ticket_id, actor, edits).await,
        }
    }

    async fn grant_approval(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        match self {
            Self::Memory(store) => store.grant_approval(ticket_id, actor).await,
            Self::Sqlite(store) => store.grant_approval(ticket_id, actor).await,
        }
    }

    async fn reject_approval(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        reason: String,
    ) -> Result<Ticket, StoreError> {
        match self {
            Self::Memory(store) => store.reject_approval(ticket_id, actor, reason).await,
            Self::Sqlite(store) => store.reject_approval(ticket_id, actor, reason).await,
        }
    }

    async fn has_approval(&self, ticket_id: &TicketId) -> Result<bool, StoreError> {
        match self {
            Self::Memory(store) => store.has_approval(ticket_id).await,
            Self::Sqlite(store) => store.has_approval(ticket_id).await,
        }
    }

    async fn add_run(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        run: Run,
    ) -> Result<Run, StoreError> {
        match self {
            Self::Memory(store) => store.add_run(ticket_id, actor, run).await,
            Self::Sqlite(store) => store.add_run(ticket_id, actor, run).await,
        }
    }

    async fn update_run_status(
        &self,
        run_id: &RunId,
        ticket_id: &TicketId,
        actor: ActorRef,
        status: RunStatus,
    ) -> Result<Run, StoreError> {
        match self {
            Self::Memory(store) => {
                store
                    .update_run_status(run_id, ticket_id, actor, status)
                    .await
            }
            Self::Sqlite(store) => {
                store
                    .update_run_status(run_id, ticket_id, actor, status)
                    .await
            }
        }
    }

    async fn list_runs(&self, ticket_id: &TicketId) -> Result<Vec<Run>, StoreError> {
        match self {
            Self::Memory(store) => store.list_runs(ticket_id).await,
            Self::Sqlite(store) => store.list_runs(ticket_id).await,
        }
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Run, StoreError> {
        match self {
            Self::Memory(store) => store.get_run(run_id).await,
            Self::Sqlite(store) => store.get_run(run_id).await,
        }
    }

    async fn accept_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        match self {
            Self::Memory(store) => store.accept_ticket(ticket_id, actor).await,
            Self::Sqlite(store) => store.accept_ticket(ticket_id, actor).await,
        }
    }

    async fn close_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        match self {
            Self::Memory(store) => store.close_ticket(ticket_id, actor).await,
            Self::Sqlite(store) => store.close_ticket(ticket_id, actor).await,
        }
    }

    async fn cancel_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        match self {
            Self::Memory(store) => store.cancel_ticket(ticket_id, actor).await,
            Self::Sqlite(store) => store.cancel_ticket(ticket_id, actor).await,
        }
    }
}

fn init_sqlite(conn: &mut Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;
        "#,
    )?;
    ensure_schema_migrations_table(conn)?;
    apply_sqlite_migrations(conn)?;
    Ok(())
}

fn ensure_schema_migrations_table(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );
        "#,
    )?;
    Ok(())
}

fn apply_sqlite_migrations(conn: &mut Connection) -> Result<(), StoreError> {
    let tx = conn.transaction()?;
    let applied = applied_sqlite_schema_version(&tx)?;
    if applied > CURRENT_SQLITE_SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchemaVersion {
            found: applied,
            supported: CURRENT_SQLITE_SCHEMA_VERSION,
        });
    }
    if applied < CURRENT_SQLITE_SCHEMA_VERSION {
        create_sqlite_v1_schema(&tx)?;
        record_sqlite_schema_version(&tx, CURRENT_SQLITE_SCHEMA_VERSION)?;
    }
    tx.commit()?;
    Ok(())
}

fn applied_sqlite_schema_version(conn: &Connection) -> Result<i64, StoreError> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get::<_, i64>(0),
    )?)
}

fn record_sqlite_schema_version(conn: &Connection, version: i64) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO schema_migrations (version) VALUES (?1)",
        params![version],
    )?;
    Ok(())
}

fn create_sqlite_v1_schema(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS tickets (
            id TEXT PRIMARY KEY,
            ordinal INTEGER NOT NULL,
            json TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS comments (
            id TEXT PRIMARY KEY,
            ticket_id TEXT NOT NULL,
            ordinal INTEGER NOT NULL,
            json TEXT NOT NULL,
            FOREIGN KEY(ticket_id) REFERENCES tickets(id)
        );

        CREATE TABLE IF NOT EXISTS events (
            id TEXT PRIMARY KEY,
            ticket_id TEXT NOT NULL,
            ordinal INTEGER NOT NULL,
            json TEXT NOT NULL,
            FOREIGN KEY(ticket_id) REFERENCES tickets(id)
        );

        CREATE TABLE IF NOT EXISTS analyses (
            ticket_id TEXT PRIMARY KEY,
            json TEXT NOT NULL,
            FOREIGN KEY(ticket_id) REFERENCES tickets(id)
        );

        CREATE TABLE IF NOT EXISTS plans (
            ticket_id TEXT PRIMARY KEY,
            json TEXT NOT NULL,
            FOREIGN KEY(ticket_id) REFERENCES tickets(id)
        );

        CREATE TABLE IF NOT EXISTS approvals (
            ticket_id TEXT PRIMARY KEY,
            approved INTEGER NOT NULL,
            rejection_reason TEXT,
            FOREIGN KEY(ticket_id) REFERENCES tickets(id)
        );

        CREATE TABLE IF NOT EXISTS runs (
            id TEXT PRIMARY KEY,
            ticket_id TEXT NOT NULL,
            ordinal INTEGER NOT NULL,
            json TEXT NOT NULL,
            FOREIGN KEY(ticket_id) REFERENCES tickets(id)
        );

        CREATE INDEX IF NOT EXISTS idx_comments_ticket_ordinal ON comments(ticket_id, ordinal);
        CREATE INDEX IF NOT EXISTS idx_events_ticket_ordinal ON events(ticket_id, ordinal);
        CREATE INDEX IF NOT EXISTS idx_runs_ticket_ordinal ON runs(ticket_id, ordinal);
        "#,
    )?;
    Ok(())
}

fn encode(value: &impl Serialize) -> Result<String, StoreError> {
    Ok(serde_json::to_string(value)?)
}

fn decode<T: serde::de::DeserializeOwned>(json: String) -> Result<T, StoreError> {
    Ok(serde_json::from_str(&json)?)
}

fn decode_rows<T: serde::de::DeserializeOwned>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>>,
) -> Result<Vec<T>, StoreError> {
    let mut values = Vec::new();
    for row in rows {
        values.push(decode(row?)?);
    }
    Ok(values)
}

fn insert_ticket(conn: &Connection, ticket: &Ticket) -> Result<(), StoreError> {
    let ordinal = next_global_ordinal(conn, "tickets")?;
    conn.execute(
        "INSERT INTO tickets (id, ordinal, json) VALUES (?1, ?2, ?3)",
        params![ticket.id.to_string(), ordinal, encode(ticket)?],
    )?;
    Ok(())
}

fn update_ticket(conn: &Connection, ticket: &Ticket) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE tickets SET json = ?1 WHERE id = ?2",
        params![encode(ticket)?, ticket.id.to_string()],
    )?;
    Ok(())
}

fn get_sqlite_ticket(conn: &Connection, ticket_id: &TicketId) -> Result<Ticket, StoreError> {
    let json = conn
        .query_row(
            "SELECT json FROM tickets WHERE id = ?1",
            params![ticket_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or(StoreError::TicketNotFound)?;
    decode(json)
}

fn ensure_sqlite_ticket(conn: &Connection, ticket_id: &TicketId) -> Result<(), StoreError> {
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM tickets WHERE id = ?1)",
        params![ticket_id.to_string()],
        |row| row.get::<_, i64>(0),
    )?;
    if exists == 1 {
        Ok(())
    } else {
        Err(StoreError::TicketNotFound)
    }
}

fn push_sqlite_event(
    conn: &Connection,
    ticket_id: &TicketId,
    actor: ActorRef,
    kind: TicketEventKind,
) -> Result<(), StoreError> {
    let event = TicketEvent::new(TicketEventId::new(), ticket_id.clone(), actor, kind);
    let ordinal = next_scoped_ordinal(conn, "events", ticket_id)?;
    conn.execute(
        "INSERT INTO events (id, ticket_id, ordinal, json) VALUES (?1, ?2, ?3, ?4)",
        params![
            event.id.0.to_string(),
            ticket_id.to_string(),
            ordinal,
            encode(&event)?
        ],
    )?;
    Ok(())
}

fn insert_run(conn: &Connection, ticket_id: &TicketId, run: &Run) -> Result<(), StoreError> {
    let ordinal = next_scoped_ordinal(conn, "runs", ticket_id)?;
    conn.execute(
        "INSERT INTO runs (id, ticket_id, ordinal, json) VALUES (?1, ?2, ?3, ?4)",
        params![
            run.id.to_string(),
            ticket_id.to_string(),
            ordinal,
            encode(run)?
        ],
    )?;
    Ok(())
}

fn get_sqlite_run(conn: &Connection, run_id: &RunId) -> Result<Run, StoreError> {
    let json = conn
        .query_row(
            "SELECT json FROM runs WHERE id = ?1",
            params![run_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or(StoreError::RunNotFound)?;
    decode(json)
}

fn list_sqlite_runs(conn: &Connection, ticket_id: &TicketId) -> Result<Vec<Run>, StoreError> {
    ensure_sqlite_ticket(conn, ticket_id)?;
    let mut statement =
        conn.prepare("SELECT json FROM runs WHERE ticket_id = ?1 ORDER BY ordinal ASC")?;
    let rows = statement.query_map(params![ticket_id.to_string()], |row| {
        row.get::<_, String>(0)
    })?;
    decode_rows(rows)
}

fn next_global_ordinal(conn: &Connection, table: &str) -> Result<i64, StoreError> {
    let sql = match table {
        "tickets" => "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM tickets",
        _ => unreachable!("unknown global ordinal table"),
    };
    Ok(conn.query_row(sql, [], |row| row.get::<_, i64>(0))?)
}

fn next_scoped_ordinal(
    conn: &Connection,
    table: &str,
    ticket_id: &TicketId,
) -> Result<i64, StoreError> {
    let sql = match table {
        "comments" => "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM comments WHERE ticket_id = ?1",
        "events" => "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM events WHERE ticket_id = ?1",
        "runs" => "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM runs WHERE ticket_id = ?1",
        _ => unreachable!("unknown scoped ordinal table"),
    };
    Ok(conn.query_row(sql, params![ticket_id.to_string()], |row| {
        row.get::<_, i64>(0)
    })?)
}

fn ensure_ticket(inner: &InnerStore, ticket_id: &TicketId) -> Result<(), StoreError> {
    if inner.tickets.contains_key(ticket_id) {
        Ok(())
    } else {
        Err(StoreError::TicketNotFound)
    }
}

fn ensure_ticket_mutable_for(ticket: &Ticket, action: &str) -> Result<(), StoreError> {
    if matches!(
        ticket.status,
        TicketStatus::Closed | TicketStatus::Cancelled
    ) {
        return Err(StoreError::InvalidTransition(format!(
            "cannot {action} ticket {} in {:?} status",
            ticket.id, ticket.status
        )));
    }
    Ok(())
}

fn ensure_ticket_can_run(ticket: &Ticket) -> Result<(), StoreError> {
    ensure_ticket_mutable_for(ticket, "run")?;
    if ticket.status == TicketStatus::Blocked {
        return Err(StoreError::InvalidTransition(format!(
            "cannot run ticket {} in {:?} status",
            ticket.id, ticket.status
        )));
    }
    Ok(())
}

fn ensure_run_belongs_to_ticket(run: &Run, ticket_id: &TicketId) -> Result<(), StoreError> {
    if run.ticket_id != *ticket_id {
        return Err(StoreError::InvalidTransition(format!(
            "run {} does not belong to ticket {ticket_id}",
            run.id
        )));
    }
    Ok(())
}

fn ticket_mut<'a>(
    inner: &'a mut InnerStore,
    ticket_id: &TicketId,
) -> Result<&'a mut Ticket, StoreError> {
    inner
        .tickets
        .get_mut(ticket_id)
        .ok_or(StoreError::TicketNotFound)
}

fn push_event(
    inner: &mut InnerStore,
    ticket_id: &TicketId,
    actor: ActorRef,
    kind: TicketEventKind,
) {
    inner
        .events
        .entry(ticket_id.clone())
        .or_default()
        .push(TicketEvent::new(
            TicketEventId::new(),
            ticket_id.clone(),
            actor,
            kind,
        ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tea_core::{ApprovalPolicy, RiskLevel, RunEvidence, TicketAnalysis};

    #[tokio::test]
    async fn create_ticket_appends_created_event() {
        let store = InMemoryTicketStore::default();
        let created = store
            .create_ticket(
                "Smoke".to_string(),
                "Create a safe plan.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();

        let events = store.ticket_events(&created.id).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, TicketEventKind::TicketCreated);
        assert_eq!(events[0].actor, ActorRef::human("vmjcv"));
    }

    #[tokio::test]
    async fn listing_tickets_is_deterministic() {
        let store = InMemoryTicketStore::default();
        store
            .create_ticket(
                "A".to_string(),
                "first".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        store
            .create_ticket(
                "B".to_string(),
                "second".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();

        let tickets = store.list_tickets().await.unwrap();
        assert_eq!(
            tickets
                .iter()
                .map(|ticket| ticket.title.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "B"]
        );
    }

    #[tokio::test]
    async fn set_analysis_appends_event_and_updates_policy() {
        let store = InMemoryTicketStore::default();
        let created = store
            .create_ticket(
                "Smoke".to_string(),
                "Create a safe plan.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        store
            .set_analysis(
                &created.id,
                ActorRef::system(),
                TicketAnalysis {
                    intent: "engineering".to_string(),
                    target_components: vec!["Tea".to_string()],
                    target_paths: vec![],
                    constraints: vec![],
                    acceptance_criteria: vec!["tests pass".to_string()],
                    missing_context: vec![],
                    risk_assessment: RiskLevel::Low,
                    confidence: 0.8,
                    recommended_policy: ApprovalPolicy::HumanBeforeExecute,
                    recommended_workflow: "mock".to_string(),
                },
            )
            .await
            .unwrap();

        let events = store.ticket_events(&created.id).await.unwrap();
        assert_eq!(events.last().unwrap().kind, TicketEventKind::TicketAnalyzed);
        let ticket = store.get_ticket(&created.id).await.unwrap();
        assert_eq!(ticket.status, TicketStatus::AnalysisReady);
        assert_eq!(ticket.risk_level, RiskLevel::Low);
        let stored = store.ticket_analysis(&created.id).await.unwrap().unwrap();
        assert_eq!(stored.intent, "engineering");
    }

    #[tokio::test]
    async fn set_plan_can_be_read_back_for_audit_export() {
        let store = InMemoryTicketStore::default();
        let created = store
            .create_ticket(
                "Smoke".to_string(),
                "Create a safe plan.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        store
            .set_plan(
                &created.id,
                ActorRef::system(),
                Plan {
                    summary: "Use the stored plan in audit export.".to_string(),
                    steps: vec![tea_core::PlanStep {
                        id: "audit".to_string(),
                        title: "Audit".to_string(),
                        description: "Read plan back through the store.".to_string(),
                    }],
                    required_tools: vec![],
                    expected_artifacts: vec![],
                    validation_strategy: vec![],
                    rollback_strategy: vec![],
                    requires_approval_before_execute: true,
                },
            )
            .await
            .unwrap();

        let stored = store.ticket_plan(&created.id).await.unwrap().unwrap();
        assert_eq!(stored.summary, "Use the stored plan in audit export.");
    }

    #[tokio::test]
    async fn update_ticket_fields_edits_and_appends_event() {
        let store = InMemoryTicketStore::default();
        let created = store
            .create_ticket(
                "Original title".to_string(),
                "Original description.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();

        let updated = store
            .update_ticket_fields(
                &created.id,
                ActorRef::human("vmjcv"),
                TicketEdits {
                    title: Some("Edited title".to_string()),
                    description: None,
                    priority: Some("high".to_string()),
                    labels: Some(vec!["area:auth".to_string()]),
                },
            )
            .await
            .unwrap();

        assert_eq!(updated.title, "Edited title");
        assert_eq!(updated.priority, "high");
        assert!(updated.labels.contains(&"area:auth".to_string()));
        // System-derived labels survive the edit.
        assert!(updated.labels.iter().any(|label| label == "source:human"));
        assert!(updated
            .labels
            .iter()
            .any(|label| label.starts_with("policy:")));

        let events = store.ticket_events(&created.id).await.unwrap();
        assert_eq!(events.last().unwrap().kind, TicketEventKind::TicketEdited);
    }

    #[tokio::test]
    async fn update_ticket_fields_rejects_terminal_ticket() {
        let store = InMemoryTicketStore::default();
        let created = store
            .create_ticket(
                "Terminal".to_string(),
                "Will be cancelled before an edit is attempted.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        store
            .cancel_ticket(&created.id, ActorRef::human("vmjcv"))
            .await
            .unwrap();

        let result = store
            .update_ticket_fields(
                &created.id,
                ActorRef::human("vmjcv"),
                TicketEdits {
                    title: Some("Too late".to_string()),
                    description: None,
                    priority: None,
                    labels: None,
                },
            )
            .await;

        assert!(matches!(result, Err(StoreError::InvalidTransition(_))));
    }

    #[tokio::test]
    async fn ticket_mutations_advance_updated_at() {
        let store = InMemoryTicketStore::default();
        let created = store
            .create_ticket(
                "Freshness".to_string(),
                "Ticket mutations should be visible to UI sorting.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(5));
        let approved = store
            .grant_approval(&created.id, ActorRef::human("vmjcv"))
            .await
            .unwrap();

        assert!(approved.updated_at > created.updated_at);
        let stored = store.get_ticket(&created.id).await.unwrap();
        assert_eq!(stored.updated_at, approved.updated_at);
    }

    #[tokio::test]
    async fn sqlite_ticket_mutations_persist_updated_at() {
        let path = temp_store_path("tea-store-sqlite-updated-at");
        let store = SqliteTicketStore::open(&path).unwrap();
        let created = store
            .create_ticket(
                "Freshness".to_string(),
                "Persist updated_at after ticket mutations.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(5));
        let approved = store
            .grant_approval(&created.id, ActorRef::human("vmjcv"))
            .await
            .unwrap();
        assert!(approved.updated_at > created.updated_at);

        let reopened = SqliteTicketStore::open(&path).unwrap();
        let stored = reopened.get_ticket(&created.id).await.unwrap();
        assert_eq!(stored.updated_at, approved.updated_at);

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn update_run_status_rejects_ticket_mismatch() {
        let store = InMemoryTicketStore::default();
        let owner = store
            .create_ticket(
                "Owner".to_string(),
                "Run belongs to this ticket.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        let other = store
            .create_ticket(
                "Other".to_string(),
                "This ticket must not receive another ticket's run event.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        let run = successful_run(&owner.id);
        store
            .add_run(&owner.id, ActorRef::system(), run.clone())
            .await
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(5));
        let err = store
            .update_run_status(
                &run.id,
                &other.id,
                ActorRef::loom("test-loom"),
                RunStatus::Stopped,
            )
            .await
            .unwrap_err();

        assert!(matches!(err, StoreError::InvalidTransition(_)));
        assert_eq!(store.get_run(&run.id).await.unwrap().status, run.status);
        assert_eq!(
            store.get_ticket(&other.id).await.unwrap().updated_at,
            other.updated_at
        );
    }

    #[tokio::test]
    async fn sqlite_update_run_status_rejects_ticket_mismatch() {
        let path = temp_store_path("tea-store-sqlite-run-ticket-mismatch");
        let store = SqliteTicketStore::open(&path).unwrap();
        let owner = store
            .create_ticket(
                "Owner".to_string(),
                "Run belongs to this ticket.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        let other = store
            .create_ticket(
                "Other".to_string(),
                "This ticket must not receive another ticket's run event.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        let run = successful_run(&owner.id);
        store
            .add_run(&owner.id, ActorRef::system(), run.clone())
            .await
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(5));
        let err = store
            .update_run_status(
                &run.id,
                &other.id,
                ActorRef::loom("test-loom"),
                RunStatus::Stopped,
            )
            .await
            .unwrap_err();

        assert!(matches!(err, StoreError::InvalidTransition(_)));
        assert_eq!(store.get_run(&run.id).await.unwrap().status, run.status);
        assert_eq!(
            store.get_ticket(&other.id).await.unwrap().updated_at,
            other.updated_at
        );

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn add_run_rejects_ticket_mismatch() {
        let store = InMemoryTicketStore::default();
        let owner = store
            .create_ticket(
                "Owner".to_string(),
                "Run declares this ticket.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        let other = store
            .create_ticket(
                "Other".to_string(),
                "This ticket must not receive another ticket's run.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        let run = successful_run(&owner.id);

        std::thread::sleep(std::time::Duration::from_millis(5));
        let err = store
            .add_run(&other.id, ActorRef::system(), run)
            .await
            .unwrap_err();

        assert!(matches!(err, StoreError::InvalidTransition(_)));
        assert!(store.list_runs(&owner.id).await.unwrap().is_empty());
        assert!(store.list_runs(&other.id).await.unwrap().is_empty());
        assert_eq!(
            store.get_ticket(&other.id).await.unwrap().updated_at,
            other.updated_at
        );
    }

    #[tokio::test]
    async fn sqlite_add_run_rejects_ticket_mismatch() {
        let path = temp_store_path("tea-store-sqlite-add-run-ticket-mismatch");
        let store = SqliteTicketStore::open(&path).unwrap();
        let owner = store
            .create_ticket(
                "Owner".to_string(),
                "Run declares this ticket.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        let other = store
            .create_ticket(
                "Other".to_string(),
                "This ticket must not receive another ticket's run.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        let run = successful_run(&owner.id);

        std::thread::sleep(std::time::Duration::from_millis(5));
        let err = store
            .add_run(&other.id, ActorRef::system(), run)
            .await
            .unwrap_err();

        assert!(matches!(err, StoreError::InvalidTransition(_)));
        assert!(store.list_runs(&owner.id).await.unwrap().is_empty());
        assert!(store.list_runs(&other.id).await.unwrap().is_empty());
        assert_eq!(
            store.get_ticket(&other.id).await.unwrap().updated_at,
            other.updated_at
        );

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn blocked_ticket_rejects_runs() {
        let store = InMemoryTicketStore::default();
        let created = store
            .create_ticket(
                "Rejected".to_string(),
                "A rejected ticket must not run automatically.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        store
            .set_approval_policy(
                &created.id,
                ActorRef::human("vmjcv"),
                ApprovalPolicy::AlwaysAuto,
            )
            .await
            .unwrap();
        let blocked = store
            .reject_approval(
                &created.id,
                ActorRef::human("vmjcv"),
                "Not acceptable".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(blocked.status, TicketStatus::Blocked);

        let err = store
            .add_run(&created.id, ActorRef::system(), successful_run(&created.id))
            .await
            .unwrap_err();

        assert!(matches!(err, StoreError::InvalidTransition(_)));
        assert_eq!(
            store.get_ticket(&created.id).await.unwrap().status,
            TicketStatus::Blocked
        );
        assert!(store.list_runs(&created.id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sqlite_blocked_ticket_rejects_runs() {
        let path = temp_store_path("tea-store-sqlite-blocked-run");
        let store = SqliteTicketStore::open(&path).unwrap();
        let created = store
            .create_ticket(
                "Rejected".to_string(),
                "A rejected ticket must not run automatically.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        store
            .set_approval_policy(
                &created.id,
                ActorRef::human("vmjcv"),
                ApprovalPolicy::AlwaysAuto,
            )
            .await
            .unwrap();
        let blocked = store
            .reject_approval(
                &created.id,
                ActorRef::human("vmjcv"),
                "Not acceptable".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(blocked.status, TicketStatus::Blocked);

        let err = store
            .add_run(&created.id, ActorRef::system(), successful_run(&created.id))
            .await
            .unwrap_err();

        assert!(matches!(err, StoreError::InvalidTransition(_)));
        assert_eq!(
            store.get_ticket(&created.id).await.unwrap().status,
            TicketStatus::Blocked
        );
        assert!(store.list_runs(&created.id).await.unwrap().is_empty());

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn close_requires_evidence() {
        let store = InMemoryTicketStore::default();
        let created = store
            .create_ticket(
                "Smoke".to_string(),
                "Create a safe plan.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();

        let err = store
            .close_ticket(&created.id, ActorRef::human("vmjcv"))
            .await
            .unwrap_err();
        assert!(matches!(err, StoreError::EvidenceRequired));
    }

    #[tokio::test]
    async fn accept_requires_evidence() {
        let store = InMemoryTicketStore::default();
        let created = store
            .create_ticket(
                "Review".to_string(),
                "Do not accept work before evidence exists.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();

        let err = store
            .accept_ticket(&created.id, ActorRef::human("vmjcv"))
            .await
            .unwrap_err();
        assert!(matches!(err, StoreError::EvidenceRequired));

        let ticket = store.get_ticket(&created.id).await.unwrap();
        assert_ne!(ticket.status, TicketStatus::Accepted);
    }

    #[tokio::test]
    async fn accept_after_run_evidence_appends_accepted_event() {
        let store = InMemoryTicketStore::default();
        let created = store
            .create_ticket(
                "Review".to_string(),
                "Accept only after evidence exists.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        store
            .add_run(&created.id, ActorRef::system(), successful_run(&created.id))
            .await
            .unwrap();

        let accepted = store
            .accept_ticket(&created.id, ActorRef::human("vmjcv"))
            .await
            .unwrap();

        assert_eq!(accepted.status, TicketStatus::Accepted);
        let events = store.ticket_events(&created.id).await.unwrap();
        assert_eq!(events.last().unwrap().kind, TicketEventKind::HumanAccepted);
    }

    #[tokio::test]
    async fn close_after_run_evidence_appends_closed_event() {
        let store = InMemoryTicketStore::default();
        let created = store
            .create_ticket(
                "Smoke".to_string(),
                "Create a safe plan.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        store
            .add_run(
                &created.id,
                ActorRef::system(),
                Run {
                    id: RunId::new(),
                    ticket_id: created.id.clone(),
                    loom_session_id: Some("mock".to_string()),
                    status: RunStatus::Succeeded,
                    evidence: Some(RunEvidence {
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
            .close_ticket(&created.id, ActorRef::human("vmjcv"))
            .await
            .unwrap();

        let events = store.ticket_events(&created.id).await.unwrap();
        assert_eq!(events.last().unwrap().kind, TicketEventKind::TicketClosed);
    }

    #[tokio::test]
    async fn cancel_ticket_appends_cancelled_event_and_freezes_ticket() {
        let store = InMemoryTicketStore::default();
        let created = store
            .create_ticket(
                "Cancel".to_string(),
                "Stop this work order before execution.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();

        let cancelled = store
            .cancel_ticket(&created.id, ActorRef::human("vmjcv"))
            .await
            .unwrap();

        assert_eq!(cancelled.status, TicketStatus::Cancelled);
        let events = store.ticket_events(&created.id).await.unwrap();
        assert_eq!(
            events.last().unwrap().kind,
            TicketEventKind::TicketCancelled
        );

        let comment_error = store
            .add_comment(
                &created.id,
                ActorRef::human("vmjcv"),
                "Do not mutate cancelled tickets.".to_string(),
            )
            .await
            .unwrap_err();
        assert!(matches!(comment_error, StoreError::InvalidTransition(_)));
    }

    #[tokio::test]
    async fn sqlite_cancel_ticket_persists_cancelled_state_after_reopen() {
        let path = temp_store_path("tea-store-sqlite-cancelled-state");
        let created = {
            let store = SqliteTicketStore::open(&path).unwrap();
            let created = store
                .create_ticket(
                    "Cancel persistent".to_string(),
                    "Cancelled tickets stay terminal after daemon restart.".to_string(),
                    TicketSource::Human,
                    ActorRef::human("vmjcv"),
                )
                .await
                .unwrap();
            let cancelled = store
                .cancel_ticket(&created.id, ActorRef::human("vmjcv"))
                .await
                .unwrap();
            assert_eq!(cancelled.status, TicketStatus::Cancelled);
            created
        };

        let reopened = SqliteTicketStore::open(&path).unwrap();
        let ticket = reopened.get_ticket(&created.id).await.unwrap();
        assert_eq!(ticket.status, TicketStatus::Cancelled);
        let events = reopened.ticket_events(&created.id).await.unwrap();
        assert_eq!(
            events.last().unwrap().kind,
            TicketEventKind::TicketCancelled
        );
        let error = reopened
            .add_comment(
                &created.id,
                ActorRef::human("vmjcv"),
                "Persistent cancelled tickets stay immutable.".to_string(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::InvalidTransition(_)));

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn closed_ticket_rejects_in_memory_mutations() {
        let store = InMemoryTicketStore::default();
        let closed = create_closed_ticket(&store).await;

        let comment_error = store
            .add_comment(
                &closed.id,
                ActorRef::human("vmjcv"),
                "Do not mutate closed tickets.".to_string(),
            )
            .await
            .unwrap_err();
        assert!(matches!(comment_error, StoreError::InvalidTransition(_)));

        let approval_error = store
            .grant_approval(&closed.id, ActorRef::human("vmjcv"))
            .await
            .unwrap_err();
        assert!(matches!(approval_error, StoreError::InvalidTransition(_)));

        let run_error = store
            .add_run(&closed.id, ActorRef::system(), successful_run(&closed.id))
            .await
            .unwrap_err();
        assert!(matches!(run_error, StoreError::InvalidTransition(_)));
    }

    #[tokio::test]
    async fn closed_ticket_rejects_sqlite_mutations_after_reopen() {
        let path = temp_store_path("tea-store-closed-state");
        let closed = {
            let store = SqliteTicketStore::open(&path).unwrap();
            create_closed_ticket(&store).await
        };

        let reopened = SqliteTicketStore::open(&path).unwrap();
        let error = reopened
            .add_comment(
                &closed.id,
                ActorRef::human("vmjcv"),
                "Persistent closed tickets stay immutable.".to_string(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::InvalidTransition(_)));

        let ticket = reopened.get_ticket(&closed.id).await.unwrap();
        assert_eq!(ticket.status, TicketStatus::Closed);

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn sqlite_store_preserves_ticket_and_events_after_reopen() {
        let path = temp_store_path("tea-store");

        let created = {
            let store = SqliteTicketStore::open(&path).unwrap();
            let created = store
                .create_ticket(
                    "Persistent".to_string(),
                    "Survive daemon restart.".to_string(),
                    TicketSource::Human,
                    ActorRef::human("vmjcv"),
                )
                .await
                .unwrap();
            store
                .add_comment(
                    &created.id,
                    ActorRef::human("vmjcv"),
                    "Persist this comment event.".to_string(),
                )
                .await
                .unwrap();
            created
        };

        let reopened = SqliteTicketStore::open(&path).unwrap();
        let ticket = reopened.get_ticket(&created.id).await.unwrap();
        assert_eq!(ticket.title, "Persistent");
        let events = reopened.ticket_events(&created.id).await.unwrap();
        assert_eq!(
            events.iter().map(|event| &event.kind).collect::<Vec<_>>(),
            vec![
                &TicketEventKind::TicketCreated,
                &TicketEventKind::CommentAdded
            ]
        );
        let comments = reopened.ticket_comments(&created.id).await.unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].body, "Persist this comment event.");

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn sqlite_store_preserves_policy_update_after_reopen() {
        let path = temp_store_path("tea-store-policy");

        let created = {
            let store = SqliteTicketStore::open(&path).unwrap();
            let created = store
                .create_ticket(
                    "Policy".to_string(),
                    "Persist explicit approval policy.".to_string(),
                    TicketSource::Human,
                    ActorRef::human("vmjcv"),
                )
                .await
                .unwrap();
            store
                .set_approval_policy(
                    &created.id,
                    ActorRef::human("vmjcv"),
                    ApprovalPolicy::ManualOnly,
                )
                .await
                .unwrap();
            created
        };

        let reopened = SqliteTicketStore::open(&path).unwrap();
        let ticket = reopened.get_ticket(&created.id).await.unwrap();
        assert_eq!(ticket.approval_policy, ApprovalPolicy::ManualOnly);
        let events = reopened.ticket_events(&created.id).await.unwrap();
        assert_eq!(events.last().unwrap().kind, TicketEventKind::PolicyUpdated);

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn sqlite_store_initializes_schema_migration_metadata() {
        let path = temp_store_path("tea-store-schema-meta");

        {
            let store = SqliteTicketStore::open(&path).unwrap();
            store
                .create_ticket(
                    "Metadata".to_string(),
                    "Schema version metadata should be durable.".to_string(),
                    TicketSource::Human,
                    ActorRef::human("vmjcv"),
                )
                .await
                .unwrap();
        }

        let conn = Connection::open(&path).unwrap();
        let version: i64 = conn
            .query_row(
                "SELECT version FROM schema_migrations ORDER BY version DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, CURRENT_SQLITE_SCHEMA_VERSION);

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn sqlite_store_reopen_keeps_single_schema_migration_record() {
        let path = temp_store_path("tea-store-schema-reopen");

        {
            let _store = SqliteTicketStore::open(&path).unwrap();
        }
        {
            let _store = SqliteTicketStore::open(&path).unwrap();
        }

        let conn = Connection::open(&path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn sqlite_store_upgrades_legacy_schema_without_metadata() {
        let path = temp_store_path("tea-store-legacy-schema");
        {
            let conn = Connection::open(&path).unwrap();
            create_sqlite_v1_schema(&conn).unwrap();
            assert!(!sqlite_table_exists(&conn, "schema_migrations").unwrap());
        }

        let _store = SqliteTicketStore::open(&path).unwrap();

        let conn = Connection::open(&path).unwrap();
        let version: i64 = conn
            .query_row(
                "SELECT version FROM schema_migrations ORDER BY version DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, CURRENT_SQLITE_SCHEMA_VERSION);

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn sqlite_store_rejects_future_schema_version() {
        let path = temp_store_path("tea-store-future-schema");
        {
            let conn = Connection::open(&path).unwrap();
            create_sqlite_v1_schema(&conn).unwrap();
            ensure_schema_migrations_table(&conn).unwrap();
            record_sqlite_schema_version(&conn, CURRENT_SQLITE_SCHEMA_VERSION + 1).unwrap();
        }

        let error = match SqliteTicketStore::open(&path) {
            Ok(_) => panic!("future schema version should be rejected"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            StoreError::UnsupportedSchemaVersion { found, supported }
                if found == CURRENT_SQLITE_SCHEMA_VERSION + 1
                    && supported == CURRENT_SQLITE_SCHEMA_VERSION
        ));

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn sqlite_store_rolls_back_schema_changes_when_migration_record_fails() {
        let path = temp_store_path("tea-store-migration-rollback");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at TEXT NOT NULL
                );
                "#,
            )
            .unwrap();
        }

        let error = match SqliteTicketStore::open(&path) {
            Ok(_) => panic!("migration should fail when schema_migrations cannot record a version"),
            Err(error) => error,
        };
        match error {
            StoreError::Database(error) => {
                assert!(error.to_string().contains("NOT NULL constraint failed"));
            }
            other => panic!("expected sqlite constraint error, got {other:?}"),
        }

        let conn = Connection::open(&path).unwrap();
        assert!(!sqlite_table_exists(&conn, "tickets").unwrap());
        assert!(!sqlite_table_exists(&conn, "comments").unwrap());
        assert!(!sqlite_table_exists(&conn, "events").unwrap());

        let _ = std::fs::remove_file(path);
    }

    async fn create_closed_ticket<S>(store: &S) -> Ticket
    where
        S: TicketStore,
    {
        let created = store
            .create_ticket(
                "Closed".to_string(),
                "This work order has finished.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        store
            .add_run(&created.id, ActorRef::system(), successful_run(&created.id))
            .await
            .unwrap();
        store
            .close_ticket(&created.id, ActorRef::human("vmjcv"))
            .await
            .unwrap()
    }

    fn successful_run(ticket_id: &TicketId) -> Run {
        Run {
            id: RunId::new(),
            ticket_id: ticket_id.clone(),
            loom_session_id: Some("mock".to_string()),
            status: RunStatus::Succeeded,
            evidence: Some(RunEvidence {
                summary: "done".to_string(),
                commands: vec![],
                artifacts: vec![],
                risks: vec![],
            }),
        }
    }

    fn temp_store_path(prefix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "{}-{}-{}.sqlite",
            prefix,
            std::process::id(),
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .expect("current timestamp should fit in nanos")
        ))
    }

    fn sqlite_table_exists(conn: &Connection, table_name: &str) -> Result<bool, StoreError> {
        let exists = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            params![table_name],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(exists == 1)
    }
}
