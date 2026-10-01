import type { TeaRun, TeaTicket, ticketAction } from "./teaClient";
import { canRetryRun, canStopRun } from "./runLifecycle";

// Completion and human acceptance are review milestones, not read-only states.
// Keep legacy terminal aliases while matching the daemon's closed/cancelled gate.
const terminalStatuses = new Set(["closed", "cancelled", "canceled", "done"]);

export const isTerminalTicket = (ticket: TeaTicket): boolean =>
  terminalStatuses.has(ticket.status.toLowerCase());

// These are status eligibility checks only. The daemon remains authoritative
// for evidence, approval policy, and concurrent transitions at submission time.
export const canAcceptTicket = (ticket: TeaTicket): boolean =>
  ticket.status.toLowerCase() === "completed";

export const canCloseTicket = (ticket: TeaTicket): boolean =>
  ["completed", "accepted"].includes(ticket.status.toLowerCase());

export function isTicketActionDisabled(
  ticket: TeaTicket,
  latestRun: TeaRun | undefined,
  action: Parameters<typeof ticketAction>[1],
  busy: boolean,
): boolean {
  return busy || isTerminalTicket(ticket) ||
    (action === "accept" && !canAcceptTicket(ticket)) ||
    (action === "close" && !canCloseTicket(ticket)) ||
    (action === "stop" && !canStopRun(latestRun)) ||
    (action === "retry" && !canRetryRun(latestRun));
}
