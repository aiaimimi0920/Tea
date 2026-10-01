import { t } from "./i18n";
import { isClosedTicket } from "./issueFormat";
import { canAcceptTicket, canCloseTicket } from "./ticketLifecycle";
import type { IssueMetrics, RepoSection } from "./issueTypes";
import { ticketAction, type TeaTicket } from "./teaClient";

export type { IssueMetrics } from "./issueTypes";

export type IssueSignalFilter = "all" | "review" | "active" | "stale" | "queued" | "resolved";

export type IssueSignal = {
  description: string;
  label: "Active" | "Needs review" | "Queued" | "Resolved" | "Stale";
  reason: string;
  tone: "active" | "muted" | "review" | "stale" | "success";
};

export type IssueActionTarget =
  | {
      kind: "action";
      action: Parameters<typeof ticketAction>[1];
      label: string;
      sectionAfterAction: RepoSection;
    }
  | {
      kind: "export";
      format: "json" | "markdown";
      label: string;
    }
  | {
      kind: "section";
      label: string;
      section: RepoSection;
    };

export type IssueActionHint = {
  description: string;
  label:
    | "Export audit record"
    | "Inspect latest run"
    | "Inspect repeated runs"
    | "Monitor active work"
    | "Ping owner or retry planning"
    | "Prioritize review"
    | "Review conversation"
    | "Review risk before run"
    | "Start analysis";
  target: IssueActionTarget;
  tone: "audit" | "muted" | "review" | "run" | "stale";
};

export type IssueSignalCountKey = Exclude<IssueSignalFilter, "all">;
export type IssueSignalCounts = Record<IssueSignalCountKey, number>;

export const issueSignalStaleMs = 72 * 60 * 60 * 1000;
export const issueSignalActiveMs = 24 * 60 * 60 * 1000;

export const normalizeFilterLabel = (value: string) => value.trim().toLowerCase();

export const badgeToneForPriority = (priority?: string) => {
  const value = normalizeFilterLabel(priority ?? "");
  if (value.includes("high") || value.includes("urgent") || value.includes("p0")) return "danger";
  if (value.includes("low") || value.includes("minor") || value.includes("p3")) return "muted";
  return "default";
};

export const badgeToneForRisk = (risk?: string) => {
  const value = normalizeFilterLabel(risk ?? "");
  if (value.includes("high") || value.includes("critical") || value.includes("severe")) return "danger";
  if (value.includes("low") || value.includes("minor")) return "muted";
  return "default";
};

export const elapsedMsSince = (value: string | undefined) => {
  if (!value) return null;
  const parsed = Date.parse(value);
  if (Number.isNaN(parsed)) return null;
  return Date.now() - parsed;
};

export const issueSignalForTicket = (ticket: TeaTicket, metrics: IssueMetrics | undefined): IssueSignal => {
  const runCount = metrics?.runs ?? 0;
  const commentCount = metrics?.comments ?? 0;
  const touchAge = elapsedMsSince(metrics?.latestTouch?.createdAt ?? ticket.updated_at ?? ticket.created_at);
  const highPriority = badgeToneForPriority(ticket.priority) === "danger";
  const highRisk = badgeToneForRisk(ticket.risk_level) === "danger";
  const touchAgeHours = touchAge === null ? null : Math.max(0, Math.floor(touchAge / (60 * 60 * 1000)));

  if (isClosedTicket(ticket)) {
    return {
      description: "Terminal state reached; keep the record available for audit and export.",
      label: "Resolved",
      reason: t("Terminal state: {status}").replace("{status}", ticket.status || "closed"),
      tone: "success",
    };
  }

  if (canCloseTicket(ticket)) {
    return {
      description: t("Review execution evidence before accepting or closing this work order."),
      label: "Needs review",
      reason: canAcceptTicket(ticket) ? t("Completion review pending") : t("Accepted; closure pending"),
      tone: "review",
    };
  }

  if (highPriority || highRisk || runCount >= 3) {
    const reason = highPriority
      ? t("High priority: {value}").replace("{value}", t(ticket.priority ?? "priority flag"))
      : highRisk
        ? t("High risk: {value}").replace("{value}", t(ticket.risk_level ?? "risk flag"))
        : t("Repeated runs: {count}").replace("{count}", String(runCount));
    return {
      description: "Risk, priority, or repeated execution activity suggests an operator should review this work order.",
      label: "Needs review",
      reason,
      tone: "review",
    };
  }

  if (touchAge !== null && touchAge >= issueSignalStaleMs) {
    return {
      description: "No recent human, AI, or daemon touch was detected; this open work order may be stalled.",
      label: "Stale",
      reason: t("No touch for {h}h").replace("{h}", String(touchAgeHours ?? 72)),
      tone: "stale",
    };
  }

  if ((touchAge !== null && touchAge <= issueSignalActiveMs) || runCount > 0 || commentCount > 0) {
    const reason =
      touchAge !== null && touchAge <= issueSignalActiveMs
        ? t("Recent activity: touched {h}h ago").replace("{h}", String(touchAgeHours ?? 0))
        : runCount > 0
          ? t("Recent activity: {count} runs").replace("{count}", String(runCount))
          : t("Recent activity: {count} comments").replace("{count}", String(commentCount));
    return {
      description: "Recent activity exists in comments, events, or runs; this work order is actively moving.",
      label: "Active",
      reason,
      tone: "active",
    };
  }

  return {
    description: "Waiting for initial analysis, routing, or operator input.",
    label: "Queued",
    reason: t("Waiting for first touch"),
    tone: "muted",
  };
};

export const issueActionHintForTicket = (
  ticket: TeaTicket,
  metrics: IssueMetrics | undefined,
  signal: IssueSignal,
): IssueActionHint => {
  const runCount = metrics?.runs ?? 0;
  const commentCount = metrics?.comments ?? 0;
  const highPriority = badgeToneForPriority(ticket.priority) === "danger";
  const highRisk = badgeToneForRisk(ticket.risk_level) === "danger";

  if (signal.label === "Resolved") {
    return {
      description: "Capture the final state, comments, runs, and event trail for audit or handoff.",
      label: "Export audit record",
      target: { format: "markdown", kind: "export", label: "Preview audit export" },
      tone: "audit",
    };
  }

  if (canCloseTicket(ticket)) {
    return {
      description: t("Review execution evidence before accepting or closing this work order."),
      label: "Inspect latest run",
      target: { kind: "section", label: "Open runs tab", section: "runs" },
      tone: "review",
    };
  }

  if (signal.label === "Needs review") {
    if (highRisk) {
      return {
        description: "Risk is elevated; verify scope, approval policy, and execution blast radius first.",
        label: "Review risk before run",
        target: { kind: "section", label: "Open review thread", section: "comments" },
        tone: "review",
      };
    }
    if (highPriority) {
      return {
        description: "Priority is elevated; move this work order ahead in the operator review queue.",
        label: "Prioritize review",
        target: { kind: "section", label: "Open review thread", section: "comments" },
        tone: "review",
      };
    }
    if (runCount >= 3) {
      return {
        description: "Multiple runs already exist; inspect the latest run evidence before retrying.",
        label: "Inspect repeated runs",
        target: { kind: "section", label: "Open runs tab", section: "runs" },
        tone: "review",
      };
    }
    return {
      description: "Review the signal reason and decide whether to approve, reject, or request a safer plan.",
      label: "Review risk before run",
      target: { kind: "section", label: "Open review thread", section: "comments" },
      tone: "review",
    };
  }

  if (signal.label === "Stale") {
    return {
      description: "The work order has not moved recently; ping the owner or retry planning to unblock it.",
      label: "Ping owner or retry planning",
      target: { action: "plan", kind: "action", label: "Retry planning", sectionAfterAction: "comments" },
      tone: "stale",
    };
  }

  if (signal.label === "Queued") {
    return {
      description: "No useful activity exists yet; start analysis to turn the request into a plan.",
      label: "Start analysis",
      target: { action: "analyze", kind: "action", label: "Start analysis", sectionAfterAction: "comments" },
      tone: "run",
    };
  }

  if (runCount > 0) {
    return {
      description: "Execution evidence exists; inspect the latest run before taking the next action.",
      label: "Inspect latest run",
      target: { kind: "section", label: "Open runs tab", section: "runs" },
      tone: "run",
    };
  }

  if (commentCount > 0) {
    return {
      description: "Human discussion is active; review the conversation before changing execution state.",
      label: "Review conversation",
      target: { kind: "section", label: "Open conversation", section: "comments" },
      tone: "review",
    };
  }

  return {
    description: "Recent activity exists; monitor progress or add a review comment if the direction is unclear.",
    label: "Monitor active work",
    target: { kind: "section", label: "Open conversation", section: "comments" },
    tone: "muted",
  };
};

export const issueSignalFilterKey = (signal: IssueSignal): Exclude<IssueSignalFilter, "all"> => {
  switch (signal.label) {
    case "Needs review":
      return "review";
    case "Active":
      return "active";
    case "Stale":
      return "stale";
    case "Resolved":
      return "resolved";
    case "Queued":
    default:
      return "queued";
  }
};

export const issueSignalFilterLabel = (filter: IssueSignalFilter) => {
  switch (filter) {
    case "review":
      return "Needs review";
    case "active":
      return "Active";
    case "stale":
      return "Stale";
    case "queued":
      return "Queued";
    case "resolved":
      return "Resolved";
    case "all":
    default:
      return "All signals";
  }
};

export const emptyIssueSignalCounts = (): IssueSignalCounts => ({
  active: 0,
  queued: 0,
  resolved: 0,
  review: 0,
  stale: 0,
});
