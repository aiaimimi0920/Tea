use tea_core::Ticket;
use tea_policy::{evaluate_run, PolicyDecision, PolicyInput};
use tea_store::TicketStore;

use crate::{ensure_ticket_can_run_for_api, ApiError};

// Retry issues execution just like start. Re-evaluate the current gate before
// calling Loom; the existence of a past run does not confer permission now.
pub(super) async fn ensure_authorized(
    store: &impl TicketStore,
    ticket: &Ticket,
) -> Result<(), ApiError> {
    ensure_ticket_can_run_for_api(ticket)?;
    let has_approval = store.has_approval(&ticket.id).await?;
    match evaluate_run(&PolicyInput {
        source: ticket.source,
        risk_level: ticket.risk_level,
        approval_policy: ticket.approval_policy,
        has_approval,
        has_evidence: false,
        validation_passed: false,
    }) {
        PolicyDecision::Allow => Ok(()),
        PolicyDecision::RequestApproval { reason } | PolicyDecision::Deny { reason } => {
            Err(ApiError::forbidden(reason))
        }
    }
}
