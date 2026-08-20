import { invoke } from "@tauri-apps/api/core";

export type JsonObject = Record<string, unknown>;

export interface TeaRuntimeConfig {
  serverUrl: string;
  authConfigured: boolean;
}

export interface TeaTicket {
  id: string;
  title: string;
  description?: string;
  status: string;
  priority?: string;
  labels?: string[];
  owner_human_id?: string;
  delegated_agent_id?: string;
  risk_level?: string;
  source?: string;
  created_at?: string;
  updated_at?: string;
  approval_policy?: string;
  analysis?: unknown;
  plan?: unknown;
}

export interface TeaEvent {
  id?: string;
  ticket_id?: string;
  kind?: string;
  message?: string;
  created_at?: string;
  actor?: unknown;
  payload?: unknown;
}

export interface TeaRun {
  id: string;
  ticket_id?: string;
  status?: string;
  created_at?: string;
  updated_at?: string;
  evidence?: unknown;
}

export interface TeaActorRef {
  kind?: string;
  id?: string;
}

export interface TeaComment {
  id: string;
  ticket_id?: string;
  actor?: TeaActorRef | string;
  body: string;
  created_at?: string;
}

export interface TeaAnalysis {
  intent?: string;
  target_components?: string[];
  target_paths?: string[];
  constraints?: string[];
  acceptance_criteria?: string[];
  missing_context?: string[];
  risk_assessment?: string;
  confidence?: number;
  recommended_policy?: string;
  recommended_workflow?: string;
}

export interface TeaPlanStep {
  id?: string;
  title?: string;
  description?: string;
}

export interface TeaPlan {
  summary?: string;
  steps?: TeaPlanStep[];
  required_tools?: string[];
  expected_artifacts?: string[];
  validation_strategy?: string[];
  rollback_strategy?: string[];
  requires_approval_before_execute?: boolean;
}

export interface TeaSnapshot {
  health: JsonObject | null;
  status: JsonObject | null;
  configuration: JsonObject | null;
  tickets: TeaTicket[];
  ticketsAvailable: boolean;
  error?: string;
}

export interface CreateTicketInput {
  title: string;
  description: string;
  approvalPolicy?: string;
  priority?: string;
  labels?: string[];
}

export interface TeaLocalConfig {
  notifications_enabled: boolean;
  human_ticket_default_approval_policy: string;
  hook_ticket_default_approval_policy: string;
}

export interface TeaClientOptions {
  serverUrl?: string;
  authToken?: string;
}

const PAGED_COLLECTION_LIMIT = 200;
const MAX_PAGED_COLLECTION_PAGES = 1_000;
const POLL_TIMEOUT_MS = 15_000;
const inFlightTicketBundleRequests = new Map<string, Promise<TeaTicketBundle>>();

type DecodedCollectionPage<T> =
  | { kind: "legacy"; items: T[] }
  | { kind: "paged"; items: T[]; nextCursor: string | null };

export async function resolveRuntimeConfig(): Promise<TeaRuntimeConfig> {
  return invoke<TeaRuntimeConfig>("resolve_tea_runtime_config");
}

export async function saveExport(fileName: string, content: string): Promise<string> {
  return invoke<string>("save_tea_export", { fileName, content });
}

async function requestJson<T>(
  method: string,
  path: string,
  body?: unknown,
  options?: TeaClientOptions,
  extras?: { idempotencyKey?: string; timeoutMs?: number },
): Promise<T> {
  return invoke<T>("tea_request", {
    method,
    path,
    body: body ?? null,
    baseUrl: options?.serverUrl ?? null,
    authToken: options?.authToken ?? null,
    ...(extras?.idempotencyKey ? { idempotencyKey: extras.idempotencyKey } : {}),
    ...(extras?.timeoutMs !== undefined ? { timeoutMs: extras.timeoutMs } : {}),
  });
}

function decodeCollectionPage<T>(value: unknown, legacyEnvelope?: string): DecodedCollectionPage<T> {
  if (Array.isArray(value)) return { items: value as T[], kind: "legacy" };
  if (typeof value !== "object" || value === null) {
    throw new Error("paged collection response must be an array or object envelope");
  }
  const object = value as Record<string, unknown>;
  if ("items" in object) {
    if (!Array.isArray(object.items)) {
      throw new Error("paged collection response field 'items' must be an array");
    }
    const cursor = object.next_cursor;
    if (cursor !== null && cursor !== undefined && (typeof cursor !== "string" || cursor.length === 0)) {
      throw new Error("paged collection next_cursor must be a non-empty string or null");
    }
    return {
      items: object.items as T[],
      kind: "paged",
      nextCursor: typeof cursor === "string" ? cursor : null,
    };
  }
  if (legacyEnvelope && Array.isArray(object[legacyEnvelope])) {
    return { items: object[legacyEnvelope] as T[], kind: "legacy" };
  }
  throw new Error("paged collection response is missing an items array");
}

async function readAllCollectionPages<T>(
  path: string,
  identityField: string,
  options?: TeaClientOptions,
  legacyEnvelope?: string,
  timeoutMs?: number,
): Promise<T[]> {
  const items: T[] = [];
  const identities = new Map<string, string>();
  const seenCursors = new Set<string>();
  let cursor: string | null = null;

  for (let pageNumber = 1; pageNumber <= MAX_PAGED_COLLECTION_PAGES; pageNumber += 1) {
    const pagePath: string = `${path}?limit=${PAGED_COLLECTION_LIMIT}${
      cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""
    }`;
    const page: DecodedCollectionPage<T> = decodeCollectionPage<T>(
      await requestJson<unknown>("GET", pagePath, undefined, options, { timeoutMs }),
      legacyEnvelope,
    );
    if (page.kind === "paged" && page.items.length > PAGED_COLLECTION_LIMIT) {
      throw new Error(
        `paged collection page ${pageNumber} exceeded the requested ${PAGED_COLLECTION_LIMIT} items`,
      );
    }

    for (const item of page.items) {
      if (typeof item !== "object" || item === null) {
        throw new Error("paged collection item must be an object");
      }
      const identity = (item as Record<string, unknown>)[identityField];
      if (typeof identity !== "string" || identity.length === 0) {
        throw new Error(`paged collection item is missing ${identityField}`);
      }
      const serialized = JSON.stringify(item);
      const previous = identities.get(identity);
      if (previous !== undefined) {
        if (previous !== serialized) {
          throw new Error(`paged collection returned conflicting records for ${identity}`);
        }
        continue;
      }
      identities.set(identity, serialized);
      items.push(item);
    }

    if (page.kind === "legacy" || page.nextCursor === null) return items;
    if (page.items.length === 0) {
      throw new Error(`paged collection page ${pageNumber} was empty but returned a cursor`);
    }
    if (seenCursors.has(page.nextCursor)) {
      throw new Error(`paged collection repeated cursor ${page.nextCursor}`);
    }
    seenCursors.add(page.nextCursor);
    cursor = page.nextCursor;
  }

  throw new Error(
    `paged collection exceeded ${MAX_PAGED_COLLECTION_PAGES} pages; refusing a partial result`,
  );
}

export async function readSnapshot(options?: TeaClientOptions): Promise<TeaSnapshot> {
  const poll = { timeoutMs: POLL_TIMEOUT_MS };
  const [healthResult, statusResult, configurationResult, ticketsResult] = await Promise.allSettled([
    requestJson<JsonObject>("GET", "/health", undefined, options, poll),
    requestJson<JsonObject>("GET", "/v1/status", undefined, options, poll),
    requestJson<JsonObject>("GET", "/v1/configuration", undefined, options, poll),
    readAllCollectionPages<TeaTicket>("/v1/tickets", "id", options, "tickets", POLL_TIMEOUT_MS),
  ]);
  const errors: string[] = [];
  const settledValue = <T>(
    label: string,
    result: PromiseSettledResult<T>,
    fallback: T,
  ): T => {
    if (result.status === "fulfilled") return result.value;
    const reason = result.reason instanceof Error ? result.reason.message : String(result.reason);
    errors.push(`${label}: ${reason}`);
    return fallback;
  };

  const health = settledValue<JsonObject | null>("health", healthResult, null);
  const status = settledValue<JsonObject | null>("status", statusResult, null);
  const configuration = settledValue<JsonObject | null>(
    "configuration",
    configurationResult,
    null,
  );
  const tickets = settledValue<TeaTicket[]>("tickets", ticketsResult, []);

  return {
    health,
    status,
    configuration,
    tickets,
    ticketsAvailable: ticketsResult.status === "fulfilled",
    error: errors.length > 0 ? errors.join("; ") : undefined,
  };
}

export async function createTicket(
  input: CreateTicketInput,
  options?: TeaClientOptions,
  idempotencyKey?: string,
): Promise<TeaTicket> {
  const body: JsonObject = {
    title: input.title,
    description: input.description,
  };
  if (input.approvalPolicy) {
    body.approval_policy = input.approvalPolicy;
  }
  if (input.priority && input.priority.trim()) {
    body.priority = input.priority.trim();
  }
  if (input.labels && input.labels.length > 0) {
    body.labels = input.labels;
  }
  return requestJson<TeaTicket>("POST", "/v1/tickets", body, options, { idempotencyKey });
}

export async function getTicket(id: string, options?: TeaClientOptions): Promise<TeaTicket> {
  return requestJson<TeaTicket>("GET", `/v1/tickets/${encodeURIComponent(id)}`, undefined, options);
}

export interface UpdateTicketInput {
  title?: string;
  description?: string;
  priority?: string;
  labels?: string[];
}

export async function updateTicket(
  id: string,
  input: UpdateTicketInput,
  options?: TeaClientOptions,
): Promise<TeaTicket> {
  const body: JsonObject = {};
  if (input.title !== undefined) {
    body.title = input.title;
  }
  if (input.description !== undefined) {
    body.description = input.description;
  }
  if (input.priority !== undefined) {
    body.priority = input.priority;
  }
  if (input.labels !== undefined) {
    body.labels = input.labels;
  }
  return requestJson<TeaTicket>(
    "PATCH",
    `/v1/tickets/${encodeURIComponent(id)}`,
    body,
    options,
  );
}

export async function listEvents(id: string, options?: TeaClientOptions): Promise<TeaEvent[]> {
  return requestJson<TeaEvent[]>(
    "GET",
    `/v1/tickets/${encodeURIComponent(id)}/events`,
    undefined,
    options,
  );
}

export async function listRuns(id: string, options?: TeaClientOptions): Promise<TeaRun[]> {
  return requestJson<TeaRun[]>(
    "GET",
    `/v1/tickets/${encodeURIComponent(id)}/runs`,
    undefined,
    options,
  );
}

export async function getAnalysis(
  id: string,
  options?: TeaClientOptions,
): Promise<TeaAnalysis | null> {
  return requestJson<TeaAnalysis | null>(
    "GET",
    `/v1/tickets/${encodeURIComponent(id)}/analysis`,
    undefined,
    options,
  );
}

export async function getPlan(id: string, options?: TeaClientOptions): Promise<TeaPlan | null> {
  return requestJson<TeaPlan | null>(
    "GET",
    `/v1/tickets/${encodeURIComponent(id)}/plan`,
    undefined,
    options,
  );
}

export async function listComments(id: string, options?: TeaClientOptions): Promise<TeaComment[]> {
  return requestJson<TeaComment[]>(
    "GET",
    `/v1/tickets/${encodeURIComponent(id)}/comments`,
    undefined,
    options,
  );
}

export async function addComment(
  id: string,
  body: string,
  options?: TeaClientOptions,
): Promise<TeaComment> {
  return requestJson<TeaComment>(
    "POST",
    `/v1/tickets/${encodeURIComponent(id)}/comments`,
    { body },
    options,
  );
}

export async function ticketAction(
  id: string,
  action:
    | "analyze"
    | "plan"
    | "decompose"
    | "approve"
    | "reject"
    | "run"
    | "accept"
    | "close"
    | "cancel"
    | "stop"
    | "retry",
  options?: TeaClientOptions,
): Promise<unknown> {
  return requestJson<unknown>(
    "POST",
    `/v1/tickets/${encodeURIComponent(id)}/${action}`,
    {},
    options,
  );
}

export async function rejectTicket(
  id: string,
  reason: string,
  options?: TeaClientOptions,
): Promise<unknown> {
  return requestJson<unknown>(
    "POST",
    `/v1/tickets/${encodeURIComponent(id)}/reject`,
    { reason },
    options,
  );
}

export async function setTicketPolicy(
  id: string,
  mode: string,
  options?: TeaClientOptions,
): Promise<unknown> {
  return requestJson<unknown>(
    "POST",
    `/v1/tickets/${encodeURIComponent(id)}/policy`,
    { mode },
    options,
  );
}

export async function stopRun(runId: string, options?: TeaClientOptions): Promise<TeaRun> {
  return requestJson<TeaRun>(
    "POST",
    `/v1/runs/${encodeURIComponent(runId)}/stop`,
    {},
    options,
  );
}

export async function retryRun(runId: string, options?: TeaClientOptions): Promise<TeaRun> {
  return requestJson<TeaRun>(
    "POST",
    `/v1/runs/${encodeURIComponent(runId)}/retry`,
    {},
    options,
  );
}

export async function updateConfiguration(
  config: Partial<TeaLocalConfig>,
  options?: TeaClientOptions,
): Promise<JsonObject> {
  return requestJson<JsonObject>("PATCH", "/v1/configuration", config, options);
}

export async function exportTicket(
  id: string,
  format: "json" | "markdown",
  options?: TeaClientOptions,
): Promise<unknown> {
  return requestJson<unknown>(
    "GET",
    `/v1/tickets/${encodeURIComponent(id)}/export/${format}`,
    undefined,
    options,
  );
}

export interface TeaIssueMetric {
  ticket_id: string;
  comments_count: number;
  runs_count: number;
  latest_comment: TeaComment | null;
  latest_event: TeaEvent | null;
}

/**
 * Aggregated per-ticket metrics for the whole list in a single request.
 * Replaces the previous per-ticket fan-out (comments+runs+events for every ticket).
 */
export async function getIssueMetrics(options?: TeaClientOptions): Promise<TeaIssueMetric[]> {
  return readAllCollectionPages<TeaIssueMetric>(
    "/v1/tickets/metrics",
    "ticket_id",
    options,
    undefined,
    POLL_TIMEOUT_MS,
  );
}

export interface TeaTicketBundle {
  ticket: TeaTicket;
  comments: TeaComment[];
  events: TeaEvent[];
  runs: TeaRun[];
  analysis: TeaAnalysis | null;
  plan: TeaPlan | null;
}

/**
 * Full ticket detail (ticket + comments + events + runs + analysis + plan) in one
 * request, replacing the previous six-call fan-out on every ticket selection.
 */
export async function getTicketBundle(
  id: string,
  options?: TeaClientOptions,
): Promise<TeaTicketBundle> {
  // Selection changes and snapshot refreshes can ask for the same detail at
  // the same time. Share only the in-flight request; completed responses are
  // never cached, so every later refresh still observes current daemon state.
  const requestKey = JSON.stringify([options?.serverUrl ?? null, options?.authToken ?? null, id]);
  const existing = inFlightTicketBundleRequests.get(requestKey);
  if (existing) return existing;

  const request = requestJson<TeaTicketBundle>(
    "GET",
    `/v1/tickets/${encodeURIComponent(id)}/bundle`,
    undefined,
    options,
    { timeoutMs: POLL_TIMEOUT_MS },
  );
  inFlightTicketBundleRequests.set(requestKey, request);
  try {
    return await request;
  } finally {
    if (inFlightTicketBundleRequests.get(requestKey) === request) {
      inFlightTicketBundleRequests.delete(requestKey);
    }
  }
}
