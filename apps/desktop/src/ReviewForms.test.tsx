// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CommentEditor, RejectReasonForm } from "./ReviewForms";
import { setLocale } from "./i18n";
import type { ReviewDraftScope, ReviewDraftSubmission } from "./reviewDraft";

let root: Root;
let container: HTMLDivElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); setLocale("en");
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => { root.unmount(); }); container.remove(); vi.unstubAllGlobals(); });
const connection = { serverUrl: "http://127.0.0.1:48910" };
const scope = (ticketId = "A", server = connection): ReviewDraftScope => ({ ticketId, connection: server });
const textarea = () => container.querySelector("textarea")!;
async function type(text: string) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(textarea(), text);
    textarea().dispatchEvent(new Event("input", { bubbles: true }));
  });
}
async function submit() { await act(async () => { container.querySelector<HTMLButtonElement>("button[type=submit]")!.click(); }); }

describe.each(["comment", "rejection"])("%s form lifetime", (kind) => {
  const render = async (owner: ReviewDraftScope, onSubmit: (value: ReviewDraftSubmission) => Promise<boolean>) => {
    await act(async () => { root.render(kind === "comment"
      ? createElement(CommentEditor, { scope: owner, onSubmit, busy: false, disabled: false })
      : createElement(RejectReasonForm, { scope: owner, onReject: onSubmit, busy: false })); });
  };
  it("clears successful drafts but retains failed same-scope drafts", async () => {
    const owner = scope(); let success = false; const onSubmit = vi.fn(async () => success);
    await render(owner, onSubmit); await type("Review text"); await submit();
    expect(textarea().value).toBe("Review text"); success = true; await submit();
    expect(textarea().value).toBe(""); expect(onSubmit).toHaveBeenLastCalledWith({ scope: owner, text: "Review text" });
  });
  it("resets a draft on a connection change even when the ticket id is unchanged", async () => {
    await render(scope(), async () => false); await type("Old daemon text");
    await render(scope("A", { serverUrl: "http://127.0.0.1:48911" }), async () => false);
    expect(textarea().value).toBe("");
  });
  it("does not let an old response erase a new same-ticket draft after away/back", async () => {
    let resolve: ((value: boolean) => void) | undefined;
    await render(scope(), () => new Promise<boolean>((done) => { resolve = done; }));
    await type("Old review"); await submit();
    await render(scope("B"), async () => false); expect(textarea().value).toBe("");
    await render(scope("A"), async () => false); await type("New review");
    await act(async () => { resolve!(true); }); expect(textarea().value).toBe("New review");
  });
});

it("resets comment preview as well as text when ownership changes", async () => {
  await act(async () => { root.render(createElement(CommentEditor, { scope: scope(), onSubmit: async () => false, busy: false, disabled: false })); });
  await type("Old preview");
  await act(async () => { [...container.querySelectorAll<HTMLButtonElement>("button")].find((node) => node.textContent === "Preview comment")!.click(); });
  expect(container.querySelector(".comment-preview")?.textContent).toContain("Old preview");
  await act(async () => { root.render(createElement(CommentEditor, { scope: scope("B"), onSubmit: async () => false, busy: false, disabled: false })); });
  expect(container.querySelector(".comment-preview")).toBeNull(); expect(textarea().value).toBe("");
});
