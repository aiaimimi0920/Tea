// @vitest-environment happy-dom
import { act, createElement, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ReviewDraftScope, ReviewDraftSubmission } from "./reviewDraft";
const { invokeMock, callbacks } = vi.hoisted(() => ({ invokeMock: vi.fn(), callbacks: {} as Record<string, {
  scope: ReviewDraftScope; submit: (value: ReviewDraftSubmission) => Promise<boolean>;
}> }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("./ReviewForms", async (original) => {
  const forms = await original<typeof import("./ReviewForms")>();
  return {
    ...forms,
    CommentEditor: (props: ComponentProps<typeof forms.CommentEditor>) => {
      callbacks.comment = { scope: props.scope, submit: props.onSubmit };
      return createElement(forms.CommentEditor, props);
    },
    RejectReasonForm: (props: ComponentProps<typeof forms.RejectReasonForm>) => {
      callbacks.rejection = { scope: props.scope, submit: props.onReject };
      return createElement(forms.RejectReasonForm, props);
    },
  };
});
import App from "./App";
import { setLocale } from "./i18n";
import type { TeaTicket } from "./teaClient";

let root: Root;
let container: HTMLDivElement;
let tickets: TeaTicket[];
let failSave: boolean;
let reviews: { id: string; action: string; body: Record<string, unknown> }[];

function button(text: string, scope: ParentNode = container): HTMLButtonElement {
  const matches = [...scope.querySelectorAll("button")].filter((node) => node.textContent?.trim() === text);
  if (matches.length !== 1) throw new Error(`Expected one button ${text}, found ${matches.length}`);
  return matches[0];
}
async function click(node: HTMLElement) { await act(async () => { node.click(); }); }
async function select(title: string) {
  const row = [...container.querySelectorAll<HTMLButtonElement>(".issue-item")]
    .find((node) => node.textContent?.includes(title));
  if (!row) throw new Error(`Missing ticket ${title}`);
  await click(row);
}
async function fillReview(selector: string, value: string) {
  const input = container.querySelector<HTMLTextAreaElement>(`${selector} textarea`)!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear(); localStorage.setItem("tea.autoRefreshEnabled", "false"); setLocale("en");
  tickets = ["A", "B"].map((id) => ({ id, title: `Issue ${id}`, description: `Description ${id}`,
    status: "open", priority: "normal", labels: ["source:human", "initial"], approval_policy: "human_before_execute" }));
  reviews = []; failSave = false;
  invokeMock.mockImplementation(async (command: string, args: { path: string; method: string; body: Record<string, unknown> }) => {
    if (command === "resolve_tea_runtime_config") return { serverUrl: "http://127.0.0.1:48910", authConfigured: true };
    const { path, method, body } = args;
    if (path === "/health") return { status: "ok" };
    if (path === "/v1/status") return { execution_provider: "mock", configuration_source: "local" };
    if (path === "/v1/configuration") return { configuration_source: "local", config: { notifications_enabled: true } };
    if (path.startsWith("/v1/tickets/metrics")) return [];
    if (path.startsWith("/v1/tickets?")) return structuredClone(tickets);
    const id = path.split("/")[3]; const ticket = tickets.find((item) => item.id === id);
    if (path.endsWith("/bundle")) return structuredClone({ ticket, comments: [], events: [], runs: [], analysis: null, plan: null });
    if (method === "POST" && ticket) {
      if (failSave) throw new Error("Synthetic review failure");
      reviews.push({ id, action: path.split("/")[4], body });
      return {};
    }

    throw new Error(`Unexpected fixture request ${method} ${path}`);
  });
  container = document.createElement("div"); document.body.append(container);
  root = createRoot(container); await act(async () => { root.render(createElement(App)); });
});
afterEach(async () => { await act(async () => { root.unmount(); }); container.remove(); vi.unstubAllGlobals(); invokeMock.mockReset(); });

describe.each([
  { name: "comment", selector: ".comment-editor", action: "comments", field: "body" },
  { name: "rejection", selector: ".reject-reason-form", action: "reject", field: "reason" },
])("$name draft identity", ({ name, selector, action, field }) => {
  it("does not transfer A's review draft to B", async () => {
    await select("Issue A"); await fillReview(selector, "Private review of A");
    await select("Issue B");
    expect(container.querySelector<HTMLTextAreaElement>(`${selector} textarea`)!.value).toBe("");
    await fillReview(selector, "Intentional review of B");
    await click(container.querySelector<HTMLButtonElement>(`${selector} button[type=submit]`)!);
    expect(reviews).toEqual([{ id: "B", action, body: { [field]: "Intentional review of B" } }]);
  });
  it("rejects a stale callback after task or connection changes", async () => {
    await select("Issue A"); const stale = callbacks[name]; await select("Issue B");
    expect(await stale.submit({ scope: stale.scope, text: "Stale task review" })).toBe(false);
    await select("Issue A"); const beforeConnection = callbacks[name]; await click(button("Settings"));
    const endpoint = container.querySelector<HTMLInputElement>(".connection-strip input")!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(endpoint, "http://127.0.0.1:48911");
      endpoint.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => { endpoint.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })); });
    expect(await beforeConnection.submit({ scope: beforeConnection.scope, text: "Stale daemon review" })).toBe(false);
    expect(reviews).toEqual([]);
  });
  it("rejects callbacks from an earlier visit to the same task", async () => {
    await select("Issue A"); const stale = callbacks[name];
    await select("Issue B"); await select("Issue A");
    let result: boolean | undefined;
    await act(async () => { result = await stale.submit({ scope: stale.scope, text: "Old visit draft" }); });
    expect(result).toBe(false); expect(reviews).toEqual([]);
  });
  it("retains a failed review draft for the same work order", async () => {
    await select("Issue A"); await fillReview(selector, "Retry this review"); failSave = true;
    await click(container.querySelector<HTMLButtonElement>(`${selector} button[type=submit]`)!);
    expect(container.querySelector<HTMLTextAreaElement>(`${selector} textarea`)!.value).toBe("Retry this review");
    expect(reviews).toEqual([]);
  });
});
