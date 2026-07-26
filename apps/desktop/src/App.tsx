import { FormEvent, memo, useCallback, useEffect, useMemo, useState } from "react";
import {
  CreateTicketInput,
  TeaActorRef,
  TeaAnalysis,
  TeaComment,
  TeaClientOptions,
  TeaEvent,
  TeaIssueMetric,
  TeaLocalConfig,
  TeaPlan,
  TeaRun,
  TeaSnapshot,
  TeaTicket,
  UpdateTicketInput,
  addComment,
  createTicket,
  exportTicket,
  getIssueMetrics,
  getTicketBundle,
  readSnapshot,
  rejectTicket,
  resolveRuntimeConfig,
  retryRun,
  saveExport,
  setTicketPolicy,
  stopRun,
  ticketAction,
  updateConfiguration,
  updateTicket,
} from "./teaClient";
import { t, useLocale, getLocale, setLocale } from "./i18n";

type IssueFilter = "open" | "closed" | "all";
type IssueSort = "updated" | "created" | "activity" | "touch";
type IssueListDensity = "compact" | "comfortable";
type IssueAuthorFilter = string | null;
type IssueSignalFilter = "all" | "review" | "active" | "stale" | "queued" | "resolved";
type IssuePriorityFilter = "all" | "high";
type IssueRiskFilter = "all" | "high";
type IssueWatchFilter = "all" | "watched";
type IssueViewPreferences = {
  density: IssueListDensity;
  signalFilter: IssueSignalFilter;
  sort: IssueSort;
};
type IssueQueueNavigation = {
  current: number;
  firstId: string | null;
  isOutsideQueue: boolean;
  lastId: string | null;
  nextId: string | null;
  previousId: string | null;
  total: number;
};
type RepoSection = "issues" | "plan" | "runs" | "comments" | "exports" | "settings";
type TicketDraft = {
  title: string;
  description: string;
  approvalPolicy: string;
  priority: string;
  labels: string;
};
type TicketEditDraft = {
  title: string;
  description: string;
  priority: string;
  labels: string;
};

// System-derived labels the daemon owns and always preserves; operators cannot
// set or remove these through a ticket edit, so the edit form hides them.
const systemLabelPrefixes = ["source:", "policy:", "context:"];
const isSystemLabel = (label: string) =>
  systemLabelPrefixes.some((prefix) => label.startsWith(prefix));
type ActivityTone = "info" | "success" | "error";
type ActivityEntry = {
  id: number;
  at: number;
  message: string;
  tone: ActivityTone;
};

type IssueMetrics = {
  comments: number;
  latestTouch?: IssueTouchSummary;
  runs: number;
};

type IssueTouchSummary = {
  actor: string;
  createdAt?: string;
  group: ConversationEntryGroup;
  label: string;
};

type IssueSignal = {
  description: string;
  label: "Active" | "Needs review" | "Queued" | "Resolved" | "Stale";
  reason: string;
  tone: "active" | "muted" | "review" | "stale" | "success";
};
type IssueActionTarget =
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
type IssueActionHint = {
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
type IssueSignalCountKey = Exclude<IssueSignalFilter, "all">;
type IssueSignalCounts = Record<IssueSignalCountKey, number>;
type ActiveIssueFilterChip = {
  key: "author" | "label" | "priority" | "risk" | "search" | "signal" | "state" | "watch";
  label: string;
  value: string;
};
type IssuePresetQueueKey = "active" | "default" | "queued" | "resolved" | "review" | "stale";
type IssuePresetQueue = {
  description: string;
  issueFilter: IssueFilter;
  key: IssuePresetQueueKey;
  label: string;
  signalFilter: IssueSignalFilter;
};

type LocalNotes = Record<string, string[]>;

type ConversationEntry = {
  actor: string;
  avatar: string;
  body?: string;
  createdAt?: string;
  id: string;
  kind: "comment" | "event";
  payload?: unknown;
  sequence: number;
  title: string;
};

type ConversationTimelineItem =
  | {
      entry: ConversationEntry;
      id: string;
      kind: "entry";
    }
  | {
      entries: ConversationEntry[];
      id: string;
      kind: "system-event-group";
    };

type ConversationFilter = "all" | "comments" | "events";
type ConversationEntryGroup = "human" | "ai" | "system";
type WorkflowActionGroupKey = "review" | "approval" | "execution" | "resolution";

const actionLabels: Array<{
  action: Parameters<typeof ticketAction>[1];
  label: string;
  tone?: "primary" | "danger";
}> = [
  { action: "analyze", label: "Analyze" },
  { action: "plan", label: "Plan" },
  { action: "decompose", label: "Decompose" },
  { action: "approve", label: "Approve", tone: "primary" },
  { action: "reject", label: "Reject", tone: "danger" },
  { action: "run", label: "Run", tone: "primary" },
  { action: "accept", label: "Accept" },
  { action: "close", label: "Close" },
  { action: "cancel", label: "Cancel", tone: "danger" },
  { action: "stop", label: "Stop run", tone: "danger" },
  { action: "retry", label: "Retry run" },
];

const workflowActionGroups: Array<{
  key: WorkflowActionGroupKey;
  title: string;
  actions: Array<(typeof actionLabels)[number]>;
}> = [
  {
    key: "review",
    title: "Review actions",
    actions: actionLabels.filter((item) => ["analyze", "plan", "decompose"].includes(item.action)),
  },
  {
    key: "approval",
    title: "Approval actions",
    actions: actionLabels.filter((item) => ["approve", "reject"].includes(item.action)),
  },
  {
    key: "execution",
    title: "Execution actions",
    actions: actionLabels.filter((item) => ["run", "retry", "stop"].includes(item.action)),
  },
  {
    key: "resolution",
    title: "Resolution actions",
    actions: actionLabels.filter((item) => ["accept", "close", "cancel"].includes(item.action)),
  },
];

const approvalPolicyOptions: Array<{ value: string; label: string }> = [
  { value: "plan_only", label: "Plan only" },
  { value: "human_before_execute", label: "Human before execute" },
  { value: "human_before_write", label: "Human before write" },
  { value: "human_before_external_network", label: "Human before external network" },
  { value: "human_before_destructive_action", label: "Human before destructive action" },
  { value: "human_before_completion", label: "Human before completion" },
  { value: "auto_if_low_risk", label: "Auto if low risk" },
  { value: "auto_if_validation_passes", label: "Auto if validation passes" },
  { value: "manual_only", label: "Manual only" },
  { value: "always_auto", label: "Always auto" },
];

const createPriorityOptions: Array<{ value: string; label: string }> = [
  { value: "", label: "Default priority (normal)" },
  { value: "low", label: "Low" },
  { value: "normal", label: "Normal" },
  { value: "high", label: "High" },
  { value: "urgent", label: "Urgent" },
];

const closedStatuses = new Set(["accepted", "cancelled", "canceled", "closed", "completed", "done"]);
const watchStorageKey = "tea.watchingTickets";
// Fresh key: the old "tea.ticketLabelOverrides" stored overlays merged with
// daemon labels, which could mask authoritative labels. Local notes are now a
// separate additive-only surface, so a new key avoids resurfacing stale merges.
const localNotesStorageKey = "tea.ticketLocalNotes";
const issueViewPreferencesStorageKey = "tea.issueViewPreferences";
const autoRefreshStorageKey = "tea.autoRefreshEnabled";
const autoRefreshIntervalMs = 8000;

const readAutoRefreshPreference = (): boolean => {
  if (typeof localStorage === "undefined") return true;
  try {
    const stored = localStorage.getItem(autoRefreshStorageKey);
    return stored === null ? true : stored === "true";
  } catch {
    return true;
  }
};
const defaultIssueListDensity: IssueListDensity = "comfortable";
const defaultIssueViewPreferences: IssueViewPreferences = {
  density: defaultIssueListDensity,
  signalFilter: "all",
  sort: "updated",
};
const issuePageSizeByDensity: Record<IssueListDensity, number> = {
  compact: 14,
  comfortable: 8,
};
const issueSignalFilterOptions: Array<{
  label: string;
  tone: IssueSignal["tone"] | "default";
  value: IssueSignalFilter;
}> = [
  { label: "All signals", tone: "default", value: "all" },
  { label: "Needs review", tone: "review", value: "review" },
  { label: "Active", tone: "active", value: "active" },
  { label: "Stale", tone: "stale", value: "stale" },
  { label: "Queued", tone: "muted", value: "queued" },
  { label: "Resolved", tone: "success", value: "resolved" },
];
const issuePresetQueues: IssuePresetQueue[] = [
  {
    description: "Default inbox for unfinished work orders.",
    issueFilter: "open",
    key: "default",
    label: "Default open queue",
    signalFilter: "all",
  },
  {
    description: "Risky, high-priority, or noisy work orders.",
    issueFilter: "open",
    key: "review",
    label: "Review queue",
    signalFilter: "review",
  },
  {
    description: "Open work orders without recent touch.",
    issueFilter: "open",
    key: "stale",
    label: "Stale queue",
    signalFilter: "stale",
  },
  {
    description: "Open work orders with recent activity.",
    issueFilter: "open",
    key: "active",
    label: "Active work",
    signalFilter: "active",
  },
  {
    description: "Waiting for first analysis or routing.",
    issueFilter: "open",
    key: "queued",
    label: "Queued intake",
    signalFilter: "queued",
  },
  {
    description: "Closed records ready for audit/export.",
    issueFilter: "closed",
    key: "resolved",
    label: "Resolved audit",
    signalFilter: "resolved",
  },
];

const pretty = (value: unknown) => JSON.stringify(value ?? null, null, 2);

const payloadSummary = (value: unknown) => {
  if (Array.isArray(value)) return `Array payload · ${value.length} item${value.length === 1 ? "" : "s"}`;
  if (value && typeof value === "object") {
    const keys = Object.keys(value);
    return `Object payload · ${keys.length} field${keys.length === 1 ? "" : "s"}`;
  }
  if (typeof value === "string") return `String payload · ${value.length} chars`;
  if (typeof value === "number") return "Numeric payload";
  if (typeof value === "boolean") return "Boolean payload";
  if (value == null) return "Empty payload";
  return "Unknown payload";
};

const statusText = (snapshot: TeaSnapshot | null) => {
  if (!snapshot) return t("Loading");
  if (snapshot.status) return t("Online");
  if (snapshot.health) return t("Health-only");
  return t("Offline");
};

const routineStatusMessages = [
  "Connecting to Tea daemon...",
  "Tea daemon connected",
  "Tea daemon is not fully ready",
];

const isRoutineStatusMessage = (message: string) =>
  routineStatusMessages.includes(message);

const activityToneForMessage = (message: string): ActivityTone => {
  const lower = message.toLowerCase();
  if (/(failed|error|cannot|required|not )/.test(lower)) return "error";
  if (/(created|added|saved|submitted|approved|rejected|set|copied|reset|watched)/.test(lower)) {
    return "success";
  }
  return "info";
};

const configurationSourceOf = (snapshot: TeaSnapshot | null): string => {
  const source = snapshot?.configuration?.configuration_source;
  return typeof source === "string" ? source : "local";
};

const configurationDetailsOf = (
  snapshot: TeaSnapshot | null,
): Record<string, unknown> | null => {
  const details = snapshot?.configuration?.configuration;
  return details && typeof details === "object" ? (details as Record<string, unknown>) : null;
};

const localConfigOf = (snapshot: TeaSnapshot | null): TeaLocalConfig => {
  const config = snapshot?.configuration?.config;
  const record = config && typeof config === "object" ? (config as Record<string, unknown>) : {};
  return {
    notifications_enabled: record.notifications_enabled !== false,
    human_ticket_default_approval_policy:
      typeof record.human_ticket_default_approval_policy === "string"
        ? record.human_ticket_default_approval_policy
        : "human_before_execute",
    hook_ticket_default_approval_policy:
      typeof record.hook_ticket_default_approval_policy === "string"
        ? record.hook_ticket_default_approval_policy
        : "plan_only",
  };
};

const formatTime = (value: string | undefined) => {
  if (!value) return "-";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
};

const exportTimestamp = () => {
  const now = new Date();
  const pad = (value: number) => String(value).padStart(2, "0");
  return (
    `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}` +
    `-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`
  );
};

const isClosedTicket = (ticket: TeaTicket) => closedStatuses.has(ticket.status.toLowerCase());

const issueStateLabel = (ticket: TeaTicket) => (isClosedTicket(ticket) ? "Closed" : "Open");

const issueNumber = (ticket: TeaTicket) => {
  const compact = ticket.id.replace(/[^a-zA-Z0-9]/g, "").slice(-6);
  return compact ? `#${compact}` : ticket.id;
};

const normalizeLabels = (labels: string[]) =>
  Array.from(new Set(labels.map((label) => label.trim()).filter(Boolean)));

const operatorLabelsForTicket = (ticket: TeaTicket) =>
  (ticket.labels ?? []).filter((label) => Boolean(label) && !isSystemLabel(label));

const baseLabelsForTicket = (ticket: TeaTicket) => {
  const daemonLabels = ticket.labels?.filter(Boolean) ?? [];
  if (daemonLabels.length > 0) return daemonLabels;
  return [ticket.status || "unknown", ticket.source || "desktop", ticket.approval_policy || "default-policy"];
};

// Authoritative labels owned by the daemon. Always shown as-is in the header and
// issue rows; local notes never mask or replace these.
const daemonLabelsForTicket = (ticket: TeaTicket) => baseLabelsForTicket(ticket);

// Local-only notes overlay. These are additive annotations kept in localStorage,
// never sent to the daemon, and displayed in a clearly separate surface.
const localNotesForTicket = (ticket: TeaTicket, localNotes?: LocalNotes) =>
  (localNotes?.[ticket.id] ?? []).filter(Boolean);

// Union of daemon labels and local notes, used for filtering and search so both
// authoritative labels and local annotations can match.
const filterableLabelsForTicket = (ticket: TeaTicket, localNotes?: LocalNotes) =>
  Array.from(
    new Set([...daemonLabelsForTicket(ticket), ...localNotesForTicket(ticket, localNotes)]),
  );

const normalizeFilterLabel = (value: string) => value.trim().toLowerCase();

const badgeToneForPriority = (priority?: string) => {
  const value = normalizeFilterLabel(priority ?? "");
  if (value.includes("high") || value.includes("urgent") || value.includes("p0")) return "danger";
  if (value.includes("low") || value.includes("minor") || value.includes("p3")) return "muted";
  return "default";
};

const badgeToneForRisk = (risk?: string) => {
  const value = normalizeFilterLabel(risk ?? "");
  if (value.includes("high") || value.includes("critical") || value.includes("severe")) return "danger";
  if (value.includes("low") || value.includes("minor")) return "muted";
  return "default";
};

const issueSignalStaleMs = 72 * 60 * 60 * 1000;
const issueSignalActiveMs = 24 * 60 * 60 * 1000;

const elapsedMsSince = (value: string | undefined) => {
  if (!value) return null;
  const parsed = Date.parse(value);
  if (Number.isNaN(parsed)) return null;
  return Date.now() - parsed;
};

const issueSignalForTicket = (ticket: TeaTicket, metrics: IssueMetrics | undefined): IssueSignal => {
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

const issueActionHintForTicket = (
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

const issueSignalFilterKey = (signal: IssueSignal): Exclude<IssueSignalFilter, "all"> => {
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

const issueSignalFilterLabel = (filter: IssueSignalFilter) => {
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

const emptyIssueSignalCounts = (): IssueSignalCounts => ({
  active: 0,
  queued: 0,
  resolved: 0,
  review: 0,
  stale: 0,
});

const issueSummary = (ticket: TeaTicket) => {
  const description = ticket.description?.trim();
  if (!description) return "No description available.";
  return description.split(/\n+/).slice(0, 2).join(" ").trim();
};

const issueAgeLabel = (ticket: TeaTicket) => {
  const source = ticket.updated_at ?? ticket.created_at;
  if (!source) return `${t("Updated")} -`;
  return `${t("Updated")} ${formatTime(source)}`;
};

const buildTicketLink = (ticket: TeaTicket, serverUrl: string) =>
  `${serverUrl.replace(/\/+$/, "")}/v1/tickets/${encodeURIComponent(ticket.id)}`;

const buildTimelineEntryLink = (entryId: string) => {
  if (typeof window === "undefined") return `#${entryId}`;
  const url = new URL(window.location.href);
  url.hash = entryId;
  return url.toString();
};

const timelineEntryReference = (entryId: string) => {
  const compact = entryId.replace(/[^a-zA-Z0-9]/g, "").slice(-7);
  return compact ? `#${compact}` : "#entry";
};

const conversationEntryGroup = (entry: ConversationEntry): ConversationEntryGroup => {
  if (entry.kind === "comment") return "human";
  const signal = `${entry.title} ${entry.body ?? ""}`.toLowerCase();
  if (
    /\b(analyze|analysis|plan|decompose|approve|run|retry|agent|loom|ai|model|execute|execution)\b/.test(signal)
  ) {
    return "ai";
  }
  return "system";
};

const isFoldableSystemEntry = (entry: ConversationEntry) =>
  entry.kind === "event" && conversationEntryGroup(entry) === "system";

const buildConversationTimelineItems = (entries: ConversationEntry[]): ConversationTimelineItem[] => {
  const items: ConversationTimelineItem[] = [];
  let systemRun: ConversationEntry[] = [];

  const flushSystemRun = () => {
    if (systemRun.length === 0) return;
    const firstEntry = systemRun[0];
    if (!firstEntry) return;
    if (systemRun.length === 1) {
      items.push({ entry: firstEntry, id: firstEntry.id, kind: "entry" });
    } else {
      items.push({
        entries: systemRun,
        id: `system-event-group-${firstEntry.id}`,
        kind: "system-event-group",
      });
    }
    systemRun = [];
  };

  entries.forEach((entry) => {
    if (isFoldableSystemEntry(entry)) {
      systemRun.push(entry);
      return;
    }
    flushSystemRun();
    items.push({ entry, id: entry.id, kind: "entry" });
  });

  flushSystemRun();
  return items;
};

const conversationEntryGroupLabel = (group: ConversationEntryGroup) => {
  if (group === "human") return t("Review comment");
  if (group === "ai") return t("AI action");
  return t("System event");
};

const conversationEntrySummary = (entry: ConversationEntry, group: ConversationEntryGroup) => {
  if (entry.kind === "comment") {
    return t("Human review note attached to this work order.");
  }
  const payload = entry.payload ? ` ${payloadSummary(entry.payload)}.` : "";
  return `${t("{group} recorded by {actor}.").replace("{group}", conversationEntryGroupLabel(group)).replace("{actor}", entry.actor)}${payload}`;
};

const latestTouchLabel = (touch: IssueTouchSummary | undefined) => {
  if (!touch) return t("No recent human or daemon touches.");
  return `${t(touch.label)} · ${touch.actor} · ${formatTime(touch.createdAt)}`;
};

const copyTimelineEntryLink = async (entryId: string) => {
  const link = buildTimelineEntryLink(entryId);
  if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(link);
    return;
  }
  if (typeof window !== "undefined") {
    window.location.hash = entryId;
  }
};

const isIssueSort = (value: unknown): value is IssueSort =>
  value === "updated" || value === "created" || value === "activity" || value === "touch";

const isIssueListDensity = (value: unknown): value is IssueListDensity =>
  value === "compact" || value === "comfortable";

const isIssueSignalFilter = (value: unknown): value is IssueSignalFilter =>
  value === "all" ||
  value === "review" ||
  value === "active" ||
  value === "stale" ||
  value === "queued" ||
  value === "resolved";

const isIssueQueueShortcutTargetEditable = (target: EventTarget | null) => {
  if (!(target instanceof HTMLElement)) return false;
  const tagName = target.tagName.toLowerCase();
  return target.isContentEditable || tagName === "input" || tagName === "select" || tagName === "textarea";
};

const readIssueViewPreferences = (): IssueViewPreferences => {
  if (typeof window === "undefined") return defaultIssueViewPreferences;
  const raw = window.localStorage.getItem(issueViewPreferencesStorageKey);
  if (!raw) return defaultIssueViewPreferences;
  try {
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    if (typeof parsed !== "object" || parsed === null) return defaultIssueViewPreferences;
    return {
      density: isIssueListDensity(parsed.density) ? parsed.density : defaultIssueViewPreferences.density,
      signalFilter: isIssueSignalFilter(parsed.signalFilter)
        ? parsed.signalFilter
        : defaultIssueViewPreferences.signalFilter,
      sort: isIssueSort(parsed.sort) ? parsed.sort : defaultIssueViewPreferences.sort,
    };
  } catch {
    return defaultIssueViewPreferences;
  }
};

const readWatchStates = (): Record<string, boolean> => {
  if (typeof window === "undefined") return {};
  const raw = window.localStorage.getItem(watchStorageKey);
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw) as Record<string, boolean>;
    return typeof parsed === "object" && parsed !== null ? parsed : {};
  } catch {
    return {};
  }
};

const readLocalNotes = (): LocalNotes => {
  if (typeof window === "undefined") return {};
  const raw = window.localStorage.getItem(localNotesStorageKey);
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    if (typeof parsed !== "object" || parsed === null) return {};
    return Object.fromEntries(
      Object.entries(parsed).map(([ticketId, value]) => [
        ticketId,
        Array.isArray(value) ? normalizeLabels(value.filter((item): item is string => typeof item === "string")) : [],
      ]),
    );
  } catch {
    return {};
  }
};

const progressForTicket = (
  ticket: TeaTicket,
  comments: TeaComment[],
  events: TeaEvent[],
  runs: TeaRun[],
) => {
  if (isClosedTicket(ticket)) return 100;
  if (runs.length > 0) return 72;
  if (comments.length > 0) return 52;
  if (events.length > 0) return 38;
  return 12;
};

const actorLabel = (actor: unknown) => {
  if (!actor) return "unknown";
  if (typeof actor === "string") return actor;
  if (typeof actor === "object" && "kind" in actor) {
    const ref = actor as TeaActorRef;
    return ref.id ? `${ref.kind ?? "actor"}:${ref.id}` : (ref.kind ?? "actor");
  }
  return "actor";
};

const timestampValue = (value: string | undefined) => {
  if (!value) return Number.MAX_SAFE_INTEGER;
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? Number.MAX_SAFE_INTEGER : parsed;
};

// Friendly titles for the daemon's snake_case TicketEventKind values so the
// timeline reads as prose instead of raw enum names. Unknown kinds fall back to
// a title-cased version of the snake_case identifier.
const eventKindLabels: Record<string, string> = {
  ticket_created: "Ticket created",
  comment_added: "Comment added",
  ticket_analyzed: "Ticket analyzed",
  plan_proposed: "Plan proposed",
  policy_updated: "Policy updated",
  ticket_edited: "Ticket edited",
  approval_requested: "Approval requested",
  approval_granted: "Approval granted",
  approval_rejected: "Approval rejected",
  run_queued: "Run queued",
  run_started: "Run started",
  run_event_received: "Run event received",
  run_failed: "Run failed",
  run_succeeded: "Run succeeded",
  evidence_attached: "Evidence attached",
  review_requested: "Review requested",
  human_accepted: "Human accepted",
  ticket_closed: "Ticket closed",
  ticket_cancelled: "Ticket cancelled",
};

const eventKindLabel = (kind: string | undefined): string => {
  if (!kind) return t("Event");
  const known = eventKindLabels[kind];
  if (known) return t(known);
  return kind
    .split(/[_\s]+/)
    .filter(Boolean)
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(" ");
};

const buildConversationEntries = (
  comments: TeaComment[],
  events: TeaEvent[],
): ConversationEntry[] => {
  const commentEntries = comments.map((comment, index): ConversationEntry => {
    const actor = actorLabel(comment.actor);
    return {
      actor,
      avatar: actor.slice(0, 2).toUpperCase(),
      body: comment.body,
      createdAt: comment.created_at,
      id: `comment-${comment.id}`,
      kind: "comment",
      sequence: index * 2,
      title: actor,
    };
  });

  const eventEntries = events.map((event, index): ConversationEntry => {
    const actor = actorLabel(event.actor);
    return {
      actor,
      avatar: "AI",
      body: event.message ?? "Event payload",
      createdAt: event.created_at,
      id: `event-${event.id ?? `${event.kind ?? "unknown"}-${index}`}`,
      kind: "event",
      payload: event.payload,
      sequence: index * 2 + 1,
      title: eventKindLabel(event.kind),
    };
  });

  return [...commentEntries, ...eventEntries].sort(
    (left, right) =>
      timestampValue(left.createdAt) - timestampValue(right.createdAt) ||
      left.sequence - right.sequence,
  );
};

export default function App() {
  const [serverUrl, setServerUrl] = useState("http://127.0.0.1:48910");
  const [authToken, setAuthToken] = useState("");
  const [authConfigured, setAuthConfigured] = useState(false);
  const [snapshot, setSnapshot] = useState<TeaSnapshot | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [selectedTicket, setSelectedTicket] = useState<TeaTicket | null>(null);
  const [comments, setComments] = useState<TeaComment[]>([]);
  const [events, setEvents] = useState<TeaEvent[]>([]);
  const [runs, setRuns] = useState<TeaRun[]>([]);
  const [analysis, setAnalysis] = useState<TeaAnalysis | null>(null);
  const [plan, setPlan] = useState<TeaPlan | null>(null);
  const [issueMetrics, setIssueMetrics] = useState<Record<string, IssueMetrics>>({});
  const [message, setMessage] = useState("Connecting to Tea daemon...");
  const [activityLog, setActivityLog] = useState<ActivityEntry[]>([]);
  const [busy, setBusy] = useState(false);
  const [creating, setCreating] = useState(false);
  const [createNotice, setCreateNotice] = useState<string>("");
  const [commentDraft, setCommentDraft] = useState("");
  const [draft, setDraft] = useState<TicketDraft>({
    title: "",
    description: "",
    approvalPolicy: "",
    priority: "",
    labels: "",
  });
  const [exportPreview, setExportPreview] = useState("");
  const initialIssueViewPreferences = useMemo(() => readIssueViewPreferences(), []);
  const [issueFilter, setIssueFilter] = useState<IssueFilter>("open");
  const [issueSort, setIssueSort] = useState<IssueSort>(initialIssueViewPreferences.sort);
  const [issueListDensity, setIssueListDensity] = useState<IssueListDensity>(initialIssueViewPreferences.density);
  const [visibleIssueLimit, setVisibleIssueLimit] = useState(issuePageSizeByDensity[initialIssueViewPreferences.density]);
  const [activeSection, setActiveSection] = useState<RepoSection>("issues");
  const [searchQuery, setSearchQuery] = useState("");
  const [showNewIssue, setShowNewIssue] = useState(false);
  const [authorFilter, setAuthorFilter] = useState<IssueAuthorFilter>(null);
  const [showAuthorFilterPanel, setShowAuthorFilterPanel] = useState(false);
  const [showAdvancedFilters, setShowAdvancedFilters] = useState(false);
  const [selectedLabelFilter, setSelectedLabelFilter] = useState<string | null>(null);
  const [issueSignalFilter, setIssueSignalFilter] = useState<IssueSignalFilter>(
    initialIssueViewPreferences.signalFilter,
  );
  const [issuePriorityFilter, setIssuePriorityFilter] = useState<IssuePriorityFilter>("all");
  const [issueRiskFilter, setIssueRiskFilter] = useState<IssueRiskFilter>("all");
  const [issueWatchFilter, setIssueWatchFilter] = useState<IssueWatchFilter>("all");
  const [showLabelFilterPanel, setShowLabelFilterPanel] = useState(false);
  const [watchStates, setWatchStates] = useState<Record<string, boolean>>(() => readWatchStates());
  const [localNotes, setLocalNotes] = useState<LocalNotes>(() => readLocalNotes());
  const [labelDraft, setLabelDraft] = useState("");
  const [showLabelEditor, setShowLabelEditor] = useState(false);
  const [showEditIssue, setShowEditIssue] = useState(false);
  const [editDraft, setEditDraft] = useState<TicketEditDraft>({
    title: "",
    description: "",
    priority: "",
    labels: "",
  });
  const [rejectReason, setRejectReason] = useState("");
  const [autoRefresh, setAutoRefresh] = useState<boolean>(() => readAutoRefreshPreference());
  const [lastRefreshedAt, setLastRefreshedAt] = useState<number | null>(null);

  const options: TeaClientOptions = useMemo(
    () => ({
      serverUrl,
      authToken: authToken.trim() || undefined,
    }),
    [authToken, serverUrl],
  );

  const notify = (rawMessage: string) => {
    const nextMessage = t(rawMessage);
    setMessage(nextMessage);
    if (isRoutineStatusMessage(rawMessage)) return;
    const tone = activityToneForMessage(rawMessage);
    setActivityLog((current) => {
      if (current.length > 0 && current[0].message === nextMessage) {
        const [head, ...rest] = current;
        return [{ ...head, at: Date.now() }, ...rest];
      }
      const entry: ActivityEntry = {
        id: Date.now() + Math.floor(Math.random() * 1000),
        at: Date.now(),
        message: nextMessage,
        tone,
      };
      return [entry, ...current].slice(0, 30);
    });
  };

  const clearActivityLog = () => setActivityLog([]);

  const tickets = useMemo(() => snapshot?.tickets ?? [], [snapshot]);
  const availableLabels = useMemo(
    () =>
      Array.from(
        new Set(
          tickets.flatMap((ticket) => filterableLabelsForTicket(ticket, localNotes).map(normalizeFilterLabel)),
        ),
      ).filter(Boolean),
    [localNotes, tickets],
  );
  const availableAuthors = useMemo(
    () =>
      Array.from(
        new Set(
          tickets.map((ticket) => ticket.owner_human_id?.trim() || "Tea local operator"),
        ),
      ).filter(Boolean),
    [tickets],
  );
  const openCount = tickets.filter((ticket) => !isClosedTicket(ticket)).length;
  const closedCount = tickets.length - openCount;
  const selectedTicketFromList = selectedId
    ? tickets.find((ticket) => ticket.id === selectedId) ?? null
    : null;
  const activeTicket = selectedTicket ?? selectedTicketFromList;

  // The filter predicates are stabilized with useCallback so the derived memos
  // below (filteredTickets, issueSignalCounts, issueTriageFilterCounts) can depend
  // on them and have their dependency arrays verified by eslint react-hooks.
  const ticketMatchesStateFilter = useCallback(
    (ticket: TeaTicket) =>
      issueFilter === "all" ||
      (issueFilter === "open" && !isClosedTicket(ticket)) ||
      (issueFilter === "closed" && isClosedTicket(ticket)),
    [issueFilter],
  );

  const ticketMatchesAuthorFilter = useCallback(
    (ticket: TeaTicket) => {
      if (authorFilter) {
        const author = ticket.owner_human_id?.trim() || "Tea local operator";
        if (author !== authorFilter) return false;
      }
      return true;
    },
    [authorFilter],
  );

  const ticketMatchesLabelFilter = useCallback(
    (ticket: TeaTicket) => {
      if (selectedLabelFilter) {
        const labels = filterableLabelsForTicket(ticket, localNotes).map(normalizeFilterLabel);
        if (!labels.includes(selectedLabelFilter)) return false;
      }
      return true;
    },
    [selectedLabelFilter, localNotes],
  );

  const ticketMatchesSearchQuery = useCallback(
    (ticket: TeaTicket) => {
      const query = searchQuery.trim().toLowerCase();
      if (!query) return true;

      return [
        ticket.id,
        ticket.title,
        ticket.description ?? "",
        ticket.status,
        ticket.source ?? "",
        ticket.priority ?? "",
        ticket.risk_level ?? "",
        ticket.owner_human_id ?? "",
        ticket.delegated_agent_id ?? "",
        ...(filterableLabelsForTicket(ticket, localNotes) ?? []),
      ]
        .join(" ")
        .toLowerCase()
        .includes(query);
    },
    [searchQuery, localNotes],
  );

  const ticketMatchesBaseIssueFilters = useCallback(
    (ticket: TeaTicket) =>
      ticketMatchesStateFilter(ticket) &&
      ticketMatchesAuthorFilter(ticket) &&
      ticketMatchesLabelFilter(ticket) &&
      ticketMatchesSearchQuery(ticket),
    [
      ticketMatchesStateFilter,
      ticketMatchesAuthorFilter,
      ticketMatchesLabelFilter,
      ticketMatchesSearchQuery,
    ],
  );

  const ticketMatchesSignalFilter = useCallback(
    (ticket: TeaTicket) => {
      if (issueSignalFilter === "all") return true;
      const issueSignal = issueSignalForTicket(ticket, issueMetrics[ticket.id]);
      return issueSignalFilterKey(issueSignal) === issueSignalFilter;
    },
    [issueSignalFilter, issueMetrics],
  );

  const ticketMatchesPriorityFilter = useCallback(
    (ticket: TeaTicket) =>
      issuePriorityFilter === "all" || badgeToneForPriority(ticket.priority) === "danger",
    [issuePriorityFilter],
  );

  const ticketMatchesRiskFilter = useCallback(
    (ticket: TeaTicket) =>
      issueRiskFilter === "all" || badgeToneForRisk(ticket.risk_level) === "danger",
    [issueRiskFilter],
  );

  const ticketMatchesWatchFilter = useCallback(
    (ticket: TeaTicket) => issueWatchFilter === "all" || Boolean(watchStates[ticket.id]),
    [issueWatchFilter, watchStates],
  );

  const issueSignalCounts = useMemo(
    () =>
      tickets.reduce<IssueSignalCounts>((counts, ticket) => {
        if (!ticketMatchesBaseIssueFilters(ticket)) return counts;
        const key = issueSignalFilterKey(issueSignalForTicket(ticket, issueMetrics[ticket.id]));
        counts[key] += 1;
        return counts;
      }, emptyIssueSignalCounts()),
    [tickets, ticketMatchesBaseIssueFilters, issueMetrics],
  );
  const issueSignalTotal = Object.values(issueSignalCounts).reduce((total, count) => total + count, 0);
  const issueSignalCountForOption = (value: IssueSignalFilter) =>
    value === "all" ? issueSignalTotal : issueSignalCounts[value];
  const issueTriageFilterCounts = useMemo(
    () => ({
      highPriority: tickets.filter(
        (ticket) => ticketMatchesBaseIssueFilters(ticket) && ticketMatchesSignalFilter(ticket) && badgeToneForPriority(ticket.priority) === "danger",
      ).length,
      highRisk: tickets.filter(
        (ticket) => ticketMatchesBaseIssueFilters(ticket) && ticketMatchesSignalFilter(ticket) && badgeToneForRisk(ticket.risk_level) === "danger",
      ).length,
      watched: tickets.filter(
        (ticket) => ticketMatchesBaseIssueFilters(ticket) && ticketMatchesSignalFilter(ticket) && Boolean(watchStates[ticket.id]),
      ).length,
    }),
    [tickets, ticketMatchesBaseIssueFilters, ticketMatchesSignalFilter, watchStates],
  );
  const ticketMatchesPresetQueue = (ticket: TeaTicket, queue: IssuePresetQueue) => {
    const matchesState =
      queue.issueFilter === "all" ||
      (queue.issueFilter === "open" && !isClosedTicket(ticket)) ||
      (queue.issueFilter === "closed" && isClosedTicket(ticket));
    if (!matchesState) return false;
    if (queue.signalFilter === "all") return true;
    const issueSignal = issueSignalForTicket(ticket, issueMetrics[ticket.id]);
    return issueSignalFilterKey(issueSignal) === queue.signalFilter;
  };
  const issuePresetQueueCount = (queue: IssuePresetQueue) =>
    tickets.filter((ticket) => ticketMatchesPresetQueue(ticket, queue)).length;

  // Memoized so the downstream sortedTickets/visibleTickets memos are effective.
  // Depends on the stabilized filter predicates, so eslint react-hooks verifies the deps.
  const filteredTickets = useMemo(
    () =>
      tickets.filter((ticket) => {
        if (!ticketMatchesBaseIssueFilters(ticket)) return false;
        return (
          ticketMatchesSignalFilter(ticket) &&
          ticketMatchesPriorityFilter(ticket) &&
          ticketMatchesRiskFilter(ticket) &&
          ticketMatchesWatchFilter(ticket)
        );
      }),
    [
      tickets,
      ticketMatchesBaseIssueFilters,
      ticketMatchesSignalFilter,
      ticketMatchesPriorityFilter,
      ticketMatchesRiskFilter,
      ticketMatchesWatchFilter,
    ],
  );
  const sortedTickets = useMemo(() => {
    const list = [...filteredTickets];
    const valueForMetrics = (ticketId: string) => {
      const metrics = issueMetrics[ticketId];
      return (metrics?.comments ?? 0) + (metrics?.runs ?? 0);
    };
    const valueForTouch = (ticket: TeaTicket) =>
      timestampValue(issueMetrics[ticket.id]?.latestTouch?.createdAt ?? ticket.updated_at ?? ticket.created_at);
    list.sort((left, right) => {
      if (issueSort === "created") {
        return timestampValue(right.created_at) - timestampValue(left.created_at);
      }
      if (issueSort === "touch") {
        return valueForTouch(right) - valueForTouch(left);
      }
      if (issueSort === "activity") {
        return (
          valueForMetrics(right.id) - valueForMetrics(left.id) ||
          timestampValue(right.updated_at ?? right.created_at) -
            timestampValue(left.updated_at ?? left.created_at)
        );
      }
      return (
        timestampValue(right.updated_at ?? right.created_at) -
        timestampValue(left.updated_at ?? left.created_at)
      );
    });
    return list;
  }, [filteredTickets, issueMetrics, issueSort]);
  const issuePageSize = issuePageSizeByDensity[issueListDensity];
  const visibleTickets = useMemo(
    () => sortedTickets.slice(0, Math.min(visibleIssueLimit, sortedTickets.length)),
    [sortedTickets, visibleIssueLimit],
  );
  const hasMoreVisibleTickets = visibleIssueLimit < sortedTickets.length;
  const canCollapseIssueList = visibleIssueLimit > issuePageSize;
  const selectedIssueQueueIndex = activeTicket
    ? sortedTickets.findIndex((ticket) => ticket.id === activeTicket.id)
    : -1;
  const issueQueueNavigation: IssueQueueNavigation = {
    current: selectedIssueQueueIndex >= 0 ? selectedIssueQueueIndex + 1 : 0,
    firstId: sortedTickets[0]?.id ?? null,
    isOutsideQueue: Boolean(activeTicket) && selectedIssueQueueIndex < 0,
    lastId: sortedTickets[sortedTickets.length - 1]?.id ?? null,
    nextId:
      selectedIssueQueueIndex >= 0 && selectedIssueQueueIndex < sortedTickets.length - 1
        ? (sortedTickets[selectedIssueQueueIndex + 1]?.id ?? null)
        : null,
    previousId:
      selectedIssueQueueIndex > 0 ? (sortedTickets[selectedIssueQueueIndex - 1]?.id ?? null) : null,
    total: sortedTickets.length,
  };
  const issueFilterLabel =
    issueFilter === "all"
      ? t("all work orders")
      : issueFilter === "closed"
        ? t("closed work orders")
        : t("open work orders");
  const issueSignalFilterSummary = t(issueSignalFilterLabel(issueSignalFilter));
  const activeIssueFilterChips: ActiveIssueFilterChip[] = [
    ...(issueFilter !== "open"
      ? [{ key: "state" as const, label: "State", value: issueFilter === "all" ? "All" : "Closed" }]
      : []),
    ...(searchQuery.trim()
      ? [{ key: "search" as const, label: "Search", value: searchQuery.trim() }]
      : []),
    ...(authorFilter ? [{ key: "author" as const, label: "Author", value: authorFilter }] : []),
    ...(selectedLabelFilter ? [{ key: "label" as const, label: "Label", value: selectedLabelFilter }] : []),
    ...(issueSignalFilter !== "all"
      ? [{ key: "signal" as const, label: "Signal", value: issueSignalFilterSummary }]
      : []),
    ...(issuePriorityFilter === "high" ? [{ key: "priority" as const, label: "Priority", value: "High" }] : []),
    ...(issueRiskFilter === "high" ? [{ key: "risk" as const, label: "Risk", value: "High" }] : []),
    ...(issueWatchFilter === "watched" ? [{ key: "watch" as const, label: "Watch", value: "Watched" }] : []),
  ];
  const hasActiveIssueFilters = activeIssueFilterChips.length > 0;
  const activeIssuePresetQueueKey =
    issuePresetQueues.find(
      (queue) =>
        queue.issueFilter === issueFilter &&
        queue.signalFilter === issueSignalFilter &&
        !searchQuery.trim() &&
        !authorFilter &&
        !selectedLabelFilter &&
        issuePriorityFilter === "all" &&
        issueRiskFilter === "all" &&
        issueWatchFilter === "all",
    )?.key ?? null;
  const activeIssuePresetQueue = issuePresetQueues.find((queue) => queue.key === activeIssuePresetQueueKey) ?? null;
  const issueSortSummary =
    issueSort === "activity"
      ? t("Most activity first")
      : issueSort === "created"
        ? t("Newest created first")
        : issueSort === "touch"
          ? t("Latest touch first")
          : t("Recently updated first");
  const issueDensitySummary =
    issueListDensity === "compact"
      ? t("Compact rows · {n} per page").replace("{n}", String(issuePageSizeByDensity.compact))
      : t("Comfortable rows · {n} per page").replace("{n}", String(issuePageSizeByDensity.comfortable));
  const issueQueueSummaryItems = [
    { label: "Matching", value: t("{n} work orders").replace("{n}", String(filteredTickets.length)) },
    { label: "Visible", value: t("{n} in view").replace("{n}", String(visibleTickets.length)) },
    { label: "Signal", value: issueSignalFilterSummary },
    { label: "Sort", value: issueSortSummary },
    { label: "Density", value: issueDensitySummary },
    {
      label: "Extra filters",
      value: hasActiveIssueFilters
        ? t("{n} active").replace("{n}", String(activeIssueFilterChips.length))
        : t("None"),
    },
  ];
  const issueViewPreferences = useMemo<IssueViewPreferences>(
    () => ({
      density: issueListDensity,
      signalFilter: issueSignalFilter,
      sort: issueSort,
    }),
    [issueListDensity, issueSignalFilter, issueSort],
  );
  const issueViewPreferencesAreDefault =
    issueViewPreferences.density === defaultIssueViewPreferences.density &&
    issueViewPreferences.signalFilter === defaultIssueViewPreferences.signalFilter &&
    issueViewPreferences.sort === defaultIssueViewPreferences.sort;
  const issueViewPreferenceStatus = issueViewPreferencesAreDefault
    ? t("Default view")
    : t("Saved view");
  const selectedMetrics = activeTicket ? issueMetrics[activeTicket.id] : null;
  const selectedSignal = activeTicket
    ? issueSignalForTicket(activeTicket, selectedMetrics ?? { comments: comments.length, runs: runs.length })
    : null;
  const selectedActionHint =
    activeTicket && selectedSignal
      ? issueActionHintForTicket(
          activeTicket,
          selectedMetrics ?? { comments: comments.length, runs: runs.length },
          selectedSignal,
        )
      : null;
  const selectedIsWatched = activeTicket ? Boolean(watchStates[activeTicket.id]) : false;
  // Authoritative daemon labels for the header/rows; never masked by local notes.
  const selectedDaemonLabels = activeTicket ? daemonLabelsForTicket(activeTicket) : [];
  // Local-only annotations shown in their own surface, independent of daemon labels.
  const selectedLocalNotes = activeTicket ? localNotesForTicket(activeTicket, localNotes) : [];
  const selectedHasLocalNotes = activeTicket
    ? Object.prototype.hasOwnProperty.call(localNotes, activeTicket.id)
    : false;

  const clearLabelFilter = () => {
    setSelectedLabelFilter(null);
    setShowLabelFilterPanel(false);
  };

  const clearAuthorFilter = () => {
    setAuthorFilter(null);
    setShowAuthorFilterPanel(false);
  };

  const removeIssueFilterChip = (key: ActiveIssueFilterChip["key"]) => {
    if (key === "state") {
      setIssueFilter("open");
      return;
    }
    if (key === "search") {
      setSearchQuery("");
      return;
    }
    if (key === "author") {
      clearAuthorFilter();
      return;
    }
    if (key === "label") {
      clearLabelFilter();
      return;
    }
    if (key === "priority") {
      setIssuePriorityFilter("all");
      return;
    }
    if (key === "risk") {
      setIssueRiskFilter("all");
      return;
    }
    if (key === "watch") {
      setIssueWatchFilter("all");
      return;
    }
    setIssueSignalFilter("all");
  };

  const clearIssueFilters = () => {
    setIssueFilter("open");
    setSearchQuery("");
    setAuthorFilter(null);
    setSelectedLabelFilter(null);
    setIssueSignalFilter("all");
    setIssuePriorityFilter("all");
    setIssueRiskFilter("all");
    setIssueWatchFilter("all");
    setShowAuthorFilterPanel(false);
    setShowLabelFilterPanel(false);
  };

  const clearWatchedIssueFilter = () => {
    setIssueWatchFilter("all");
  };

  const resetIssueViewPreferences = () => {
    setIssueSort(defaultIssueViewPreferences.sort);
    setIssueListDensity(defaultIssueViewPreferences.density);
    setIssueSignalFilter(defaultIssueViewPreferences.signalFilter);
    setVisibleIssueLimit(issuePageSizeByDensity[defaultIssueViewPreferences.density]);
    notify("Reset issue view preferences");
  };

  const applyIssuePresetQueue = (queue: IssuePresetQueue) => {
    setIssueFilter(queue.issueFilter);
    setIssueSignalFilter(queue.signalFilter);
    setSearchQuery("");
    setAuthorFilter(null);
    setSelectedLabelFilter(null);
    setIssuePriorityFilter("all");
    setIssueRiskFilter("all");
    setIssueWatchFilter("all");
    setShowAuthorFilterPanel(false);
    setShowLabelFilterPanel(false);
  };

  const showMoreIssues = () => {
    setVisibleIssueLimit((current) => Math.min(sortedTickets.length, current + issuePageSize));
  };

  const collapseIssueList = () => {
    setVisibleIssueLimit(issuePageSize);
  };

  const ensureIssueVisibleInList = useCallback(
    (ticketId: string) => {
      const targetIndex = sortedTickets.findIndex((ticket) => ticket.id === ticketId);
      if (targetIndex < 0) return;
      setVisibleIssueLimit((current) => Math.max(current, targetIndex + 1));
    },
    [sortedTickets],
  );

  const navigateIssueQueue = useCallback(
    (ticketId: string | null) => {
      if (!ticketId) return;
      ensureIssueVisibleInList(ticketId);
      setSelectedId(ticketId);
      setSelectedTicket(sortedTickets.find((ticket) => ticket.id === ticketId) ?? null);
    },
    [ensureIssueVisibleInList, sortedTickets],
  );

  useEffect(() => {
    const handleIssueQueueShortcut = (event: KeyboardEvent) => {
      if (!event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
      if (isIssueQueueShortcutTargetEditable(event.target)) return;
      if (event.key === "ArrowUp") {
        event.preventDefault();
        navigateIssueQueue(issueQueueNavigation.previousId);
      }
      if (event.key === "ArrowDown") {
        event.preventDefault();
        navigateIssueQueue(issueQueueNavigation.nextId);
      }
      if (event.key === "Home") {
        event.preventDefault();
        navigateIssueQueue(issueQueueNavigation.firstId);
      }
      if (event.key === "End") {
        event.preventDefault();
        navigateIssueQueue(issueQueueNavigation.lastId);
      }
    };
    window.addEventListener("keydown", handleIssueQueueShortcut);
    return () => window.removeEventListener("keydown", handleIssueQueueShortcut);
  }, [
    issueQueueNavigation.firstId,
    issueQueueNavigation.lastId,
    issueQueueNavigation.nextId,
    issueQueueNavigation.previousId,
    navigateIssueQueue,
  ]);

  const refreshIssueMetrics = async (ticketIds: string[]) => {
    if (ticketIds.length === 0) {
      setIssueMetrics({});
      return;
    }

    // Single aggregated request replaces the previous comments+runs+events fan-out
    // (which was 3 requests per ticket, re-run on every auto-refresh poll).
    const wanted = new Set(ticketIds);
    let metrics: TeaIssueMetric[];
    try {
      metrics = await getIssueMetrics(options);
    } catch (error) {
      notify(`Failed to read issue metrics: ${String(error)}`);
      return;
    }

    const entries = metrics
      .filter((metric) => wanted.has(metric.ticket_id))
      .map((metric) => {
        const latestComment = metric.latest_comment;
        const latestEvent = metric.latest_event;
        const latestTouch = (() => {
          const commentTouch = latestComment
            ? {
                actor: actorLabel(latestComment.actor),
                createdAt: latestComment.created_at,
                group: "human" as const,
                label: "Latest human review",
              }
            : null;
          const latestEventEntry = latestEvent
            ? {
                actor: actorLabel(latestEvent.actor),
                avatar: "AI",
                body: latestEvent.message,
                createdAt: latestEvent.created_at,
                id: `event-${latestEvent.id ?? "latest"}`,
                kind: "event" as const,
                payload: latestEvent.payload,
                sequence: 0,
                title: latestEvent.kind ?? "event",
              }
            : null;
          const latestEventGroup = latestEventEntry ? conversationEntryGroup(latestEventEntry) : null;
          const eventTouch = latestEventEntry
            ? {
                actor: latestEventEntry.actor,
                createdAt: latestEventEntry.createdAt,
                group: latestEventGroup ?? "system",
                label: latestEventGroup === "ai" ? t("Latest AI action") : t("Latest system event"),
              }
            : null;
          if (commentTouch && eventTouch) {
            return timestampValue(commentTouch.createdAt) >= timestampValue(eventTouch.createdAt)
              ? commentTouch
              : eventTouch;
          }
          return commentTouch ?? eventTouch ?? undefined;
        })();
        return [
          metric.ticket_id,
          { comments: metric.comments_count, latestTouch, runs: metric.runs_count },
        ] as const;
      });
    setIssueMetrics(Object.fromEntries(entries));
  };

  const applyTemplate = (template: "investigation" | "implementation" | "release") => {
    const templates: Record<typeof template, TicketDraft> = {
      investigation: {
        title: "Investigate AI workflow failure",
        description:
          "## Background\nDescribe the failed workflow, current evidence, and expected behavior.\n\n## Acceptance criteria\n- Root cause is identified from logs or runtime data.\n- A minimal fix or follow-up plan is proposed.\n- Verification evidence is attached.",
        approvalPolicy: "plan_only",
        priority: "high",
        labels: "kind:investigation",
      },
      implementation: {
        title: "Implement AI work-order change",
        description:
          "## Goal\nDescribe the product or workflow change.\n\n## Constraints\n- Keep Tea standalone.\n- Preserve UI/headless dual mode.\n\n## Acceptance criteria\n- Code is implemented.\n- Typecheck/build/tests pass.\n- Release smoke is updated if needed.",
        approvalPolicy: "human_before_execute",
        priority: "normal",
        labels: "kind:implementation",
      },
      release: {
        title: "Prepare Tea release validation",
        description:
          "## Release target\nDescribe the package or runtime to validate.\n\n## Checks\n- tea.exe UI launches.\n- tea-daemon.exe headless mode works.\n- tea-cli.exe lifecycle smoke passes.\n- Manifest and checksums are generated.",
        approvalPolicy: "manual_only",
        priority: "normal",
        labels: "kind:release",
      },
    };
    setDraft(templates[template]);
  };

  const refresh = async () => {
    setBusy(true);
    try {
      const next = await readSnapshot(options);
      setSnapshot(next);
      setLastRefreshedAt(Date.now());
      notify(next.status ? "Tea daemon connected" : "Tea daemon is not fully ready");
      if (!selectedId && next.tickets.length > 0) {
        setSelectedId(next.tickets[0].id);
      }
      void refreshIssueMetrics(next.tickets.map((ticket) => ticket.id));
    } catch (error) {
      setSnapshot({ health: null, status: null, configuration: null, tickets: [], error: String(error) });
      setIssueMetrics({});
      notify(`Connection failed: ${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const refreshDetail = async (id: string) => {
    try {
      // Single aggregated request replaces the previous six-call fan-out
      // (ticket + comments + events + runs + analysis + plan) per selection.
      const bundle = await getTicketBundle(id, options);
      setSelectedTicket(bundle.ticket);
      setComments(bundle.comments);
      setEvents(bundle.events);
      setRuns(bundle.runs);
      setAnalysis(bundle.analysis);
      setPlan(bundle.plan);
      setIssueMetrics((current) => ({
        ...current,
        [id]: { comments: bundle.comments.length, runs: bundle.runs.length },
      }));
    } catch (error) {
      notify(`Failed to read ticket: ${String(error)}`);
    }
  };

  useEffect(() => {
    resolveRuntimeConfig()
      .then((config) => {
        setServerUrl(config.serverUrl);
        setAuthConfigured(config.authConfigured);
      })
      .catch((error) => notify(`Failed to read runtime config: ${String(error)}`));
  }, []);

  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [options.serverUrl, options.authToken]);

  useEffect(() => {
    if (!autoRefresh) return;
    const timer = window.setInterval(() => {
      void refresh();
    }, autoRefreshIntervalMs);
    return () => window.clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoRefresh, options.serverUrl, options.authToken]);

  useEffect(() => {
    try {
      localStorage.setItem(autoRefreshStorageKey, String(autoRefresh));
    } catch {
      // ignore persistence failures in restricted environments
    }
  }, [autoRefresh]);

  useEffect(() => {
    setSelectedTicket(null);
    setExportPreview("");
    if (selectedId) {
      void refreshDetail(selectedId);
    } else {
      setComments([]);
      setEvents([]);
      setRuns([]);
      setAnalysis(null);
      setPlan(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedId, options.serverUrl, options.authToken]);

  useEffect(() => {
    if (typeof window === "undefined") return;
    window.localStorage.setItem(watchStorageKey, JSON.stringify(watchStates));
  }, [watchStates]);

  useEffect(() => {
    if (typeof window === "undefined") return;
    window.localStorage.setItem(issueViewPreferencesStorageKey, JSON.stringify(issueViewPreferences));
  }, [issueViewPreferences]);

  useEffect(() => {
    if (typeof window === "undefined") return;
    window.localStorage.setItem(localNotesStorageKey, JSON.stringify(localNotes));
  }, [localNotes]);

  useEffect(() => {
    setLabelDraft("");
    setShowLabelEditor(false);
  }, [selectedId]);

  useEffect(() => {
    setVisibleIssueLimit(issuePageSize);
  }, [
    authorFilter,
    issueFilter,
    issuePageSize,
    issuePriorityFilter,
    issueRiskFilter,
    issueWatchFilter,
    issueSignalFilter,
    issueSort,
    searchQuery,
    selectedLabelFilter,
  ]);

  const submitTicket = async (event?: FormEvent<HTMLFormElement>) => {
    event?.preventDefault();
    setCreateNotice("");
    const trimmedTitle = draft.title.trim();
    if (trimmedTitle.length < 3) {
      const msg = t("Title must be at least 3 characters");
      setCreateNotice(msg);
      notify(msg);
      return;
    }
    if (trimmedTitle.length > 200) {
      const msg = t("Title must be at most 200 characters");
      setCreateNotice(msg);
      notify(msg);
      return;
    }
    const trimmedDescription = draft.description.trim() || "Created from Tea desktop.";
    if (trimmedDescription.length < 10) {
      const msg = t("Description must be at least 10 characters");
      setCreateNotice(msg);
      notify(msg);
      return;
    }
    setCreating(true);
    setCreateNotice(t("Creating..."));
    try {
      const draftLabels = draft.labels
        .split(/[,\n]/)
        .map((label) => label.trim())
        .filter(Boolean);
      const ticket = await createTicket(
        {
          title: trimmedTitle,
          description: trimmedDescription,
          approvalPolicy: draft.approvalPolicy.trim() || undefined,
          priority: draft.priority.trim() || undefined,
          labels: draftLabels.length > 0 ? draftLabels : undefined,
        },
        options,
      );
      setDraft({ title: "", description: "", approvalPolicy: "", priority: "", labels: "" });
      setIssueFilter("open");
      setShowNewIssue(false);
      setCreateNotice("");
      setSelectedId(ticket.id);
      notify(t("Created work order") + ` ${ticket.id}`);
      await refresh();
    } catch (error) {
      const msg = t("Create failed") + `: ${String(error)}`;
      setCreateNotice(msg);
      notify(msg);
    } finally {
      setCreating(false);
    }
  };

  const runAction = async (action: Parameters<typeof ticketAction>[1]) => {
    if (!selectedId) return;
    setBusy(true);
    try {
      await ticketAction(selectedId, action, options);
      notify(`Action submitted: ${action}`);
      await refresh();
      await refreshDetail(selectedId);
    } catch (error) {
      notify(`Action failed: ${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const submitReject = async (reason: string) => {
    if (!selectedId) return;
    const trimmed = reason.trim();
    if (!trimmed) {
      notify("Reject reason is required");
      return;
    }
    setBusy(true);
    try {
      await rejectTicket(selectedId, trimmed, options);
      setRejectReason("");
      notify("Approval rejected");
      await refresh();
      await refreshDetail(selectedId);
    } catch (error) {
      notify(`Reject failed: ${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const applyTicketPolicy = async (mode: string) => {
    if (!selectedId || !mode) return;
    setBusy(true);
    try {
      await setTicketPolicy(selectedId, mode, options);
      notify(`Approval policy set: ${mode}`);
      await refresh();
      await refreshDetail(selectedId);
    } catch (error) {
      notify(`Policy change failed: ${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const beginEditIssue = () => {
    if (!activeTicket) return;
    // Seed labels from the ticket's authoritative daemon labels, minus the
    // system-derived ones the daemon manages (source:/policy:/context:). This is
    // the server label set, independent of the local filter overlay.
    const operatorLabels = operatorLabelsForTicket(activeTicket);
    setEditDraft({
      title: activeTicket.title ?? "",
      description: activeTicket.description ?? "",
      priority: activeTicket.priority ?? "",
      labels: operatorLabels.join(", "),
    });
    setShowEditIssue(true);
  };

  const cancelEditIssue = () => {
    setShowEditIssue(false);
  };

  const submitTicketEdit = async () => {
    if (!selectedId || !activeTicket) return;
    const nextTitle = editDraft.title.trim();
    if (!nextTitle) {
      notify("Work order title is required");
      return;
    }
    // Only send fields the operator actually changed so an edit cannot clobber
    // untouched fields; the daemon leaves `undefined` fields alone.
    const input: UpdateTicketInput = {};
    if (nextTitle !== (activeTicket.title ?? "")) {
      input.title = nextTitle;
    }
    if (editDraft.description !== (activeTicket.description ?? "")) {
      input.description = editDraft.description;
    }
    const nextPriority = editDraft.priority.trim();
    if (nextPriority !== (activeTicket.priority ?? "")) {
      input.priority = nextPriority;
    }
    // Compare operator labels (system-derived labels are managed by the daemon
    // and never sent from the edit form). Only send labels when they changed.
    const currentOperatorLabels = operatorLabelsForTicket(activeTicket);
    const nextOperatorLabels = editDraft.labels
      .split(/[,\n]/)
      .map((label) => label.trim())
      .filter(Boolean)
      .filter((label) => !isSystemLabel(label));
    const dedupedNextLabels = Array.from(new Set(nextOperatorLabels));
    const labelsChanged =
      dedupedNextLabels.length !== currentOperatorLabels.length ||
      dedupedNextLabels.some((label, index) => label !== currentOperatorLabels[index]);
    if (labelsChanged) {
      input.labels = dedupedNextLabels;
    }
    if (
      input.title === undefined &&
      input.description === undefined &&
      input.priority === undefined &&
      input.labels === undefined
    ) {
      notify("No changes to save");
      setShowEditIssue(false);
      return;
    }
    setBusy(true);
    try {
      await updateTicket(selectedId, input, options);
      setShowEditIssue(false);
      notify("Work order updated");
      await refresh();
      await refreshDetail(selectedId);
    } catch (error) {
      notify(`Edit failed: ${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const saveLocalConfiguration = async (config: TeaLocalConfig) => {
    setBusy(true);
    try {
      await updateConfiguration(config, options);
      notify("Tea local configuration saved");
      await refresh();
    } catch (error) {
      notify(`Save configuration failed: ${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const stopRunAction = async (runId: string) => {
    setBusy(true);
    try {
      await stopRun(runId, options);
      notify(`Run stop submitted: ${runId}`);
      await refresh();
      if (selectedId) await refreshDetail(selectedId);
    } catch (error) {
      notify(`Run stop failed: ${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const retryRunAction = async (runId: string) => {
    setBusy(true);
    try {
      await retryRun(runId, options);
      notify(`Run retry submitted: ${runId}`);
      await refresh();
      if (selectedId) await refreshDetail(selectedId);
    } catch (error) {
      notify(`Run retry failed: ${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const submitComment = async (body: string) => {
    if (!selectedId) return;
    const trimmed = body.trim();
    if (!trimmed) {
      notify("Review comment cannot be empty");
      return;
    }
    setBusy(true);
    try {
      await addComment(selectedId, trimmed, options);
      setCommentDraft("");
      notify("Review comment added");
      await refresh();
      await refreshDetail(selectedId);
    } catch (error) {
      notify(`Comment failed: ${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const previewExport = async (format: "json" | "markdown") => {
    if (!selectedId) return;
    try {
      const exported = await exportTicket(selectedId, format, options);
      setExportPreview(typeof exported === "string" ? exported : pretty(exported));
    } catch (error) {
      setExportPreview(`Export failed: ${String(error)}`);
    }
  };

  const downloadExport = async (format: "json" | "markdown") => {
    if (!selectedId || !activeTicket) return;
    setBusy(true);
    try {
      const exported = await exportTicket(selectedId, format, options);
      const content = typeof exported === "string" ? exported : pretty(exported);
      const extension = format === "json" ? "json" : "md";
      const fileName = `tea-${issueNumber(activeTicket)}-${exportTimestamp()}.${extension}`;
      const savedPath = await saveExport(fileName, content);
      setExportPreview(content);
      notify(`Saved ${format} export to ${savedPath}`);
    } catch (error) {
      notify(`Export download failed: ${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const copyIssueLink = async () => {
    if (!activeTicket) return;
    const link = buildTicketLink(activeTicket, serverUrl);
    try {
      if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(link);
        notify(`Copied issue link: ${link}`);
        return;
      }
      notify(`Issue link ready: ${link}`);
    } catch (error) {
      notify(`Copy link failed: ${String(error)}`);
    }
  };

  const toggleWatchIssue = () => {
    if (!activeTicket) return;
    setWatchStates((current) => ({
      ...current,
      [activeTicket.id]: !current[activeTicket.id],
    }));
    notify(selectedIsWatched ? "Issue un-watched locally" : "Issue watched locally");
  };

  const addLocalNote = () => {
    if (!activeTicket) return;
    const nextNote = labelDraft.trim();
    if (!nextNote) {
      notify("Note text is required");
      return;
    }
    // Local notes are additive annotations, kept only in this browser and never
    // sent to the daemon. They do not seed from or replace daemon labels.
    const merged = normalizeLabels([...selectedLocalNotes, nextNote]);
    setLocalNotes((current) => ({
      ...current,
      [activeTicket.id]: merged,
    }));
    setLabelDraft("");
    setShowLabelEditor(true);
    notify(`Added local note: ${nextNote}`);
  };

  const removeLocalNote = (note: string) => {
    if (!activeTicket) return;
    const nextNotes = selectedLocalNotes.filter((currentNote) => currentNote !== note);
    setLocalNotes((current) => ({
      ...current,
      [activeTicket.id]: nextNotes,
    }));
    notify(`Removed local note: ${note}`);
  };

  const resetLocalNotes = () => {
    if (!activeTicket) return;
    setLocalNotes((current) => {
      const next = { ...current };
      delete next[activeTicket.id];
      return next;
    });
    setLabelDraft("");
    notify("Cleared local notes for this work order");
  };

  return (
    <main className="issue-shell">
      <header className="repo-header">
        <div className="repo-identity">
          <span className="repo-owner">{t("Tea")}</span>
          <h1>{t("AI Work Orders")}</h1>
          <span className="repo-connection-inline">
            <span className={`state-dot ${snapshot?.status ? "ok" : "warn"}`} />
            {snapshot?.status ? t("Tea daemon connected") : statusText(snapshot)}
          </span>
        </div>
        <div className="repo-actions">
          <div className="refresh-control">
            <button className="ghost-button" disabled={busy} onClick={() => void refresh()}>
              {busy ? t("Refreshing...") : t("Refresh")}
            </button>
            <button
              aria-pressed={autoRefresh}
              className={`auto-refresh-toggle ${autoRefresh ? "on" : "off"}`}
              onClick={() => setAutoRefresh((value) => !value)}
              type="button"
            >
              {autoRefresh ? t("Auto-refresh on") : t("Auto-refresh off")}
            </button>
            <span className="refresh-status">
              {lastRefreshedAt
                ? `${t("Updated")} ${formatTime(new Date(lastRefreshedAt).toISOString())}`
                : t("Not refreshed yet")}
            </span>
          </div>
          <button className="new-issue-button" onClick={() => setShowNewIssue((value) => !value)}>
            {t("New Work Order")}
          </button>
        </div>
      </header>

      <nav className="repo-tabs" aria-label={t("Tea work-order sections")} role="tablist">
        <button
          aria-selected={activeSection === "issues"}
          className={activeSection === "issues" ? "active" : ""}
          onClick={() => setActiveSection("issues")}
          role="tab"
          type="button"
        >
          {t("Issues")} <span>{tickets.length}</span>
        </button>
        <button
          aria-selected={activeSection === "runs"}
          className={activeSection === "runs" ? "active" : ""}
          onClick={() => setActiveSection("runs")}
          role="tab"
          type="button"
        >
          {t("Runs")} <span>{activeTicket ? runs.length : "-"}</span>
        </button>
        <button
          aria-selected={activeSection === "plan"}
          className={activeSection === "plan" ? "active" : ""}
          onClick={() => setActiveSection("plan")}
          role="tab"
          type="button"
        >
          {t("Plan")} <span>{activeTicket ? (plan ? "✓" : analysis ? "~" : "-") : "-"}</span>
        </button>
        <button
          aria-selected={activeSection === "comments"}
          className={activeSection === "comments" ? "active" : ""}
          onClick={() => setActiveSection("comments")}
          role="tab"
          type="button"
        >
          {t("Comments")} <span>{activeTicket ? comments.length : "-"}</span>
        </button>
        <button
          aria-selected={activeSection === "exports"}
          className={activeSection === "exports" ? "active" : ""}
          onClick={() => setActiveSection("exports")}
          role="tab"
          type="button"
        >
          {t("Exports")}
        </button>
        <button
          aria-selected={activeSection === "settings"}
          className={activeSection === "settings" ? "active" : ""}
          onClick={() => setActiveSection("settings")}
          role="tab"
          type="button"
        >
          {t("Settings")}
        </button>
      </nav>

      {activeSection === "settings" ? (
        <section className="connection-strip">
          <div className="connection-state">
            <span className={`state-dot ${snapshot?.status ? "ok" : "warn"}`} />
            <strong>{statusText(snapshot)}</strong>
            <span>{message}</span>
          </div>
          <label>
            <span>{t("Tea daemon")}</span>
            <input value={serverUrl} onChange={(event) => setServerUrl(event.target.value)} />
          </label>
          <label>
            <span>{t("Bearer token")} {authConfigured ? t("(launcher/env configured)") : t("(optional override)")}</span>
            <input
              value={authToken}
              onChange={(event) => setAuthToken(event.target.value)}
              placeholder={t("Leave blank to use TEA_AUTH_TOKEN/dev-token")}
              type="password"
            />
          </label>
        </section>
      ) : null}

      <details className="activity-log" open={activityLog.length > 0}>
        <summary className="activity-log-summary">
          <span>{t("Activity log")}</span>
          <span className="activity-log-count">{activityLog.length}</span>
          {activityLog.length > 0 ? (
            <button
              className="activity-log-clear"
              onClick={(event) => {
                event.preventDefault();
                clearActivityLog();
              }}
              type="button"
            >
              {t("Clear log")}
            </button>
          ) : null}
        </summary>
        {activityLog.length === 0 ? (
          <p className="activity-log-empty">{t("No operator actions recorded yet.")}</p>
        ) : (
          <ul className="activity-log-list">
            {activityLog.map((entry) => (
              <li className={`activity-log-entry tone-${entry.tone}`} key={entry.id}>
                <span className="activity-log-time">
                  {formatTime(new Date(entry.at).toISOString())}
                </span>
                <span className="activity-log-message">{entry.message}</span>
              </li>
            ))}
          </ul>
        )}
      </details>

      <section className="issue-workspace">
        <section className="issue-index" aria-label={t("Work order index")}>
          <div className="issue-toolbar">
            <div className="issue-preset-queue-lane" aria-label={t("Preset work-order queues")}>
              {issuePresetQueues.map((queue) => (
                <button
                  aria-pressed={activeIssuePresetQueueKey === queue.key}
                  className="issue-preset-queue-card"
                  key={queue.key}
                  onClick={() => applyIssuePresetQueue(queue)}
                  type="button"
                >
                  <span>
                    <strong>{issuePresetQueueCount(queue)}</strong>
                    <span>{t(queue.label)}</span>
                  </span>
                  <small>{t(queue.description)}</small>
                </button>
              ))}
            </div>
            <div className="issue-queue-summary-card">
              <div>
                <span>{t("Current queue")}</span>
                <strong>{activeIssuePresetQueue ? t(activeIssuePresetQueue.label) : t("Custom filtered queue")}</strong>
                <small>
                  {activeIssuePresetQueue
                    ? t(activeIssuePresetQueue.description)
                    : t("A custom combination of state, signal, search, author, label, priority, and risk filters.")}
                </small>
                <span className={`issue-view-preference-status ${issueViewPreferencesAreDefault ? "default" : "saved"}`}>
                  {issueViewPreferenceStatus}
                </span>
                <div className="issue-queue-summary-actions">
                  <button disabled={issueViewPreferencesAreDefault} onClick={resetIssueViewPreferences} type="button">
                    {t("Reset view")}
                  </button>
                </div>
              </div>
              <div className="issue-queue-summary-grid" aria-label={t("Current queue summary")}>
                {issueQueueSummaryItems.map((item) => (
                  <span key={t(item.label)}>
                    <small>{t(item.label)}</small>
                    <strong>{item.value}</strong>
                  </span>
                ))}
              </div>
            </div>
            <div className="issue-filter-tabs" role="tablist" aria-label={t("Issue state filters")}>
              <button
                aria-selected={issueFilter === "open"}
                className={issueFilter === "open" ? "active" : ""}
                onClick={() => setIssueFilter("open")}
                role="tab"
              >
                {t("Open")} <span>{openCount}</span>
              </button>
              <button
                aria-selected={issueFilter === "closed"}
                className={issueFilter === "closed" ? "active" : ""}
                onClick={() => setIssueFilter("closed")}
                role="tab"
              >
                {t("Closed")} <span>{closedCount}</span>
              </button>
              <button
                aria-selected={issueFilter === "all"}
                className={issueFilter === "all" ? "active" : ""}
                onClick={() => setIssueFilter("all")}
                role="tab"
              >
                {t("All")} <span>{tickets.length}</span>
              </button>
              <button
                aria-expanded={showAdvancedFilters}
                aria-pressed={showAdvancedFilters}
                className={`issue-advanced-filter-toggle ${showAdvancedFilters ? "active" : ""}`}
                onClick={() => setShowAdvancedFilters((value) => !value)}
                type="button"
              >
                {t("Filters")}
                {activeIssueFilterChips.length > 0 ? <span>{activeIssueFilterChips.length}</span> : null}
              </button>
            </div>
            <div className="issue-query-bar">
              <button disabled={!hasActiveIssueFilters} onClick={clearIssueFilters} type="button">
                {t("Clear filters")}
              </button>
              <input
                aria-label={t("Search work orders")}
                className="issue-search"
                onChange={(event) => setSearchQuery(event.target.value)}
                placeholder={t("Search issues...")}
                value={searchQuery}
              />
              <button type="button" onClick={() => setShowAuthorFilterPanel((value) => !value)}>
                {t("Author")} {authorFilter ? `(${authorFilter})` : ""}
              </button>
              <button type="button" onClick={() => setShowLabelFilterPanel((value) => !value)}>
                {t("Labels")} {selectedLabelFilter ? `(${selectedLabelFilter})` : ""}
              </button>
              <label className="issue-signal-filter-control">
                <span>{t("Signal")}</span>
                <select
                  aria-label={t("Filter work orders by signal")}
                  onChange={(event) => setIssueSignalFilter(event.target.value as IssueSignalFilter)}
                  value={issueSignalFilter}
                >
                  {issueSignalFilterOptions.map((option) => (
                    <option key={option.value} value={option.value}>
                      {t(option.label)}
                    </option>
                  ))}
                </select>
              </label>
              <label className="issue-sort-control">
                <span>{t("Sort")}</span>
                <select
                  aria-label={t("Sort work orders")}
                  onChange={(event) => setIssueSort(event.target.value as IssueSort)}
                  value={issueSort}
                >
                  <option value="updated">{t("Recently updated")}</option>
                  <option value="created">{t("Newest created")}</option>
                  <option value="activity">{t("Most activity")}</option>
                  <option value="touch">{t("Latest touch")}</option>
                </select>
              </label>
              <label className="issue-density-control">
                <span>{t("Density")}</span>
                <select
                  aria-label={t("Issue list density")}
                  onChange={(event) => setIssueListDensity(event.target.value as IssueListDensity)}
                  value={issueListDensity}
                >
                  <option value="comfortable">{t("Comfortable")}</option>
                  <option value="compact">{t("Compact")}</option>
                </select>
              </label>
            </div>
            {showAdvancedFilters ? (
              <>
                <div className="issue-signal-filter-lane" aria-label={t("Signal queue filters")}>
                  {issueSignalFilterOptions.map((option) => (
                    <button
                      aria-pressed={issueSignalFilter === option.value}
                      className={`issue-signal-filter-pill tone-${option.tone}`}
                      key={option.value}
                      onClick={() => setIssueSignalFilter(option.value)}
                      type="button"
                    >
                      <span>{t(option.label)}</span>
                      <strong>{issueSignalCountForOption(option.value)}</strong>
                    </button>
                  ))}
                </div>
                <div className="issue-triage-filter-lane" aria-label={t("Priority and risk quick filters")}>
                  <button
                    aria-pressed={issuePriorityFilter === "high"}
                    className="issue-triage-filter-button priority"
                    onClick={() => setIssuePriorityFilter((current) => (current === "high" ? "all" : "high"))}
                    type="button"
                  >
                    <span>{t("High priority")}</span>
                    <strong>{issueTriageFilterCounts.highPriority}</strong>
                  </button>
                  <button
                    aria-pressed={issueRiskFilter === "high"}
                    className="issue-triage-filter-button risk"
                    onClick={() => setIssueRiskFilter((current) => (current === "high" ? "all" : "high"))}
                    type="button"
                  >
                    <span>{t("High risk")}</span>
                    <strong>{issueTriageFilterCounts.highRisk}</strong>
                  </button>
                  <button
                    aria-pressed={issueWatchFilter === "watched"}
                    className="issue-triage-filter-button watch"
                    onClick={() => setIssueWatchFilter((current) => (current === "watched" ? "all" : "watched"))}
                    type="button"
                  >
                    <span>{t("Watched")}</span>
                    <strong>{issueTriageFilterCounts.watched}</strong>
                  </button>
                </div>
              </>
            ) : null}
            {hasActiveIssueFilters ? (
              <div className="active-filter-chip-row" aria-label={t("Active issue filters")}>
                <span className="active-filter-chip-label">{t("Active filters")}</span>
                {activeIssueFilterChips.map((chip) => (
                  <button
                    aria-label={`Remove filter ${t(chip.label)}: ${chip.value}`}
                    className="active-filter-chip"
                    key={chip.key}
                    onClick={() => removeIssueFilterChip(chip.key)}
                    type="button"
                  >
                    <span>{t(chip.label)}</span>
                    <strong>{chip.value}</strong>
                    <span aria-hidden="true">×</span>
                  </button>
                ))}
                <button className="clear-filter-link" onClick={clearIssueFilters} type="button">
                  {t("Clear all filters")}
                </button>
              </div>
            ) : null}
            <div className="author-filter-panel">
              {showAuthorFilterPanel ? (
                <>
                  <div className="label-filter-panel-header">
                    <strong>{t("Author filters")}</strong>
                    <button type="button" onClick={clearAuthorFilter}>
                      {t("Clear author")}
                    </button>
                  </div>
                  <div className="label-filter-list">
                    {availableAuthors.length > 0 ? (
                      availableAuthors.map((author) => (
                        <button
                          aria-pressed={authorFilter === author}
                          className={authorFilter === author ? "active" : ""}
                          key={author}
                          onClick={() => setAuthorFilter((current) => (current === author ? null : author))}
                          type="button"
                        >
                          {author}
                        </button>
                      ))
                    ) : (
                      <span className="label-filter-empty">{t("No authors available for filtering.")}</span>
                    )}
                  </div>
                </>
              ) : authorFilter ? (
                <div className="label-filter-summary">
                  <span>
                    {t("Filtering by author:")} <strong>{authorFilter}</strong>
                  </span>
                  <button type="button" onClick={clearAuthorFilter}>
                    {t("Clear author")}
                  </button>
                </div>
              ) : null}
            </div>
            <div className="label-filter-panel">
              {showLabelFilterPanel ? (
                <>
                  <div className="label-filter-panel-header">
                    <strong>{t("Label filters")}</strong>
                    <button type="button" onClick={clearLabelFilter}>
                      {t("Clear filter")}
                    </button>
                  </div>
                  <div className="label-filter-list">
                    {availableLabels.length > 0 ? (
                      availableLabels.map((label) => (
                        <button
                          aria-pressed={selectedLabelFilter === label}
                          className={selectedLabelFilter === label ? "active" : ""}
                          key={label}
                          onClick={() =>
                            setSelectedLabelFilter((current) => (current === label ? null : label))
                          }
                          type="button"
                        >
                          {label}
                        </button>
                      ))
                    ) : (
                      <span className="label-filter-empty">{t("No labels available for filtering.")}</span>
                    )}
                  </div>
                </>
              ) : selectedLabelFilter ? (
                <div className="label-filter-summary">
                  <span>
                    {t("Filtering by label:")} <strong>{selectedLabelFilter}</strong>
                  </span>
                  <button type="button" onClick={clearLabelFilter}>
                    {t("Clear filter")}
                  </button>
                </div>
              ) : null}
            </div>
            <p className="issue-count-summary">
              {t("Showing")} {visibleTickets.length} / {filteredTickets.length} {issueFilterLabel}
              {"，"}
              {t("Signal focus:")} {issueSignalFilterSummary}
              {"，"}
              {openCount} {t("open")}
              {"，"}
              {closedCount} {t("closed")}
            </p>
          </div>

          {showNewIssue ? (
            <form className="new-issue-panel" onSubmit={(event) => void submitTicket(event)}>
              <div className="new-issue-header">
                <div>
                  <h2>{t("New Work Order")}</h2>
                  <p>{t("Create an AI issue with a goal, context, and acceptance criteria.")}</p>
                </div>
                <button className="link-button" onClick={() => setShowNewIssue(false)} type="button">
                  {t("Cancel")}
                </button>
              </div>
              <input
                autoFocus
                onChange={(event) => setDraft({ ...draft, title: event.target.value })}
                placeholder={t("Title: e.g. Investigate provider failure in login flow")}
                value={draft.title}
              />
              <textarea
                onChange={(event) => setDraft({ ...draft, description: event.target.value })}
                placeholder={t("Write a markdown description: background, expected result, constraints, and evidence needed.")}
                value={draft.description}
              />
              <section className="template-panel" aria-label={t("Suggested work-order templates")}>
                <h3>{t("Suggested work-order templates")}</h3>
                <div className="quick-template-grid">
                  <button onClick={() => applyTemplate("investigation")} type="button">
                    {t("Investigation")}
                  </button>
                  <button onClick={() => applyTemplate("implementation")} type="button">
                    {t("Implementation")}
                  </button>
                  <button onClick={() => applyTemplate("release")} type="button">
                    {t("Release validation")}
                  </button>
                </div>
              </section>
              <div className="new-issue-metadata" aria-label={t("New work order metadata")}>
                <label className="new-issue-priority">
                  <span>{t("Priority")}</span>
                  <select
                    onChange={(event) => setDraft({ ...draft, priority: event.target.value })}
                    value={draft.priority}
                  >
                    <option value="">{t("Default priority (normal)")}</option>
                    {createPriorityOptions.map((option) => (
                      <option key={option.value} value={option.value}>
                        {t(option.label)}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="new-issue-labels">
                  <span>{t("Initial labels")}</span>
                  <input
                    onChange={(event) => setDraft({ ...draft, labels: event.target.value })}
                    placeholder={t("Comma-separated, e.g. area:auth, needs-triage")}
                    value={draft.labels}
                  />
                </label>
              </div>
              <div className="form-row">
                <select
                  onChange={(event) => setDraft({ ...draft, approvalPolicy: event.target.value })}
                  value={draft.approvalPolicy}
                >
                  <option value="">{t("Default approval policy")}</option>
                  {approvalPolicyOptions.map((option) => (
                    <option key={option.value} value={option.value}>
                      {t(option.label)}
                    </option>
                  ))}
                </select>
                <button
                  className="new-issue-button"
                  disabled={creating}
                  type="button"
                  onClick={() => void submitTicket()}
                >
                  {creating ? t("Creating...") : t("Submit work order")}
                </button>
              </div>
              {createNotice ? (
                <p className="new-issue-notice" role="status">
                  {createNotice}
                </p>
              ) : null}
            </form>
          ) : null}

          <div className="issue-list" data-density={issueListDensity} role="list">
            {visibleTickets.map((ticket) => {
              const metrics = issueMetrics[ticket.id];
              const latestTouch = metrics?.latestTouch;
              const issueSignal = issueSignalForTicket(ticket, metrics);
              const actionHint = issueActionHintForTicket(ticket, metrics, issueSignal);
              const ownerLabel = `${t("Owner")} ${ticket.owner_human_id ?? t("Tea local operator")}`;
              const agentLabel = `${t("Agent")} ${ticket.delegated_agent_id ?? t("not delegated")}`;
              return (
                <button
                  aria-current={selectedId === ticket.id ? "true" : undefined}
                  className={`issue-item ${selectedId === ticket.id ? "selected issue-item-selected" : ""}`}
                  key={ticket.id}
                  onClick={() => setSelectedId(ticket.id)}
                  role="listitem"
                >
                  <span className={`issue-state ${isClosedTicket(ticket) ? "closed" : "open"}`}>
                    {t(issueStateLabel(ticket))}
                  </span>
                  <span className="issue-item-main">
                    <span className="issue-item-topline">
                      <strong>{ticket.title}</strong>
                      <small>{issueNumber(ticket)}</small>
                    </span>
                    <small>{t("opened")} {formatTime(ticket.created_at)} · Tea</small>
                  </span>
                  <span className="issue-item-summary">{issueSummary(ticket)}</span>
                  <div className="issue-item-context">
                    <span className="issue-item-meta">
                      <span>{issueAgeLabel(ticket)}</span>
                      {metrics ? (
                        <span>
                          {metrics.comments} {t("comments")} · {metrics.runs} {t("runs")}
                        </span>
                      ) : (
                        <span>{t("Activity loading")}</span>
                      )}
                    </span>
                    {metrics?.latestTouch ? (
                      <span className="issue-item-latest-touch">
                        <span className={`issue-touch-badge ${metrics.latestTouch.group}`}>
                          {conversationEntryGroupLabel(metrics.latestTouch.group)}
                        </span>
                        <span>{latestTouchLabel(metrics.latestTouch)}</span>
                      </span>
                    ) : null}
                    <span className="issue-item-routing">
                      <span>{ownerLabel}</span>
                      <span>{agentLabel}</span>
                    </span>
                  </div>
                  <span className="issue-item-compact-scanline" aria-label={`Compact scanline for ${ticket.title}`}>
                    <span className="issue-signal-with-reason">
                      <span className={`issue-signal-chip tone-${issueSignal.tone}`} title={issueSignal.description}>
                        {t(issueSignal.label)}
                      </span>
                      <span className="issue-signal-reason">{issueSignal.reason}</span>
                    </span>
                    <span className={`issue-action-hint tone-${actionHint.tone}`} title={actionHint.description}>
                      {t(actionHint.label)}
                    </span>
                    {latestTouch ? (
                      <span className="issue-item-latest-touch">
                        <span className={`issue-touch-badge ${latestTouch.group}`}>
                          {conversationEntryGroupLabel(latestTouch.group)}
                        </span>
                        <span>{latestTouchLabel(latestTouch)}</span>
                      </span>
                    ) : (
                      <span className="issue-item-latest-touch muted">
                        <span className="issue-touch-badge system">{t("Idle")}</span>
                        <span>{latestTouchLabel(undefined)}</span>
                      </span>
                    )}
                    <span className="issue-item-routing">
                      <span>{ownerLabel}</span>
                      <span>{agentLabel}</span>
                    </span>
                  </span>
                  <span className="issue-item-badges">
                    <span className="issue-signal-with-reason">
                      <span className={`issue-signal-chip tone-${issueSignal.tone}`} title={issueSignal.description}>
                        {t(issueSignal.label)}
                      </span>
                      <span className="issue-signal-reason">{issueSignal.reason}</span>
                    </span>
                    <span className={`issue-action-hint tone-${actionHint.tone}`} title={actionHint.description}>
                      {t(actionHint.label)}
                    </span>
                    <span className={`issue-badge tone-${badgeToneForPriority(ticket.priority)}`}>
                      {t("Priority")} {t(ticket.priority ?? "normal")}
                    </span>
                    <span className={`issue-badge tone-${badgeToneForRisk(ticket.risk_level)}`}>
                      {t("Risk")} {t(ticket.risk_level ?? "medium")}
                    </span>
                    {watchStates[ticket.id] ? (
                      <span className="issue-badge tone-watch">{t("Watched")}</span>
                    ) : null}
                  </span>
                  <span className="issue-item-footer">
                    <span className="issue-item-labels">
                      {daemonLabelsForTicket(ticket).map((label) => (
                        <span key={label}>{label}</span>
                      ))}
                      {localNotesForTicket(ticket, localNotes).map((note) => (
                        <span className="issue-item-note" key={`note-${note}`}>
                          {note}
                        </span>
                      ))}
                    </span>
                    <span className="issue-metrics" aria-label={`Activity for ${ticket.title}`}>
                      {metrics ? (
                        <>
                          <span>{metrics.comments} {t("comments")}</span>
                          <span>{metrics.runs} {t("runs")}</span>
                        </>
                      ) : (
                        <span>{t("Activity loading")}</span>
                      )}
                    </span>
                  </span>
                </button>
              );
            })}
            {filteredTickets.length === 0 ? (
              issueWatchFilter === "watched" ? (
                <div className="watched-empty-state">
                  <strong>{t("No watched work orders match this queue.")}</strong>
                  <span>{t("Watch a work order locally, or show all work orders for the current filters.")}</span>
                  <button type="button" onClick={clearWatchedIssueFilter}>
                    {t("Show all work orders")}
                  </button>
                </div>
              ) : (
                <p className="empty">{t("No work orders match this filter.")}</p>
              )
            ) : null}
            {filteredTickets.length > 0 ? (
              <div className="issue-list-pagination">
                <span>
                  {t("Showing")} {visibleTickets.length} / {sortedTickets.length}{" "}
                  {t("work orders in the current queue.")}
                </span>
                <div className="issue-list-pagination-actions">
                  {hasMoreVisibleTickets ? (
                    <button type="button" onClick={showMoreIssues}>
                      {t("Show more work orders")}
                    </button>
                  ) : null}
                  {canCollapseIssueList ? (
                    <button type="button" onClick={collapseIssueList}>
                      {t("Collapse list")}
                    </button>
                  ) : null}
                </div>
              </div>
            ) : null}
          </div>
        </section>

        <IssueDetail
          activeSection={activeSection}
          activeTicket={activeTicket}
          analysis={analysis}
          busy={busy}
          commentDraft={commentDraft}
          comments={comments}
          hasLocalNotes={selectedHasLocalNotes}
          isWatched={selectedIsWatched}
          events={events}
          exportPreview={exportPreview}
          labelDraft={labelDraft}
          daemonLabels={selectedDaemonLabels}
          localNotes={selectedLocalNotes}
          onAddLabel={addLocalNote}
          onCopyLink={() => void copyIssueLink()}
          onAction={(action) => void runAction(action)}
          onApplyPolicy={(mode) => void applyTicketPolicy(mode)}
          onComment={(body) => void submitComment(body)}
          onCommentDraftChange={setCommentDraft}
          onDownloadExport={(format) => void downloadExport(format)}
          onExport={(format) => void previewExport(format)}
          onNavigateIssueQueue={navigateIssueQueue}
          onLabelDraftChange={setLabelDraft}
          onReject={(reason) => void submitReject(reason)}
          onRejectReasonChange={setRejectReason}
          onRemoveLabel={removeLocalNote}
          onResetLabels={resetLocalNotes}
          onRetryRun={(runId) => void retryRunAction(runId)}
          onSaveConfiguration={(config) => void saveLocalConfiguration(config)}
          onSectionChange={setActiveSection}
          onStopRun={(runId) => void stopRunAction(runId)}
          plan={plan}
          onToggleWatch={toggleWatchIssue}
          onToggleLabelEditor={() => setShowLabelEditor((current) => !current)}
          onBeginEdit={beginEditIssue}
          onCancelEdit={cancelEditIssue}
          onEditDraftChange={(patch) => setEditDraft((current) => ({ ...current, ...patch }))}
          onSubmitEdit={() => void submitTicketEdit()}
          showEditIssue={showEditIssue}
          editDraft={editDraft}
          rejectReason={rejectReason}
          runs={runs}
          queueNavigation={issueQueueNavigation}
          selectedActionHint={selectedActionHint}
          selectedSignal={selectedSignal}
          showLabelEditor={showLabelEditor}
          snapshot={snapshot}
          ticket={activeTicket}
        />
      </section>
    </main>
  );
}

function IssueDetail({
  activeSection,
  activeTicket,
  analysis,
  busy,
  commentDraft,
  comments,
  hasLocalNotes,
  events,
  exportPreview,
  isWatched,
  labelDraft,
  daemonLabels,
  localNotes,
  onAddLabel,
  onCopyLink,
  onAction,
  onApplyPolicy,
  onComment,
  onCommentDraftChange,
  onDownloadExport,
  onExport,
  onNavigateIssueQueue,
  onLabelDraftChange,
  onReject,
  onRejectReasonChange,
  onRemoveLabel,
  onResetLabels,
  onRetryRun,
  onSaveConfiguration,
  onSectionChange,
  onStopRun,
  onToggleWatch,
  onToggleLabelEditor,
  onBeginEdit,
  onCancelEdit,
  onEditDraftChange,
  onSubmitEdit,
  showEditIssue,
  editDraft,
  plan,
  queueNavigation,
  rejectReason,
  runs,
  selectedActionHint,
  selectedSignal,
  showLabelEditor,
  snapshot,
  ticket,
}: {
  activeSection: RepoSection;
  activeTicket: TeaTicket | null;
  analysis: TeaAnalysis | null;
  busy: boolean;
  commentDraft: string;
  comments: TeaComment[];
  hasLocalNotes: boolean;
  events: TeaEvent[];
  exportPreview: string;
  plan: TeaPlan | null;
  isWatched: boolean;
  labelDraft: string;
  daemonLabels: string[];
  localNotes: string[];
  onAddLabel: () => void;
  onCopyLink: () => void;
  onAction: (action: Parameters<typeof ticketAction>[1]) => void;
  onApplyPolicy: (mode: string) => void;
  onComment: (body: string) => void;
  onCommentDraftChange: (body: string) => void;
  onDownloadExport: (format: "json" | "markdown") => void;
  onExport: (format: "json" | "markdown") => void;
  onNavigateIssueQueue: (ticketId: string | null) => void;
  onLabelDraftChange: (value: string) => void;
  onReject: (reason: string) => void;
  onRejectReasonChange: (value: string) => void;
  onRemoveLabel: (label: string) => void;
  onResetLabels: () => void;
  onRetryRun: (runId: string) => void;
  onSaveConfiguration: (config: TeaLocalConfig) => void;
  onSectionChange: (section: RepoSection) => void;
  onStopRun: (runId: string) => void;
  onToggleWatch: () => void;
  onToggleLabelEditor: () => void;
  onBeginEdit: () => void;
  onCancelEdit: () => void;
  onEditDraftChange: (patch: Partial<TicketEditDraft>) => void;
  onSubmitEdit: () => void;
  showEditIssue: boolean;
  editDraft: TicketEditDraft;
  queueNavigation: IssueQueueNavigation;
  rejectReason: string;
  runs: TeaRun[];
  selectedActionHint: IssueActionHint | null;
  selectedSignal: IssueSignal | null;
  showLabelEditor: boolean;
  snapshot: TeaSnapshot | null;
  ticket: TeaTicket | null;
}) {
  if (!ticket) {
    return (
      <article className="issue-detail empty-detail">
        <h2>{t("Issue / Work Order")}</h2>
        <p>{t("Select a work order from the list, or create a new one to start an AI task.")}</p>
      </article>
    );
  }

  const handleSuggestedActionHint = () => {
    if (!selectedActionHint) return;
    const target = selectedActionHint.target;
    switch (target.kind) {
      case "section":
        onSectionChange(target.section);
        return;
      case "export":
        onSectionChange("exports");
        onExport(target.format);
        return;
      case "action":
        onSectionChange(target.sectionAfterAction);
        onAction(target.action);
        return;
    }
  };

  return (
    <article className="issue-detail">
      <header className="issue-titlebar">
        <div className="issue-title-primary">
          <div className="issue-title-topline">
            <span className={`issue-state ${isClosedTicket(ticket) ? "closed" : "open"}`}>
              {t(issueStateLabel(ticket))}
            </span>
            <span className="issue-number-badge">{issueNumber(ticket)}</span>
            <span>{ticket.status}</span>
          </div>
          <div className="issue-title-heading">
            <h2>{ticket.title}</h2>
          </div>
          <p className="issue-title-subline">
            {t("Work Order")} {issueNumber(ticket)} · {t("opened")} {formatTime(ticket.created_at)}
          </p>
          <div className="issue-title-meta" aria-label={t("Issue metadata summary")}>
            <span>{issueAgeLabel(ticket)}</span>
            <span>{comments.length} {t("comments")}</span>
            <span>{runs.length} {t("runs")}</span>
            <span>{ticket.approval_policy ?? t("default policy")}</span>
          </div>
        </div>
        <div className="issue-title-secondary">
          <div
            className={`issue-queue-navigation ${queueNavigation.isOutsideQueue ? "outside-queue" : "in-queue"}`}
            aria-label={t("Issue queue navigation")}
          >
            <button
              disabled={!queueNavigation.previousId}
              onClick={() => onNavigateIssueQueue(queueNavigation.previousId)}
              type="button"
            >
              {t("Previous issue")}
            </button>
            <span className="issue-queue-position">
              {queueNavigation.isOutsideQueue ? t("Selected outside current queue") : t("Queue position")}{" "}
              {queueNavigation.current > 0 ? queueNavigation.current : "-"} {t("of")} {queueNavigation.total}
            </span>
            <small>{t("Alt+ArrowUp / Alt+ArrowDown · Alt+Home / Alt+End")}</small>
            {queueNavigation.isOutsideQueue ? (
              <button
                disabled={!queueNavigation.firstId}
                onClick={() => onNavigateIssueQueue(queueNavigation.firstId)}
                type="button"
              >
                {t("Select first matching issue")}
              </button>
            ) : null}
            <button
              disabled={!queueNavigation.nextId}
              onClick={() => onNavigateIssueQueue(queueNavigation.nextId)}
              type="button"
            >
              {t("Next issue")}
            </button>
          </div>
          <div className="issue-detail-actions">
            <button type="button" onClick={onCopyLink}>
              {t("Copy issue link")}
            </button>
            <button className={isWatched ? "watching" : ""} type="button" onClick={onToggleWatch}>
              {isWatched ? t("Watching issue") : t("Watch issue")}
            </button>
            <button type="button" onClick={onToggleLabelEditor}>
              {showLabelEditor ? t("Hide local notes") : t("Local notes")}
            </button>
            {!isClosedTicket(ticket) ? (
              <button type="button" onClick={showEditIssue ? onCancelEdit : onBeginEdit}>
                {showEditIssue ? t("Cancel edit") : t("Edit issue")}
              </button>
            ) : null}
          </div>
          {showEditIssue && !isClosedTicket(ticket) ? (
            <form
              className="issue-edit-form"
              onSubmit={(event) => {
                event.preventDefault();
                onSubmitEdit();
              }}
            >
              <label>
                <span>{t("Title")}</span>
                <input
                  value={editDraft.title}
                  onChange={(event) => onEditDraftChange({ title: event.target.value })}
                  placeholder={t("Work order title")}
                />
              </label>
              <label>
                <span>{t("Description")}</span>
                <textarea
                  value={editDraft.description}
                  onChange={(event) => onEditDraftChange({ description: event.target.value })}
                  placeholder={t("Work order description")}
                  rows={4}
                />
              </label>
              <label>
                <span>{t("Priority")}</span>
                <input
                  value={editDraft.priority}
                  onChange={(event) => onEditDraftChange({ priority: event.target.value })}
                  placeholder={t("e.g. high, normal, low")}
                />
              </label>
              <label>
                <span>{t("Labels")}</span>
                <input
                  value={editDraft.labels}
                  onChange={(event) => onEditDraftChange({ labels: event.target.value })}
                  placeholder={t("comma-separated operator labels")}
                />
                <small>
                  Saved to the daemon. System labels (source:, policy:, context:) are preserved
                  automatically.
                </small>
              </label>
              <div className="issue-edit-actions">
                <button type="submit" disabled={busy}>
                  {t("Save changes")}
                </button>
                <button type="button" onClick={onCancelEdit} disabled={busy}>
                  {t("Cancel")}
                </button>
              </div>
            </form>
          ) : null}
          <div className="issue-title-context" aria-label={t("Issue routing context")}>
            <span>{t("Owner")} {ticket.owner_human_id ?? t("Tea local operator")}</span>
            <span>{t("Agent")} {ticket.delegated_agent_id ?? t("not delegated")}</span>
            <span>{t("Priority")} {t(ticket.priority ?? "normal")}</span>
            <span>{t("Risk")} {t(ticket.risk_level ?? "medium")}</span>
            <span className={`issue-title-watch ${isWatched ? "watched" : "unwatched"}`}>
              {isWatched ? t("Watched locally") : t("Not watched")}
            </span>
            <span>{t("Source")} {t(ticket.source ?? "desktop")}</span>
          </div>
          <div className="issue-title-labels" aria-label={t("Issue labels in header")}>
            {daemonLabels.length > 0 ? (
              daemonLabels.map((label) => <span key={label}>{label}</span>)
            ) : (
              <span>{t("No labels")}</span>
            )}
          </div>
          {localNotes.length > 0 ? (
            <div className="issue-title-notes" aria-label={t("Local notes in header")}>
              {localNotes.map((note) => (
                <span className="local-note-chip" key={note}>
                  {note}
                </span>
              ))}
            </div>
          ) : null}
          {showLabelEditor || hasLocalNotes ? (
            <LabelEditor
              draft={labelDraft}
              hasNotes={hasLocalNotes}
              notes={localNotes}
              onAddNote={onAddLabel}
              onChange={onLabelDraftChange}
              onRemoveNote={onRemoveLabel}
              onResetNotes={onResetLabels}
            />
          ) : null}
        </div>
      </header>

      <div className="issue-body-layout">
        <section className="issue-conversation">
          <div className="conversation-header">
            <div>
              <h3>{t("Conversation")}</h3>
              <p>
                {t("{c} review comments, {e} timeline events, {r} run records.")
                  .replace("{c}", String(comments.length))
                  .replace("{e}", String(events.length))
                  .replace("{r}", String(runs.length))}
              </p>
            </div>
            <span>{ticket.status}</span>
          </div>

          <div className="comment-with-avatar">
            <span className="comment-avatar">T</span>
            <section className="issue-comment issue-description">
              <header className="issue-comment-header">
                <strong>{t("Tea work-order description")}</strong>
                <span>{t("edited")} {formatTime(ticket.updated_at)}</span>
              </header>
              <div className="markdown-body">
                {ticket.description ? (
                  ticket.description.split(/\n{2,}/).map((paragraph, index) => (
                    <p key={`${ticket.id}-paragraph-${index}`}>{paragraph}</p>
                  ))
                ) : (
                  <p>{t("No description was provided.")}</p>
                )}
              </div>
            </section>
          </div>

          <FocusedIssueSection
            activeSection={activeSection}
            analysis={analysis}
            busy={busy}
            commentDraft={commentDraft}
            comments={comments}
            events={events}
            exportPreview={exportPreview}
            onComment={onComment}
            onCommentDraftChange={onCommentDraftChange}
            onDownloadExport={onDownloadExport}
            onExport={onExport}
            onRetryRun={onRetryRun}
            onSaveConfiguration={onSaveConfiguration}
            onStopRun={onStopRun}
            plan={plan}
            runs={runs}
            snapshot={snapshot}
            ticket={ticket}
          />
        </section>

        <aside className="issue-meta-sidebar">
          <section className="meta-section">
            <div className="issue-stage-card">
              <h3>{t("Current stage")}</h3>
              <div className="meta-highlight">
                <strong>{ticket.status}</strong>
                <span>
                  {isClosedTicket(ticket)
                    ? t("This work order reached a terminal state.")
                    : runs.length > 0
                      ? t("Execution evidence is already present for operator review.")
                      : comments.length > 0
                        ? t("Human review exists and the work order is waiting on the next action.")
                        : events.length > 0
                          ? t("Tea has already started planning or routing the work order.")
                          : t("The work order is waiting for initial analysis or approval.")}
                </span>
              </div>
            </div>
          </section>

          <section className="meta-section">
            <h3>{t("Assignee")}</h3>
            <p>{t("Tea daemon")}</p>
            <small>{t("Local AI operator")}</small>
          </section>

          <section className="meta-section meta-routing">
            <h3>{t("Review and routing")}</h3>
            <dl className="meta-list">
              <div>
                <dt>{t("Owner")}</dt>
                <dd>{ticket.owner_human_id ?? t("Tea local operator")}</dd>
              </div>
              <div>
                <dt>{t("Agent")}</dt>
                <dd>{ticket.delegated_agent_id ?? t("not delegated")}</dd>
              </div>
              <div>
                <dt>{t("Source")}</dt>
                <dd>{ticket.source ?? t("desktop")}</dd>
              </div>
              <div>
                <dt>{t("Policy")}</dt>
                <dd>{ticket.approval_policy ?? t("default")}</dd>
              </div>
            </dl>
          </section>

          <section className="meta-section">
            <h3>{t("Signal summary")}</h3>
            {selectedSignal ? (
              <div className="issue-signal-panel">
                <span className="issue-signal-with-reason">
                  <span className={`issue-signal-chip tone-${selectedSignal.tone}`}>{t(selectedSignal.label)}</span>
                  <span className="issue-signal-reason">{selectedSignal.reason}</span>
                </span>
                <p>{selectedSignal.description}</p>
              </div>
            ) : null}
            {selectedActionHint ? (
              <div className="issue-action-hint-panel">
                <span className={`issue-action-hint tone-${selectedActionHint.tone}`}>
                  {t(selectedActionHint.label)}
                </span>
                <p>{selectedActionHint.description}</p>
                <button
                  className="issue-action-hint-cta"
                  disabled={busy && selectedActionHint.target.kind === "action"}
                  onClick={handleSuggestedActionHint}
                  type="button"
                >
                  {t(selectedActionHint.target.label)}
                </button>
              </div>
            ) : null}
            <div className="meta-highlight">
              <strong>{activeTicket ? issueAgeLabel(activeTicket) : t("Updated -")}</strong>
              <span>
                {activeTicket ? `${issueSummary(activeTicket).slice(0, 80)}` : t("No ticket selected.")}
              </span>
            </div>
          </section>

          <section className="meta-section">
            <h3>{t("Labels")}</h3>
            <div className="label-stack">
              {daemonLabels.length > 0 ? (
                daemonLabels.map((label) => <span key={label}>{label}</span>)
              ) : (
                <span>{t("No labels")}</span>
              )}
            </div>
            {localNotes.length > 0 ? (
              <div className="label-stack label-stack-notes" aria-label={t("Local notes")}>
                {localNotes.map((note) => (
                  <span className="local-note-chip" key={`note-${note}`}>
                    {note}
                  </span>
                ))}
              </div>
            ) : null}
          </section>

          <section className="meta-section">
            <h3>{t("Execution progress")}</h3>
            <div
              aria-label={t("Milestone progress")}
              aria-valuemax={100}
              aria-valuemin={0}
              aria-valuenow={progressForTicket(ticket, comments, events, runs)}
              className="milestone-progress"
              role="progressbar"
            >
              <span style={{ width: `${progressForTicket(ticket, comments, events, runs)}%` }} />
            </div>
            <small>
              {isClosedTicket(ticket)
                ? t("Terminal issue state reached.")
                : runs.length > 0
                  ? t("Execution evidence is available.")
                  : comments.length > 0
                    ? t("Human review is captured in comments.")
                  : events.length > 0
                    ? t("Planning and review are in progress.")
                    : t("Waiting for analysis or approval.")}
            </small>
          </section>

          <section className="meta-section">
            <h3>{t("Workflow actions")}</h3>
            <div className="workflow-action-groups">
              {workflowActionGroups.map((group) => (
                <section className="workflow-action-group" key={group.key}>
                  <h4>{t(group.title)}</h4>
                  <div className="workflow-actions">
                    {group.actions
                      .filter((item) => item.action !== "reject")
                      .map((item) => (
                        <button
                          className={item.tone ? `action-${item.tone}` : ""}
                          disabled={busy}
                          key={item.action}
                          onClick={() => onAction(item.action)}
                        >
                          {t(item.label)}
                        </button>
                      ))}
                  </div>
                  {group.key === "approval" ? (
                    <>
                      <form
                        className="reject-reason-form"
                        onSubmit={(event) => {
                          event.preventDefault();
                          onReject(rejectReason);
                        }}
                      >
                        <label htmlFor="reject-reason-input">{t("Reject with reason")}</label>
                        <textarea
                          id="reject-reason-input"
                          onChange={(event) => onRejectReasonChange(event.target.value)}
                          placeholder={t("Explain why this approval is rejected.")}
                          rows={2}
                          value={rejectReason}
                        />
                        <button className="action-danger" disabled={busy} type="submit">
                          {t("Reject approval")}
                        </button>
                      </form>
                      <div className="policy-editor">
                        <label htmlFor="policy-editor-select">{t("Approval policy")}</label>
                        <select
                          disabled={busy}
                          id="policy-editor-select"
                          onChange={(event) => onApplyPolicy(event.target.value)}
                          value={ticket.approval_policy ?? ""}
                        >
                          {ticket.approval_policy ? null : (
                            <option value="">{t("Select approval policy")}</option>
                          )}
                          {approvalPolicyOptions.map((option) => (
                            <option key={option.value} value={option.value}>
                              {t(option.label)}
                            </option>
                          ))}
                        </select>
                        <small>{t("Changing the policy retightens or relaxes the run gate for this work order.")}</small>
                      </div>
                    </>
                  ) : null}
                </section>
              ))}
            </div>
          </section>

          <section className="meta-section">
            <h3>{t("Export")}</h3>
            <div className="export-actions">
              <button onClick={() => onExport("json")}>{t("Preview JSON")}</button>
              <button onClick={() => onExport("markdown")}>{t("Preview Markdown")}</button>
            </div>
            <div className="export-download-actions">
              <button disabled={busy} onClick={() => onDownloadExport("json")}>
                {t("Download JSON")}
              </button>
              <button disabled={busy} onClick={() => onDownloadExport("markdown")}>
                {t("Download Markdown")}
              </button>
            </div>
          </section>

          <section className="meta-section">
            <h3>{t("Details")}</h3>
            <dl className="meta-list">
              <div>
                <dt>{t("Status")}</dt>
                <dd>{ticket.status}</dd>
              </div>
              <div>
                <dt>{t("Priority")}</dt>
                <dd>{ticket.priority ?? "normal"}</dd>
              </div>
              <div>
                <dt>{t("Risk")}</dt>
                <dd>{ticket.risk_level ?? "medium"}</dd>
              </div>
              <div>
                <dt>{t("Comments")}</dt>
                <dd>{comments.length}</dd>
              </div>
              <div>
                <dt>{t("Runs")}</dt>
                <dd>{runs.length}</dd>
              </div>
            </dl>
          </section>

          <details className="meta-section raw-details">
            <summary>{t("Raw ticket JSON")}</summary>
            <pre>{pretty(ticket)}</pre>
          </details>

          <details className="meta-section raw-details">
            <summary>{t("Daemon status")}</summary>
            <pre>{pretty(snapshot?.status ?? snapshot?.health ?? snapshot?.error ?? null)}</pre>
          </details>
        </aside>
      </div>
    </article>
  );
}

function LabelEditor({
  draft,
  hasNotes,
  notes,
  onAddNote,
  onChange,
  onRemoveNote,
  onResetNotes,
}: {
  draft: string;
  hasNotes: boolean;
  notes: string[];
  onAddNote: () => void;
  onChange: (value: string) => void;
  onRemoveNote: (note: string) => void;
  onResetNotes: () => void;
}) {
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    onAddNote();
  };

  return (
    <section className="label-editor" aria-label={t("Local notes editor")}>
      <div className="label-editor-header">
        <strong>{t("Local notes")}</strong>
        <span>
          Private annotations kept on this machine. They never change the ticket&apos;s daemon labels
          and are not sent to the server.
        </span>
      </div>
      <div className="label-editor-list">
        {notes.length > 0 ? (
          notes.map((note) => (
            <span className="local-note-chip" key={note}>
              <span>{note}</span>
              <button
                aria-label={`Remove local note ${note}`}
                className="label-remove"
                onClick={() => onRemoveNote(note)}
                type="button"
              >
                {t("Remove note")}
              </button>
            </span>
          ))
        ) : (
          <p className="label-empty">{t("No local notes yet. Add a private note for this ticket.")}</p>
        )}
      </div>
      <form className="label-editor-form" onSubmit={submit}>
        <input
          onChange={(event) => onChange(event.target.value)}
          placeholder={t("Add a local note")}
          value={draft}
        />
        <div className="label-editor-actions">
          <button type="submit">{t("Add note")}</button>
          <button disabled={!hasNotes} onClick={onResetNotes} type="button">
            {t("Clear notes")}
          </button>
        </div>
      </form>
    </section>
  );
}

function SettingsConfigEditor({
  busy,
  config,
  fallbackReason,
  onSave,
}: {
  busy: boolean;
  config: TeaLocalConfig;
  fallbackReason: string | null;
  onSave: (config: TeaLocalConfig) => void;
}) {
  const [draft, setDraft] = useState<TeaLocalConfig>(config);

  useEffect(() => {
    setDraft(config);
    // Intentionally reset the draft only when a persisted config field we edit changes,
    // not on every `config` object identity change (which would discard in-progress edits).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    config.notifications_enabled,
    config.human_ticket_default_approval_policy,
    config.hook_ticket_default_approval_policy,
  ]);

  const dirty =
    draft.notifications_enabled !== config.notifications_enabled ||
    draft.human_ticket_default_approval_policy !== config.human_ticket_default_approval_policy ||
    draft.hook_ticket_default_approval_policy !== config.hook_ticket_default_approval_policy;

  return (
    <form
      className="settings-config-editor"
      onSubmit={(event) => {
        event.preventDefault();
        onSave(draft);
      }}
    >
      <div className="settings-config-intro">
        <strong>{t("Tea local settings")}</strong>
        <span>{t("Tea owns these settings until Loom claims Tea configuration.")}</span>
      </div>
      {fallbackReason ? (
        <p className="settings-fallback-reason">Fallback: {fallbackReason}</p>
      ) : null}
      <label className="settings-config-toggle">
        <input
          checked={draft.notifications_enabled}
          disabled={busy}
          onChange={(event) =>
            setDraft((current) => ({ ...current, notifications_enabled: event.target.checked }))
          }
          type="checkbox"
        />
        <span>{t("Enable notifications")}</span>
      </label>
      <label className="settings-config-field">
        <span>{t("Human ticket default approval policy")}</span>
        <select
          disabled={busy}
          onChange={(event) =>
            setDraft((current) => ({
              ...current,
              human_ticket_default_approval_policy: event.target.value,
            }))
          }
          value={draft.human_ticket_default_approval_policy}
        >
          {approvalPolicyOptions.map((option) => (
            <option key={option.value} value={option.value}>
              {t(option.label)}
            </option>
          ))}
        </select>
      </label>
      <label className="settings-config-field">
        <span>{t("Hook ticket default approval policy")}</span>
        <select
          disabled={busy}
          onChange={(event) =>
            setDraft((current) => ({
              ...current,
              hook_ticket_default_approval_policy: event.target.value,
            }))
          }
          value={draft.hook_ticket_default_approval_policy}
        >
          {approvalPolicyOptions.map((option) => (
            <option key={option.value} value={option.value}>
              {t(option.label)}
            </option>
          ))}
        </select>
      </label>
      <div className="settings-config-actions">
        <button className="action-primary" disabled={busy || !dirty} type="submit">
          {t("Save Tea settings")}
        </button>
        <button
          disabled={busy || !dirty}
          onClick={() => setDraft(config)}
          type="button"
        >
          {t("Reset changes")}
        </button>
      </div>
    </form>
  );
}

function AnalysisPlanChipList({ items }: { items: string[] }) {
  if (items.length === 0) {
    return <span className="analysis-plan-empty-value">{t("None recorded")}</span>;
  }
  return (
    <ul className="analysis-plan-chip-list">
      {items.map((item, index) => (
        <li key={`${item}-${index}`}>{item}</li>
      ))}
    </ul>
  );
}

const AnalysisPlanView = memo(function AnalysisPlanView({
  analysis,
  plan,
}: {
  analysis: TeaAnalysis | null;
  plan: TeaPlan | null;
}) {
  const confidencePercent =
    analysis && typeof analysis.confidence === "number"
      ? Math.round(Math.max(0, Math.min(1, analysis.confidence)) * 100)
      : null;

  return (
    <div className="analysis-plan-view">
      {analysis ? (
        <section className="analysis-card" aria-label={t("Ticket analysis")}>
          <header className="analysis-card-header">
            <h4>{t("Analysis")}</h4>
            <div className="analysis-card-badges">
              <span className={`issue-badge tone-${badgeToneForRisk(analysis.risk_assessment)}`}>
                {t("risk:")} {analysis.risk_assessment ? t(analysis.risk_assessment) : t("unknown")}
              </span>
              {confidencePercent !== null ? (
                <span className="issue-badge tone-default">{t("confidence:")} {confidencePercent}%</span>
              ) : null}
              {analysis.recommended_policy ? (
                <span className="issue-badge tone-default">{t("policy:")} {analysis.recommended_policy}</span>
              ) : null}
            </div>
          </header>
          {analysis.intent ? (
            <p className="analysis-intent">
              <strong>{t("Intent:")}</strong> {analysis.intent}
            </p>
          ) : null}
          {analysis.recommended_workflow ? (
            <p className="analysis-workflow">
              <strong>{t("Recommended workflow:")}</strong> <code>{analysis.recommended_workflow}</code>
            </p>
          ) : null}
          <dl className="analysis-plan-facts">
            <div>
              <dt>{t("Target components")}</dt>
              <dd>
                <AnalysisPlanChipList items={analysis.target_components ?? []} />
              </dd>
            </div>
            <div>
              <dt>{t("Target paths")}</dt>
              <dd>
                <AnalysisPlanChipList items={analysis.target_paths ?? []} />
              </dd>
            </div>
            <div>
              <dt>{t("Constraints")}</dt>
              <dd>
                <AnalysisPlanChipList items={analysis.constraints ?? []} />
              </dd>
            </div>
            <div>
              <dt>{t("Acceptance criteria")}</dt>
              <dd>
                <AnalysisPlanChipList items={analysis.acceptance_criteria ?? []} />
              </dd>
            </div>
            <div>
              <dt>{t("Missing context")}</dt>
              <dd>
                <AnalysisPlanChipList items={analysis.missing_context ?? []} />
              </dd>
            </div>
          </dl>
        </section>
      ) : null}
      {plan ? (
        <section className="plan-card" aria-label={t("Ticket plan")}>
          <header className="plan-card-header">
            <h4>{t("Plan")}</h4>
            <span
              className={`issue-badge tone-${plan.requires_approval_before_execute ? "warn" : "default"}`}
            >
              {plan.requires_approval_before_execute ? t("approval required") : t("no gate")}
            </span>
          </header>
          {plan.summary ? <p className="plan-summary">{plan.summary}</p> : null}
          {plan.steps && plan.steps.length > 0 ? (
            <ol className="plan-step-list">
              {plan.steps.map((step, index) => (
                <li className="plan-step" key={step.id || `step-${index}`}>
                  <strong>{step.title || `Step ${index + 1}`}</strong>
                  {step.description ? <p>{step.description}</p> : null}
                </li>
              ))}
            </ol>
          ) : (
            <p className="analysis-plan-empty-value">{t("No plan steps recorded.")}</p>
          )}
          <dl className="analysis-plan-facts">
            <div>
              <dt>{t("Required tools")}</dt>
              <dd>
                <AnalysisPlanChipList items={plan.required_tools ?? []} />
              </dd>
            </div>
            <div>
              <dt>{t("Expected artifacts")}</dt>
              <dd>
                <AnalysisPlanChipList items={plan.expected_artifacts ?? []} />
              </dd>
            </div>
            <div>
              <dt>{t("Validation strategy")}</dt>
              <dd>
                <AnalysisPlanChipList items={plan.validation_strategy ?? []} />
              </dd>
            </div>
            <div>
              <dt>{t("Rollback strategy")}</dt>
              <dd>
                <AnalysisPlanChipList items={plan.rollback_strategy ?? []} />
              </dd>
            </div>
          </dl>
        </section>
      ) : null}
    </div>
  );
});

function FocusedIssueSection({
  activeSection,
  analysis,
  busy,
  commentDraft,
  comments,
  events,
  exportPreview,
  onComment,
  onCommentDraftChange,
  onDownloadExport,
  onExport,
  onRetryRun,
  onSaveConfiguration,
  onStopRun,
  plan,
  runs,
  snapshot,
  ticket,
}: {
  activeSection: RepoSection;
  analysis: TeaAnalysis | null;
  busy: boolean;
  commentDraft: string;
  comments: TeaComment[];
  events: TeaEvent[];
  exportPreview: string;
  onComment: (body: string) => void;
  onCommentDraftChange: (body: string) => void;
  onDownloadExport: (format: "json" | "markdown") => void;
  onExport: (format: "json" | "markdown") => void;
  onRetryRun: (runId: string) => void;
  onSaveConfiguration: (config: TeaLocalConfig) => void;
  onStopRun: (runId: string) => void;
  plan: TeaPlan | null;
  runs: TeaRun[];
  snapshot: TeaSnapshot | null;
  ticket: TeaTicket;
}) {
  const editor = (
    <CommentEditor
      busy={busy}
      disabled={isClosedTicket(ticket)}
      onChange={onCommentDraftChange}
      onSubmit={onComment}
      value={commentDraft}
    />
  );

  if (activeSection === "comments") {
    return (
      <section className="section-focus-card" aria-label={t("Focused comments section")}>
        <header>
          <div>
            <h3>{t("Comments and daemon events")}</h3>
            <p>{t("Review the durable human comments and Tea daemon events for this work order.")}</p>
          </div>
          <span>{comments.length} {t("comments")}</span>
        </header>
        <p className="section-focus-hint">
          {t("Comments are durable review records; daemon events explain how Tea moved the issue through analysis, approval, and execution.")}
        </p>
        {comments.length === 0 && events.length === 0 ? (
          <div className="section-focus-empty">
            <strong>{t("No conversation yet.")}</strong>
            <span>{t("Add a review comment or run an action to populate the timeline.")}</span>
          </div>
        ) : (
          <ConversationStream comments={comments} events={events} />
        )}
        {editor}
      </section>
    );
  }

  if (activeSection === "plan") {
    return (
      <section className="section-focus-card" aria-label={t("Focused analysis and plan section")}>
        <header>
          <div>
            <h3>{t("AI analysis and plan")}</h3>
            <p>{t("Structured decomposition produced by the Tea BrainProvider or Loom for this work order.")}</p>
          </div>
          <span>{plan ? t("Plan ready") : analysis ? t("Analyzed") : t("Not analyzed")}</span>
        </header>
        <p className="section-focus-hint">
          {t("Run Analyze or Decompose from the workflow actions to generate these records. They persist in Tea and are included in JSON/Markdown exports.")}
        </p>
        {!analysis && !plan ? (
          <div className="section-focus-empty">
            <strong>{t("No analysis yet.")}</strong>
            <span>{t("Use the Review actions to analyze or decompose this work order into a plan.")}</span>
          </div>
        ) : (
          <AnalysisPlanView analysis={analysis} plan={plan} />
        )}
      </section>
    );
  }

  if (activeSection === "runs") {
    return (
      <section className="section-focus-card" aria-label={t("Focused runs section")}>
        <header>
          <div>
            <h3>{t("Run records")}</h3>
            <p>{t("Inspect execution attempts, statuses, and evidence returned from Loom or fallback runners.")}</p>
          </div>
          <span>{runs.length} {t("runs")}</span>
        </header>
        <p className="section-focus-hint">
          {t("Run cards show execution attempts and evidence. Stop/retry actions stay tied to the selected work order.")}
        </p>
        {runs.length === 0 ? (
          <div className="section-focus-empty">
            <strong>{t("No runs yet.")}</strong>
            <span>{t("Approve and launch an AI task to see execution attempts here.")}</span>
          </div>
        ) : (
          <Runs
            busy={busy}
            disabled={isClosedTicket(ticket)}
            onRetryRun={onRetryRun}
            onStopRun={onStopRun}
            runs={runs}
          />
        )}
      </section>
    );
  }

  if (activeSection === "exports") {
    return (
      <section className="section-focus-card" aria-label={t("Focused export section")}>
        <header>
          <div>
            <h3>{t("Export this work order")}</h3>
            <p>{t("Generate portable JSON or Markdown evidence, including comments, events, runs, and ticket state.")}</p>
          </div>
          <span>{issueNumber(ticket)}</span>
        </header>
        <p className="section-focus-hint">
          {t("Use JSON for machine review and Markdown for human handoff, incident notes, or release evidence.")}
        </p>
        <div className="focused-actions">
          <button onClick={() => onExport("json")}>{t("Preview JSON export")}</button>
          <button onClick={() => onExport("markdown")}>{t("Preview Markdown export")}</button>
        </div>
        <div className="focused-actions export-download-actions">
          <button disabled={busy} onClick={() => onDownloadExport("json")}>
            {t("Download JSON export")}
          </button>
          <button disabled={busy} onClick={() => onDownloadExport("markdown")}>
            {t("Download Markdown export")}
          </button>
        </div>
        <div className="section-focus-empty">
          <strong>{t("Quick actions")}</strong>
          <span>{t("Use exports to hand off this work order to logs, reviews, or external systems.")}</span>
        </div>
        <ExportPreview exportPreview={exportPreview} />
      </section>
    );
  }

  if (activeSection === "settings") {
    const configurationSource = configurationSourceOf(snapshot);
    const loomManaged = configurationSource === "loom-managed";
    const details = configurationDetailsOf(snapshot);
    const localConfig = localConfigOf(snapshot);
    const loomPanelUrl =
      typeof details?.loom_panel_url === "string" ? details.loom_panel_url : null;
    const fallbackReason = typeof details?.reason === "string" ? details.reason : null;

    return (
      <section className="section-focus-card" aria-label={t("Focused settings section")}>
        <header>
          <div>
            <h3>{t("Connection and ownership")}</h3>
            <p>{t("Verify which local Tea daemon and configuration surface currently own this work order view.")}</p>
          </div>
          <span>{statusText(snapshot)}</span>
        </header>
        <p className="section-focus-hint">
          {t("This panel summarizes the local daemon connection and where Tea-specific configuration should be owned.")}
        </p>
        <div className="section-focus-empty">
          <strong>{t("Current connection")}</strong>
          <span>{snapshot?.status ? t("Tea daemon connected.") : t("Tea daemon offline or health-only.")}</span>
        </div>
        <dl className="focused-settings-list">
          <div>
            <dt>{t("Configuration source")}</dt>
            <dd className={`config-source-badge ${configurationSource}`}>{configurationSource}</dd>
          </div>
          <div>
            <dt>{t("Daemon mode")}</dt>
            <dd>{snapshot?.status ? t("HTTP API online") : snapshot?.health ? t("Health endpoint only") : t("Offline")}</dd>
          </div>
          <div>
            <dt>{t("Store backend")}</dt>
            <dd>{String(snapshot?.status?.store_backend ?? snapshot?.status?.store ?? "unknown")}</dd>
          </div>
        </dl>
        {loomManaged ? (
          <div className="loom-managed-settings">
            <strong>{t("Loom manages Tea configuration.")}</strong>
            <p>
              Tea-local settings are read-only while Loom owns Tea configuration. Change these settings
              from Loom instead.
            </p>
            {loomPanelUrl ? (
              <a className="loom-settings-link" href={loomPanelUrl} rel="noreferrer" target="_blank">
                {t("Open Loom Tea settings")}
              </a>
            ) : (
              <span className="loom-settings-missing">
                {t("Loom did not provide a Tea configuration panel URL.")}
              </span>
            )}
          </div>
        ) : (
          <SettingsConfigEditor
            busy={busy}
            config={localConfig}
            fallbackReason={configurationSource === "fallback" ? fallbackReason : null}
            onSave={onSaveConfiguration}
          />
        )}
        <details className="raw-details">
          <summary>{t("Configuration JSON")}</summary>
          <pre>{pretty(snapshot?.configuration ?? null)}</pre>
        </details>
      </section>
    );
  }

  return (
    <>
      <ConversationStream comments={comments} events={events} />
      {editor}
      <Runs
        busy={busy}
        disabled={isClosedTicket(ticket)}
        onRetryRun={onRetryRun}
        onStopRun={onStopRun}
        runs={runs}
      />
      <ExportPreview exportPreview={exportPreview} />
    </>
  );
}

const ExportPreview = memo(function ExportPreview({ exportPreview }: { exportPreview: string }) {
  if (!exportPreview) return null;

  return (
    <section className="issue-comment">
      <header className="issue-comment-header">
        <strong>{t("Export preview")}</strong>
        <span>{t("JSON / Markdown")}</span>
      </header>
      <pre className="export-preview">{exportPreview}</pre>
    </section>
  );
});

const ConversationStream = memo(function ConversationStream({
  comments,
  events,
}: {
  comments: TeaComment[];
  events: TeaEvent[];
}) {
  const [copiedEntryId, setCopiedEntryId] = useState<string | null>(null);
  const [conversationFilter, setConversationFilter] = useState<ConversationFilter>("all");
  const [activeEntryHash, setActiveEntryHash] = useState(() => {
    if (typeof window === "undefined") return "";
    return decodeURIComponent(window.location.hash.replace(/^#/, ""));
  });
  // Derive the timeline via useMemo so the map/sort/group work only re-runs when the
  // underlying comments/events (or the active filter) change — not on every parent
  // poll, copy-button click, or hashchange re-render.
  const entries = useMemo(() => buildConversationEntries(comments, events), [comments, events]);
  const filteredConversationEntries = useMemo(
    () =>
      entries.filter((entry) => {
        if (conversationFilter === "comments") return entry.kind === "comment";
        if (conversationFilter === "events") return entry.kind === "event";
        return true;
      }),
    [entries, conversationFilter],
  );
  const filteredConversationTimelineItems = useMemo(
    () => buildConversationTimelineItems(filteredConversationEntries),
    [filteredConversationEntries],
  );

  useEffect(() => {
    if (!copiedEntryId) return undefined;
    const timer = window.setTimeout(() => setCopiedEntryId(null), 1800);
    return () => window.clearTimeout(timer);
  }, [copiedEntryId]);

  useEffect(() => {
    if (typeof window === "undefined") return undefined;
    const syncHash = () => setActiveEntryHash(decodeURIComponent(window.location.hash.replace(/^#/, "")));
    syncHash();
    window.addEventListener("hashchange", syncHash);
    return () => window.removeEventListener("hashchange", syncHash);
  }, []);

  return (
    <section className="conversation-stream" aria-label={t("Conversation timeline")}>
      <div className="conversation-stream-header">
        <div>
          <h3>{t("Conversation timeline")}</h3>
          <p>{t("Human review comments and daemon state changes in one issue thread.")}</p>
        </div>
        <div className="conversation-stats" aria-label={t("Timeline activity summary")}>
          <span>{comments.length} {t("comments")}</span>
          <span>{events.length} {t("events")}</span>
          <span>{entries.length} {t("entries")}</span>
        </div>
      </div>
      <div className="conversation-filter-tabs" role="tablist" aria-label={t("Timeline filters")}>
        <button
          aria-selected={conversationFilter === "all"}
          className={conversationFilter === "all" ? "active" : ""}
          onClick={() => setConversationFilter("all")}
          role="tab"
          type="button"
        >
          {t("All")} <span>{entries.length}</span>
        </button>
        <button
          aria-selected={conversationFilter === "comments"}
          className={conversationFilter === "comments" ? "active" : ""}
          onClick={() => setConversationFilter("comments")}
          role="tab"
          type="button"
        >
          {t("Comments")} <span>{comments.length}</span>
        </button>
        <button
          aria-selected={conversationFilter === "events"}
          className={conversationFilter === "events" ? "active" : ""}
          onClick={() => setConversationFilter("events")}
          role="tab"
          type="button"
        >
          {t("Events")} <span>{events.length}</span>
        </button>
      </div>
      {entries.length === 0 ? <p className="empty">{t("No comments or events yet.")}</p> : null}
      {entries.length > 0 && filteredConversationTimelineItems.length === 0 ? (
        <p className="empty">{t("No timeline entries match this filter.")}</p>
      ) : null}
      <div className="timeline">
        <div className="comment-list" role="list">
          {filteredConversationTimelineItems.map((item) => {
            if (item.kind === "system-event-group") {
              const firstEntry = item.entries[0];
              const lastEntry = item.entries[item.entries.length - 1];
              const isLinkedEntry = item.entries.some((entry) => activeEntryHash === entry.id);
              return (
                <article
                  aria-current={isLinkedEntry ? "location" : undefined}
                  className={`conversation-entry event timeline-event-group ${isLinkedEntry ? "linked" : ""}`}
                  data-entry-group="system"
                  data-entry-kind="event"
                  id={item.id}
                  key={item.id}
                  role="listitem"
                >
                  <span className="timeline-marker" />
                  <div className="comment-with-avatar">
                    <span className="comment-avatar small">{t("SY")}</span>
                    <div className="issue-comment">
                      <header className="issue-comment-header">
                        <span className="conversation-entry-title">
                          <strong>{t("System event batch")}</strong>
                          <span className="conversation-entry-kind event">{t("Daemon events")}</span>
                          <span className="conversation-entry-group system">{t("System event")}</span>
                        </span>
                        <span className="conversation-entry-actions">
                          <span className="conversation-entry-meta">
                            {item.entries.length} {t("daemon events")} · {formatTime(firstEntry?.createdAt)}
                            {lastEntry && lastEntry.id !== firstEntry?.id ? ` ${t("to")} ${formatTime(lastEntry.createdAt)}` : ""}
                          </span>
                          <a className="timeline-entry-anchor" href={`#${item.id}`}>
                            {timelineEntryReference(item.id)}
                          </a>
                          <button
                            className={`timeline-entry-link ${copiedEntryId === item.id ? "copied" : ""}`}
                            onClick={() => {
                              setCopiedEntryId(item.id);
                              void copyTimelineEntryLink(item.id);
                            }}
                            type="button"
                          >
                            {copiedEntryId === item.id ? t("Copied entry link") : t("Copy entry link")}
                          </button>
                        </span>
                      </header>
                      <p className="timeline-event-group-summary">
                        {t("Consecutive low-level daemon events folded to keep the work-order discussion readable.")}
                      </p>
                      <ul className="timeline-event-group-list">
                        {item.entries.map((entry) => {
                          const isLinkedGroupMember = activeEntryHash === entry.id;
                          return (
                            <li className={isLinkedGroupMember ? "linked" : ""} id={entry.id} key={entry.id}>
                              <div className="timeline-event-group-member">
                                <span>
                                  <strong>{entry.title}</strong>
                                  <small>{formatTime(entry.createdAt)}</small>
                                </span>
                                <p>{conversationEntrySummary(entry, "system")}</p>
                              </div>
                              {entry.payload ? (
                                <details className="timeline-payload-collapsed">
                                  <summary className="timeline-payload-toggle">
                                    <span>{t("Show event payload")}</span>
                                    <span className="timeline-payload-summary">{payloadSummary(entry.payload)}</span>
                                  </summary>
                                  <pre className="event-payload">{pretty(entry.payload)}</pre>
                                </details>
                              ) : null}
                            </li>
                          );
                        })}
                      </ul>
                    </div>
                  </div>
                </article>
              );
            }
            const { entry } = item;
            const isLinkedEntry = activeEntryHash === entry.id;
            const entryGroup = conversationEntryGroup(entry);
            return (
              <article
                aria-current={isLinkedEntry ? "location" : undefined}
                className={`conversation-entry ${entry.kind} ${isLinkedEntry ? "linked" : ""}`}
                data-entry-group={entryGroup}
                data-entry-kind={entry.kind}
                id={entry.id}
                key={entry.id}
                role="listitem"
              >
                <span className="timeline-marker" />
                <div className="comment-with-avatar">
                  <span className={`comment-avatar ${entry.kind === "event" ? "small" : ""}`}>
                    {entry.avatar}
                  </span>
                  <div className="issue-comment">
                    <header className="issue-comment-header">
                      <span className="conversation-entry-title">
                        <strong>{entry.title}</strong>
                        <span className={`conversation-entry-kind ${entry.kind}`}>
                          {entry.kind === "comment" ? t("Comment") : t("Daemon event")}
                        </span>
                        <span className={`conversation-entry-group ${entryGroup}`}>
                          {conversationEntryGroupLabel(entryGroup)}
                        </span>
                      </span>
                      <span className="conversation-entry-actions">
                        <span className="conversation-entry-meta">
                          {entry.kind === "comment" ? t("review comment") : t("daemon event")} -{" "}
                          {formatTime(entry.createdAt)}
                        </span>
                        <a className="timeline-entry-anchor" href={`#${entry.id}`}>
                          {timelineEntryReference(entry.id)}
                        </a>
                        <button
                          className={`timeline-entry-link ${copiedEntryId === entry.id ? "copied" : ""}`}
                          onClick={() => {
                            setCopiedEntryId(entry.id);
                            void copyTimelineEntryLink(entry.id);
                          }}
                          type="button"
                        >
                          {copiedEntryId === entry.id ? t("Copied entry link") : t("Copy entry link")}
                        </button>
                      </span>
                    </header>
                    <div className="markdown-body">
                      {(entry.body ?? "").split(/\n{2,}/).map((paragraph, index) => (
                        <p key={`${entry.id}-paragraph-${index}`}>{paragraph || t("Event payload")}</p>
                      ))}
                    </div>
                    <p className="conversation-entry-summary">{conversationEntrySummary(entry, entryGroup)}</p>
                    {entry.payload ? (
                      <details className="timeline-payload-collapsed">
                        <summary className="timeline-payload-toggle">
                          <span>{t("Show event payload")}</span>
                          <span className="timeline-payload-summary">{payloadSummary(entry.payload)}</span>
                        </summary>
                        <pre className="event-payload">{pretty(entry.payload)}</pre>
                      </details>
                    ) : null}
                  </div>
                </div>
              </article>
            );
          })}
        </div>
      </div>
    </section>
  );
});

function CommentEditor({
  busy,
  disabled,
  onChange,
  onSubmit,
  value,
}: {
  busy: boolean;
  disabled: boolean;
  onChange: (body: string) => void;
  onSubmit: (body: string) => void;
  value: string;
}) {
  const [mode, setMode] = useState<"write" | "preview">("write");

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    onSubmit(value);
  };

  return (
    <form className="comment-editor" onSubmit={submit}>
      <div className="comment-with-avatar">
        <span className="comment-avatar">{t("Me")}</span>
        <section className="issue-comment">
          <header className="issue-comment-header">
            <strong>{t("Leave a review comment")}</strong>
            <span>{disabled ? t("terminal ticket") : t("markdown supported")}</span>
          </header>
          <div className="comment-editor-tabs" role="tablist" aria-label={t("Comment editor mode")}>
            <button
              aria-selected={mode === "write"}
              className={mode === "write" ? "active" : ""}
              onClick={() => setMode("write")}
              role="tab"
              type="button"
            >
              {t("Write")}
            </button>
            <button
              aria-selected={mode === "preview"}
              className={mode === "preview" ? "active" : ""}
              onClick={() => setMode("preview")}
              role="tab"
              type="button"
            >
              {t("Preview comment")}
            </button>
          </div>
          {mode === "write" ? (
            <textarea
              disabled={disabled || busy}
              onChange={(event) => onChange(event.target.value)}
              placeholder={
                disabled
                  ? "Closed and cancelled tickets are read-only."
                  : "Write context, decisions, review notes, or acceptance evidence."
              }
              value={value}
            />
          ) : (
            <div className="comment-preview">
              <div className="markdown-body">
                {value.trim() ? (
                  value.split(/\n{2,}/).map((paragraph, index) => (
                    <p key={`comment-preview-${index}`}>{paragraph}</p>
                  ))
                ) : (
                  <p>{t("Nothing to preview yet.")}</p>
                )}
              </div>
            </div>
          )}
          <footer className="comment-editor-footer">
            <span>{t("Comments are durable and included in JSON/Markdown exports.")}</span>
            <button disabled={disabled || busy || !value.trim()} type="submit">
              {t("Comment")}
            </button>
          </footer>
        </section>
      </div>
    </form>
  );
}

function Runs({
  busy = false,
  disabled = false,
  onRetryRun,
  onStopRun,
  runs,
}: {
  busy?: boolean;
  disabled?: boolean;
  onRetryRun?: (runId: string) => void;
  onStopRun?: (runId: string) => void;
  runs: TeaRun[];
}) {
  const actionsEnabled = Boolean(onStopRun || onRetryRun);
  return (
    <section className="runs-panel" aria-label={t("Run history")}>
      <h3>{t("Run history")}</h3>
      {runs.length === 0 ? <p className="empty">{t("No runs yet.")}</p> : null}
      {runs.map((run) => (
        <article className="run-card" key={run.id}>
          <div>
            <strong>{run.id}</strong>
            <span>{run.status ?? "unknown"}</span>
          </div>
          <small>
            {formatTime(run.created_at)} - {formatTime(run.updated_at)}
          </small>
          {run.evidence ? <pre>{pretty(run.evidence)}</pre> : null}
          {actionsEnabled ? (
            <div className="run-actions">
              {onStopRun ? (
                <button
                  className="run-action-stop action-danger"
                  disabled={busy || disabled}
                  onClick={() => onStopRun(run.id)}
                  type="button"
                >
                  {t("Stop run")}
                </button>
              ) : null}
              {onRetryRun ? (
                <button
                  className="run-action-retry"
                  disabled={busy || disabled}
                  onClick={() => onRetryRun(run.id)}
                  type="button"
                >
                  {t("Retry run")}
                </button>
              ) : null}
            </div>
          ) : null}
          {disabled && actionsEnabled ? (
            <small className="run-actions-note">
              {t("Run actions are disabled for terminal work orders.")}
            </small>
          ) : null}
        </article>
      ))}
    </section>
  );
}
