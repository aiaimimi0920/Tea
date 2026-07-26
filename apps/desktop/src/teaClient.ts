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
): Promise<T> {
  return invoke<T>("tea_request", {
    method,
    path,
    body: body ?? null,
    baseUrl: options?.serverUrl ?? null,
    authToken: options?.authToken ?? null,
  });
}

export async function readSnapshot(options?: TeaClientOptions): Promise<TeaSnapshot> {
  const [health, status, configuration, tickets] = await Promise.all([
    requestJson<JsonObject>("GET", "/health", undefined, options).catch(() => null),
    requestJson<JsonObject>("GET", "/v1/status", undefined, options).catch(() => null),
    requestJson<JsonObject>("GET", "/v1/configuration", undefined, options).catch(() => null),
    requestJson<TeaTicket[]>("GET", "/v1/tickets", undefined, options).catch(() => []),
  ]);

  return {
    health,
    status,
    configuration,
    tickets,
  };
}

export async function createTicket(
  input: CreateTicketInput,
  options?: TeaClientOptions,
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
  return requestJson<TeaTicket>("POST", "/v1/tickets", body, options);
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
  config: TeaLocalConfig,
  options?: TeaClientOptions,
): Promise<JsonObject> {
  return requestJson<JsonObject>("PUT", "/v1/configuration", config, options);
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
  return requestJson<TeaIssueMetric[]>("GET", "/v1/tickets/metrics", undefined, options);
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
  return requestJson<TeaTicketBundle>(
    "GET",
    `/v1/tickets/${encodeURIComponent(id)}/bundle`,
    undefined,
    options,
  );
}
