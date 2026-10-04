import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  CreateTicketInput,
  TeaAnalysis,
  TeaClientOptions,
  TeaComment,
  TeaEvent,
  TeaIssueMetric,
  TeaLocalConfig,
  TeaPlan,
  TeaRun,
  TeaSnapshot,
  TeaTicket,
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
import { autoRefreshDelayMs, nextAutoRefreshFailureCount } from "./refreshBackoff";
import { buildExportPreview } from "./exportPreview";
import { setLocale, t, useLocale } from "./i18n";
import { mergeIssueMetricCounts, reuseUnchangedIssueMetrics } from "./issueMetrics";
import {
  reuseIfDeepEqual,
  reuseUnchangedSnapshot,
  ticketSelectionPresence,
} from "./snapshotReuse";
import {
  parseLocalNotes,
  parseWatchStates,
  removeTicketLocalNote,
  toggleWatchedTicket,
  type LocalNotes,
  type WatchStates,
} from "./localMetadata";
import { actorLabel, conversationEntryGroup, timestampValue } from "./conversation";
import {
  buildTicketLink,
  daemonLabelsForTicket,
  executionProviderOf,
  exportTimestamp,
  filterableLabelsForTicket,
  formatTime,
  isClosedTicket,
  issueNumber,
  localNotesForTicket,
  normalizeLabels,
  pretty,
  statusText,
} from "./issueFormat";
import type { ReviewDraftSubmission } from "./reviewDraft";
import { ticketEditPatch, type TicketEditSubmission } from "./ticketEditing";
import { IssueDetail } from "./IssueDetail";
import { IssueQueue } from "./IssueQueue";
import { NewIssuePanel } from "./NewIssuePanel";
import {
  badgeToneForPriority,
  badgeToneForRisk,
  emptyIssueSignalCounts,
  issueActionHintForTicket,
  issueSignalFilterKey,
  issueSignalFilterLabel,
  issueSignalForTicket,
  normalizeFilterLabel,
  type IssueActionHint,
  type IssueSignal,
  type IssueSignalCounts,
  type IssueSignalFilter,
} from "./issueSignals";
import type {
  IssueFilter,
  IssueListDensity,
  IssueMetrics,
  IssueQueueNavigation,
  IssueSort,
  RepoSection,
  TicketDraft,
} from "./issueTypes";

type IssueAuthorFilter = string | null;
type IssuePriorityFilter = "all" | "high";
type IssueRiskFilter = "all" | "high";
type IssueWatchFilter = "all" | "watched";
type IssueViewPreferences = {
  density: IssueListDensity;
  signalFilter: IssueSignalFilter;
  sort: IssueSort;
};
type ActivityTone = "info" | "success" | "error";
type ActivityEntry = {
  id: number;
  at: number;
  message: string;
  tone: ActivityTone;
};
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

const watchStorageKey = "tea.watchingTickets";
// Fresh key: the old "tea.ticketLabelOverrides" stored overlays merged with
// daemon labels, which could mask authoritative labels. Local notes are now a
// separate additive-only surface, so a new key avoids resurfacing stale merges.
const localNotesStorageKey = "tea.ticketLocalNotes";
const issueViewPreferencesStorageKey = "tea.issueViewPreferences";
const autoRefreshStorageKey = "tea.autoRefreshEnabled";

const readLocalStorage = (key: string): string | null => {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
};

const writeLocalStorage = (key: string, value: string) => {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Persistence is optional in restricted WebViews and private browser contexts.
  }
};

const readAutoRefreshPreference = (): boolean => {
  const stored = readLocalStorage(autoRefreshStorageKey);
  return stored === null ? true : stored === "true";
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
  const raw = readLocalStorage(issueViewPreferencesStorageKey);
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

const readWatchStates = (): WatchStates => {
  return parseWatchStates(readLocalStorage(watchStorageKey));
};

const readLocalNotes = (): LocalNotes => {
  return parseLocalNotes(readLocalStorage(localNotesStorageKey));
};

export default function App() {
  const locale = useLocale();
  const [serverUrl, setServerUrl] = useState("http://127.0.0.1:48910");
  const [authToken, setAuthToken] = useState("");
  // Local echo of the settings inputs. Only the applied serverUrl/authToken
  // feed the options memo (and thus the refresh effects); the applied values
  // update on blur, Enter, or a short debounce — not on every keystroke.
  const [serverUrlInput, setServerUrlInput] = useState(serverUrl);
  const [authTokenInput, setAuthTokenInput] = useState(authToken);
  const [authConfigured, setAuthConfigured] = useState(false);
  const [runtimeConfigResolved, setRuntimeConfigResolved] = useState(false);
  const [detailError, setDetailError] = useState("");
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
  const [refreshing, setRefreshing] = useState(false);
  const [exportDownloadBusy, setExportDownloadBusy] = useState(false);
  const [creating, setCreating] = useState(false);
  const creatingRef = useRef(false);
  const createRequestRef = useRef<{ fingerprint: string; key: string } | null>(null);
  const [createNotice, setCreateNotice] = useState<string>("");
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
  const [watchStates, setWatchStates] = useState<WatchStates>(() => readWatchStates());
  const [localNotes, setLocalNotes] = useState<LocalNotes>(() => readLocalNotes());
  const [showLabelEditor, setShowLabelEditor] = useState(false);
  const [editOwner, setEditOwner] = useState<{ ticketId: string; connection: TeaClientOptions } | null>(null);
  const [autoRefresh, setAutoRefresh] = useState<boolean>(() => readAutoRefreshPreference());
  const [lastRefreshedAt, setLastRefreshedAt] = useState<number | null>(null);

  const options: TeaClientOptions = useMemo(
    () => ({
      serverUrl,
      authToken: authToken.trim() || undefined,
    }),
    [authToken, serverUrl],
  );
  const reviewScope = useMemo(() => ({ ticketId: selectedId ?? "", connection: options }), [selectedId, options]);
  const reviewScopeRef = useRef(reviewScope);
  const optionsRef = useRef(options);
  const selectedIdRef = useRef(selectedId);
  const activeTicketRef = useRef<TeaTicket | null>(null);
  const sortedTicketsRef = useRef<TeaTicket[]>([]);
  const selectedIsWatchedRef = useRef(false);
  const issueQueueNavigationRef = useRef<IssueQueueNavigation | null>(null);
  const serverUrlRef = useRef(serverUrl);
  const refreshGenerationRef = useRef(0);
  const refreshInFlightRef = useRef(false);
  const refreshPendingRef = useRef(false);
  const autoRefreshFailureCountRef = useRef(0);
  const autoRefreshTimerRef = useRef<number | null>(null);
  const autoRefreshRef = useRef(autoRefresh);
  const mutationInFlightRef = useRef(false);
  const exportDownloadInFlightRef = useRef(false);
  const detailGenerationRef = useRef(0);
  reviewScopeRef.current = reviewScope;
  optionsRef.current = options;
  selectedIdRef.current = selectedId;
  autoRefreshRef.current = autoRefresh;
  serverUrlRef.current = serverUrl;

  // Stable identities (empty deps; they only touch refs, setters, and module
  // helpers) so the useCallback handlers below — and, through them, the memo'd
  // IssueDetail — keep the same identity across App re-renders.
  const beginMutation = useCallback(() => {
    if (mutationInFlightRef.current) return false;
    mutationInFlightRef.current = true;
    setBusy(true);
    return true;
  }, []);

  const endMutation = useCallback(() => {
    mutationInFlightRef.current = false;
    setBusy(false);
  }, []);

  const notify = useCallback((rawMessage: string) => {
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
  }, []);

  const clearActivityLog = useCallback(() => setActivityLog([]), []);

  const tickets = useMemo(() => snapshot?.tickets ?? [], [snapshot]);
  const ticketSignals = useMemo(() => {
    const signals: Record<string, IssueSignal> = {};
    for (const ticket of tickets) {
      signals[ticket.id] = issueSignalForTicket(ticket, issueMetrics[ticket.id]);
    }
    return signals;
  }, [issueMetrics, tickets]);
  // The filter key for a ticket's signal is derived once here. The signal counts,
  // the preset-queue counts and the signal predicate each used to recompute it,
  // which meant re-running the switch (and the signal fallback) three times per
  // ticket per render.
  const ticketSignalKeys = useMemo(() => {
    const keys: Record<string, Exclude<IssueSignalFilter, "all">> = {};
    for (const ticket of tickets) {
      keys[ticket.id] = issueSignalFilterKey(
        ticketSignals[ticket.id] ?? issueSignalForTicket(ticket, issueMetrics[ticket.id]),
      );
    }
    return keys;
  }, [issueMetrics, ticketSignals, tickets]);
  const ticketActionHints = useMemo(() => {
    const hints: Record<string, IssueActionHint> = {};
    for (const ticket of tickets) {
      const issueSignal = ticketSignals[ticket.id];
      if (!issueSignal) continue;
      hints[ticket.id] = issueActionHintForTicket(ticket, issueMetrics[ticket.id], issueSignal);
    }
    return hints;
  }, [issueMetrics, ticketSignals, tickets]);
  // One pass builds each ticket's lowercase search haystack and normalized label
  // list. The search and label predicates run once per ticket per filter pass, and
  // there are three such passes (signal counts, triage counts, filteredTickets), so
  // deriving these inline made every keystroke rebuild the same strings three times.
  const ticketFilterIndex = useMemo(() => {
    const index: Record<string, { haystack: string; labels: string[] }> = {};
    for (const ticket of tickets) {
      const labels = filterableLabelsForTicket(ticket, localNotes);
      index[ticket.id] = {
        haystack: [
          ticket.id,
          ticket.title,
          ticket.description ?? "",
          ticket.status,
          ticket.source ?? "",
          ticket.priority ?? "",
          ticket.risk_level ?? "",
          ticket.owner_human_id ?? "",
          ticket.delegated_agent_id ?? "",
          ...labels,
        ]
          .join(" ")
          .toLowerCase(),
        labels: labels.map(normalizeFilterLabel),
      };
    }
    return index;
  }, [localNotes, tickets]);
  const availableLabels = useMemo(
    () =>
      Array.from(
        new Set(Object.values(ticketFilterIndex).flatMap((entry) => entry.labels)),
      ).filter(Boolean),
    [ticketFilterIndex],
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
  // Memoized so the aggregate props handed to the memoized IssueQueue keep a
  // stable identity across the auto-refresh re-renders of this component.
  const openCount = useMemo(
    () => tickets.reduce((count, ticket) => (isClosedTicket(ticket) ? count : count + 1), 0),
    [tickets],
  );
  const closedCount = tickets.length - openCount;
  const activeTicket = useMemo(() => {
    const fromList = selectedId ? tickets.find((ticket) => ticket.id === selectedId) ?? null : null;
    return fromList ?? (snapshot === null ? selectedTicket : null);
  }, [selectedId, selectedTicket, snapshot, tickets]);

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
      if (!selectedLabelFilter) return true;
      return (ticketFilterIndex[ticket.id]?.labels ?? []).includes(selectedLabelFilter);
    },
    [selectedLabelFilter, ticketFilterIndex],
  );

  // Normalized once per keystroke instead of once per ticket per filter pass.
  const normalizedSearchQuery = useMemo(() => searchQuery.trim().toLowerCase(), [searchQuery]);

  const ticketMatchesSearchQuery = useCallback(
    (ticket: TeaTicket) => {
      if (!normalizedSearchQuery) return true;
      return (ticketFilterIndex[ticket.id]?.haystack ?? "").includes(normalizedSearchQuery);
    },
    [normalizedSearchQuery, ticketFilterIndex],
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
    (ticket: TeaTicket) =>
      issueSignalFilter === "all" || ticketSignalKeys[ticket.id] === issueSignalFilter,
    [issueSignalFilter, ticketSignalKeys],
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
        counts[ticketSignalKeys[ticket.id]] += 1;
        return counts;
      }, emptyIssueSignalCounts()),
    [ticketMatchesBaseIssueFilters, ticketSignalKeys, tickets],
  );
  const issueSignalTotal = useMemo(
    () => Object.values(issueSignalCounts).reduce((total, count) => total + count, 0),
    [issueSignalCounts],
  );
  const issueSignalCountForOption = useCallback(
    (value: IssueSignalFilter) => (value === "all" ? issueSignalTotal : issueSignalCounts[value]),
    [issueSignalCounts, issueSignalTotal],
  );
  // Single pass: the base and signal predicates used to run three times per
  // ticket (once per counter), each re-deriving the ticket's signal.
  const issueTriageFilterCounts = useMemo(() => {
    let highPriority = 0;
    let highRisk = 0;
    let watched = 0;
    for (const ticket of tickets) {
      if (!ticketMatchesBaseIssueFilters(ticket)) continue;
      if (!ticketMatchesSignalFilter(ticket)) continue;
      if (badgeToneForPriority(ticket.priority) === "danger") highPriority += 1;
      if (badgeToneForRisk(ticket.risk_level) === "danger") highRisk += 1;
      if (watchStates[ticket.id]) watched += 1;
    }
    return { highPriority, highRisk, watched };
  }, [tickets, ticketMatchesBaseIssueFilters, ticketMatchesSignalFilter, watchStates]);
  // One pass over the tickets computes all six preset-queue counts, memoized so the
  // preset-queue cards do not redo this work on unrelated re-renders.
  const issuePresetQueueCounts = useMemo(() => {
    const counts = Object.fromEntries(issuePresetQueues.map((queue) => [queue.key, 0])) as Record<
      IssuePresetQueueKey,
      number
    >;
    for (const ticket of tickets) {
      const closed = isClosedTicket(ticket);
      const signalKey = ticketSignalKeys[ticket.id];
      for (const queue of issuePresetQueues) {
        const matchesState =
          queue.issueFilter === "all" ||
          (queue.issueFilter === "open" && !closed) ||
          (queue.issueFilter === "closed" && closed);
        if (!matchesState) continue;
        if (queue.signalFilter !== "all" && signalKey !== queue.signalFilter) continue;
        counts[queue.key] += 1;
      }
    }
    return counts;
  }, [ticketSignalKeys, tickets]);

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
    // Decorate-sort-undecorate: the old comparator called timestampValue (Date.parse)
    // two to four times per comparison, so sorting cost O(n log n) date parses. The
    // keys are now derived exactly once per ticket.
    const decorated = filteredTickets.map((ticket) => {
      const metrics = issueMetrics[ticket.id];
      return {
        activity: (metrics?.comments ?? 0) + (metrics?.runs ?? 0),
        created: timestampValue(ticket.created_at),
        ticket,
        touch: timestampValue(
          metrics?.latestTouch?.createdAt ?? ticket.updated_at ?? ticket.created_at,
        ),
        updated: timestampValue(ticket.updated_at ?? ticket.created_at),
      };
    });
    decorated.sort((left, right) => {
      if (issueSort === "created") return right.created - left.created;
      if (issueSort === "touch") return right.touch - left.touch;
      if (issueSort === "activity") {
        return right.activity - left.activity || right.updated - left.updated;
      }
      return right.updated - left.updated;
    });
    return decorated.map((entry) => entry.ticket);
  }, [filteredTickets, issueMetrics, issueSort]);
  const issuePageSize = issuePageSizeByDensity[issueListDensity];
  const visibleTickets = useMemo(
    () => sortedTickets.slice(0, Math.min(visibleIssueLimit, sortedTickets.length)),
    [sortedTickets, visibleIssueLimit],
  );
  const hasMoreVisibleTickets = visibleIssueLimit < sortedTickets.length;
  const canCollapseIssueList = visibleIssueLimit > issuePageSize;
  // Memoized so the memo'd IssueDetail receives a stable object across renders
  // where neither the queue nor the selection changed. reuseIfDeepEqual keeps
  // the previous identity when sortedTickets is a new array with the same
  // navigation ids (e.g. other tickets' metrics updated).
  const issueQueueNavigation = useMemo<IssueQueueNavigation>(() => {
    const selectedIssueQueueIndex = activeTicket
      ? sortedTickets.findIndex((ticket) => ticket.id === activeTicket.id)
      : -1;
    const next: IssueQueueNavigation = {
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
    const reused = reuseIfDeepEqual(issueQueueNavigationRef.current ?? next, next);
    issueQueueNavigationRef.current = reused;
    return reused;
  }, [activeTicket, sortedTickets]);
  const issueFilterLabel =
    issueFilter === "all"
      ? t("all work orders")
      : issueFilter === "closed"
        ? t("closed work orders")
        : t("open work orders");
  const issueSignalFilterSummary = t(issueSignalFilterLabel(issueSignalFilter));
  // The array/object props below feed the memoized IssueQueue, so they are
  // memoized too. Their dependencies are all primitives or already-translated
  // strings, which is what makes a locale switch propagate without t() having to
  // appear inside a memo body (t() reads a module-level locale that eslint
  // react-hooks cannot see).
  const activeIssueFilterChips = useMemo<ActiveIssueFilterChip[]>(
    () => [
      ...(issueFilter !== "open"
        ? [{ key: "state" as const, label: "State", value: issueFilter === "all" ? "All" : "Closed" }]
        : []),
      ...(searchQuery.trim()
        ? [{ key: "search" as const, label: "Search", value: searchQuery.trim() }]
        : []),
      ...(authorFilter ? [{ key: "author" as const, label: "Author", value: authorFilter }] : []),
      ...(selectedLabelFilter
        ? [{ key: "label" as const, label: "Label", value: selectedLabelFilter }]
        : []),
      ...(issueSignalFilter !== "all"
        ? [{ key: "signal" as const, label: "Signal", value: issueSignalFilterSummary }]
        : []),
      ...(issuePriorityFilter === "high"
        ? [{ key: "priority" as const, label: "Priority", value: "High" }]
        : []),
      ...(issueRiskFilter === "high" ? [{ key: "risk" as const, label: "Risk", value: "High" }] : []),
      ...(issueWatchFilter === "watched"
        ? [{ key: "watch" as const, label: "Watch", value: "Watched" }]
        : []),
    ],
    [
      authorFilter,
      issueFilter,
      issuePriorityFilter,
      issueRiskFilter,
      issueSignalFilter,
      issueSignalFilterSummary,
      issueWatchFilter,
      searchQuery,
      selectedLabelFilter,
    ],
  );
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
  const activeIssuePresetQueue = useMemo(
    () => issuePresetQueues.find((queue) => queue.key === activeIssuePresetQueueKey) ?? null,
    [activeIssuePresetQueueKey],
  );
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
  const issueMatchingSummary = t("{n} work orders").replace("{n}", String(filteredTickets.length));
  const issueVisibleSummary = t("{n} in view").replace("{n}", String(visibleTickets.length));
  const issueExtraFilterSummary = hasActiveIssueFilters
    ? t("{n} active").replace("{n}", String(activeIssueFilterChips.length))
    : t("None");
  const issueQueueSummaryItems = useMemo(
    () => [
      { label: "Matching", value: issueMatchingSummary },
      { label: "Visible", value: issueVisibleSummary },
      { label: "Signal", value: issueSignalFilterSummary },
      { label: "Sort", value: issueSortSummary },
      { label: "Density", value: issueDensitySummary },
      { label: "Extra filters", value: issueExtraFilterSummary },
    ],
    [
      issueDensitySummary,
      issueExtraFilterSummary,
      issueMatchingSummary,
      issueSignalFilterSummary,
      issueSortSummary,
      issueVisibleSummary,
    ],
  );
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
  // Memoized objects/arrays: the memo'd IssueDetail must not receive a fresh
  // identity for these on renders where the underlying inputs are unchanged.
  const selectedSignal = useMemo(() => {
    if (!activeTicket) return null;
    if (selectedMetrics) {
      return ticketSignals[activeTicket.id] ?? issueSignalForTicket(activeTicket, selectedMetrics);
    }
    return issueSignalForTicket(activeTicket, { comments: comments.length, runs: runs.length });
  }, [activeTicket, comments, runs, selectedMetrics, ticketSignals]);
  const selectedActionHint = useMemo(
    () =>
      activeTicket && selectedSignal
        ? issueActionHintForTicket(
            activeTicket,
            selectedMetrics ?? { comments: comments.length, runs: runs.length },
            selectedSignal,
          )
        : null,
    [activeTicket, selectedMetrics, selectedSignal, comments, runs],
  );
  const selectedIsWatched = activeTicket ? Boolean(watchStates[activeTicket.id]) : false;
  activeTicketRef.current = activeTicket;
  sortedTicketsRef.current = sortedTickets;
  selectedIsWatchedRef.current = selectedIsWatched;
  // Authoritative daemon labels for the header/rows; never masked by local notes.
  const selectedDaemonLabels = useMemo(
    () => (activeTicket ? daemonLabelsForTicket(activeTicket) : []),
    [activeTicket],
  );
  // Local-only annotations shown in their own surface, independent of daemon labels.
  const selectedLocalNotes = useMemo(
    () => (activeTicket ? localNotesForTicket(activeTicket, localNotes) : []),
    [activeTicket, localNotes],
  );
  const selectedHasLocalNotes = activeTicket
    ? Object.prototype.hasOwnProperty.call(localNotes, activeTicket.id)
    : false;

  // These handlers are all passed to the memoized IssueQueue, so each one keeps a
  // stable identity via useCallback; a fresh arrow per render would defeat the memo.
  const clearLabelFilter = useCallback(() => {
    setSelectedLabelFilter(null);
    setShowLabelFilterPanel(false);
  }, []);

  const clearAuthorFilter = useCallback(() => {
    setAuthorFilter(null);
    setShowAuthorFilterPanel(false);
  }, []);

  const removeIssueFilterChip = useCallback(
    (key: ActiveIssueFilterChip["key"]) => {
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
    },
    [clearAuthorFilter, clearLabelFilter],
  );

  const clearIssueFilters = useCallback(() => {
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
  }, []);

  const clearWatchedIssueFilter = useCallback(() => {
    setIssueWatchFilter("all");
  }, []);

  const resetIssueViewPreferences = useCallback(() => {
    setIssueSort(defaultIssueViewPreferences.sort);
    setIssueListDensity(defaultIssueViewPreferences.density);
    setIssueSignalFilter(defaultIssueViewPreferences.signalFilter);
    setVisibleIssueLimit(issuePageSizeByDensity[defaultIssueViewPreferences.density]);
    notify("Reset issue view preferences");
  }, [notify]);

  const applyIssuePresetQueue = useCallback((queue: IssuePresetQueue) => {
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
  }, []);

  // Reads the queue length through sortedTicketsRef so this callback does not have
  // to be rebuilt whenever the sorted queue is recomputed.
  const showMoreIssues = useCallback(() => {
    setVisibleIssueLimit((current) =>
      Math.min(sortedTicketsRef.current.length, current + issuePageSize),
    );
  }, [issuePageSize]);

  const collapseIssueList = useCallback(() => {
    setVisibleIssueLimit(issuePageSize);
  }, [issuePageSize]);

  // Stable identity so the memo'd IssueListRow does not re-render when App does.
  const handleSelectIssue = useCallback((ticketId: string) => setSelectedId(ticketId), []);

  const navigateIssueQueue = useCallback((ticketId: string | null) => {
    if (!ticketId) return;
    const list = sortedTicketsRef.current;
    const targetIndex = list.findIndex((ticket) => ticket.id === ticketId);
    if (targetIndex >= 0) {
      setVisibleIssueLimit((current) => Math.max(current, targetIndex + 1));
    }
    setSelectedId(ticketId);
    setSelectedTicket(list.find((ticket) => ticket.id === ticketId) ?? null);
  }, []);

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

  const refreshIssueMetrics = useCallback(async (
    ticketIds: string[],
    requestOptions: TeaClientOptions,
    generation: number,
  ) => {
    if (ticketIds.length === 0) {
      if (generation === refreshGenerationRef.current) setIssueMetrics({});
      return;
    }

    // Single aggregated request replaces the previous comments+runs+events fan-out
    // (which was 3 requests per ticket, re-run on every auto-refresh poll).
    const wanted = new Set(ticketIds);
    let metrics: TeaIssueMetric[];
    try {
      metrics = await getIssueMetrics(requestOptions);
    } catch (error) {
      if (generation === refreshGenerationRef.current) {
        notify(`Failed to read issue metrics: ${String(error)}`);
      }
      return;
    }
    if (generation !== refreshGenerationRef.current) return;

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
    const nextIssueMetrics = Object.fromEntries(entries);
    setIssueMetrics((current) => reuseUnchangedIssueMetrics(current, nextIssueMetrics));
  }, [notify]);

  const clearAutoRefreshTimer = useCallback(() => {
    if (autoRefreshTimerRef.current === null) return;
    window.clearTimeout(autoRefreshTimerRef.current);
    autoRefreshTimerRef.current = null;
  }, []);

  // refresh() and scheduleNextAutoRefresh() call each other, so the schedule
  // goes through a ref to keep both useCallback identities stable without a
  // circular dependency.
  const refreshRef = useRef<() => Promise<void>>(async () => {});

  // Schedules the next auto-refresh poll. Called from refresh()'s finally block
  // (so the next tick is chained off the previous outcome and honors the failure
  // backoff) and from the auto-refresh effect when the toggle/connection changes.
  const scheduleNextAutoRefresh = useCallback(() => {
    clearAutoRefreshTimer();
    if (!autoRefreshRef.current) return;
    if (document.visibilityState !== "visible") return;
    autoRefreshTimerRef.current = window.setTimeout(() => {
      autoRefreshTimerRef.current = null;
      void refreshRef.current();
    }, autoRefreshDelayMs(autoRefreshFailureCountRef.current));
  }, [clearAutoRefreshTimer]);

  const refreshDetail = useCallback(async (id: string) => {
    const generation = ++detailGenerationRef.current;
    const requestOptions = optionsRef.current;
    try {
      // Single aggregated request replaces the previous six-call fan-out
      // (ticket + comments + events + runs + analysis + plan) per selection.
      const bundle = await getTicketBundle(id, requestOptions);
      if (generation !== detailGenerationRef.current || selectedIdRef.current !== id) return;
      setDetailError("");
      // Skip each state update when the refreshed payload is deep-equal to the
      // current value, so the memo'd ConversationStream/AnalysisPlanView (and
      // anything keyed on these references) do not re-render on every poll.
      setSelectedTicket((current) => reuseIfDeepEqual(current, bundle.ticket));
      setComments((current) => reuseIfDeepEqual(current, bundle.comments));
      setEvents((current) => reuseIfDeepEqual(current, bundle.events));
      setRuns((current) => reuseIfDeepEqual(current, bundle.runs));
      setAnalysis((current) => reuseIfDeepEqual(current, bundle.analysis));
      setPlan((current) => reuseIfDeepEqual(current, bundle.plan));
      setIssueMetrics((current) =>
        mergeIssueMetricCounts(current, id, bundle.comments.length, bundle.runs.length),
      );
    } catch (error) {
      if (generation !== detailGenerationRef.current || selectedIdRef.current !== id) return;
      // Keep the last successful detail payload visible during a transient
      // refresh failure. Selection/configuration changes clear the old payload
      // before starting a new request; an auto-refresh failure should not turn
      // a temporarily unavailable bundle into an empty conversation.
      const message = `Failed to read ticket: ${String(error)}`;
      setDetailError(message);
      notify(message);
    }
  }, [notify]);

  const refresh = useCallback(async () => {
    if (refreshInFlightRef.current) {
      refreshPendingRef.current = true;
      refreshGenerationRef.current += 1;
      return;
    }
    refreshInFlightRef.current = true;
    const generation = ++refreshGenerationRef.current;
    const requestOptions = optionsRef.current;
    let refreshOutcomeRecorded = false;
    setRefreshing(true);
    try {
      const next = await readSnapshot(requestOptions);
      if (generation !== refreshGenerationRef.current) return;
      // Preserve object identity for unchanged tickets/snapshot parts so the
      // memos keyed on snapshot/tickets do not recompute on every poll tick.
      setSnapshot((current) => reuseUnchangedSnapshot(current, next));
      setLastRefreshedAt(Date.now());
      if (next.error) {
        notify(
          next.status
            ? `Tea data partially unavailable: ${next.error}`
            : `Connection failed: ${next.error}`,
        );
      } else {
        notify(next.status ? "Tea daemon connected" : "Tea daemon is not fully ready");
      }
      const followupRequests: Promise<void>[] = [];
      if (!selectedIdRef.current && next.tickets.length > 0) {
        setSelectedId(next.tickets[0].id);
      } else if (selectedIdRef.current) {
        const presence = ticketSelectionPresence(next, selectedIdRef.current);
        if (presence !== "missing") {
          followupRequests.push(refreshDetail(selectedIdRef.current));
        } else {
          selectedIdRef.current = null;
          setSelectedId(null);
        }
      }
      if (next.ticketsAvailable) {
        followupRequests.push(
          refreshIssueMetrics(
            next.tickets.map((ticket) => ticket.id),
            requestOptions,
            generation,
          ),
        );
      }
      await Promise.all(followupRequests);
      if (generation === refreshGenerationRef.current) {
        autoRefreshFailureCountRef.current = nextAutoRefreshFailureCount(
          autoRefreshFailureCountRef.current,
          Boolean(next.error || !next.status),
        );
        refreshOutcomeRecorded = true;
      }
    } catch (error) {
      if (generation !== refreshGenerationRef.current) return;
      autoRefreshFailureCountRef.current = nextAutoRefreshFailureCount(
        autoRefreshFailureCountRef.current,
        true,
      );
      refreshOutcomeRecorded = true;
      setSnapshot({
        health: null,
        status: null,
        configuration: null,
        tickets: [],
        ticketsAvailable: false,
        error: String(error),
      });
      setIssueMetrics({});
      notify(`Connection failed: ${String(error)}`);
    } finally {
      refreshInFlightRef.current = false;
      if (refreshOutcomeRecorded) {
        scheduleNextAutoRefresh();
      }
      if (refreshPendingRef.current) {
        refreshPendingRef.current = false;
        void refreshRef.current();
      } else if (generation === refreshGenerationRef.current) {
        setRefreshing(false);
      }
    }
  }, [notify, refreshDetail, refreshIssueMetrics, scheduleNextAutoRefresh]);
  refreshRef.current = refresh;

  useEffect(() => {
    resolveRuntimeConfig()
      .then((config) => {
        setServerUrl(config.serverUrl);
        setAuthConfigured(config.authConfigured);
      })
      .catch((error) => notify(`Failed to read runtime config: ${String(error)}`))
      .finally(() => setRuntimeConfigResolved(true));
  }, [notify]);

  useEffect(() => {
    if (!runtimeConfigResolved) return;
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [options.serverUrl, options.authToken, runtimeConfigResolved]);

  useEffect(() => {
    if (!autoRefresh) return;
    const handleVisibilityChange = () => {
      clearAutoRefreshTimer();
      if (document.visibilityState === "visible") void refresh();
    };
    // Kick off the polling chain; each completed refresh schedules the next
    // tick from its finally block (with backoff), so no schedule-version state
    // is needed to re-run this effect per poll.
    scheduleNextAutoRefresh();
    document.addEventListener("visibilitychange", handleVisibilityChange);
    return () => {
      clearAutoRefreshTimer();
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoRefresh, options.serverUrl, options.authToken]);

  useEffect(() => {
    writeLocalStorage(autoRefreshStorageKey, String(autoRefresh));
  }, [autoRefresh]);

  // Keep the settings inputs in sync when the applied value changes elsewhere
  // (e.g. the runtime config resolves at startup). Committing an unchanged
  // input is a no-op because the values are already equal.
  useEffect(() => {
    setServerUrlInput(serverUrl);
  }, [serverUrl]);

  useEffect(() => {
    setAuthTokenInput(authToken);
  }, [authToken]);

  // Debounced commit: apply the typed value after the operator pauses, so the
  // refresh effects keyed on options.serverUrl/authToken do not fire per key.
  useEffect(() => {
    if (serverUrlInput === serverUrl) return;
    const timer = window.setTimeout(() => setServerUrl(serverUrlInput), 600);
    return () => window.clearTimeout(timer);
  }, [serverUrl, serverUrlInput]);

  useEffect(() => {
    if (authTokenInput === authToken) return;
    const timer = window.setTimeout(() => setAuthToken(authTokenInput), 600);
    return () => window.clearTimeout(timer);
  }, [authToken, authTokenInput]);

  useEffect(() => {
    detailGenerationRef.current += 1;
    setEditOwner(null);
    setSelectedTicket(null);
    setExportPreview("");
    setDetailError("");
    setComments([]);
    setEvents([]);
    setRuns([]);
    setAnalysis(null);
    setPlan(null);
    if (selectedId) {
      void refreshDetail(selectedId);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedId, options.serverUrl, options.authToken]);

  useEffect(() => {
    writeLocalStorage(watchStorageKey, JSON.stringify(watchStates));
  }, [watchStates]);

  useEffect(() => {
    writeLocalStorage(issueViewPreferencesStorageKey, JSON.stringify(issueViewPreferences));
  }, [issueViewPreferences]);

  useEffect(() => {
    writeLocalStorage(localNotesStorageKey, JSON.stringify(localNotes));
  }, [localNotes]);

  // Closing the editor also unmounts LabelEditor, which owns (and thereby
  // resets) the note draft the previous selection may have left behind.
  useEffect(() => {
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

  // The draft now lives in NewIssuePanel; the panel passes the composed draft
  // up on submit. The synchronous in-flight gate (creatingRef) and the
  // fingerprint/idempotency-key reuse semantics are unchanged: an unchanged
  // draft resubmitted after an uncertain failure reuses the same key.
  const submitTicket = useCallback(async (draft: TicketDraft) => {
    if (creatingRef.current) return;
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
    if (!beginMutation()) return;
    creatingRef.current = true;
    setCreating(true);
    setCreateNotice(t("Creating..."));
    try {
      const draftLabels = draft.labels
        .split(/[,\n]/)
        .map((label) => label.trim())
        .filter(Boolean);
      const createInput = {
        title: trimmedTitle,
        description: trimmedDescription,
        approvalPolicy: draft.approvalPolicy.trim() || undefined,
        priority: draft.priority.trim() || undefined,
        labels: draftLabels.length > 0 ? draftLabels : undefined,
      };
      const fingerprint = JSON.stringify(createInput);
      if (createRequestRef.current?.fingerprint !== fingerprint) {
        createRequestRef.current = {
          fingerprint,
          key: `tea-desktop-${globalThis.crypto.randomUUID()}`,
        };
      }
      const ticket = await createTicket(createInput, options, createRequestRef.current.key);
      createRequestRef.current = null;
      // Hiding the panel unmounts NewIssuePanel, which clears the local draft.
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
      creatingRef.current = false;
      setCreating(false);
      endMutation();
    }
  }, [beginMutation, endMutation, notify, options, refresh]);

  const cancelNewIssue = useCallback(() => {
    createRequestRef.current = null;
    setShowNewIssue(false);
  }, []);

  const runAction = useCallback(async (action: Parameters<typeof ticketAction>[1]) => {
    const id = selectedIdRef.current;
    if (!id) return;
    if (!beginMutation()) return;
    try {
      await ticketAction(id, action, optionsRef.current);
      notify(`Action submitted: ${action}`);
      await refreshRef.current();
    } catch (error) {
      notify(`Action failed: ${String(error)}`);
    } finally {
      endMutation();
    }
  }, [beginMutation, endMutation, notify]);

  // Returns whether the reject was applied so RejectReasonForm can clear its
  // locally-owned reason only on success (mirrors the previous behavior where
  // the shared rejectReason state was cleared inside the success path).
  const submitReject = useCallback(async (submission: ReviewDraftSubmission): Promise<boolean> => {
    const { scope, text } = submission;
    if (!scope.ticketId || scope !== reviewScopeRef.current ||
      scope.ticketId !== selectedIdRef.current || scope.connection !== optionsRef.current) return false;
    const id = scope.ticketId;
    const trimmed = text.trim();
    if (!trimmed) {
      notify("Reject reason is required");
      return false;
    }
    if (!beginMutation()) return false;
    try {
      await rejectTicket(id, trimmed, scope.connection);
      notify("Approval rejected");
      await refreshRef.current();
      return true;
    } catch (error) {
      notify(`Reject failed: ${String(error)}`);
      return false;
    } finally {
      endMutation();
    }
  }, [beginMutation, endMutation, notify]);

  const applyTicketPolicy = useCallback(async (mode: string) => {
    const id = selectedIdRef.current;
    if (!id || !mode) return;
    if (!beginMutation()) return;
    try {
      await setTicketPolicy(id, mode, optionsRef.current);
      notify(`Approval policy set: ${mode}`);
      await refreshRef.current();
    } catch (error) {
      notify(`Policy change failed: ${String(error)}`);
    } finally {
      endMutation();
    }
  }, [beginMutation, endMutation, notify]);

  // IssueEditForm seeds its local draft from the ticket when it mounts (i.e.
  // when editing opens), so opening the editor no longer copies state here.
  const beginEditIssue = useCallback(() => {
    const ticketId = selectedIdRef.current;
    if (ticketId) setEditOwner({ ticketId, connection: optionsRef.current });
  }, []);

  const cancelEditIssue = useCallback(() => {
    setEditOwner(null);
  }, []);

  const submitTicketEdit = useCallback(async (submission: TicketEditSubmission) => {
    const owner = editOwner;
    // Bind the save to the visible editor session, not whichever task/daemon
    // happens to be selected when an older callback is invoked.
    if (!owner || owner.ticketId !== submission.ticketId ||
      selectedIdRef.current !== submission.ticketId || owner.connection !== optionsRef.current) return;
    if (!submission.draft.title.trim()) {
      notify("Work order title is required");
      return;
    }
    const input = ticketEditPatch(submission.baseline, submission.draft);
    const closeThisEditor = () => setEditOwner((current) => current === owner ? null : current);
    if (Object.keys(input).length === 0) {
      notify("No changes to save");
      closeThisEditor();
      return;
    }
    if (!beginMutation()) return;
    try {
      await updateTicket(submission.ticketId, input, owner.connection);
      closeThisEditor();
      notify("Work order updated");
      await refreshRef.current();
    } catch (error) {
      notify(`Edit failed: ${String(error)}`);
    } finally {
      endMutation();
    }
  }, [beginMutation, editOwner, endMutation, notify]);

  const saveLocalConfiguration = useCallback(async (config: Partial<TeaLocalConfig>, connection: TeaClientOptions) => {
    if (connection !== optionsRef.current || !beginMutation()) return;
    try {
      await updateConfiguration(config, connection);
      notify("Tea local configuration saved");
      await refreshRef.current();
    } catch (error) {
      notify(`Save configuration failed: ${String(error)}`);
    } finally {
      endMutation();
    }
  }, [beginMutation, endMutation, notify]);

  const stopRunAction = useCallback(async (runId: string) => {
    if (!beginMutation()) return;
    try {
      await stopRun(runId, optionsRef.current);
      notify(`Run stop submitted: ${runId}`);
      await refreshRef.current();
    } catch (error) {
      notify(`Run stop failed: ${String(error)}`);
    } finally {
      endMutation();
    }
  }, [beginMutation, endMutation, notify]);

  const retryRunAction = useCallback(async (runId: string) => {
    if (!beginMutation()) return;
    try {
      await retryRun(runId, optionsRef.current);
      notify(`Run retry submitted: ${runId}`);
      await refreshRef.current();
    } catch (error) {
      notify(`Run retry failed: ${String(error)}`);
    } finally {
      endMutation();
    }
  }, [beginMutation, endMutation, notify]);

  // Returns whether the comment was persisted so CommentEditor can clear its
  // locally-owned draft only on success (mirrors the previous behavior where
  // the shared commentDraft state was cleared inside the success path).
  const submitComment = useCallback(async (submission: ReviewDraftSubmission): Promise<boolean> => {
    const { scope, text } = submission;
    if (!scope.ticketId || scope !== reviewScopeRef.current ||
      scope.ticketId !== selectedIdRef.current || scope.connection !== optionsRef.current) return false;
    const id = scope.ticketId;
    const trimmed = text.trim();
    if (!trimmed) {
      notify("Review comment cannot be empty");
      return false;
    }
    if (!beginMutation()) return false;
    try {
      await addComment(id, trimmed, scope.connection);
      notify("Review comment added");
      await refreshRef.current();
      return true;
    } catch (error) {
      notify(`Comment failed: ${String(error)}`);
      return false;
    } finally {
      endMutation();
    }
  }, [beginMutation, endMutation, notify]);

  const previewExport = useCallback(async (format: "json" | "markdown") => {
    const id = selectedIdRef.current;
    if (!id) return;
    try {
      const exported = await exportTicket(id, format, optionsRef.current);
      const content = typeof exported === "string" ? exported : pretty(exported);
      setExportPreview(
        buildExportPreview(content, t("Preview truncated. Download the full export.")),
      );
    } catch (error) {
      const message = `Export failed: ${String(error)}`;
      setExportPreview(message);
      setDetailError(message);
      notify(message);
    }
  }, [notify]);

  const downloadExport = useCallback(async (format: "json" | "markdown") => {
    const id = selectedIdRef.current;
    const ticket = activeTicketRef.current;
    if (!id || !ticket) return;
    if (exportDownloadInFlightRef.current) return;
    exportDownloadInFlightRef.current = true;
    setExportDownloadBusy(true);
    try {
      const exported = await exportTicket(id, format, optionsRef.current);
      const content = typeof exported === "string" ? exported : pretty(exported);
      const extension = format === "json" ? "json" : "md";
      const fileName = `tea-${issueNumber(ticket)}-${exportTimestamp()}.${extension}`;
      const savedPath = await saveExport(fileName, content);
      setExportPreview(
        buildExportPreview(content, t("Preview truncated. Download the full export.")),
      );
      notify(`Saved ${format} export to ${savedPath}`);
    } catch (error) {
      notify(`Export download failed: ${String(error)}`);
    } finally {
      exportDownloadInFlightRef.current = false;
      setExportDownloadBusy(false);
    }
  }, [notify]);

  const copyIssueLink = useCallback(async () => {
    const ticket = activeTicketRef.current;
    if (!ticket) return;
    const link = buildTicketLink(ticket, serverUrlRef.current);
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
  }, [notify]);

  const toggleWatchIssue = useCallback(() => {
    const ticket = activeTicketRef.current;
    if (!ticket) return;
    setWatchStates((current) => toggleWatchedTicket(current, ticket.id));
    notify(selectedIsWatchedRef.current ? "Issue un-watched locally" : "Issue watched locally");
  }, [notify]);

  const toggleLabelEditor = useCallback(() => {
    setShowLabelEditor((current) => !current);
  }, []);

  // Returns whether the note was added so LabelEditor can clear its
  // locally-owned draft only on success.
  const addLocalNote = useCallback((note: string): boolean => {
    const ticket = activeTicketRef.current;
    if (!ticket) return false;
    const nextNote = note.trim();
    if (!nextNote) {
      notify("Note text is required");
      return false;
    }
    // Local notes are additive annotations, kept only in this browser and never
    // sent to the daemon. They do not seed from or replace daemon labels.
    setLocalNotes((current) => ({
      ...current,
      [ticket.id]: normalizeLabels([
        ...(current[ticket.id] ?? []).filter(Boolean),
        nextNote,
      ]),
    }));
    setShowLabelEditor(true);
    notify(`Added local note: ${nextNote}`);
    return true;
  }, [notify]);

  const removeLocalNote = useCallback((note: string) => {
    const ticket = activeTicketRef.current;
    if (!ticket) return;
    setLocalNotes((current) => removeTicketLocalNote(current, ticket.id, note));
    notify(`Removed local note: ${note}`);
  }, [notify]);

  const resetLocalNotes = useCallback(() => {
    const ticket = activeTicketRef.current;
    if (!ticket) return;
    setLocalNotes((current) => {
      const next = { ...current };
      delete next[ticket.id];
      return next;
    });
    notify("Cleared local notes for this work order");
  }, [notify]);

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
          <span
            className={`execution-provider-badge ${executionProviderOf(snapshot)}`}
            data-provider={executionProviderOf(snapshot)}
            data-testid="execution-provider"
          >
            {executionProviderOf(snapshot) === "loom"
              ? t("Loom execution")
              : executionProviderOf(snapshot) === "mock"
                ? t("Simulation mode")
                : t("Execution provider unknown")}
          </span>
        </div>
        <div className="repo-actions">
          <button
            aria-label={t("Switch language")}
            className="ghost-button locale-toggle"
            data-testid="locale-toggle"
            onClick={() => setLocale(locale === "zh" ? "en" : "zh")}
            title={t("Switch language")}
            type="button"
          >
            {locale === "zh" ? "EN" : "中文"}
          </button>
          <div className="refresh-control">
            <button className="ghost-button" disabled={refreshing} onClick={() => void refresh()}>
              {refreshing ? t("Refreshing...") : t("Refresh")}
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
          <button
            className="new-issue-button"
            onClick={() =>
              setShowNewIssue((value) => {
                if (value) createRequestRef.current = null;
                return !value;
              })
            }
          >
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

      {snapshot?.error || detailError ? (
        <div className="runtime-error-banner" role="alert">
          <strong>{t("Data temporarily unavailable")}</strong>
          <span>{detailError || snapshot?.error}</span>
        </div>
      ) : null}

      {activeSection === "settings" ? (
        <section className="connection-strip">
          <div className="connection-state">
            <span className={`state-dot ${snapshot?.status ? "ok" : "warn"}`} />
            <strong>{statusText(snapshot)}</strong>
            <span>{message}</span>
          </div>
          <label>
            <span>{t("Tea daemon")}</span>
            <input
              value={serverUrlInput}
              onBlur={() => setServerUrl(serverUrlInput)}
              onChange={(event) => setServerUrlInput(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") setServerUrl(serverUrlInput);
              }}
            />
          </label>
          <label>
            <span>{t("Bearer token")} {authConfigured ? t("(launcher/env configured)") : t("(optional override)")}</span>
            <input
              value={authTokenInput}
              onBlur={() => setAuthToken(authTokenInput)}
              onChange={(event) => setAuthTokenInput(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") setAuthToken(authTokenInput);
              }}
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
        </summary>
        {activityLog.length > 0 ? (
          <div className="activity-log-actions">
            <button
              className="activity-log-clear"
              onClick={clearActivityLog}
              type="button"
            >
              {t("Clear log")}
            </button>
          </div>
        ) : null}
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
        <IssueQueue
          activeIssueFilterChips={activeIssueFilterChips}
          activeIssuePresetQueue={activeIssuePresetQueue}
          activeIssuePresetQueueKey={activeIssuePresetQueueKey}
          authorFilter={authorFilter}
          availableAuthors={availableAuthors}
          availableLabels={availableLabels}
          canCollapseIssueList={canCollapseIssueList}
          closedCount={closedCount}
          filteredTickets={filteredTickets}
          hasActiveIssueFilters={hasActiveIssueFilters}
          hasMoreVisibleTickets={hasMoreVisibleTickets}
          issueFilter={issueFilter}
          issueFilterLabel={issueFilterLabel}
          issueListDensity={issueListDensity}
          issueMetrics={issueMetrics}
          issuePresetQueueCounts={issuePresetQueueCounts}
          issuePresetQueues={issuePresetQueues}
          issuePriorityFilter={issuePriorityFilter}
          issueQueueSummaryItems={issueQueueSummaryItems}
          issueRiskFilter={issueRiskFilter}
          issueSignalCountForOption={issueSignalCountForOption}
          issueSignalFilter={issueSignalFilter}
          issueSignalFilterOptions={issueSignalFilterOptions}
          issueSignalFilterSummary={issueSignalFilterSummary}
          issueSort={issueSort}
          issueTriageFilterCounts={issueTriageFilterCounts}
          issueViewPreferenceStatus={issueViewPreferenceStatus}
          issueViewPreferencesAreDefault={issueViewPreferencesAreDefault}
          issueWatchFilter={issueWatchFilter}
          localNotes={localNotes}
          onApplyIssuePresetQueue={applyIssuePresetQueue}
          onClearAuthorFilter={clearAuthorFilter}
          onClearIssueFilters={clearIssueFilters}
          onClearLabelFilter={clearLabelFilter}
          onClearWatchedIssueFilter={clearWatchedIssueFilter}
          onCollapseIssueList={collapseIssueList}
          onRemoveIssueFilterChip={removeIssueFilterChip}
          onResetIssueViewPreferences={resetIssueViewPreferences}
          onSearchChange={setSearchQuery}
          onSelectIssue={handleSelectIssue}
          onShowMoreIssues={showMoreIssues}
          openCount={openCount}
          searchQuery={searchQuery}
          selectedId={selectedId}
          selectedLabelFilter={selectedLabelFilter}
          setAuthorFilter={setAuthorFilter}
          setIssueFilter={setIssueFilter}
          setIssueListDensity={setIssueListDensity}
          setIssuePriorityFilter={setIssuePriorityFilter}
          setIssueRiskFilter={setIssueRiskFilter}
          setIssueSignalFilter={setIssueSignalFilter}
          setIssueSort={setIssueSort}
          setIssueWatchFilter={setIssueWatchFilter}
          setSelectedLabelFilter={setSelectedLabelFilter}
          setShowAdvancedFilters={setShowAdvancedFilters}
          setShowAuthorFilterPanel={setShowAuthorFilterPanel}
          setShowLabelFilterPanel={setShowLabelFilterPanel}
          showAdvancedFilters={showAdvancedFilters}
          showAuthorFilterPanel={showAuthorFilterPanel}
          showLabelFilterPanel={showLabelFilterPanel}
          sortedTicketsLength={sortedTickets.length}
          ticketActionHints={ticketActionHints}
          ticketCount={tickets.length}
          ticketSignals={ticketSignals}
          visibleTickets={visibleTickets}
          watchStates={watchStates}
        >
          {showNewIssue ? (
            <NewIssuePanel
              createNotice={createNotice}
              creating={creating}
              onCancel={cancelNewIssue}
              onSubmit={submitTicket}
            />
          ) : null}
        </IssueQueue>

        <IssueDetail
          reviewScope={reviewScope}
          activeSection={activeSection}
          analysis={analysis}
          busy={busy}
          comments={comments}
          hasLocalNotes={selectedHasLocalNotes}
          isWatched={selectedIsWatched}
          events={events}
          exportDownloadBusy={exportDownloadBusy}
          exportPreview={exportPreview}
          daemonLabels={selectedDaemonLabels}
          localNotes={selectedLocalNotes}
          onAddLabel={addLocalNote}
          onCopyLink={copyIssueLink}
          onAction={runAction}
          onApplyPolicy={applyTicketPolicy}
          onComment={submitComment}
          onDownloadExport={downloadExport}
          onExport={previewExport}
          onNavigateIssueQueue={navigateIssueQueue}
          onReject={submitReject}
          onRemoveLabel={removeLocalNote}
          onResetLabels={resetLocalNotes}
          onRetryRun={retryRunAction}
          onSaveConfiguration={saveLocalConfiguration}
          onSectionChange={setActiveSection}
          onStopRun={stopRunAction}
          plan={plan}
          onToggleWatch={toggleWatchIssue}
          onToggleLabelEditor={toggleLabelEditor}
          onBeginEdit={beginEditIssue}
          onCancelEdit={cancelEditIssue}
          onSubmitEdit={submitTicketEdit}
          showEditIssue={editOwner?.ticketId === activeTicket?.id && editOwner?.connection === options}
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
