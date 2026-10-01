// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
import App from "./App";
import { setLocale } from "./i18n";
import type { TeaTicket } from "./teaClient";

let root: Root;
let container: HTMLDivElement;
let tickets: TeaTicket[];
let patches: { id: string; body: Record<string, unknown> }[];
let failSave: boolean;

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
function field(label: string): HTMLInputElement | HTMLTextAreaElement {
  const input = [...container.querySelectorAll(".issue-edit-form label")]
    .find((node) => node.querySelector("span")?.textContent === label)?.querySelector("input, textarea");
  if (!input) throw new Error(`Missing editor field ${label}`);
  return input as HTMLInputElement | HTMLTextAreaElement;
}
async function fill(label: string, value: string) {
  const input = field(label);
  await act(async () => {
    const prototype = input instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, "value")!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}
async function edit(title = "Issue A") { await select(title); await click(button("Edit issue")); }

beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear(); localStorage.setItem("tea.autoRefreshEnabled", "false"); setLocale("en");
  tickets = ["A", "B"].map((id) => ({ id, title: `Issue ${id}`, description: `Description ${id}`,
    status: "open", priority: "normal", labels: ["source:human", "initial"], approval_policy: "human_before_execute" }));
  patches = []; failSave = false;
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
    if (method === "PATCH" && ticket) {
      if (failSave) throw new Error("Synthetic save failure");
      patches.push({ id, body }); Object.assign(ticket, body); return structuredClone(ticket);
    }
    throw new Error(`Unexpected fixture request ${method} ${path}`);
  });
  container = document.createElement("div"); document.body.append(container);
  root = createRoot(container); await act(async () => { root.render(createElement(App)); });
});
afterEach(async () => { await act(async () => { root.unmount(); }); container.remove(); vi.unstubAllGlobals(); invokeMock.mockReset(); });

describe("editor sessions protect work-order identity and untouched fields", () => {
  it("closes A's draft when switching to B instead of editing the wrong work order", async () => {
    await edit(); await fill("Title", "Unsaved A draft"); await select("Issue B");
    expect(container.querySelector(".issue-edit-form")).toBeNull();
    await click(button("Edit issue")); expect(field("Title").value).toBe("Issue B");
    await fill("Title", "Intentional B title"); await click(button("Save changes"));
    expect(patches).toEqual([{ id: "B", body: { title: "Intentional B title" } }]);
    expect(tickets[0].title).toBe("Issue A");
  });
  it("does not overwrite remotely refreshed fields the operator never edited", async () => {
    await edit(); await fill("Title", "Intentional title");
    Object.assign(tickets[0], { description: "Remote description", priority: "high", labels: ["source:human", "remote"] });
    await click(button("Refresh")); await click(button("Save changes"));
    expect(patches).toEqual([{ id: "A", body: { title: "Intentional title" } }]);
    expect(tickets[0].description).toBe("Remote description");
  });
  it("sends no patch when an untouched draft is saved after refresh", async () => {
    await edit(); tickets[0].description = "Remote description"; await click(button("Refresh"));
    await click(button("Save changes")); expect(patches).toEqual([]);
  });
  it("discards the editor when its daemon connection changes", async () => {
    await edit(); await fill("Title", "Old daemon draft"); await click(button("Settings"));
    const endpoint = container.querySelector<HTMLInputElement>(".connection-strip input")!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(endpoint, "http://127.0.0.1:48911");
      endpoint.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => { endpoint.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })); });
    expect(container.querySelector(".issue-edit-form")).toBeNull();
    expect(patches).toEqual([]);
  });
  it("does not let an older save close a newly opened editor session", async () => {
    await edit(); await fill("Title", "Saved A title");
    let release: (() => void) | undefined;
    const previous = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (command, args) => {
      if (args?.method === "PATCH") await new Promise<void>((resolve) => { release = resolve; });
      return previous(command, args);
    });
    await click(button("Save changes"));
    await click(button("Cancel edit")); await click(button("Edit issue"));
    await fill("Title", "New unsaved draft");
    await act(async () => { release!(); });
    expect(field("Title").value).toBe("New unsaved draft");
    expect(patches).toEqual([{ id: "A", body: { title: "Saved A title" } }]);
  });
  it("keeps a failed draft and takes a fresh baseline after cancel/reopen", async () => {
    await edit(); await fill("Title", "Retry this title"); failSave = true;
    await click(button("Save changes")); expect(field("Title").value).toBe("Retry this title");
    failSave = false; await click(button("Cancel", container.querySelector(".issue-edit-form")!)); tickets[0].description = "Latest description";
    await click(button("Refresh")); await click(button("Edit issue"));
    expect(field("Description").value).toBe("Latest description");
    await click(button("Save changes")); expect(patches).toEqual([]);
  });
});
