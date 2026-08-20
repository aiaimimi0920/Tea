import type { TeaSnapshot, TeaTicket } from "./teaClient";

// Every auto-refresh poll deserializes a brand-new snapshot, so plain object
// identity always changes even when nothing the UI renders has changed. These
// helpers keep the previous object/array identities whenever a refresh produced
// deep-equal data, so downstream useMemo/memo() boundaries stay effective.
// JSON.stringify equality is used deliberately: the payloads are modest, the
// daemon serializes fields in a stable order, and a false negative only costs
// one avoidable re-render (never stale data).

const deepEqualJson = (left: unknown, right: unknown): boolean => {
  if (left === right) return true;
  return JSON.stringify(left) === JSON.stringify(right);
};

/**
 * Reuse per-ticket object identity (matched by id) when a refreshed ticket is
 * deep-equal to the current one. When every ticket was reused in the same
 * order and the lengths match, the current array identity is kept too.
 */
export function reuseUnchangedTickets(current: TeaTicket[], next: TeaTicket[]): TeaTicket[] {
  if (current === next) return current;
  const currentById = new Map(current.map((ticket) => [ticket.id, ticket]));
  let reusedAll = current.length === next.length;
  const merged = next.map((ticket, index) => {
    const previous = currentById.get(ticket.id);
    if (previous && deepEqualJson(previous, ticket)) {
      if (previous !== current[index]) reusedAll = false;
      return previous;
    }
    reusedAll = false;
    return ticket;
  });
  return reusedAll ? current : merged;
}

/**
 * Reuse the whole snapshot object when the tickets array identity was kept and
 * the health/status/configuration/error parts are deep-equal. Otherwise return
 * the next snapshot carrying over any reused ticket identities.
 */
export function reuseUnchangedSnapshot(
  current: TeaSnapshot | null,
  next: TeaSnapshot,
): TeaSnapshot {
  if (!current) return next;
  const tickets = next.ticketsAvailable
    ? reuseUnchangedTickets(current.tickets, next.tickets)
    : current.tickets;
  if (
    tickets === current.tickets &&
    deepEqualJson(current.health, next.health) &&
    deepEqualJson(current.status, next.status) &&
    deepEqualJson(current.configuration, next.configuration) &&
    current.ticketsAvailable === next.ticketsAvailable &&
    (current.error ?? null) === (next.error ?? null)
  ) {
    return current;
  }
  if (tickets === next.tickets) return next;
  return { ...next, tickets };
}

/**
 * Generic "skip the state update" helper for detail payloads (comments, events,
 * runs, analysis, plan): keep the current reference when the refreshed value is
 * deep-equal, so React bails out of the re-render.
 */
export function reuseIfDeepEqual<T>(current: T, next: T): T {
  return deepEqualJson(current, next) ? current : next;
}

export type TicketSelectionPresence = "present" | "missing" | "unknown";

export function ticketSelectionPresence(
  snapshot: TeaSnapshot,
  ticketId: string,
): TicketSelectionPresence {
  if (!snapshot.ticketsAvailable) return "unknown";
  return snapshot.tickets.some((ticket) => ticket.id === ticketId) ? "present" : "missing";
}
