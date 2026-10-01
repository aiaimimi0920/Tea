// @vitest-environment happy-dom
import { act, createElement, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const { invokeMock, captured } = vi.hoisted(() => ({ invokeMock: vi.fn(), captured: {} as {
  settings: ComponentProps<typeof import("./TeaSettingsPanel").TeaSettingsPanel>;
} }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("./TeaSettingsPanel", async (original) => {
  const actual = await original<typeof import("./TeaSettingsPanel")>();
  return { TeaSettingsPanel: (props: ComponentProps<typeof actual.TeaSettingsPanel>) => {
    captured.settings = props;
    return createElement(actual.TeaSettingsPanel, props);
  } };
});
import App from "./App";
import { setLocale } from "./i18n";
import type { TeaLocalConfig, TeaTicket } from "./teaClient";

let root: Root;
let container: HTMLDivElement;
let tickets: TeaTicket[];
let config: TeaLocalConfig;
let source: string;
let writes: Record<string, unknown>[];
let finishSave: (() => void) | undefined;
let deferSave: boolean;
let writeConnections: (string | undefined)[];
function button(text: string, scope: ParentNode = container): HTMLButtonElement {
  const matches = [...scope.querySelectorAll("button")].filter((node) => node.textContent?.trim() === text);
  if (matches.length !== 1) throw new Error(`Expected one ${text} button, found ${matches.length}`);
  return matches[0];
}
async function click(node: HTMLElement) { await act(async () => { node.click(); }); }
const toggle = () => container.querySelector<HTMLInputElement>(".settings-config-editor input[type=checkbox]")!;

beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear(); localStorage.setItem("tea.autoRefreshEnabled", "false"); setLocale("en");
  tickets = []; writes = []; source = "local"; deferSave = false; finishSave = undefined; writeConnections = [];
  config = { notifications_enabled: true, human_ticket_default_approval_policy: "human_before_execute", hook_ticket_default_approval_policy: "plan_only" };
  invokeMock.mockImplementation(async (command: string, args: { path: string; method: string; body: Record<string, unknown>; baseUrl?: string }) => {
    if (command === "resolve_tea_runtime_config") return { serverUrl: "http://127.0.0.1:48910", authConfigured: true };
    const { path, method, body } = args;
    if (path === "/health") return { status: "ok" };
    if (path === "/v1/status") return { execution_provider: "mock", configuration_source: source };
    if (path === "/v1/configuration") {
      if (method === "PATCH") {
        writes.push(body); writeConnections.push(args.baseUrl);
        if (deferSave) await new Promise<void>((resolve) => { finishSave = resolve; });
        config = { ...config, ...body };
      }
      return { configuration_source: source, config: { ...config }, configuration: { loom_panel_url: "loom://settings/tea" } };
    }
    if (path.startsWith("/v1/tickets/metrics")) return [];
    if (path.startsWith("/v1/tickets?")) return structuredClone(tickets);
    if (path.endsWith("/bundle")) return { ticket: tickets[0], comments: [], events: [], runs: [], analysis: null, plan: null };
    throw new Error(`Unexpected fixture request ${method} ${path}`);
  });
  container = document.createElement("div"); document.body.append(container);
  root = createRoot(container); await act(async () => { root.render(createElement(App)); });
});
afterEach(async () => { await act(async () => { root.unmount(); }); container.remove(); vi.unstubAllGlobals(); invokeMock.mockReset(); });

it("opens local settings before the first ticket without writing configuration", async () => {
  await click(button("Settings"));
  expect(toggle()).not.toBeNull();
  expect(toggle().checked).toBe(true);
  expect(button("Save Tea settings").disabled).toBe(true);
  expect(writes).toEqual([]);
});

it("discards interrupted edits on navigation and saves only an explicitly changed field", async () => {
  await click(button("Settings")); await click(toggle());
  await click(container.querySelector<HTMLButtonElement>(".repo-tabs button")!); await click(button("Settings"));
  expect(toggle().checked).toBe(true); expect(writes).toEqual([]);
  await click(toggle()); await click(button("Reset changes"));
  expect(toggle().checked).toBe(true); expect(writes).toEqual([]);
  await click(toggle()); await click(button("Save Tea settings"));
  expect(writes).toEqual([{ notifications_enabled: false }]);
  expect(button("Save Tea settings").disabled).toBe(true);
});

it("continues to show settings when the first work order appears", async () => {
  await click(button("Settings"));
  tickets = [{ id: "first", title: "First work order", description: "First task", status: "open" }];
  await click(button("Refresh", container.querySelector(".refresh-control")!));
  expect(toggle()).not.toBeNull(); expect(writes).toEqual([]);
  await click(container.querySelector<HTMLButtonElement>(".repo-tabs button")!);
  expect(container.querySelector(".issue-titlebar")?.textContent).toContain("First work order");
  await click(button("Settings")); expect(toggle()).not.toBeNull();
});

it("honors Loom ownership in an empty workspace", async () => {
  source = "loom-managed";
  await click(button("Refresh", container.querySelector(".refresh-control")!));
  await click(button("Settings"));
  expect(toggle()).toBeNull();
  expect(container.querySelector(".loom-settings-link")?.getAttribute("href")).toBe("loom://settings/tea");
  expect(writes).toEqual([]);
});

it("refreshes connection settings without a write when switching daemons", async () => {
  await click(button("Settings"));
  const endpoint = container.querySelector<HTMLInputElement>(".connection-strip input")!;
  config = { ...config, notifications_enabled: false };
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(endpoint, "http://127.0.0.1:48911");
    endpoint.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => { endpoint.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })); });
  expect(toggle().checked).toBe(false); expect(writes).toEqual([]);
  expect(button("Save Tea settings").disabled).toBe(true);
});

it("drops an interrupted draft when switching to an equally configured daemon", async () => {
  await click(button("Settings")); await click(toggle());
  const endpoint = container.querySelector<HTMLInputElement>(".connection-strip input")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(endpoint, "http://127.0.0.1:48912");
    endpoint.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => { endpoint.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })); });
  expect(toggle().checked).toBe(true); expect(writes).toEqual([]);
  expect(button("Save Tea settings").disabled).toBe(true);
});

async function switchConnection(url: string) {
  const endpoint = container.querySelector<HTMLInputElement>(".connection-strip input")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(endpoint, url);
    endpoint.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => { endpoint.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })); });
}

it("rejects a previous connection's save callback even after returning to that URL", async () => {
  await click(button("Settings")); const stale = captured.settings;
  await switchConnection("http://127.0.0.1:48911");
  await switchConnection("http://127.0.0.1:48910");
  await act(async () => { stale.onSaveConfiguration({ notifications_enabled: false }, stale.connection); });
  expect(writes).toEqual([]); expect(toggle().checked).toBe(true);
});

it("keeps an in-flight save bound to its original daemon across navigation", async () => {
  await click(button("Settings")); deferSave = true;
  await click(toggle()); await click(button("Save Tea settings"));
  expect(writes).toEqual([{ notifications_enabled: false }]);
  await switchConnection("http://127.0.0.1:48911");
  await switchConnection("http://127.0.0.1:48910");
  expect(toggle().disabled).toBe(true);
  await act(async () => { finishSave!(); });
  expect(writeConnections).toEqual(["http://127.0.0.1:48910"]);
  expect(writes).toHaveLength(1); expect(toggle().disabled).toBe(false);
  await click(toggle());
  expect(button("Save Tea settings").disabled).toBe(false);
});
