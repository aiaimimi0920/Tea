import { beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import { getTicketBundle, ticketAction, updateTicket, type TeaTicketBundle } from "./teaClient";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((settle) => { resolve = settle; });
  return { promise, resolve };
}

function bundle(title: string): TeaTicketBundle {
  return {
    ticket: { id: "ticket-1", title, status: "open" },
    comments: [], events: [], runs: [], analysis: null, plan: null,
  };
}

describe("detail refresh after a mutation", () => {
  beforeEach(() => { invokeMock.mockReset(); });

  it("does not reuse a pre-edit read or let its cleanup remove the new read", async () => {
    const old = deferred<TeaTicketBundle>();
    const fresh = deferred<TeaTicketBundle>();
    invokeMock.mockReturnValueOnce(old.promise).mockResolvedValueOnce({})
      .mockReturnValueOnce(fresh.promise);

    const beforeEdit = getTicketBundle("ticket-1");
    await updateTicket("ticket-1", { title: "Edited" });
    const afterEdit = getTicketBundle("ticket-1");
    const requestsAfterEdit = invokeMock.mock.calls.length;

    old.resolve(bundle("Old"));
    await beforeEdit;
    const concurrentRefresh = getTicketBundle("ticket-1");
    const requestsAfterOldCleanup = invokeMock.mock.calls.length;
    fresh.resolve(bundle("Edited"));
    expect(await afterEdit).toEqual(bundle("Edited"));
    expect(await concurrentRefresh).toEqual(bundle("Edited"));
    expect(requestsAfterEdit).toBe(3);
    expect(requestsAfterOldCleanup).toBe(3);
  });

  it("refreshes after an uncertain mutation failure that may have committed", async () => {
    const old = deferred<TeaTicketBundle>();
    invokeMock.mockReturnValueOnce(old.promise).mockRejectedValueOnce(new Error("response lost"))
      .mockResolvedValueOnce(bundle("Approved"));
    const beforeApproval = getTicketBundle("ticket-1");
    await expect(ticketAction("ticket-1", "approve")).rejects.toThrow("response lost");
    const afterApproval = getTicketBundle("ticket-1");
    const requestsAfterApproval = invokeMock.mock.calls.length;
    old.resolve(bundle("Old"));
    await beforeApproval;
    expect(await afterApproval).toEqual(bundle("Approved"));
    expect(requestsAfterApproval).toBe(3);
  });

  it.each([
    { serverUrl: "https://other.test", authToken: "same-token" },
    { serverUrl: "https://tea.test", authToken: "other-token" },
  ])("keeps reads on an independent connection coalesced: %j", async (otherOptions) => {
    const pending = deferred<TeaTicketBundle>();
    invokeMock.mockReturnValueOnce(pending.promise).mockResolvedValueOnce({});
    const first = getTicketBundle("ticket-1", otherOptions);
    await updateTicket("ticket-1", { title: "Edited" }, {
      serverUrl: "https://tea.test", authToken: "same-token",
    });
    const second = getTicketBundle("ticket-1", otherOptions);
    expect(invokeMock).toHaveBeenCalledTimes(2);
    pending.resolve(bundle("Independent"));
    expect(await first).toEqual(await second);
  });
});
