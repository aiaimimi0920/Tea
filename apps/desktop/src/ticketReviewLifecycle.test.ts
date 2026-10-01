import { createElement, type ComponentProps } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it } from "vitest";
import { IssueDetail } from "./IssueDetail";
import { setLocale } from "./i18n";
import { isClosedTicket, issueStateLabel, progressForTicket } from "./issueFormat";
import { issueActionHintForTicket, issueSignalFilterKey, issueSignalForTicket } from "./issueSignals";
import type { TeaTicket } from "./teaClient";

const noop = () => {};
const ticket = (status: string): TeaTicket => ({ id: "ticket-1", title: "Implement feature", status });

function renderDetail(status: string, busy = false): string {
  const props: ComponentProps<typeof IssueDetail> = {
    activeSection: "comments", analysis: null, busy, comments: [], events: [],
    hasLocalNotes: false, exportDownloadBusy: false, exportPreview: "", plan: null,
    isWatched: false, daemonLabels: [], localNotes: [],
    onAddLabel: () => false, onCopyLink: noop, onAction: noop, onApplyPolicy: noop,
    onComment: async () => true, onDownloadExport: noop, onExport: noop,
    onNavigateIssueQueue: noop, onReject: async () => true, onRemoveLabel: noop,
    onResetLabels: noop, onRetryRun: noop, onSaveConfiguration: noop,
    onSectionChange: noop, onStopRun: noop, onToggleWatch: noop,
    onToggleLabelEditor: noop, onBeginEdit: noop, onCancelEdit: noop, onSubmitEdit: noop,
    showEditIssue: false, reviewScope: { ticketId: "ticket-1", connection: {} },
    queueNavigation: {
      current: 1, total: 1, firstId: "ticket-1", lastId: "ticket-1",
      isOutsideQueue: false, previousId: null, nextId: null,
    },
    runs: [{ id: "run-1", status: "succeeded", evidence: { summary: "Tests passed" } }],
    selectedActionHint: null, selectedSignal: null, showLabelEditor: false,
    snapshot: null, ticket: ticket(status),
  };
  return renderToStaticMarkup(createElement(IssueDetail, props));
}

function disabledButton(html: string, label: string): boolean {
  const match = html.match(new RegExp(`<button\\b([^>]*)>${label}</button>`));
  expect(match, `missing ${label} button`).not.toBeNull();
  return /\bdisabled(?:=|\s|$)/.test(match![1]);
}

function commentDisabled(html: string): boolean {
  const match = html.match(/<textarea\b([^>]*)>/);
  expect(match, "missing review comment textarea").not.toBeNull();
  return /\bdisabled(?:=|\s|$)/.test(match![1]);
}

beforeEach(() => { setLocale("en"); });

describe("completed work remains reviewable until explicitly closed", () => {
  it.each(["completed", "accepted"])("keeps %s in the open review queue", (status) => {
    const current = ticket(status);
    expect(isClosedTicket(current)).toBe(false);
    expect(issueStateLabel(current)).toBe("Open");
    expect(progressForTicket(current, [], [], [{ id: "run-1", status: "succeeded" }])).toBeLessThan(100);
    const signal = issueSignalForTicket(current, { comments: 0, runs: 1 });
    expect(issueSignalFilterKey(signal)).toBe("review");
    expect(issueActionHintForTicket(current, { comments: 0, runs: 1 }, signal).target)
      .toMatchObject({ kind: "section", section: "runs" });
  });

  it("explains pending acceptance and closure in the default Chinese locale", () => {
    setLocale("zh");
    expect(issueSignalForTicket(ticket("completed"), undefined).reason).toBe("执行已完成，等待验收");
    expect(issueSignalForTicket(ticket("accepted"), undefined).reason).toBe("已验收，等待关闭");
  });

  it("enables acceptance, completion approval, closure and review comments for completed work", () => {
    const html = renderDetail("completed");
    for (const label of ["Accept", "Approve", "Close", "Reject approval"]) expect(disabledButton(html, label)).toBe(false);
    expect(commentDisabled(html)).toBe(false);
    expect(html).toContain('class="issue-state open"');
  });

  it("allows accepted work to close without offering a second acceptance", () => {
    const html = renderDetail("accepted");
    expect(disabledButton(html, "Accept")).toBe(true);
    expect(disabledButton(html, "Close")).toBe(false);
    expect(commentDisabled(html)).toBe(false);
  });

  it.each(["closed", "cancelled", "canceled", "done"])("keeps %s read-only", (status) => {
    expect(isClosedTicket(ticket(status))).toBe(true);
    const html = renderDetail(status);
    for (const label of ["Accept", "Approve", "Close", "Run", "Reject approval"]) expect(disabledButton(html, label)).toBe(true);
    expect(commentDisabled(html)).toBe(true);
    expect(html).toContain('class="issue-state closed"');
    expect(html).toMatch(/<select\b[^>]*disabled=""[^>]*id="policy-editor-select"/);
    expect(html).toMatch(/<textarea\b[^>]*disabled=""[^>]*id="reject-reason-input"/);
  });

  it.each(["open", "plan_ready", "running", "failed", "needs_review"])("does not offer premature accept/close for %s", (status) => {
    const html = renderDetail(status);
    expect(disabledButton(html, "Accept")).toBe(true);
    expect(disabledButton(html, "Close")).toBe(true);
  });

  it("keeps mutation controls disabled while busy", () => {
    const html = renderDetail("completed", true);
    expect(disabledButton(html, "Accept")).toBe(true);
    expect(disabledButton(html, "Close")).toBe(true);
    expect(commentDisabled(html)).toBe(true);
  });
});
