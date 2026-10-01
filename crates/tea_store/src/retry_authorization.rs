use tea_core::Ticket;
use tea_policy::{evaluate_run, PolicyDecision, PolicyInput};

use crate::{helpers::ensure_ticket_can_run, StoreError};

// Called only when failed/stopped transitions to retrying, under the memory
// mutex or SQLite write transaction. A concurrent policy/rejection change must
// not be overwritten by a previously authorized retry response. This cannot
// undo a Loom side effect that already happened outside the store boundary.
pub(crate) fn ensure_authorized(ticket: &Ticket, has_approval: bool) -> Result<(), StoreError> {
    ensure_ticket_can_run(ticket)?;
    match evaluate_run(&PolicyInput {
        source: ticket.source,
        risk_level: ticket.risk_level,
        approval_policy: ticket.approval_policy,
        has_approval,
        has_evidence: false,
        validation_passed: false,
    }) {
        PolicyDecision::Allow => Ok(()),
        PolicyDecision::RequestApproval { .. } => Err(StoreError::ApprovalRequired),
        PolicyDecision::Deny { reason } => Err(StoreError::InvalidTransition(reason)),
    }
}
