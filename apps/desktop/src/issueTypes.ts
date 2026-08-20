import type { ConversationEntryGroup } from "./conversation";

export type RepoSection = "issues" | "plan" | "runs" | "comments" | "exports" | "settings";

export type IssueFilter = "open" | "closed" | "all";
export type IssueSort = "updated" | "created" | "activity" | "touch";
export type IssueListDensity = "compact" | "comfortable";

export type TicketDraft = {
  title: string;
  description: string;
  approvalPolicy: string;
  priority: string;
  labels: string;
};

export type TicketEditDraft = {
  title: string;
  description: string;
  priority: string;
  labels: string;
};

export type IssueQueueNavigation = {
  current: number;
  firstId: string | null;
  isOutsideQueue: boolean;
  lastId: string | null;
  nextId: string | null;
  previousId: string | null;
  total: number;
};

export type IssueMetrics = {
  comments: number;
  latestTouch?: IssueTouchSummary;
  runs: number;
};

export type IssueTouchSummary = {
  actor: string;
  createdAt?: string;
  group: ConversationEntryGroup;
  label: string;
};
