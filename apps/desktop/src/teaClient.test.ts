import { beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

import {
  createTicket,
  getIssueMetrics,
  getTicket,
  getTicketBundle,
  readSnapshot,
  retryRun,
  setTicketPolicy,
  stopRun,
  ticketAction,
  updateConfiguration,
  updateTicket,
} from "./teaClient";

describe("teaClient", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("returns successful snapshot sections when other requests fail", async () => {
    invokeMock.mockImplementation((_command: string, arguments_: { path: string }) => {
      switch (arguments_.path) {
        case "/health":
          return Promise.resolve({ status: "ok" });
        case "/v1/status":
          return Promise.reject(new Error("status unavailable"));
        case "/v1/configuration":
          return Promise.reject("configuration denied");
        case "/v1/tickets?limit=200":
          return Promise.resolve([{ id: "ticket-1", status: "open", title: "First" }]);
        default:
          return Promise.reject(new Error(`unexpected path: ${arguments_.path}`));
      }
    });

    const snapshot = await readSnapshot({
      authToken: "test-token",
      serverUrl: "http://127.0.0.1:48910",
    });

    expect(snapshot).toEqual({
      health: { status: "ok" },
      status: null,
      configuration: null,
      tickets: [{ id: "ticket-1", status: "open", title: "First" }],
      ticketsAvailable: true,
      error: "status: status unavailable; configuration: configuration denied",
    });
    expect(invokeMock).toHaveBeenCalledTimes(4);
    expect(invokeMock.mock.calls.every(([, arguments_]) => arguments_.timeoutMs === 15000)).toBe(
      true,
    );
  });

  it("marks the ticket collection unavailable instead of treating a failure as empty", async () => {
    invokeMock.mockImplementation((_command: string, arguments_: { path: string }) => {
      if (arguments_.path === "/v1/tickets?limit=200") {
        return Promise.reject(new Error("ticket store unavailable"));
      }
      return Promise.resolve({ status: "ok" });
    });

    const snapshot = await readSnapshot();

    expect(snapshot.tickets).toEqual([]);
    expect(snapshot.ticketsAvailable).toBe(false);
    expect(snapshot.error).toContain("tickets: ticket store unavailable");
  });

  it("collects cursor pages for tickets and metrics", async () => {
    invokeMock.mockImplementation((_command: string, arguments_: { path: string }) => {
      switch (arguments_.path) {
        case "/health":
          return Promise.resolve({ status: "ok" });
        case "/v1/status":
          return Promise.resolve({ status: "ok" });
        case "/v1/configuration":
          return Promise.resolve({ configuration_source: "local" });
        case "/v1/tickets?limit=200":
          return Promise.resolve({
            items: [{ id: "ticket-1", status: "open", title: "First" }],
            next_cursor: "v1-0000000000000000-0-0",
          });
        case "/v1/tickets?limit=200&cursor=v1-0000000000000000-0-0":
          return Promise.resolve({
            items: [{ id: "ticket-2", status: "open", title: "Second" }],
            next_cursor: null,
          });
        case "/v1/tickets/metrics?limit=200":
          return Promise.resolve({
            items: [
              {
                comments_count: 1,
                latest_comment: null,
                latest_event: null,
                runs_count: 0,
                ticket_id: "ticket-1",
              },
            ],
            next_cursor: "v1-0000000000000000-0-0",
          });
        case "/v1/tickets/metrics?limit=200&cursor=v1-0000000000000000-0-0":
          return Promise.resolve({
            items: [
              {
                comments_count: 0,
                latest_comment: null,
                latest_event: null,
                runs_count: 2,
                ticket_id: "ticket-2",
              },
            ],
            next_cursor: null,
          });
        default:
          return Promise.reject(new Error(`unexpected path: ${arguments_.path}`));
      }
    });

    const snapshot = await readSnapshot();
    const metrics = await getIssueMetrics();

    expect(snapshot.tickets.map((ticket) => ticket.id)).toEqual(["ticket-1", "ticket-2"]);
    expect(metrics.map((metric) => metric.ticket_id)).toEqual(["ticket-1", "ticket-2"]);
    expect(invokeMock.mock.calls.map((call) => call[1].path)).toContain(
      "/v1/tickets?limit=200&cursor=v1-0000000000000000-0-0",
    );
    expect(invokeMock.mock.calls.every(([, arguments_]) => arguments_.timeoutMs === 15000)).toBe(
      true,
    );
  });

  it("rejects a repeated collection cursor instead of returning a partial list", async () => {
    invokeMock.mockImplementation((_command: string, arguments_: { path: string }) => {
      if (arguments_.path === "/v1/tickets/metrics?limit=200") {
        return Promise.resolve({
          items: [
            {
              comments_count: 0,
              latest_comment: null,
              latest_event: null,
              runs_count: 0,
              ticket_id: "ticket-1",
            },
          ],
          next_cursor: "repeated",
        });
      }
      return Promise.resolve({
        items: [
          {
            comments_count: 0,
            latest_comment: null,
            latest_event: null,
            runs_count: 0,
            ticket_id: "ticket-2",
          },
        ],
        next_cursor: "repeated",
      });
    });

    await expect(getIssueMetrics()).rejects.toThrow("repeated cursor");
  });

  it("omits blank optional fields and forwards create idempotency", async () => {
    invokeMock.mockResolvedValue({ id: "ticket-2", status: "open", title: "Second" });

    await createTicket(
      {
        approvalPolicy: "",
        description: "Description",
        labels: [],
        priority: "   ",
        title: "Second",
      },
      undefined,
      "desktop-create-1",
    );

    expect(invokeMock).toHaveBeenCalledOnce();
    expect(invokeMock).toHaveBeenCalledWith("tea_request", {
      authToken: null,
      baseUrl: null,
      body: {
        description: "Description",
        title: "Second",
      },
      method: "POST",
      path: "/v1/tickets",
      idempotencyKey: "desktop-create-1",
    });
  });

  it("encodes ticket identifiers and preserves explicit empty update fields", async () => {
    invokeMock.mockResolvedValue({ id: "ticket", status: "open", title: "Ticket" });

    await getTicket("a/b ?");
    await updateTicket(
      "a/b ?",
      {
        description: undefined,
        labels: [],
        priority: " ",
        title: "",
      },
      { authToken: "token", serverUrl: "http://tea.test" },
    );

    expect(invokeMock).toHaveBeenNthCalledWith(1, "tea_request", {
      authToken: null,
      baseUrl: null,
      body: null,
      method: "GET",
      path: "/v1/tickets/a%2Fb%20%3F",
    });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "tea_request", {
      authToken: "token",
      baseUrl: "http://tea.test",
      body: {
        labels: [],
        priority: " ",
        title: "",
      },
      method: "PATCH",
      path: "/v1/tickets/a%2Fb%20%3F",
    });

    invokeMock.mockResolvedValue({
      analysis: null,
      comments: [],
      events: [],
      plan: null,
      runs: [],
      ticket: { id: "ticket", status: "open", title: "Ticket" },
    });
    await getTicketBundle("a/b ?");
    expect(invokeMock).toHaveBeenLastCalledWith("tea_request", {
      authToken: null,
      baseUrl: null,
      body: null,
      method: "GET",
      path: "/v1/tickets/a%2Fb%20%3F/bundle",
      timeoutMs: 15000,
    });
  });

  it("shares an in-flight ticket bundle request for the same ticket and options", async () => {
    let resolveBundle: (value: unknown) => void = () => {};
    const pending = new Promise<unknown>((resolve) => {
      resolveBundle = resolve;
    });
    invokeMock.mockReturnValue(pending);
    const options = { authToken: "token", serverUrl: "http://tea.test" };

    const first = getTicketBundle("ticket-1", options);
    const second = getTicketBundle("ticket-1", options);

    expect(invokeMock).toHaveBeenCalledOnce();
    resolveBundle({
      analysis: null,
      comments: [],
      events: [],
      plan: null,
      runs: [],
      ticket: { id: "ticket-1", status: "open", title: "Ticket" },
    });

    const [firstBundle, secondBundle] = await Promise.all([first, second]);
    expect(firstBundle).toEqual(secondBundle);
    expect(invokeMock).toHaveBeenCalledOnce();
  });

  it("removes failed ticket bundle requests so a later read can retry", async () => {
    invokeMock.mockRejectedValueOnce(new Error("temporary failure"));
    await expect(getTicketBundle("ticket-2")).rejects.toThrow("temporary failure");

    invokeMock.mockResolvedValueOnce({
      analysis: null,
      comments: [],
      events: [],
      plan: null,
      runs: [],
      ticket: { id: "ticket-2", status: "open", title: "Ticket" },
    });
    await expect(getTicketBundle("ticket-2")).resolves.toMatchObject({
      ticket: { id: "ticket-2" },
    });

    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it("keeps lifecycle and run action payloads aligned with the daemon API", async () => {
    invokeMock.mockResolvedValue({ id: "run", status: "running" });
    const options = { authToken: "token", serverUrl: "http://tea.test" };

    await ticketAction("ticket/1", "close", options);
    await setTicketPolicy("ticket/1", "manual_only", options);
    await stopRun("run/1", options);
    await retryRun("run/1", options);

    expect(invokeMock.mock.calls).toEqual([
      [
        "tea_request",
        {
          authToken: "token",
          baseUrl: "http://tea.test",
          body: {},
          method: "POST",
          path: "/v1/tickets/ticket%2F1/close",
        },
      ],
      [
        "tea_request",
        {
          authToken: "token",
          baseUrl: "http://tea.test",
          body: { mode: "manual_only" },
          method: "POST",
          path: "/v1/tickets/ticket%2F1/policy",
        },
      ],
      [
        "tea_request",
        {
          authToken: "token",
          baseUrl: "http://tea.test",
          body: {},
          method: "POST",
          path: "/v1/runs/run%2F1/stop",
        },
      ],
      [
        "tea_request",
        {
          authToken: "token",
          baseUrl: "http://tea.test",
          body: {},
          method: "POST",
          path: "/v1/runs/run%2F1/retry",
        },
      ],
    ]);
  });

  it("patches only the supplied local configuration fields without renaming them", async () => {
    invokeMock.mockResolvedValue({ configuration_source: "local" });
    const config = {
      notifications_enabled: false,
    };

    await updateConfiguration(config, {
      authToken: "token",
      serverUrl: "http://tea.test",
    });

    expect(invokeMock).toHaveBeenCalledWith("tea_request", {
      authToken: "token",
      baseUrl: "http://tea.test",
      body: config,
      method: "PATCH",
      path: "/v1/configuration",
    });
  });
});
