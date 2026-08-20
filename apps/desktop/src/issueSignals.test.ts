import { describe, expect, it } from "vitest";

import type { IssueMetrics } from "./issueTypes";
import type { TeaTicket } from "./teaClient";
import {
  badgeToneForPriority,
  badgeToneForRisk,
  issueSignalActiveMs,
  issueSignalFilterKey,
  issueSignalForTicket,
  issueSignalStaleMs,
} from "./issueSignals";

const hoursAgo = (hours: number) => new Date(Date.now() - hours * 60 * 60 * 1000).toISOString();

const ticket = (overrides: Partial<TeaTicket> = {}): TeaTicket => ({
  id: "ticket-1",
  title: "Fixture work order",
  status: "open",
  ...overrides,
});

const metrics = (overrides: Partial<IssueMetrics> = {}): IssueMetrics => ({
  comments: 0,
  runs: 0,
  ...overrides,
});

describe("issueSignalForTicket", () => {
  it("classifies closed tickets as resolved", () => {
    const signal = issueSignalForTicket(ticket({ status: "closed" }), undefined);

    expect(signal.label).toBe("Resolved");
    expect(signal.tone).toBe("success");
    expect(issueSignalFilterKey(signal)).toBe("resolved");
  });

  it("classifies high-priority open tickets as needs review", () => {
    const signal = issueSignalForTicket(ticket({ priority: "high", updated_at: hoursAgo(2) }), metrics());

    expect(signal.label).toBe("Needs review");
    expect(signal.tone).toBe("review");
    expect(issueSignalFilterKey(signal)).toBe("review");
  });

  it("classifies high-risk open tickets as needs review", () => {
    const signal = issueSignalForTicket(ticket({ risk_level: "critical", updated_at: hoursAgo(2) }), metrics());

    expect(signal.label).toBe("Needs review");
    expect(signal.tone).toBe("review");
    expect(issueSignalFilterKey(signal)).toBe("review");
  });

  it("classifies tickets with repeated runs as needs review", () => {
    const signal = issueSignalForTicket(ticket({ updated_at: hoursAgo(2) }), metrics({ runs: 3 }));

    expect(signal.label).toBe("Needs review");
    expect(signal.tone).toBe("review");
    expect(issueSignalFilterKey(signal)).toBe("review");
  });

  it("classifies open tickets without recent touch as stale", () => {
    const staleHours = issueSignalStaleMs / (60 * 60 * 1000) + 1;
    const signal = issueSignalForTicket(ticket({ updated_at: hoursAgo(staleHours) }), metrics());

    expect(signal.label).toBe("Stale");
    expect(signal.tone).toBe("stale");
    expect(issueSignalFilterKey(signal)).toBe("stale");
  });

  it("classifies tickets with recent touch as active", () => {
    const activeHours = Math.max(1, issueSignalActiveMs / (60 * 60 * 1000) - 1);
    const signal = issueSignalForTicket(ticket({ updated_at: hoursAgo(activeHours) }), metrics());

    expect(signal.label).toBe("Active");
    expect(signal.tone).toBe("active");
    expect(issueSignalFilterKey(signal)).toBe("active");
  });

  it("classifies tickets with comment activity as active when touch is neither fresh nor stale", () => {
    const signal = issueSignalForTicket(ticket({ updated_at: hoursAgo(48) }), metrics({ comments: 2 }));

    expect(signal.label).toBe("Active");
    expect(signal.tone).toBe("active");
    expect(issueSignalFilterKey(signal)).toBe("active");
  });

  it("classifies untouched open tickets as queued", () => {
    const signal = issueSignalForTicket(ticket({ updated_at: hoursAgo(48) }), metrics());

    expect(signal.label).toBe("Queued");
    expect(signal.tone).toBe("muted");
    expect(issueSignalFilterKey(signal)).toBe("queued");
  });
});

describe("badgeToneForPriority", () => {
  it("marks high, urgent, and p0 priorities as danger", () => {
    expect(badgeToneForPriority("high")).toBe("danger");
    expect(badgeToneForPriority("Urgent")).toBe("danger");
    expect(badgeToneForPriority("p0")).toBe("danger");
  });

  it("marks low, minor, and p3 priorities as muted", () => {
    expect(badgeToneForPriority("low")).toBe("muted");
    expect(badgeToneForPriority("minor")).toBe("muted");
    expect(badgeToneForPriority("p3")).toBe("muted");
  });

  it("marks other priorities as default", () => {
    expect(badgeToneForPriority("normal")).toBe("default");
    expect(badgeToneForPriority(undefined)).toBe("default");
  });
});

describe("badgeToneForRisk", () => {
  it("marks high, critical, and severe risks as danger", () => {
    expect(badgeToneForRisk("high")).toBe("danger");
    expect(badgeToneForRisk("Critical")).toBe("danger");
    expect(badgeToneForRisk("severe")).toBe("danger");
  });

  it("marks low and minor risks as muted", () => {
    expect(badgeToneForRisk("low")).toBe("muted");
    expect(badgeToneForRisk("minor")).toBe("muted");
  });

  it("marks other risks as default", () => {
    expect(badgeToneForRisk("medium")).toBe("default");
    expect(badgeToneForRisk(undefined)).toBe("default");
  });
});

describe("issueSignalFilterKey", () => {
  it("maps each signal label onto its filter key", () => {
    expect(issueSignalFilterKey({ description: "", label: "Needs review", reason: "", tone: "review" })).toBe(
      "review",
    );
    expect(issueSignalFilterKey({ description: "", label: "Active", reason: "", tone: "active" })).toBe("active");
    expect(issueSignalFilterKey({ description: "", label: "Stale", reason: "", tone: "stale" })).toBe("stale");
    expect(issueSignalFilterKey({ description: "", label: "Queued", reason: "", tone: "muted" })).toBe("queued");
    expect(issueSignalFilterKey({ description: "", label: "Resolved", reason: "", tone: "success" })).toBe(
      "resolved",
    );
  });
});
