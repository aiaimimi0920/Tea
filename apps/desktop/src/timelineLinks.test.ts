import { afterEach, describe, expect, it, vi } from "vitest";
import { copyTimelineEntryLink, decodeTimelineEntryHash } from "./timelineLinks";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("timeline links", () => {
  it("decodes valid hashes and preserves malformed hashes", () => {
    expect(decodeTimelineEntryHash("#entry%20one")).toBe("entry one");
    expect(decodeTimelineEntryHash("#%")).toBe("%");
  });

  it("reports a successful clipboard write", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    const location = { hash: "", href: "http://localhost/tickets/one" };
    vi.stubGlobal("window", { location });
    vi.stubGlobal("navigator", { clipboard: { writeText } });

    await expect(copyTimelineEntryLink("event-1")).resolves.toBe(true);

    expect(writeText).toHaveBeenCalledWith("http://localhost/tickets/one#event-1");
    expect(location.hash).toBe("");
  });

  it("falls back to the page hash when clipboard access is denied", async () => {
    const writeText = vi.fn().mockRejectedValue(new Error("clipboard denied"));
    const location = { hash: "", href: "http://localhost/tickets/one" };
    vi.stubGlobal("window", { location });
    vi.stubGlobal("navigator", { clipboard: { writeText } });

    await expect(copyTimelineEntryLink("event-2")).resolves.toBe(false);

    expect(writeText).toHaveBeenCalledOnce();
    expect(location.hash).toBe("event-2");
  });
});
