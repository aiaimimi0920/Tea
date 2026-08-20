import { t } from "./i18n";
import type { LocalNotes } from "./localMetadata";
import type { IssueTouchSummary } from "./issueTypes";
import {
  TeaComment,
  TeaEvent,
  TeaLocalConfig,
  TeaRun,
  TeaSnapshot,
  TeaTicket,
  ticketAction,
} from "./teaClient";

type WorkflowActionGroupKey = "review" | "approval" | "execution" | "resolution";

export const actionLabels: Array<{
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

export const workflowActionGroups: Array<{
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

export const approvalPolicyOptions: Array<{ value: string; label: string }> = [
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

export const createPriorityOptions: Array<{ value: string; label: string }> = [
  { value: "", label: "Default priority (normal)" },
  { value: "low", label: "Low" },
  { value: "normal", label: "Normal" },
  { value: "high", label: "High" },
  { value: "urgent", label: "Urgent" },
];

const closedStatuses = new Set(["accepted", "cancelled", "canceled", "closed", "completed", "done"]);

// System-derived labels the daemon owns and always preserves; operators cannot
// set or remove these through a ticket edit, so the edit form hides them.
const systemLabelPrefixes = ["source:", "policy:", "context:"];
export const isSystemLabel = (label: string) =>
  systemLabelPrefixes.some((prefix) => label.startsWith(prefix));

export const pretty = (value: unknown) => JSON.stringify(value ?? null, null, 2);

export const statusText = (snapshot: TeaSnapshot | null) => {
  if (!snapshot) return t("Loading");
  if (snapshot.status) return t("Online");
  if (snapshot.health) return t("Health-only");
  return t("Offline");
};

export const configurationSourceOf = (snapshot: TeaSnapshot | null): string => {
  const source = snapshot?.configuration?.configuration_source;
  return typeof source === "string" ? source : "local";
};

export const executionProviderOf = (snapshot: TeaSnapshot | null): string => {
  const provider = snapshot?.status?.execution_provider;
  return typeof provider === "string" ? provider : "unknown";
};

export const storeBackendOf = (snapshot: TeaSnapshot | null): string => {
  const store = snapshot?.status?.store;
  if (store && typeof store === "object" && "backend" in store) {
    const backend = (store as Record<string, unknown>).backend;
    if (typeof backend === "string") return backend;
  }
  const legacyBackend = snapshot?.status?.store_backend;
  return typeof legacyBackend === "string" ? legacyBackend : "unknown";
};

export const configurationDetailsOf = (
  snapshot: TeaSnapshot | null,
): Record<string, unknown> | null => {
  const details = snapshot?.configuration?.configuration;
  return details && typeof details === "object" ? (details as Record<string, unknown>) : null;
};

export const localConfigOf = (snapshot: TeaSnapshot | null): TeaLocalConfig => {
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

export const formatTime = (value: string | undefined) => {
  if (!value) return "-";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
};

export const exportTimestamp = () => {
  const now = new Date();
  const pad = (value: number) => String(value).padStart(2, "0");
  return (
    `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}` +
    `-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`
  );
};

export const isClosedTicket = (ticket: TeaTicket) => closedStatuses.has(ticket.status.toLowerCase());

export const issueStateLabel = (ticket: TeaTicket) => (isClosedTicket(ticket) ? "Closed" : "Open");

export const issueNumber = (ticket: TeaTicket) => {
  const compact = ticket.id.replace(/[^a-zA-Z0-9]/g, "").slice(-6);
  return compact ? `#${compact}` : ticket.id;
};

export const normalizeLabels = (labels: string[]) =>
  Array.from(new Set(labels.map((label) => label.trim()).filter(Boolean)));

export const operatorLabelsForTicket = (ticket: TeaTicket) =>
  (ticket.labels ?? []).filter((label) => Boolean(label) && !isSystemLabel(label));

const baseLabelsForTicket = (ticket: TeaTicket) => {
  const daemonLabels = ticket.labels?.filter(Boolean) ?? [];
  if (daemonLabels.length > 0) return daemonLabels;
  return [ticket.status || "unknown", ticket.source || "desktop", ticket.approval_policy || "default-policy"];
};

// Authoritative labels owned by the daemon. Always shown as-is in the header and
// issue rows; local notes never mask or replace these.
export const daemonLabelsForTicket = (ticket: TeaTicket) => baseLabelsForTicket(ticket);

// Local-only notes overlay. These are additive annotations kept in localStorage,
// never sent to the daemon, and displayed in a clearly separate surface.
export const localNotesForTicket = (ticket: TeaTicket, localNotes?: LocalNotes) =>
  (localNotes?.[ticket.id] ?? []).filter(Boolean);

// Union of daemon labels and local notes, used for filtering and search so both
// authoritative labels and local annotations can match.
export const filterableLabelsForTicket = (ticket: TeaTicket, localNotes?: LocalNotes) =>
  Array.from(
    new Set([...daemonLabelsForTicket(ticket), ...localNotesForTicket(ticket, localNotes)]),
  );

export const issueSummary = (ticket: TeaTicket) => {
  const description = ticket.description?.trim();
  if (!description) return "No description available.";
  return description.split(/\n+/).slice(0, 2).join(" ").trim();
};

export const issueAgeLabel = (ticket: TeaTicket) => {
  const source = ticket.updated_at ?? ticket.created_at;
  if (!source) return `${t("Updated")} -`;
  return `${t("Updated")} ${formatTime(source)}`;
};

export const buildTicketLink = (ticket: TeaTicket, serverUrl: string) =>
  `${serverUrl.replace(/\/+$/, "")}/v1/tickets/${encodeURIComponent(ticket.id)}`;

export const timelineEntryReference = (entryId: string) => {
  const compact = entryId.replace(/[^a-zA-Z0-9]/g, "").slice(-7);
  return compact ? `#${compact}` : "#entry";
};

export const latestTouchLabel = (touch: IssueTouchSummary | undefined) => {
  if (!touch) return t("No recent human or daemon touches.");
  return `${t(touch.label)} · ${touch.actor} · ${formatTime(touch.createdAt)}`;
};

export const progressForTicket = (
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
