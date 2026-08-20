import { describe, expect, it } from "vitest";

import {
  reuseIfDeepEqual,
  reuseUnchangedSnapshot,
  reuseUnchangedTickets,
  ticketSelectionPresence,
} from "./snapshotReuse";
import type { TeaSnapshot, TeaTicket } from "./teaClient";

const ticket = (overrides: Partial<TeaTicket> = {}): TeaTicket => ({
  id: "ticket-1",
  title: "Investigate failure",
  status: "open",
  updated_at: "2026-08-11T00:00:00Z",
  labels: ["kind:investigation"],
  ...overrides,
});

const snapshot = (tickets: TeaTicket[], overrides: Partial<TeaSnapshot> = {}): TeaSnapshot => ({
  health: { ok: true },
  status: { ready: true },
  configuration: { source: "tea-local" },
  tickets,
  ticketsAvailable: true,
  ...overrides,
});

describe("reuseUnchangedTickets", () => {
  it("keeps the current array when every ticket is deep-equal", () => {
    const current = [ticket(), ticket({ id: "ticket-2", title: "Second" })];
    const next = [ticket(), ticket({ id: "ticket-2", title: "Second" })];

    expect(reuseUnchangedTickets(current, next)).toBe(current);
  });

  it("reuses unchanged ticket objects while replacing changed ones", () => {
    const current = [ticket(), ticket({ id: "ticket-2", title: "Second" })];
    const next = [
      ticket(),
      ticket({ id: "ticket-2", title: "Second", status: "closed", updated_at: "2026-08-12T00:00:00Z" }),
    ];

    const merged = reuseUnchangedTickets(current, next);

    expect(merged).not.toBe(current);
    expect(merged[0]).toBe(current[0]);
    expect(merged[1]).toBe(next[1]);
  });

  it("returns a fresh array when a ticket was added or removed", () => {
    const current = [ticket()];
    const next = [ticket(), ticket({ id: "ticket-2", title: "Second" })];

    const merged = reuseUnchangedTickets(current, next);

    expect(merged).not.toBe(current);
    expect(merged).toHaveLength(2);
    expect(merged[0]).toBe(current[0]);
  });

  it("does not keep the current array identity when order changed", () => {
    const current = [ticket(), ticket({ id: "ticket-2", title: "Second" })];
    const next = [ticket({ id: "ticket-2", title: "Second" }), ticket()];

    const merged = reuseUnchangedTickets(current, next);

    expect(merged).not.toBe(current);
    expect(merged[0]).toBe(current[1]);
    expect(merged[1]).toBe(current[0]);
  });

  it("replaces a ticket when a rendered mutable field changed", () => {
    const current = [ticket()];
    const next = [ticket({ status: "in_progress" })];

    const merged = reuseUnchangedTickets(current, next);

    expect(merged[0]).toBe(next[0]);
  });
});

describe("reuseUnchangedSnapshot", () => {
  it("returns the next snapshot when there is no current snapshot", () => {
    const next = snapshot([ticket()]);

    expect(reuseUnchangedSnapshot(null, next)).toBe(next);
  });

  it("keeps the whole current snapshot when nothing changed", () => {
    const current = snapshot([ticket()]);
    const next = snapshot([ticket()]);

    expect(reuseUnchangedSnapshot(current, next)).toBe(current);
  });

  it("keeps ticket identities when only status parts changed", () => {
    const current = snapshot([ticket()]);
    const next = snapshot([ticket()], { status: { ready: true, uptime: 42 } });

    const merged = reuseUnchangedSnapshot(current, next);

    expect(merged).not.toBe(current);
    expect(merged.tickets).toBe(current.tickets);
    expect(merged.status).toEqual({ ready: true, uptime: 42 });
  });

  it("returns the next snapshot when the error text changed", () => {
    const current = snapshot([ticket()]);
    const next = snapshot([ticket()], { error: "tickets: request failed" });

    const merged = reuseUnchangedSnapshot(current, next);

    expect(merged).not.toBe(current);
    expect(merged.error).toBe("tickets: request failed");
    expect(merged.tickets).toBe(current.tickets);
  });

  it("preserves the last known ticket list when the collection is unavailable", () => {
    const current = snapshot([ticket()]);
    const next = snapshot([], {
      ticketsAvailable: false,
      error: "tickets: request failed",
    });

    const merged = reuseUnchangedSnapshot(current, next);

    expect(merged).not.toBe(current);
    expect(merged.tickets).toBe(current.tickets);
    expect(merged.ticketsAvailable).toBe(false);
  });

  it("does not reuse an unavailable snapshot after ticket availability recovers", () => {
    const current = snapshot([ticket()], { ticketsAvailable: false });
    const next = snapshot([ticket()]);

    const merged = reuseUnchangedSnapshot(current, next);

    expect(merged).not.toBe(current);
    expect(merged.ticketsAvailable).toBe(true);
  });
});

describe("ticketSelectionPresence", () => {
  it("distinguishes an absent ticket from an unavailable collection", () => {
    expect(ticketSelectionPresence(snapshot([ticket()]), "ticket-1")).toBe("present");
    expect(ticketSelectionPresence(snapshot([]), "ticket-1")).toBe("missing");
    expect(
      ticketSelectionPresence(snapshot([], { ticketsAvailable: false }), "ticket-1"),
    ).toBe("unknown");
  });
});

describe("reuseIfDeepEqual", () => {
  it("keeps the current value when the next value is deep-equal", () => {
    const current = [{ id: "comment-1", body: "Looks good" }];
    const next = [{ id: "comment-1", body: "Looks good" }];

    expect(reuseIfDeepEqual(current, next)).toBe(current);
  });

  it("returns the next value when contents changed", () => {
    const current = [{ id: "comment-1", body: "Looks good" }];
    const next = [
      { id: "comment-1", body: "Looks good" },
      { id: "comment-2", body: "Needs a follow-up" },
    ];

    expect(reuseIfDeepEqual(current, next)).toBe(next);
  });

  it("treats null and a value as different", () => {
    expect(reuseIfDeepEqual<{ summary: string } | null>(null, { summary: "plan" })).toEqual({
      summary: "plan",
    });
  });
});
