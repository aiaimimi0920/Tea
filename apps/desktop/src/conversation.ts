import { t } from "./i18n";
import type { TeaActorRef, TeaComment, TeaEvent } from "./teaClient";

export type ConversationEntry = {
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

export type ConversationTimelineItem =
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

export type ConversationFilter = "all" | "comments" | "events";
export type ConversationEntryGroup = "human" | "ai" | "system";

export const payloadSummary = (value: unknown) => {
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

export const actorLabel = (actor: unknown) => {
  if (!actor) return "unknown";
  if (typeof actor === "string") return actor;
  if (typeof actor === "object" && "kind" in actor) {
    const ref = actor as TeaActorRef;
    return ref.id ? `${ref.kind ?? "actor"}:${ref.id}` : (ref.kind ?? "actor");
  }
  return "actor";
};

export const timestampValue = (value: string | undefined) => {
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

export const eventKindLabel = (kind: string | undefined): string => {
  if (!kind) return t("Event");
  const known = eventKindLabels[kind];
  if (known) return t(known);
  return kind
    .split(/[_\s]+/)
    .filter(Boolean)
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(" ");
};

export const conversationEntryGroup = (entry: ConversationEntry): ConversationEntryGroup => {
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

export const buildConversationTimelineItems = (entries: ConversationEntry[]): ConversationTimelineItem[] => {
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

export const conversationEntryGroupLabel = (group: ConversationEntryGroup) => {
  if (group === "human") return t("Review comment");
  if (group === "ai") return t("AI action");
  return t("System event");
};

export const conversationEntrySummary = (entry: ConversationEntry, group: ConversationEntryGroup) => {
  if (entry.kind === "comment") {
    return t("Human review note attached to this work order.");
  }
  const payload = entry.payload ? ` ${payloadSummary(entry.payload)}.` : "";
  return `${t("{group} recorded by {actor}.").replace("{group}", conversationEntryGroupLabel(group)).replace("{actor}", entry.actor)}${payload}`;
};

export const buildConversationEntries = (
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
