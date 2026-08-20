import { describe, expect, it } from "vitest";

import {
  parseLocalNotes,
  parseWatchStates,
  removeTicketLocalNote,
  toggleWatchedTicket,
} from "./localMetadata";

describe("local metadata persistence", () => {
  it("keeps only explicit watched states", () => {
    expect(
      parseWatchStates(JSON.stringify({ watched: true, falseEntry: false, stringEntry: "true", "": true })),
    ).toEqual({ watched: true });
  });

  it("normalizes notes and removes empty entries", () => {
    expect(
      parseLocalNotes(
        JSON.stringify({
          ticket: [" first ", "first", "", 3, "second"],
          empty: ["  "],
          invalid: "note",
          "": ["hidden"],
        }),
      ),
    ).toEqual({ ticket: ["first", "second"] });
  });

  it("rejects malformed and non-object payloads", () => {
    expect(parseWatchStates("not-json")).toEqual({});
    expect(parseLocalNotes("[]")).toEqual({});
    expect(parseLocalNotes(null)).toEqual({});
  });

  it("removes unwatched and empty-note keys instead of persisting false values", () => {
    expect(toggleWatchedTicket({ ticket: true }, "ticket")).toEqual({});
    expect(toggleWatchedTicket({}, "ticket")).toEqual({ ticket: true });
    expect(removeTicketLocalNote({ ticket: ["only"] }, "ticket", "only")).toEqual({});
    expect(removeTicketLocalNote({ ticket: ["first", "second"] }, "ticket", "first")).toEqual({
      ticket: ["second"],
    });
  });
});
