use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tea_core::{
    ActorRef, ApprovalPolicy, Plan, Run, RunId, Ticket, TicketAnalysis, TicketComment,
    TicketCreateOptions, TicketEdits, TicketEvent, TicketEventId, TicketEventKind, TicketId,
    TicketSource, TicketStatus,
};

use crate::helpers::{
    decode, decode_value_map, decode_values, encode, ensure_run_belongs_to_ticket,
    ensure_run_update, ensure_ticket_can_run, ensure_ticket_mutable_for, finish_page,
    run_creation_event_kinds, run_outcome_event_kinds, sync_ticket_status_from_run,
    validate_ticket_page_request,
};
use crate::{
    IdempotencyRequest, StoreError, StoreStatus, TicketBundle, TicketListPage, TicketMetrics,
    TicketMetricsPage, TicketPageRequest, TicketStore,
};

pub(crate) const CURRENT_SQLITE_SCHEMA_VERSION: i64 = 4;

/// Builds the ticket list page SQL for the given filter combination. SQLite
/// plans a statement before parameters are bound, so a single statement with
/// `(?N IS NULL OR status = ?N)` guards can never use the v4 filter indexes;
/// each filter combination gets its own (cached) statement instead. Bind
/// parameters in order: cursor ordinal, status (if filtered), source (if
/// filtered), limit.
pub(crate) fn sqlite_list_tickets_page_sql(filter_status: bool, filter_source: bool) -> String {
    let mut sql = String::from("SELECT ordinal, json FROM tickets WHERE ordinal > COALESCE(?, -1)");
    if filter_status {
        sql.push_str(" AND status = ?");
    }
    if filter_source {
        sql.push_str(" AND source = ?");
    }
    sql.push_str(" ORDER BY ordinal ASC LIMIT ?");
    sql
}

/// Builds the ticket metrics page SQL for the given filter combination; see
/// `sqlite_list_tickets_page_sql` for the per-combination rationale and bind
/// parameter order.
pub(crate) fn sqlite_ticket_metrics_page_sql(filter_status: bool, filter_source: bool) -> String {
    let mut sql = String::from(
        "SELECT t.ordinal, t.json, \
        (SELECT COUNT(*) FROM comments c WHERE c.ticket_id = t.id), \
        (SELECT COUNT(*) FROM runs r WHERE r.ticket_id = t.id), \
        (SELECT c.json FROM comments c WHERE c.ticket_id = t.id ORDER BY c.ordinal DESC LIMIT 1), \
        (SELECT e.json FROM events e WHERE e.ticket_id = t.id ORDER BY e.ordinal DESC LIMIT 1) \
     FROM tickets t \
     WHERE t.ordinal > COALESCE(?, -1)",
    );
    if filter_status {
        sql.push_str(" AND t.status = ?");
    }
    if filter_source {
        sql.push_str(" AND t.source = ?");
    }
    sql.push_str(" ORDER BY t.ordinal ASC LIMIT ?");
    sql
}

pub(crate) struct RawTicketBundle {
    ticket: String,
    comments: Vec<String>,
    events: Vec<String>,
    runs: Vec<String>,
    analysis: Option<String>,
    plan: Option<String>,
}

#[derive(Clone)]
pub struct SqliteTicketStore {
    pub(crate) conn: Arc<Mutex<Connection>>,
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

    /// Runs blocking rusqlite work on the tokio blocking pool so store calls do
    /// not stall async worker threads. The connection mutex is locked inside the
    /// blocking task, keeping lock waits off the async runtime as well.
    async fn with_conn<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, StoreError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().map_err(|_| StoreError::LockPoisoned)?;
            f(&mut guard)
        })
        .await
        .map_err(|error| {
            StoreError::Io(std::io::Error::other(format!(
                "sqlite store task failed: {error}"
            )))
        })?
    }
}

#[async_trait]
impl TicketStore for SqliteTicketStore {
    async fn store_status(&self) -> Result<StoreStatus, StoreError> {
        self.with_conn(|conn| {
            let idempotency_key_count =
                conn.query_row("SELECT COUNT(*) FROM idempotency_keys", [], |row| {
                    row.get::<_, u64>(0)
                })?;
            let sqlite_page_count =
                conn.query_row("PRAGMA page_count", [], |row| row.get::<_, u64>(0))?;
            let sqlite_freelist_count =
                conn.query_row("PRAGMA freelist_count", [], |row| row.get::<_, u64>(0))?;
            Ok(StoreStatus::sqlite(
                applied_sqlite_schema_version(conn)?,
                CURRENT_SQLITE_SCHEMA_VERSION,
                idempotency_key_count,
                sqlite_page_count,
                sqlite_freelist_count,
            ))
        })
        .await
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
        self.with_conn(move |db| {
            let conn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(request) = idempotency.as_ref() {
                let existing = conn
                    .query_row(
                        "SELECT request_hash, response_json FROM idempotency_keys \
                         WHERE scope = ?1 AND key = ?2",
                        params![request.scope, request.key],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()?;
                if let Some((request_hash, response_json)) = existing {
                    if request_hash != request.request_hash {
                        return Err(StoreError::IdempotencyConflict);
                    }
                    let ticket = decode(response_json)?;
                    conn.commit()?;
                    return Ok(ticket);
                }
            }
            let ticket = Ticket::new_with_options(
                TicketId::new(),
                title,
                description,
                source,
                actor.clone(),
                approval_policy,
                options,
            );
            insert_ticket(&conn, &ticket)?;
            push_sqlite_event(&conn, &ticket.id, actor, TicketEventKind::TicketCreated)?;
            if let Some(request) = idempotency {
                conn.execute(
                    "INSERT INTO idempotency_keys \
                     (scope, key, request_hash, ticket_id, response_json) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        request.scope,
                        request.key,
                        request.request_hash,
                        ticket.id.to_string(),
                        encode(&ticket)?
                    ],
                )?;
            }
            conn.commit()?;
            Ok(ticket)
        })
        .await
    }

    async fn list_tickets(&self) -> Result<Vec<Ticket>, StoreError> {
        let json = self
            .with_conn(|conn| {
                let mut statement =
                    conn.prepare_cached("SELECT json FROM tickets ORDER BY ordinal ASC")?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                collect_json_rows(rows)
            })
            .await?;
        decode_values(json)
    }

    async fn list_tickets_page(
        &self,
        request: TicketPageRequest,
    ) -> Result<TicketListPage, StoreError> {
        let query_limit = validate_ticket_page_request(request)?;
        let (status, source) = sqlite_page_filter_values(request)?;
        let page_limit = request.limit;
        let raw_rows = self
            .with_conn(move |conn| {
                let raw_rows = {
                    let sql = sqlite_list_tickets_page_sql(status.is_some(), source.is_some());
                    let mut statement = conn.prepare_cached(&sql)?;
                    let binds =
                        ticket_page_binds(&request.after_ordinal, &status, &source, &query_limit);
                    let rows = statement.query_map(binds.as_slice(), |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                    })?;
                    rows.collect::<Result<Vec<_>, _>>()?
                };
                Ok(raw_rows)
            })
            .await?;
        let entries = raw_rows
            .into_iter()
            .map(|(ordinal, json)| Ok((ordinal, decode(json)?)))
            .collect::<Result<Vec<_>, StoreError>>()?;
        let (items, next_ordinal) = finish_page(entries, page_limit);
        Ok(TicketListPage {
            items,
            next_ordinal,
        })
    }

    async fn ticket_metrics(&self) -> Result<Vec<TicketMetrics>, StoreError> {
        let (ticket_json, comment_counts, run_counts, latest_comment_json, latest_event_json) =
            self.with_conn(|db| {
                let conn = db.transaction()?;
                // A constant number of aggregate queries (not one-per-ticket): counts via
                // GROUP BY and the latest comment/event via MAX(ordinal), all backed by the
                // ticket_ordinal indexes.
                let ticket_json = {
                    let mut statement =
                        conn.prepare_cached("SELECT json FROM tickets ORDER BY ordinal ASC")?;
                    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                    collect_json_rows(rows)?
                };
                let comment_counts = count_by_ticket(&conn, "comments")?;
                let run_counts = count_by_ticket(&conn, "runs")?;
                let latest_comment_json = latest_json_by_ticket(&conn, "comments")?;
                let latest_event_json = latest_json_by_ticket(&conn, "events")?;
                conn.commit()?;
                Ok((
                    ticket_json,
                    comment_counts,
                    run_counts,
                    latest_comment_json,
                    latest_event_json,
                ))
            })
            .await?;
        let tickets: Vec<Ticket> = decode_values(ticket_json)?;
        let latest_comments: BTreeMap<String, TicketComment> =
            decode_value_map(latest_comment_json)?;
        let latest_events: BTreeMap<String, TicketEvent> = decode_value_map(latest_event_json)?;
        Ok(tickets
            .into_iter()
            .map(|ticket| {
                let key = ticket.id.to_string();
                TicketMetrics {
                    comments_count: comment_counts.get(&key).copied().unwrap_or(0),
                    runs_count: run_counts.get(&key).copied().unwrap_or(0),
                    latest_comment: latest_comments.get(&key).cloned(),
                    latest_event: latest_events.get(&key).cloned(),
                    ticket_id: ticket.id,
                }
            })
            .collect())
    }

    async fn ticket_metrics_page(
        &self,
        request: TicketPageRequest,
    ) -> Result<TicketMetricsPage, StoreError> {
        let query_limit = validate_ticket_page_request(request)?;
        let (status, source) = sqlite_page_filter_values(request)?;
        let page_limit = request.limit;
        let raw_rows = self
            .with_conn(move |conn| {
                let raw_rows = {
                    // Each correlated lookup is bounded by the at-most-201-row keyset page and
                    // probes a ticket-scoped index. Materializing aggregate CTEs instead makes
                    // SQLite scan the complete comments, runs, and events tables for every page.
                    let sql = sqlite_ticket_metrics_page_sql(status.is_some(), source.is_some());
                    let mut statement = conn.prepare_cached(&sql)?;
                    let binds =
                        ticket_page_binds(&request.after_ordinal, &status, &source, &query_limit);
                    let rows = statement.query_map(binds.as_slice(), |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                        ))
                    })?;
                    rows.collect::<Result<Vec<_>, _>>()?
                };
                Ok(raw_rows)
            })
            .await?;
        let entries = raw_rows
            .into_iter()
            .map(
                |(
                    ordinal,
                    ticket_json,
                    comments_count,
                    runs_count,
                    latest_comment,
                    latest_event,
                )| {
                    let ticket: Ticket = decode(ticket_json)?;
                    Ok((
                        ordinal,
                        TicketMetrics {
                            ticket_id: ticket.id,
                            comments_count: comments_count.max(0) as usize,
                            runs_count: runs_count.max(0) as usize,
                            latest_comment: latest_comment.map(decode).transpose()?,
                            latest_event: latest_event.map(decode).transpose()?,
                        },
                    ))
                },
            )
            .collect::<Result<Vec<_>, StoreError>>()?;
        let (items, next_ordinal) = finish_page(entries, page_limit);
        Ok(TicketMetricsPage {
            items,
            next_ordinal,
        })
    }

    async fn ticket_bundle(&self, id: &TicketId) -> Result<TicketBundle, StoreError> {
        let id = id.clone();
        let raw = self
            .with_conn(move |db| {
                let conn = db.transaction()?;
                // Materialize one SQLite snapshot inside the transaction. The raw JSON is
                // returned before decoding so the connection mutex is released promptly.
                let ticket = conn
                    .query_row(
                        "SELECT json FROM tickets WHERE id = ?1",
                        params![id.to_string()],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?
                    .ok_or(StoreError::TicketNotFound)?;
                let comments = {
                    let mut statement = conn.prepare_cached(
                        "SELECT json FROM comments WHERE ticket_id = ?1 ORDER BY ordinal ASC",
                    )?;
                    let rows = statement
                        .query_map(params![id.to_string()], |row| row.get::<_, String>(0))?;
                    collect_json_rows(rows)?
                };
                let events = {
                    let mut statement = conn.prepare_cached(
                        "SELECT json FROM events WHERE ticket_id = ?1 ORDER BY ordinal ASC",
                    )?;
                    let rows = statement
                        .query_map(params![id.to_string()], |row| row.get::<_, String>(0))?;
                    collect_json_rows(rows)?
                };
                let runs = {
                    let mut statement = conn.prepare_cached(
                        "SELECT json FROM runs WHERE ticket_id = ?1 ORDER BY ordinal ASC",
                    )?;
                    let rows = statement
                        .query_map(params![id.to_string()], |row| row.get::<_, String>(0))?;
                    collect_json_rows(rows)?
                };
                let analysis = conn
                    .query_row(
                        "SELECT json FROM analyses WHERE ticket_id = ?1",
                        params![id.to_string()],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;
                let plan = conn
                    .query_row(
                        "SELECT json FROM plans WHERE ticket_id = ?1",
                        params![id.to_string()],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;
                conn.commit()?;
                let raw = RawTicketBundle {
                    ticket,
                    comments,
                    events,
                    runs,
                    analysis,
                    plan,
                };
                Ok(raw)
            })
            .await?;
        Ok(TicketBundle {
            ticket: decode(raw.ticket)?,
            comments: decode_values(raw.comments)?,
            events: decode_values(raw.events)?,
            runs: decode_values(raw.runs)?,
            analysis: raw.analysis.map(decode).transpose()?,
            plan: raw.plan.map(decode).transpose()?,
        })
    }

    async fn get_ticket(&self, id: &TicketId) -> Result<Ticket, StoreError> {
        let id = id.clone();
        let json = self
            .with_conn(move |conn| get_sqlite_ticket_json(conn, &id))
            .await?;
        decode(json)
    }

    async fn ticket_events(&self, id: &TicketId) -> Result<Vec<TicketEvent>, StoreError> {
        let id = id.clone();
        let json = self
            .with_conn(move |conn| {
                ensure_sqlite_ticket(conn, &id)?;
                let mut statement = conn.prepare_cached(
                    "SELECT json FROM events WHERE ticket_id = ?1 ORDER BY ordinal ASC",
                )?;
                let rows =
                    statement.query_map(params![id.to_string()], |row| row.get::<_, String>(0))?;
                collect_json_rows(rows)
            })
            .await?;
        decode_values(json)
    }

    async fn ticket_comments(&self, id: &TicketId) -> Result<Vec<TicketComment>, StoreError> {
        let id = id.clone();
        let json = self
            .with_conn(move |conn| {
                ensure_sqlite_ticket(conn, &id)?;
                let mut statement = conn.prepare_cached(
                    "SELECT json FROM comments WHERE ticket_id = ?1 ORDER BY ordinal ASC",
                )?;
                let rows =
                    statement.query_map(params![id.to_string()], |row| row.get::<_, String>(0))?;
                collect_json_rows(rows)
            })
            .await?;
        decode_values(json)
    }

    async fn add_comment(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        body: String,
    ) -> Result<TicketComment, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |db| {
            let conn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut ticket = get_sqlite_ticket(&conn, &ticket_id)?;
            ensure_ticket_mutable_for(&ticket, "add comment to")?;
            ticket.touch();
            update_ticket(&conn, &ticket)?;
            let comment = TicketComment::new(ticket_id.clone(), actor.clone(), body);
            let ordinal = next_scoped_ordinal(&conn, "comments", &ticket_id)?;
            conn.execute(
                "INSERT INTO comments (id, ticket_id, ordinal, json) VALUES (?1, ?2, ?3, ?4)",
                params![
                    comment.id.0.to_string(),
                    ticket_id.to_string(),
                    ordinal,
                    encode(&comment)?
                ],
            )?;
            push_sqlite_event(&conn, &ticket_id, actor, TicketEventKind::CommentAdded)?;
            conn.commit()?;
            Ok(comment)
        })
        .await
    }

    async fn set_analysis(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        analysis: TicketAnalysis,
    ) -> Result<TicketAnalysis, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |db| {
            let conn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut ticket = get_sqlite_ticket(&conn, &ticket_id)?;
            ensure_ticket_mutable_for(&ticket, "analyze")?;
            let policy_changed = ticket.approval_policy != analysis.recommended_policy;
            ticket.status = if analysis.missing_context.is_empty() {
                TicketStatus::AnalysisReady
            } else {
                TicketStatus::NeedsInfo
            };
            ticket.risk_level = analysis.risk_assessment;
            ticket.set_approval_policy(analysis.recommended_policy);
            update_ticket(&conn, &ticket)?;
            if policy_changed {
                conn.execute(
                    "DELETE FROM approvals WHERE ticket_id = ?1",
                    params![ticket_id.to_string()],
                )?;
            }
            conn.execute(
                "INSERT INTO analyses (ticket_id, json) VALUES (?1, ?2)
                 ON CONFLICT(ticket_id) DO UPDATE SET json = excluded.json",
                params![ticket_id.to_string(), encode(&analysis)?],
            )?;
            push_sqlite_event(&conn, &ticket_id, actor, TicketEventKind::TicketAnalyzed)?;
            conn.commit()?;
            Ok(analysis)
        })
        .await
    }

    async fn set_plan(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        plan: Plan,
    ) -> Result<Plan, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |db| {
            let conn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut ticket = get_sqlite_ticket(&conn, &ticket_id)?;
            ensure_ticket_mutable_for(&ticket, "plan")?;
            if ticket.status != TicketStatus::NeedsInfo {
                ticket.status = if plan.requires_approval_before_execute {
                    TicketStatus::AwaitingApproval
                } else {
                    TicketStatus::PlanReady
                };
            }
            ticket.touch();
            update_ticket(&conn, &ticket)?;
            conn.execute(
                "INSERT INTO plans (ticket_id, json) VALUES (?1, ?2)
                 ON CONFLICT(ticket_id) DO UPDATE SET json = excluded.json",
                params![ticket_id.to_string(), encode(&plan)?],
            )?;
            push_sqlite_event(&conn, &ticket_id, actor, TicketEventKind::PlanProposed)?;
            conn.commit()?;
            Ok(plan)
        })
        .await
    }

    async fn ticket_analysis(
        &self,
        ticket_id: &TicketId,
    ) -> Result<Option<TicketAnalysis>, StoreError> {
        let ticket_id = ticket_id.clone();
        let json = self
            .with_conn(move |conn| {
                ensure_sqlite_ticket(conn, &ticket_id)?;
                Ok(conn
                    .query_row(
                        "SELECT json FROM analyses WHERE ticket_id = ?1",
                        params![ticket_id.to_string()],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?)
            })
            .await?;
        json.map(decode).transpose()
    }

    async fn ticket_plan(&self, ticket_id: &TicketId) -> Result<Option<Plan>, StoreError> {
        let ticket_id = ticket_id.clone();
        let json = self
            .with_conn(move |conn| {
                ensure_sqlite_ticket(conn, &ticket_id)?;
                Ok(conn
                    .query_row(
                        "SELECT json FROM plans WHERE ticket_id = ?1",
                        params![ticket_id.to_string()],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?)
            })
            .await?;
        json.map(decode).transpose()
    }

    async fn set_approval_policy(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        policy: ApprovalPolicy,
    ) -> Result<Ticket, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |db| {
            let conn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut ticket = get_sqlite_ticket(&conn, &ticket_id)?;
            ensure_ticket_mutable_for(&ticket, "update policy for")?;
            let policy_changed = ticket.approval_policy != policy;
            ticket.set_approval_policy(policy);
            update_ticket(&conn, &ticket)?;
            if policy_changed {
                conn.execute(
                    "DELETE FROM approvals WHERE ticket_id = ?1",
                    params![ticket_id.to_string()],
                )?;
            }
            push_sqlite_event(&conn, &ticket_id, actor, TicketEventKind::PolicyUpdated)?;
            conn.commit()?;
            Ok(ticket)
        })
        .await
    }

    async fn update_ticket_fields(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        edits: TicketEdits,
    ) -> Result<Ticket, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |db| {
            let conn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut ticket = get_sqlite_ticket(&conn, &ticket_id)?;
            ensure_ticket_mutable_for(&ticket, "edit")?;
            let changed = ticket.apply_edits(edits);
            if changed {
                update_ticket(&conn, &ticket)?;
                push_sqlite_event(&conn, &ticket_id, actor, TicketEventKind::TicketEdited)?;
            }
            conn.commit()?;
            Ok(ticket)
        })
        .await
    }

    async fn grant_approval(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |db| {
            let conn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut ticket = get_sqlite_ticket(&conn, &ticket_id)?;
            ensure_ticket_mutable_for(&ticket, "approve")?;
            if !matches!(ticket.status, TicketStatus::Completed | TicketStatus::Accepted) {
                ticket.status = TicketStatus::Approved;
            }
            ticket.touch();
            update_ticket(&conn, &ticket)?;
            conn.execute(
                "INSERT INTO approvals (ticket_id, approved, rejection_reason) VALUES (?1, 1, NULL)
                 ON CONFLICT(ticket_id) DO UPDATE SET approved = excluded.approved, rejection_reason = NULL",
                params![ticket_id.to_string()],
            )?;
            push_sqlite_event(&conn, &ticket_id, actor, TicketEventKind::ApprovalGranted)?;
            conn.commit()?;
            Ok(ticket)
        })
        .await
    }

    async fn reject_approval(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        reason: String,
    ) -> Result<Ticket, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |db| {
            let conn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut ticket = get_sqlite_ticket(&conn, &ticket_id)?;
            ensure_ticket_mutable_for(&ticket, "reject approval for")?;
            ticket.status = TicketStatus::Blocked;
            ticket.touch();
            update_ticket(&conn, &ticket)?;
            conn.execute(
                "INSERT INTO approvals (ticket_id, approved, rejection_reason) VALUES (?1, 0, ?2)
                 ON CONFLICT(ticket_id) DO UPDATE SET approved = excluded.approved, rejection_reason = excluded.rejection_reason",
                params![ticket_id.to_string(), reason],
            )?;
            push_sqlite_event(&conn, &ticket_id, actor, TicketEventKind::ApprovalRejected)?;
            conn.commit()?;
            Ok(ticket)
        })
        .await
    }

    async fn has_approval(&self, ticket_id: &TicketId) -> Result<bool, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |conn| {
            ensure_sqlite_ticket(conn, &ticket_id)?;
            let approved = conn
                .query_row(
                    "SELECT approved FROM approvals WHERE ticket_id = ?1",
                    params![ticket_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            Ok(approved.unwrap_or(0) == 1)
        })
        .await
    }

    async fn add_run(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        run: Run,
    ) -> Result<Run, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |db| {
            let conn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            ensure_sqlite_run_id_available(&conn, &run.id)?;
            let mut ticket = get_sqlite_ticket(&conn, &ticket_id)?;
            ensure_ticket_can_run(&ticket)?;
            ensure_run_belongs_to_ticket(&run, &ticket_id)?;
            sync_ticket_status_from_run(&mut ticket, run.status);
            update_ticket(&conn, &ticket)?;
            insert_run(&conn, &ticket_id, &run)?;
            for kind in run_creation_event_kinds(&run) {
                push_sqlite_event(&conn, &ticket_id, actor.clone(), kind)?;
            }
            conn.commit()?;
            Ok(run)
        })
        .await
    }

    async fn update_run(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        run: Run,
    ) -> Result<Run, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |db| {
            let conn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            apply_sqlite_run_update(&conn, &ticket_id, actor, None, false, &run)?;
            conn.commit()?;
            Ok(run)
        })
        .await
    }

    async fn update_run_if_unchanged(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        expected: Run,
        run: Run,
    ) -> Result<Run, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |db| {
            let conn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            apply_sqlite_run_update(&conn, &ticket_id, actor, Some(&expected), false, &run)?;
            conn.commit()?;
            Ok(run)
        })
        .await
    }

    async fn update_latest_run_if_unchanged(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        expected: Run,
        run: Run,
    ) -> Result<Run, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |db| {
            let conn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            apply_sqlite_run_update(&conn, &ticket_id, actor, Some(&expected), true, &run)?;
            conn.commit()?;
            Ok(run)
        })
        .await
    }

    async fn list_runs(&self, ticket_id: &TicketId) -> Result<Vec<Run>, StoreError> {
        let ticket_id = ticket_id.clone();
        let json = self
            .with_conn(move |conn| list_sqlite_run_json(conn, &ticket_id))
            .await?;
        decode_values(json)
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Run, StoreError> {
        let run_id = run_id.clone();
        let json = self
            .with_conn(move |conn| get_sqlite_run_json(conn, &run_id))
            .await?;
        decode(json)
    }

    async fn has_run_evidence(&self, ticket_id: &TicketId) -> Result<bool, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |conn| {
            ensure_sqlite_ticket(conn, &ticket_id)?;
            sqlite_has_run_evidence(conn, &ticket_id)
        })
        .await
    }

    async fn latest_run(&self, ticket_id: &TicketId) -> Result<Option<Run>, StoreError> {
        let ticket_id = ticket_id.clone();
        let row = self
            .with_conn(move |conn| {
                ensure_sqlite_ticket(conn, &ticket_id)?;
                latest_sqlite_run_row(conn, &ticket_id)
            })
            .await?;
        row.map(|(_, json)| decode(json)).transpose()
    }

    async fn accept_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |db| {
            let conn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut ticket = get_sqlite_ticket(&conn, &ticket_id)?;
            ensure_ticket_mutable_for(&ticket, "accept")?;
            if !sqlite_has_run_evidence(&conn, &ticket_id)? {
                return Err(StoreError::EvidenceRequired);
            }
            if ticket.status != TicketStatus::Completed {
                return Err(StoreError::InvalidTransition(
                    "only a completed ticket can be accepted".to_string(),
                ));
            }
            ticket.status = TicketStatus::Accepted;
            ticket.touch();
            update_ticket(&conn, &ticket)?;
            push_sqlite_event(&conn, &ticket_id, actor, TicketEventKind::HumanAccepted)?;
            conn.commit()?;
            Ok(ticket)
        })
        .await
    }

    async fn close_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |db| {
            let conn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut ticket = get_sqlite_ticket(&conn, &ticket_id)?;
            ensure_ticket_mutable_for(&ticket, "close")?;
            if !sqlite_has_run_evidence(&conn, &ticket_id)? {
                return Err(StoreError::EvidenceRequired);
            }
            if !matches!(
                ticket.status,
                TicketStatus::Completed | TicketStatus::Accepted
            ) {
                return Err(StoreError::InvalidTransition(
                    "only a completed or accepted ticket can be closed".to_string(),
                ));
            }
            if ticket.approval_policy == ApprovalPolicy::HumanBeforeCompletion {
                let approved = conn
                    .query_row(
                        "SELECT approved FROM approvals WHERE ticket_id = ?1",
                        params![ticket_id.to_string()],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?
                    .unwrap_or(0)
                    == 1;
                if !approved {
                    return Err(StoreError::ApprovalRequired);
                }
            }
            ticket.status = TicketStatus::Closed;
            ticket.touch();
            update_ticket(&conn, &ticket)?;
            push_sqlite_event(&conn, &ticket_id, actor, TicketEventKind::TicketClosed)?;
            conn.commit()?;
            Ok(ticket)
        })
        .await
    }

    async fn cancel_ticket(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
    ) -> Result<Ticket, StoreError> {
        let ticket_id = ticket_id.clone();
        self.with_conn(move |db| {
            let conn = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut ticket = get_sqlite_ticket(&conn, &ticket_id)?;
            ensure_ticket_mutable_for(&ticket, "cancel")?;
            ticket.status = TicketStatus::Cancelled;
            ticket.touch();
            update_ticket(&conn, &ticket)?;
            push_sqlite_event(&conn, &ticket_id, actor, TicketEventKind::TicketCancelled)?;
            conn.commit()?;
            Ok(ticket)
        })
        .await
    }
}

pub(crate) fn init_sqlite(conn: &mut Connection) -> Result<(), StoreError> {
    const SQLITE_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
    conn.busy_timeout(SQLITE_LOCK_WAIT)?;
    let deadline = std::time::Instant::now() + SQLITE_LOCK_WAIT;
    loop {
        match init_sqlite_once(conn) {
            Ok(()) => return Ok(()),
            Err(error)
                if is_sqlite_lock_contention(&error) && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(error) => return Err(error),
        }
    }
}

pub(crate) fn init_sqlite_once(conn: &mut Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;
        -- Safe with WAL: only fsyncs the WAL at checkpoints, dramatically reducing
        -- per-write fsync cost while preserving durability across app crashes.
        PRAGMA synchronous = NORMAL;
        -- Keep temporary indices/tables in memory rather than spilling to disk.
        PRAGMA temp_store = MEMORY;
        "#,
    )?;
    ensure_schema_migrations_table(conn)?;
    apply_sqlite_migrations(conn)?;
    // Repair required unique indexes and remove obsolete same-column indexes on
    // every open. The latter otherwise double SQLite's ordinal-index write work.
    ensure_sqlite_indexes(conn)?;
    Ok(())
}

pub(crate) fn is_sqlite_lock_contention(error: &StoreError) -> bool {
    matches!(
        error,
        StoreError::Database(rusqlite::Error::SqliteFailure(inner, _))
            if matches!(
                inner.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            )
    )
}

pub(crate) fn ensure_sqlite_indexes(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"
        CREATE UNIQUE INDEX IF NOT EXISTS uidx_tickets_ordinal ON tickets(ordinal);
        CREATE UNIQUE INDEX IF NOT EXISTS uidx_comments_ticket_ordinal ON comments(ticket_id, ordinal);
        CREATE UNIQUE INDEX IF NOT EXISTS uidx_events_ticket_ordinal ON events(ticket_id, ordinal);
        CREATE UNIQUE INDEX IF NOT EXISTS uidx_runs_ticket_ordinal ON runs(ticket_id, ordinal);

        DROP INDEX IF EXISTS idx_tickets_ordinal;
        DROP INDEX IF EXISTS idx_comments_ticket_ordinal;
        DROP INDEX IF EXISTS idx_events_ticket_ordinal;
        DROP INDEX IF EXISTS idx_runs_ticket_ordinal;
        "#,
    )?;
    Ok(())
}

pub(crate) fn ensure_schema_migrations_table(conn: &Connection) -> Result<(), StoreError> {
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

pub(crate) fn apply_sqlite_migrations(conn: &mut Connection) -> Result<(), StoreError> {
    // Serialize first-open migrations across processes before reading the current
    // version. A deferred transaction can let two processes both observe version 0.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let applied = applied_sqlite_schema_version(&tx)?;
    if applied > CURRENT_SQLITE_SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchemaVersion {
            found: applied,
            supported: CURRENT_SQLITE_SCHEMA_VERSION,
        });
    }
    if applied < 1 {
        create_sqlite_v1_schema(&tx)?;
        record_sqlite_schema_version(&tx, 1)?;
    }
    if applied < 2 {
        create_sqlite_v2_schema(&tx)?;
        record_sqlite_schema_version(&tx, 2)?;
    }
    if applied < 3 {
        create_sqlite_v3_schema(&tx)?;
        record_sqlite_schema_version(&tx, 3)?;
    }
    if applied < 4 {
        create_sqlite_v4_schema(&tx)?;
        record_sqlite_schema_version(&tx, 4)?;
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn applied_sqlite_schema_version(conn: &Connection) -> Result<i64, StoreError> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get::<_, i64>(0),
    )?)
}

pub(crate) fn record_sqlite_schema_version(
    conn: &Connection,
    version: i64,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO schema_migrations (version) VALUES (?1)",
        params![version],
    )?;
    Ok(())
}

pub(crate) fn create_sqlite_v1_schema(conn: &Connection) -> Result<(), StoreError> {
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
        "#,
    )?;
    // Ordinal indexes are created by v2 and repaired on every open.
    Ok(())
}

pub(crate) fn create_sqlite_v2_schema(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"
        CREATE UNIQUE INDEX IF NOT EXISTS uidx_tickets_ordinal ON tickets(ordinal);
        CREATE UNIQUE INDEX IF NOT EXISTS uidx_comments_ticket_ordinal ON comments(ticket_id, ordinal);
        CREATE UNIQUE INDEX IF NOT EXISTS uidx_events_ticket_ordinal ON events(ticket_id, ordinal);
        CREATE UNIQUE INDEX IF NOT EXISTS uidx_runs_ticket_ordinal ON runs(ticket_id, ordinal);
        "#,
    )?;
    Ok(())
}

pub(crate) fn create_sqlite_v3_schema(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS idempotency_keys (
            scope TEXT NOT NULL,
            key TEXT NOT NULL,
            request_hash TEXT NOT NULL,
            ticket_id TEXT NOT NULL,
            response_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            PRIMARY KEY(scope, key),
            FOREIGN KEY(ticket_id) REFERENCES tickets(id)
        );
        "#,
    )?;
    Ok(())
}

/// v4: indexed status/source ticket filters. The generated columns must be
/// VIRTUAL because SQLite only supports ADD COLUMN for virtual generated
/// columns (STORED would require a table rebuild); indexes over virtual
/// columns are materialized, so filtered pages become index searches instead
/// of json_extract calls over every scanned row.
pub(crate) fn create_sqlite_v4_schema(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"
        ALTER TABLE tickets ADD COLUMN status TEXT
            GENERATED ALWAYS AS (json_extract(json, '$.status')) VIRTUAL;
        ALTER TABLE tickets ADD COLUMN source TEXT
            GENERATED ALWAYS AS (json_extract(json, '$.source')) VIRTUAL;
        CREATE INDEX IF NOT EXISTS idx_tickets_status_ordinal ON tickets(status, ordinal);
        CREATE INDEX IF NOT EXISTS idx_tickets_source_ordinal ON tickets(source, ordinal);
        "#,
    )?;
    Ok(())
}

pub(crate) fn sqlite_page_filter_values(
    request: TicketPageRequest,
) -> Result<(Option<String>, Option<String>), StoreError> {
    Ok((
        serialized_page_filter(request.status)?,
        serialized_page_filter(request.source)?,
    ))
}

/// Bind values matching the parameter order of the page SQL builders: cursor
/// ordinal, then each present filter, then the limit.
pub(crate) fn ticket_page_binds<'a>(
    after_ordinal: &'a Option<i64>,
    status: &'a Option<String>,
    source: &'a Option<String>,
    query_limit: &'a i64,
) -> Vec<&'a dyn rusqlite::ToSql> {
    let mut binds: Vec<&dyn rusqlite::ToSql> = vec![after_ordinal];
    if let Some(status) = status {
        binds.push(status);
    }
    if let Some(source) = source {
        binds.push(source);
    }
    binds.push(query_limit);
    binds
}

pub(crate) fn serialized_page_filter<T: Serialize>(
    value: Option<T>,
) -> Result<Option<String>, StoreError> {
    let Some(value) = value else {
        return Ok(None);
    };
    match serde_json::to_value(value)? {
        serde_json::Value::String(value) => Ok(Some(value)),
        _ => Err(StoreError::InvalidPageRequest(
            "ticket page filter did not serialize as a string".to_string(),
        )),
    }
}

pub(crate) fn collect_json_rows(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>>,
) -> Result<Vec<String>, StoreError> {
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Count child rows per ticket in one GROUP BY query (keyed by ticket id string).
pub(crate) fn count_by_ticket(
    conn: &Connection,
    table: &str,
) -> Result<BTreeMap<String, usize>, StoreError> {
    let sql = match table {
        "comments" => "SELECT ticket_id, COUNT(*) FROM comments GROUP BY ticket_id",
        "runs" => "SELECT ticket_id, COUNT(*) FROM runs GROUP BY ticket_id",
        _ => unreachable!("unknown count table"),
    };
    let mut statement = conn.prepare_cached(sql)?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut map = BTreeMap::new();
    for row in rows {
        let (ticket_id, count) = row?;
        map.insert(ticket_id, count.max(0) as usize);
    }
    Ok(map)
}

/// Fetch the single latest (max-ordinal) child JSON row per ticket in one query.
pub(crate) fn latest_json_by_ticket(
    conn: &Connection,
    table: &str,
) -> Result<BTreeMap<String, String>, StoreError> {
    let sql = match table {
        "comments" => {
            "SELECT c.ticket_id, c.json FROM comments c \
             JOIN (SELECT ticket_id, MAX(ordinal) AS mo FROM comments GROUP BY ticket_id) m \
             ON c.ticket_id = m.ticket_id AND c.ordinal = m.mo"
        }
        "events" => {
            "SELECT e.ticket_id, e.json FROM events e \
             JOIN (SELECT ticket_id, MAX(ordinal) AS mo FROM events GROUP BY ticket_id) m \
             ON e.ticket_id = m.ticket_id AND e.ordinal = m.mo"
        }
        _ => unreachable!("unknown latest table"),
    };
    let mut statement = conn.prepare_cached(sql)?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut map = BTreeMap::new();
    for row in rows {
        let (ticket_id, json) = row?;
        map.insert(ticket_id, json);
    }
    Ok(map)
}

pub(crate) fn insert_ticket(conn: &Connection, ticket: &Ticket) -> Result<(), StoreError> {
    let ordinal = next_global_ordinal(conn, "tickets")?;
    conn.execute(
        "INSERT INTO tickets (id, ordinal, json) VALUES (?1, ?2, ?3)",
        params![ticket.id.to_string(), ordinal, encode(ticket)?],
    )?;
    Ok(())
}

pub(crate) fn update_ticket(conn: &Connection, ticket: &Ticket) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE tickets SET json = ?1 WHERE id = ?2",
        params![encode(ticket)?, ticket.id.to_string()],
    )?;
    Ok(())
}

pub(crate) fn get_sqlite_ticket(
    conn: &Connection,
    ticket_id: &TicketId,
) -> Result<Ticket, StoreError> {
    decode(get_sqlite_ticket_json(conn, ticket_id)?)
}

pub(crate) fn get_sqlite_ticket_json(
    conn: &Connection,
    ticket_id: &TicketId,
) -> Result<String, StoreError> {
    conn.query_row(
        "SELECT json FROM tickets WHERE id = ?1",
        params![ticket_id.to_string()],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .ok_or(StoreError::TicketNotFound)
}

pub(crate) fn ensure_sqlite_ticket(
    conn: &Connection,
    ticket_id: &TicketId,
) -> Result<(), StoreError> {
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

pub(crate) fn push_sqlite_event(
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

pub(crate) fn ensure_sqlite_run_id_available(
    conn: &Connection,
    run_id: &RunId,
) -> Result<(), StoreError> {
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE id = ?1)",
        params![run_id.to_string()],
        |row| row.get::<_, i64>(0),
    )?;
    if exists != 0 {
        return Err(StoreError::InvalidRunTransition(format!(
            "run {run_id} already exists"
        )));
    }
    Ok(())
}

pub(crate) fn apply_sqlite_run_update(
    conn: &Connection,
    ticket_id: &TicketId,
    actor: ActorRef,
    expected: Option<&Run>,
    require_latest: bool,
    run: &Run,
) -> Result<(), StoreError> {
    let mut ticket = get_sqlite_ticket(conn, ticket_id)?;
    ensure_ticket_mutable_for(&ticket, "update run status for")?;
    let previous = get_sqlite_run(conn, &run.id)?;
    if previous.ticket_id != *ticket_id || run.ticket_id != *ticket_id {
        return Err(StoreError::InvalidRunTransition(format!(
            "run {} does not belong to ticket {ticket_id}",
            run.id
        )));
    }
    if expected.is_some_and(|expected| previous != *expected) {
        return Err(StoreError::RunConflict(run.id.clone()));
    }
    // One latest-run fetch serves both the `require_latest` guard and the
    // post-update ticket status sync. At least one run exists here because
    // `previous` was just loaded for this ticket.
    let (latest_id, latest_json) =
        latest_sqlite_run_row(conn, ticket_id)?.ok_or(StoreError::RunNotFound)?;
    let run_is_latest = latest_id == run.id.to_string();
    if require_latest && !run_is_latest {
        return Err(StoreError::RunConflict(run.id.clone()));
    }
    ensure_run_update(&previous, run)?;
    conn.execute(
        "UPDATE runs SET json = ?1 WHERE id = ?2",
        params![encode(run)?, run.id.to_string()],
    )?;
    let latest_status = if run_is_latest {
        run.status
    } else {
        decode::<Run>(latest_json)?.status
    };
    sync_ticket_status_from_run(&mut ticket, latest_status);
    update_ticket(conn, &ticket)?;
    push_sqlite_event(
        conn,
        ticket_id,
        actor.clone(),
        TicketEventKind::RunEventReceived,
    )?;
    push_sqlite_run_outcome_events(conn, ticket_id, actor, &previous, run)?;
    Ok(())
}

pub(crate) fn push_sqlite_run_outcome_events(
    conn: &Connection,
    ticket_id: &TicketId,
    actor: ActorRef,
    previous: &Run,
    updated: &Run,
) -> Result<(), StoreError> {
    for kind in run_outcome_event_kinds(previous.status, previous.evidence.is_some(), updated) {
        push_sqlite_event(conn, ticket_id, actor.clone(), kind)?;
    }
    Ok(())
}

pub(crate) fn insert_run(
    conn: &Connection,
    ticket_id: &TicketId,
    run: &Run,
) -> Result<(), StoreError> {
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

pub(crate) fn get_sqlite_run(conn: &Connection, run_id: &RunId) -> Result<Run, StoreError> {
    decode(get_sqlite_run_json(conn, run_id)?)
}

pub(crate) fn get_sqlite_run_json(conn: &Connection, run_id: &RunId) -> Result<String, StoreError> {
    conn.query_row(
        "SELECT json FROM runs WHERE id = ?1",
        params![run_id.to_string()],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .ok_or(StoreError::RunNotFound)
}

/// Tx-scoped equivalent of `TicketStore::has_run_evidence`, usable inside the
/// immediate accept/close transactions. `json_extract` returns SQL NULL for
/// both a missing key and a JSON null, matching `Run::evidence.is_some()` on
/// the decoded value.
pub(crate) fn sqlite_has_run_evidence(
    conn: &Connection,
    ticket_id: &TicketId,
) -> Result<bool, StoreError> {
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM runs \
         WHERE ticket_id = ?1 AND json_extract(json, '$.evidence') IS NOT NULL)",
        params![ticket_id.to_string()],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(exists == 1)
}

/// (id, json) of the max-ordinal run for the ticket, if any.
pub(crate) fn latest_sqlite_run_row(
    conn: &Connection,
    ticket_id: &TicketId,
) -> Result<Option<(String, String)>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT id, json FROM runs WHERE ticket_id = ?1 ORDER BY ordinal DESC LIMIT 1",
            params![ticket_id.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?)
}

pub(crate) fn list_sqlite_run_json(
    conn: &Connection,
    ticket_id: &TicketId,
) -> Result<Vec<String>, StoreError> {
    ensure_sqlite_ticket(conn, ticket_id)?;
    let mut statement =
        conn.prepare_cached("SELECT json FROM runs WHERE ticket_id = ?1 ORDER BY ordinal ASC")?;
    let rows = statement.query_map(params![ticket_id.to_string()], |row| {
        row.get::<_, String>(0)
    })?;
    collect_json_rows(rows)
}

pub(crate) fn next_global_ordinal(conn: &Connection, table: &str) -> Result<i64, StoreError> {
    let sql = match table {
        "tickets" => "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM tickets",
        _ => unreachable!("unknown global ordinal table"),
    };
    Ok(conn.query_row(sql, [], |row| row.get::<_, i64>(0))?)
}

pub(crate) fn next_scoped_ordinal(
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
