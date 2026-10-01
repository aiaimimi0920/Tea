import { describe, expect, it } from "vitest";
import { ticketEditDraft, ticketEditPatch } from "./ticketEditing";

const baseline = { title: "Original", description: "Details", priority: "normal", labels: "one, two" };
describe("ticket edit patch", () => {
  it("preserves the opening draft as an immutable comparison baseline", () => {
    const opened = ticketEditDraft({ id: "A", title: "Original", status: "open", labels: ["source:human", "one"] });
    expect(opened.labels).toBe("one");
    expect(ticketEditPatch(opened, { ...opened, title: " Changed " })).toEqual({ title: "Changed" });
    expect(opened.title).toBe("Original");
  });
  it("ignores normalization-only edits and system labels", () => {
    expect(ticketEditPatch(baseline, { ...baseline, title: " Original ", priority: " normal ", labels: "one, two, two, source:hook, policy:manual-only" })).toEqual({});
  });
  it("keeps deliberate empty replacements instead of omitting them", () => {
    expect(ticketEditPatch(baseline, { ...baseline, description: "", priority: "", labels: "" }))
      .toEqual({ description: "", priority: "", labels: [] });
  });
});
