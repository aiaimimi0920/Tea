use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tea_core::{
    ActorRef, ApprovalPolicy, Plan, Run, RunId, RunStatus, Ticket, TicketAnalysis, TicketComment,
    TicketCreateOptions, TicketEdits, TicketEvent, TicketEventId, TicketEventKind, TicketId,
    TicketSource, TicketStatus,
};

use crate::helpers::{
    ensure_run_belongs_to_ticket, ensure_run_update, ensure_ticket_can_run,
    ensure_ticket_mutable_for, finish_page, run_creation_event_kinds, run_outcome_event_kinds,
    sync_ticket_status_from_run, ticket_matches_page_request, validate_ticket_page_request,
};
use crate::{
    IdempotencyRequest, StoreError, StoreStatus, TicketBundle, TicketListPage, TicketMetrics,
    TicketMetricsPage, TicketPageRequest, TicketStore,
};

#[derive(Default, Clone)]
pub struct InMemoryTicketStore {
    inner: Arc<Mutex<InnerStore>>,
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
    idempotency: BTreeMap<(String, String), IdempotencyRecord>,
}

struct IdempotencyRecord {
    request_hash: String,
    response: Ticket,
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
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        if let Some(request) = idempotency.as_ref() {
            if let Some(record) = inner
                .idempotency
                .get(&(request.scope.clone(), request.key.clone()))
            {
                if record.request_hash != request.request_hash {
                    return Err(StoreError::IdempotencyConflict);
                }
                return Ok(record.response.clone());
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
        let event = TicketEvent::new(
            TicketEventId::new(),
            ticket.id.clone(),
            actor,
            TicketEventKind::TicketCreated,
        );
        inner.events.insert(ticket.id.clone(), vec![event]);
        inner.ticket_order.push(ticket.id.clone());
        inner.tickets.insert(ticket.id.clone(), ticket.clone());
        if let Some(request) = idempotency {
            inner.idempotency.insert(
                (request.scope, request.key),
                IdempotencyRecord {
                    request_hash: request.request_hash,
                    response: ticket.clone(),
                },
            );
        }
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

    async fn list_tickets_page(
        &self,
        request: TicketPageRequest,
    ) -> Result<TicketListPage, StoreError> {
        validate_ticket_page_request(request)?;
        let inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        // The memory ordinal is the ticket's index in `ticket_order`, so a cursor
        // can slice directly past already-served entries instead of rescanning
        // from index zero on every page.
        let start = memory_page_start(&inner, request);
        let entries = inner.ticket_order[start..]
            .iter()
            .enumerate()
            .filter_map(|(offset, id)| {
                let ordinal = i64::try_from(start + offset).ok()?;
                let ticket = inner.tickets.get(id)?;
                ticket_matches_page_request(ticket, request).then(|| (ordinal, ticket.clone()))
            })
            .take(request.limit + 1)
            .collect();
        let (items, next_ordinal) = finish_page(entries, request.limit);
        Ok(TicketListPage {
            items,
            next_ordinal,
        })
    }

    async fn ticket_metrics(&self) -> Result<Vec<TicketMetrics>, StoreError> {
        let inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        Ok(inner
            .ticket_order
            .iter()
            .filter(|id| inner.tickets.contains_key(id))
            .map(|id| memory_ticket_metrics(&inner, id))
            .collect())
    }

    async fn ticket_metrics_page(
        &self,
        request: TicketPageRequest,
    ) -> Result<TicketMetricsPage, StoreError> {
        validate_ticket_page_request(request)?;
        let inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        let start = memory_page_start(&inner, request);
        let entries = inner.ticket_order[start..]
            .iter()
            .enumerate()
            .filter_map(|(offset, id)| {
                let ordinal = i64::try_from(start + offset).ok()?;
                let ticket = inner.tickets.get(id)?;
                ticket_matches_page_request(ticket, request)
                    .then(|| (ordinal, memory_ticket_metrics(&inner, id)))
            })
            .take(request.limit + 1)
            .collect();
        let (items, next_ordinal) = finish_page(entries, request.limit);
        Ok(TicketMetricsPage {
            items,
            next_ordinal,
        })
    }

    async fn ticket_bundle(&self, id: &TicketId) -> Result<TicketBundle, StoreError> {
        let inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        let ticket = inner
            .tickets
            .get(id)
            .cloned()
            .ok_or(StoreError::TicketNotFound)?;
        let runs = inner
            .runs_by_ticket
            .get(id)
            .into_iter()
            .flatten()
            .filter_map(|run_id| inner.runs.get(run_id).cloned())
            .collect();
        Ok(TicketBundle {
            ticket,
            comments: inner.comments.get(id).cloned().unwrap_or_default(),
            events: inner.events.get(id).cloned().unwrap_or_default(),
            runs,
            analysis: inner.analyses.get(id).cloned(),
            plan: inner.plans.get(id).cloned(),
        })
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
        let policy_changed = ticket.approval_policy != analysis.recommended_policy;
        ticket.status = if analysis.missing_context.is_empty() {
            TicketStatus::AnalysisReady
        } else {
            TicketStatus::NeedsInfo
        };
        ticket.risk_level = analysis.risk_assessment;
        ticket.set_approval_policy(analysis.recommended_policy);
        if policy_changed {
            inner.approvals.remove(ticket_id);
            inner.approval_rejections.remove(ticket_id);
        }
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
        if ticket.status != TicketStatus::NeedsInfo {
            ticket.status = if plan.requires_approval_before_execute {
                TicketStatus::AwaitingApproval
            } else {
                TicketStatus::PlanReady
            };
        }
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
        let policy_changed = ticket.approval_policy != policy;
        ticket.set_approval_policy(policy);
        if policy_changed {
            inner.approvals.remove(ticket_id);
            inner.approval_rejections.remove(ticket_id);
        }
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
        if !matches!(
            ticket.status,
            TicketStatus::Completed | TicketStatus::Accepted
        ) {
            ticket.status = TicketStatus::Approved;
        }
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
        if inner.runs.contains_key(&run.id) {
            return Err(StoreError::InvalidRunTransition(format!(
                "run {} already exists",
                run.id
            )));
        }
        let ticket = ticket_mut(&mut inner, ticket_id)?;
        ensure_ticket_can_run(ticket)?;
        ensure_run_belongs_to_ticket(&run, ticket_id)?;
        sync_ticket_status_from_run(ticket, run.status);
        inner
            .runs_by_ticket
            .entry(ticket_id.clone())
            .or_default()
            .push(run.id.clone());
        inner.runs.insert(run.id.clone(), run.clone());
        for kind in run_creation_event_kinds(&run) {
            push_event(&mut inner, ticket_id, actor.clone(), kind);
        }
        Ok(run)
    }

    async fn update_run(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        run: Run,
    ) -> Result<Run, StoreError> {
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        apply_memory_run_update(&mut inner, ticket_id, actor, None, false, run)
    }

    async fn update_run_if_unchanged(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        expected: Run,
        run: Run,
    ) -> Result<Run, StoreError> {
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        apply_memory_run_update(&mut inner, ticket_id, actor, Some(&expected), false, run)
    }

    async fn update_latest_run_if_unchanged(
        &self,
        ticket_id: &TicketId,
        actor: ActorRef,
        expected: Run,
        run: Run,
    ) -> Result<Run, StoreError> {
        let mut inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        apply_memory_run_update(&mut inner, ticket_id, actor, Some(&expected), true, run)
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

    async fn has_run_evidence(&self, ticket_id: &TicketId) -> Result<bool, StoreError> {
        let inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_ticket(&inner, ticket_id)?;
        Ok(memory_has_run_evidence(&inner, ticket_id))
    }

    async fn latest_run(&self, ticket_id: &TicketId) -> Result<Option<Run>, StoreError> {
        let inner = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        ensure_ticket(&inner, ticket_id)?;
        Ok(inner
            .runs_by_ticket
            .get(ticket_id)
            .and_then(|run_ids| run_ids.last())
            .and_then(|run_id| inner.runs.get(run_id))
            .cloned())
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
        if !memory_has_run_evidence(&inner, ticket_id) {
            return Err(StoreError::EvidenceRequired);
        }
        if inner
            .tickets
            .get(ticket_id)
            .is_none_or(|ticket| ticket.status != TicketStatus::Completed)
        {
            return Err(StoreError::InvalidTransition(
                "only a completed ticket can be accepted".to_string(),
            ));
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
        if !memory_has_run_evidence(&inner, ticket_id) {
            return Err(StoreError::EvidenceRequired);
        }
        if inner.tickets.get(ticket_id).is_none_or(|ticket| {
            !matches!(
                ticket.status,
                TicketStatus::Completed | TicketStatus::Accepted
            )
        }) {
            return Err(StoreError::InvalidTransition(
                "only a completed or accepted ticket can be closed".to_string(),
            ));
        }
        let requires_completion_approval = inner
            .tickets
            .get(ticket_id)
            .is_some_and(|ticket| ticket.approval_policy == ApprovalPolicy::HumanBeforeCompletion);
        let has_approval = inner.approvals.get(ticket_id).copied().unwrap_or(false);
        if requires_completion_approval && !has_approval {
            return Err(StoreError::ApprovalRequired);
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

fn ensure_ticket(inner: &InnerStore, ticket_id: &TicketId) -> Result<(), StoreError> {
    if inner.tickets.contains_key(ticket_id) {
        Ok(())
    } else {
        Err(StoreError::TicketNotFound)
    }
}

/// First `ticket_order` index a page cursor should scan. The memory ordinal is
/// the vector index, so `after_ordinal` translates directly to a slice start.
fn memory_page_start(inner: &InnerStore, request: TicketPageRequest) -> usize {
    request
        .after_ordinal
        .map_or(0, |after| (after as usize).saturating_add(1))
        .min(inner.ticket_order.len())
}

fn memory_ticket_metrics(inner: &InnerStore, id: &TicketId) -> TicketMetrics {
    TicketMetrics {
        ticket_id: id.clone(),
        comments_count: inner.comments.get(id).map_or(0, Vec::len),
        runs_count: inner.runs_by_ticket.get(id).map_or(0, Vec::len),
        latest_comment: inner.comments.get(id).and_then(|c| c.last().cloned()),
        latest_event: inner.events.get(id).and_then(|e| e.last().cloned()),
    }
}

fn memory_has_run_evidence(inner: &InnerStore, ticket_id: &TicketId) -> bool {
    inner
        .runs_by_ticket
        .get(ticket_id)
        .into_iter()
        .flatten()
        .filter_map(|id| inner.runs.get(id))
        .any(|run| run.evidence.is_some())
}

fn apply_memory_run_update(
    inner: &mut InnerStore,
    ticket_id: &TicketId,
    actor: ActorRef,
    expected: Option<&Run>,
    require_latest: bool,
    run: Run,
) -> Result<Run, StoreError> {
    ensure_ticket_mutable_for(
        inner
            .tickets
            .get(ticket_id)
            .ok_or(StoreError::TicketNotFound)?,
        "update run status for",
    )?;
    let stored = inner.runs.get(&run.id).ok_or(StoreError::RunNotFound)?;
    if stored.ticket_id != *ticket_id || run.ticket_id != *ticket_id {
        return Err(StoreError::InvalidRunTransition(format!(
            "run {} does not belong to ticket {ticket_id}",
            run.id
        )));
    }
    if expected.is_some_and(|expected| stored != expected) {
        return Err(StoreError::RunConflict(run.id.clone()));
    }
    if require_latest
        && inner
            .runs_by_ticket
            .get(ticket_id)
            .and_then(|run_ids| run_ids.last())
            != Some(&run.id)
    {
        return Err(StoreError::RunConflict(run.id.clone()));
    }
    ensure_run_update(stored, &run)?;
    let previous_status = stored.status;
    let had_evidence = stored.evidence.is_some();
    inner.runs.insert(run.id.clone(), run.clone());

    let latest_status = inner
        .runs_by_ticket
        .get(ticket_id)
        .and_then(|run_ids| run_ids.last())
        .and_then(|run_id| inner.runs.get(run_id))
        .map(|latest_run| latest_run.status)
        .ok_or(StoreError::RunNotFound)?;
    let ticket = ticket_mut(inner, ticket_id)?;
    sync_ticket_status_from_run(ticket, latest_status);
    push_event(
        inner,
        ticket_id,
        actor.clone(),
        TicketEventKind::RunEventReceived,
    );
    push_run_outcome_events(inner, ticket_id, actor, previous_status, had_evidence, &run);
    Ok(run)
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

fn push_run_outcome_events(
    inner: &mut InnerStore,
    ticket_id: &TicketId,
    actor: ActorRef,
    previous_status: RunStatus,
    had_evidence: bool,
    updated: &Run,
) {
    for kind in run_outcome_event_kinds(previous_status, had_evidence, updated) {
        push_event(inner, ticket_id, actor.clone(), kind);
    }
}
