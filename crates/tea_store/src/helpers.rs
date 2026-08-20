use serde::Serialize;
use std::collections::BTreeMap;
use tea_core::{Run, RunStatus, Ticket, TicketEventKind, TicketId, TicketStatus};

use crate::{StoreError, TicketPageRequest, MAX_TICKET_PAGE_SIZE};

pub(crate) fn validate_ticket_page_request(request: TicketPageRequest) -> Result<i64, StoreError> {
    if request.limit == 0 || request.limit > MAX_TICKET_PAGE_SIZE {
        return Err(StoreError::InvalidPageRequest(format!(
            "limit must be between 1 and {MAX_TICKET_PAGE_SIZE}"
        )));
    }
    if request.after_ordinal.is_some_and(|ordinal| ordinal < 0) {
        return Err(StoreError::InvalidPageRequest(
            "cursor ordinal must not be negative".to_string(),
        ));
    }
    i64::try_from(request.limit + 1).map_err(|_| {
        StoreError::InvalidPageRequest("limit could not be represented by SQLite".to_string())
    })
}

pub(crate) fn ticket_matches_page_request(ticket: &Ticket, request: TicketPageRequest) -> bool {
    request.status.is_none_or(|status| ticket.status == status)
        && request.source.is_none_or(|source| ticket.source == source)
}

pub(crate) fn finish_page<T>(mut entries: Vec<(i64, T)>, limit: usize) -> (Vec<T>, Option<i64>) {
    let has_more = entries.len() > limit;
    if has_more {
        entries.truncate(limit);
    }
    let next_ordinal = has_more.then(|| {
        entries
            .last()
            .expect("a validated non-zero page limit has a last entry")
            .0
    });
    (
        entries.into_iter().map(|(_, value)| value).collect(),
        next_ordinal,
    )
}

pub(crate) fn encode(value: &impl Serialize) -> Result<String, StoreError> {
    Ok(serde_json::to_string(value)?)
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(json: String) -> Result<T, StoreError> {
    Ok(serde_json::from_str(&json)?)
}

pub(crate) fn decode_values<T: serde::de::DeserializeOwned>(
    values: Vec<String>,
) -> Result<Vec<T>, StoreError> {
    values.into_iter().map(decode).collect()
}

pub(crate) fn decode_value_map<T: serde::de::DeserializeOwned>(
    values: BTreeMap<String, String>,
) -> Result<BTreeMap<String, T>, StoreError> {
    values
        .into_iter()
        .map(|(key, value)| Ok((key, decode(value)?)))
        .collect()
}

pub(crate) fn ensure_ticket_mutable_for(ticket: &Ticket, action: &str) -> Result<(), StoreError> {
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

pub(crate) fn ensure_ticket_can_run(ticket: &Ticket) -> Result<(), StoreError> {
    ensure_ticket_mutable_for(ticket, "run")?;
    if matches!(
        ticket.status,
        TicketStatus::Blocked | TicketStatus::NeedsInfo
    ) {
        return Err(StoreError::InvalidTransition(format!(
            "cannot run ticket {} in {:?} status",
            ticket.id, ticket.status
        )));
    }
    Ok(())
}

pub(crate) fn sync_ticket_status_from_run(ticket: &mut Ticket, status: RunStatus) {
    ticket.status = match status {
        RunStatus::Queued | RunStatus::Running | RunStatus::Retrying => TicketStatus::Running,
        RunStatus::Succeeded => TicketStatus::Completed,
        RunStatus::Failed => TicketStatus::Failed,
        RunStatus::Stopped => TicketStatus::NeedsReview,
    };
    ticket.touch();
}

pub(crate) fn ensure_run_belongs_to_ticket(
    run: &Run,
    ticket_id: &TicketId,
) -> Result<(), StoreError> {
    if run.ticket_id != *ticket_id {
        return Err(StoreError::InvalidRunTransition(format!(
            "run {} does not belong to ticket {ticket_id}",
            run.id
        )));
    }
    Ok(())
}

pub(crate) fn ensure_run_update(previous: &Run, updated: &Run) -> Result<(), StoreError> {
    if !previous.status.can_transition_to(updated.status) {
        return Err(StoreError::InvalidRunTransition(format!(
            "cannot update run {} from {:?} to {:?}",
            previous.id, previous.status, updated.status
        )));
    }
    if previous.status == RunStatus::Succeeded && previous != updated {
        return Err(StoreError::InvalidRunTransition(format!(
            "cannot mutate succeeded run {}",
            previous.id
        )));
    }

    Ok(())
}

/// Event kinds appended when a run is first recorded, in emission order.
/// Shared by both backends so their event streams stay identical.
pub(crate) fn run_creation_event_kinds(run: &Run) -> Vec<TicketEventKind> {
    let mut kinds = vec![TicketEventKind::RunQueued];
    if run.status != RunStatus::Queued {
        kinds.push(TicketEventKind::RunStarted);
    }
    match run.status {
        RunStatus::Succeeded => {
            kinds.push(TicketEventKind::RunSucceeded);
            if run.evidence.is_some() {
                kinds.push(TicketEventKind::EvidenceAttached);
            }
        }
        RunStatus::Failed => kinds.push(TicketEventKind::RunFailed),
        _ => {}
    }
    kinds
}

/// Event kinds appended after a run update, in emission order. Shared by both
/// backends so their event streams stay identical.
pub(crate) fn run_outcome_event_kinds(
    previous_status: RunStatus,
    had_evidence: bool,
    updated: &Run,
) -> Vec<TicketEventKind> {
    let mut kinds = Vec::new();
    if previous_status != updated.status {
        match updated.status {
            RunStatus::Succeeded => kinds.push(TicketEventKind::RunSucceeded),
            RunStatus::Failed => kinds.push(TicketEventKind::RunFailed),
            RunStatus::Stopped => kinds.push(TicketEventKind::RunStopped),
            RunStatus::Retrying => kinds.push(TicketEventKind::RunRetrying),
            _ => {}
        }
    }
    if !had_evidence && updated.evidence.is_some() {
        kinds.push(TicketEventKind::EvidenceAttached);
    }
    kinds
}
