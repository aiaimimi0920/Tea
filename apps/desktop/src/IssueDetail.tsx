import { FormEvent, memo, useEffect, useMemo, useState } from "react";

import {
  conversationEntryGroup,
  conversationEntryGroupLabel,
  conversationEntrySummary,
  payloadSummary,
  buildConversationEntries,
  buildConversationTimelineItems,
  type ConversationFilter,
} from "./conversation";
import { TeaSettingsPanel } from "./TeaSettingsPanel";
import { t, useLocale } from "./i18n";
import {
  approvalPolicyOptions,
  formatTime,
  isClosedTicket,
  issueAgeLabel,
  issueNumber,
  issueStateLabel,
  pretty,
  progressForTicket,
  timelineEntryReference,
  workflowActionGroups,
} from "./issueFormat";
import { badgeToneForRisk, type IssueActionHint, type IssueSignal } from "./issueSignals";
import type { IssueQueueNavigation, RepoSection } from "./issueTypes";
import { canRetryRun, canStopRun } from "./runLifecycle";
import { CommentEditor, RejectReasonForm } from "./ReviewForms";
import type { ReviewDraftScope, ReviewDraftSubmission } from "./reviewDraft";
import { IssueEditForm } from "./IssueEditForm";
import type { TicketEditSubmission } from "./ticketEditing";
import { isTicketActionDisabled } from "./ticketLifecycle";
import {
  TeaAnalysis,
  TeaClientOptions,
  TeaComment,
  TeaEvent,
  TeaLocalConfig,
  TeaPlan,
  TeaRun,
  TeaSnapshot,
  TeaTicket,
  ticketAction,
} from "./teaClient";
import { copyTimelineEntryLink, decodeTimelineEntryHash } from "./timelineLinks";

export const IssueDetail = memo(function IssueDetail({
  activeSection,
  analysis,
  busy,
  comments,
  reviewScope,
  hasLocalNotes,
  events,
  exportDownloadBusy,
  exportPreview,
  isWatched,
  daemonLabels,
  localNotes,
  onAddLabel,
  onCopyLink,
  onAction,
  onApplyPolicy,
  onComment,
  onDownloadExport,
  onExport,
  onNavigateIssueQueue,
  onReject,
  onRemoveLabel,
  onResetLabels,
  onRetryRun,
  onSaveConfiguration,
  onSectionChange,
  onStopRun,
  onToggleWatch,
  onToggleLabelEditor,
  onBeginEdit,
  onCancelEdit,
  onSubmitEdit,
  showEditIssue,
  plan,
  queueNavigation,
  runs,
  selectedActionHint,
  selectedSignal,
  showLabelEditor,
  snapshot,
  ticket,
}: {
  activeSection: RepoSection;
  analysis: TeaAnalysis | null;
  busy: boolean;
  comments: TeaComment[];
  reviewScope: ReviewDraftScope;
  hasLocalNotes: boolean;
  events: TeaEvent[];
  exportDownloadBusy: boolean;
  exportPreview: string;
  plan: TeaPlan | null;
  isWatched: boolean;
  daemonLabels: string[];
  localNotes: string[];
  onAddLabel: (note: string) => boolean;
  onCopyLink: () => void;
  onAction: (action: Parameters<typeof ticketAction>[1]) => void;
  onApplyPolicy: (mode: string) => void;
  onComment: (submission: ReviewDraftSubmission) => Promise<boolean>;
  onDownloadExport: (format: "json" | "markdown") => void;
  onExport: (format: "json" | "markdown") => void;
  onNavigateIssueQueue: (ticketId: string | null) => void;
  onReject: (submission: ReviewDraftSubmission) => Promise<boolean>;
  onRemoveLabel: (label: string) => void;
  onResetLabels: () => void;
  onRetryRun: (runId: string) => void;
  onSaveConfiguration: (config: Partial<TeaLocalConfig>, connection: TeaClientOptions) => void;
  onSectionChange: (section: RepoSection) => void;
  onStopRun: (runId: string) => void;
  onToggleWatch: () => void;
  onToggleLabelEditor: () => void;
  onBeginEdit: () => void;
  onCancelEdit: () => void;
  onSubmitEdit: (submission: TicketEditSubmission) => void;
  showEditIssue: boolean;
  queueNavigation: IssueQueueNavigation;
  runs: TeaRun[];
  selectedActionHint: IssueActionHint | null;
  selectedSignal: IssueSignal | null;
  showLabelEditor: boolean;
  snapshot: TeaSnapshot | null;
  ticket: TeaTicket | null;
}) {
  // memo'd component: subscribe to locale changes so a language toggle
  // re-renders the t() output even when all props are unchanged.
  useLocale();
  if (activeSection === "settings") {
    return (
      <article className="issue-detail">
        <TeaSettingsPanel
          busy={busy}
          connection={reviewScope.connection}
          snapshot={snapshot}
          onSaveConfiguration={onSaveConfiguration}
        />
      </article>
    );
  }
  if (!ticket) {
    return (
      <article className="issue-detail empty-detail">
        <h2>{t("Issue / Work Order")}</h2>
        <p>{t("Select a work order from the list, or create a new one to start an AI task.")}</p>
      </article>
    );
  }

  const latestRun = runs[runs.length - 1];
  const workflowActionDisabled = (action: Parameters<typeof ticketAction>[1]): boolean =>
    isTicketActionDisabled(ticket, latestRun, action, busy);

  const handleSuggestedActionHint = () => {
    if (!selectedActionHint) return;
    const target = selectedActionHint.target;
    switch (target.kind) {
      case "section":
        onSectionChange(target.section);
        return;
      case "export":
        onSectionChange("exports");
        onExport(target.format);
        return;
      case "action":
        onSectionChange(target.sectionAfterAction);
        onAction(target.action);
        return;
    }
  };

  return (
    <article className="issue-detail">
      <header className="issue-titlebar">
        <div className="issue-title-primary">
          <div className="issue-title-topline">
            <span className={`issue-state ${isClosedTicket(ticket) ? "closed" : "open"}`}>
              {t(issueStateLabel(ticket))}
            </span>
            <span className="issue-number-badge">{issueNumber(ticket)}</span>
            <span>{ticket.status}</span>
          </div>
          <div className="issue-title-heading">
            <h2>{ticket.title}</h2>
          </div>
          <div className="issue-title-meta" aria-label={t("Issue metadata summary")}>
            <span>{issueAgeLabel(ticket)}</span>
            <span>{comments.length} {t("comments")}</span>
            <span>{runs.length} {t("runs")}</span>
            <span>{ticket.approval_policy ?? t("default policy")}</span>
          </div>
        </div>
        <div className="issue-title-secondary">
          <div
            className={`issue-queue-navigation ${queueNavigation.isOutsideQueue ? "outside-queue" : "in-queue"}`}
            aria-label={t("Issue queue navigation")}
          >
            <button
              disabled={!queueNavigation.previousId}
              onClick={() => onNavigateIssueQueue(queueNavigation.previousId)}
              type="button"
            >
              {t("Previous issue")}
            </button>
            <span className="issue-queue-position">
              {queueNavigation.isOutsideQueue ? t("Selected outside current queue") : t("Queue position")}{" "}
              {queueNavigation.current > 0 ? queueNavigation.current : "-"} {t("of")} {queueNavigation.total}
            </span>
            {queueNavigation.isOutsideQueue ? (
              <button
                disabled={!queueNavigation.firstId}
                onClick={() => onNavigateIssueQueue(queueNavigation.firstId)}
                type="button"
              >
                {t("Select first matching issue")}
              </button>
            ) : null}
            <button
              disabled={!queueNavigation.nextId}
              onClick={() => onNavigateIssueQueue(queueNavigation.nextId)}
              type="button"
            >
              {t("Next issue")}
            </button>
          </div>
          <div className="issue-detail-actions">
            <button type="button" onClick={onCopyLink}>
              {t("Copy issue link")}
            </button>
            <button className={isWatched ? "watching" : ""} type="button" onClick={onToggleWatch}>
              {isWatched ? t("Watching issue") : t("Watch issue")}
            </button>
            <button
              aria-expanded={showLabelEditor}
              data-testid="local-notes-toggle"
              type="button"
              onClick={onToggleLabelEditor}
            >
              {showLabelEditor ? t("Hide local notes") : t("Local notes")}
            </button>
            {!isClosedTicket(ticket) ? (
              <button type="button" onClick={showEditIssue ? onCancelEdit : onBeginEdit}>
                {showEditIssue ? t("Cancel edit") : t("Edit issue")}
              </button>
            ) : null}
          </div>
          {showEditIssue && !isClosedTicket(ticket) ? (
            <IssueEditForm
              key={ticket.id}
              busy={busy}
              onCancel={onCancelEdit}
              onSubmit={onSubmitEdit}
              ticket={ticket}
            />
          ) : null}
        </div>
      </header>

      <div className="issue-body-layout">
        <section className="issue-conversation">
          <div className="conversation-header">
            <div>
              <h3>{t("Conversation")}</h3>
              <p>
                {t("{c} review comments, {e} timeline events, {r} run records.")
                  .replace("{c}", String(comments.length))
                  .replace("{e}", String(events.length))
                  .replace("{r}", String(runs.length))}
              </p>
            </div>
          </div>

          <div className="comment-with-avatar">
            <span className="comment-avatar">T</span>
            <section className="issue-comment issue-description">
              <header className="issue-comment-header">
                <strong>{t("Tea work-order description")}</strong>
                <span>{t("edited")} {formatTime(ticket.updated_at)}</span>
              </header>
              <div className="markdown-body">
                {ticket.description ? (
                  ticket.description.split(/\n{2,}/).map((paragraph, index) => (
                    <p key={`${ticket.id}-paragraph-${index}`}>{paragraph}</p>
                  ))
                ) : (
                  <p>{t("No description was provided.")}</p>
                )}
              </div>
            </section>
          </div>

          <FocusedIssueSection
            activeSection={activeSection}
            analysis={analysis}
            busy={busy}
            comments={comments}
            reviewScope={reviewScope}
            events={events}
            exportDownloadBusy={exportDownloadBusy}
            exportPreview={exportPreview}
            onComment={onComment}
            onDownloadExport={onDownloadExport}
            onExport={onExport}
            onRetryRun={onRetryRun}
            onStopRun={onStopRun}
            plan={plan}
            runs={runs}
            ticket={ticket}
          />
        </section>

        <aside className="issue-meta-sidebar">
          <section className="meta-section meta-routing">
            <h3>{t("Review and routing")}</h3>
            <dl className="meta-list">
              <div>
                <dt>{t("Owner")}</dt>
                <dd>{ticket.owner_human_id ?? t("Tea local operator")}</dd>
              </div>
              <div>
                <dt>{t("Agent")}</dt>
                <dd>{ticket.delegated_agent_id ?? t("not delegated")}</dd>
              </div>
              <div>
                <dt>{t("Source")}</dt>
                <dd>{ticket.source ?? t("desktop")}</dd>
              </div>
              <div>
                <dt>{t("Policy")}</dt>
                <dd>{ticket.approval_policy ?? t("default")}</dd>
              </div>
              <div>
                <dt>{t("Priority")}</dt>
                <dd>{t(ticket.priority ?? "normal")}</dd>
              </div>
              <div>
                <dt>{t("Risk")}</dt>
                <dd>{t(ticket.risk_level ?? "medium")}</dd>
              </div>
            </dl>
          </section>

          <section className="meta-section">
            <h3>{t("Signal summary")}</h3>
            {selectedSignal ? (
              <div className="issue-signal-panel">
                <span className="issue-signal-with-reason">
                  <span className={`issue-signal-chip tone-${selectedSignal.tone}`}>{t(selectedSignal.label)}</span>
                  <span className="issue-signal-reason">{selectedSignal.reason}</span>
                </span>
                <p>{selectedSignal.description}</p>
              </div>
            ) : null}
            {selectedActionHint ? (
              <div className="issue-action-hint-panel">
                <span className={`issue-action-hint tone-${selectedActionHint.tone}`}>
                  {t(selectedActionHint.label)}
                </span>
                <p>{selectedActionHint.description}</p>
                <button
                  className="issue-action-hint-cta"
                  disabled={busy && selectedActionHint.target.kind === "action"}
                  onClick={handleSuggestedActionHint}
                  type="button"
                >
                  {t(selectedActionHint.target.label)}
                </button>
              </div>
            ) : null}
          </section>

          <section className="meta-section">
            <h3>{t("Labels")}</h3>
            <div className="label-stack">
              {daemonLabels.length > 0 ? (
                daemonLabels.map((label) => <span key={label}>{label}</span>)
              ) : (
                <span>{t("No labels")}</span>
              )}
            </div>
            {localNotes.length > 0 && !showLabelEditor ? (
              <div className="label-stack label-stack-notes" aria-label={t("Local notes")}>
                {localNotes.map((note) => (
                  <span className="local-note-chip" key={`note-${note}`}>
                    {note}
                  </span>
                ))}
              </div>
            ) : null}
            {showLabelEditor ? (
              <LabelEditor
                hasNotes={hasLocalNotes}
                notes={localNotes}
                onAddNote={onAddLabel}
                onRemoveNote={onRemoveLabel}
                onResetNotes={onResetLabels}
              />
            ) : null}
          </section>

          <section className="meta-section">
            <h3>{t("Execution progress")}</h3>
            <div
              aria-label={t("Milestone progress")}
              aria-valuemax={100}
              aria-valuemin={0}
              aria-valuenow={progressForTicket(ticket, comments, events, runs)}
              className="milestone-progress"
              role="progressbar"
            >
              <span style={{ width: `${progressForTicket(ticket, comments, events, runs)}%` }} />
            </div>
          </section>

          <section className="meta-section">
            <h3>{t("Workflow actions")}</h3>
            <div className="workflow-action-groups">
              {workflowActionGroups.map((group) => (
                <section className="workflow-action-group" key={group.key}>
                  <h4>{t(group.title)}</h4>
                  <div className="workflow-actions">
                    {group.actions
                      .filter((item) => item.action !== "reject")
                      .map((item) => (
                        <button
                          className={item.tone ? `action-${item.tone}` : ""}
                          disabled={workflowActionDisabled(item.action)}
                          key={item.action}
                          onClick={() => onAction(item.action)}
                        >
                          {t(item.label)}
                        </button>
                      ))}
                  </div>
                  {group.key === "approval" ? (
                    <>
                      <RejectReasonForm scope={reviewScope} busy={busy || isClosedTicket(ticket)} onReject={onReject} />
                      <div className="policy-editor">
                        <label htmlFor="policy-editor-select">{t("Approval policy")}</label>
                        <select
                          disabled={busy || isClosedTicket(ticket)}
                          id="policy-editor-select"
                          onChange={(event) => onApplyPolicy(event.target.value)}
                          value={ticket.approval_policy ?? ""}
                        >
                          {ticket.approval_policy ? null : (
                            <option value="">{t("Select approval policy")}</option>
                          )}
                          {approvalPolicyOptions.map((option) => (
                            <option key={option.value} value={option.value}>
                              {t(option.label)}
                            </option>
                          ))}
                        </select>
                      </div>
                    </>
                  ) : null}
                </section>
              ))}
            </div>
          </section>

          <details className="meta-section raw-details">
            <summary>{t("Raw ticket JSON")}</summary>
            <pre>{pretty(ticket)}</pre>
          </details>

          <details className="meta-section raw-details">
            <summary>{t("Daemon status")}</summary>
            <pre>{pretty(snapshot?.status ?? snapshot?.health ?? snapshot?.error ?? null)}</pre>
          </details>
        </aside>
      </div>
    </article>
  );
});
function LabelEditor({
  hasNotes,
  notes,
  onAddNote,
  onRemoveNote,
  onResetNotes,
}: {
  hasNotes: boolean;
  notes: string[];
  onAddNote: (note: string) => boolean;
  onRemoveNote: (note: string) => void;
  onResetNotes: () => void;
}) {
  // The note draft is owned here so typing re-renders only this editor, not
  // the whole App. It clears when a note is added successfully, when notes are
  // reset, and on unmount (the editor closes on ticket switch).
  const [draft, setDraft] = useState("");

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (onAddNote(draft)) setDraft("");
  };

  const resetNotes = () => {
    onResetNotes();
    setDraft("");
  };

  return (
    <section className="label-editor" aria-label={t("Local notes editor")} data-testid="local-notes-editor">
      <div className="label-editor-header">
        <strong>{t("Local notes")}</strong>
        <span>{t("Local only")}</span>
      </div>
      <div className="label-editor-list">
        {notes.length > 0 ? (
          notes.map((note) => (
            <span className="local-note-chip" key={note}>
              <span>{note}</span>
              <button
                aria-label={`Remove local note ${note}`}
                className="label-remove"
                data-testid={`local-note-remove-${note}`}
                onClick={() => onRemoveNote(note)}
                type="button"
              >
                {t("Remove note")}
              </button>
            </span>
          ))
        ) : (
          <p className="label-empty">{t("No local notes yet. Add a private note for this ticket.")}</p>
        )}
      </div>
      <form className="label-editor-form" onSubmit={submit}>
        <input
          data-testid="local-notes-input"
          onChange={(event) => setDraft(event.target.value)}
          placeholder={t("Add a local note")}
          value={draft}
        />
        <div className="label-editor-actions">
          <button data-testid="local-notes-add" type="submit">{t("Add note")}</button>
          <button data-testid="local-notes-clear" disabled={!hasNotes} onClick={resetNotes} type="button">
            {t("Clear notes")}
          </button>
        </div>
      </form>
    </section>
  );
}



function AnalysisPlanChipList({ items }: { items: string[] }) {
  if (items.length === 0) {
    return <span className="analysis-plan-empty-value">{t("None recorded")}</span>;
  }
  return (
    <ul className="analysis-plan-chip-list">
      {items.map((item, index) => (
        <li key={`${item}-${index}`}>{item}</li>
      ))}
    </ul>
  );
}

const AnalysisPlanView = memo(function AnalysisPlanView({
  analysis,
  plan,
}: {
  analysis: TeaAnalysis | null;
  plan: TeaPlan | null;
}) {
  // memo'd component: subscribe to locale changes so a language toggle
  // re-renders the t() output even when analysis/plan props are unchanged.
  useLocale();
  const confidencePercent =
    analysis && typeof analysis.confidence === "number"
      ? Math.round(Math.max(0, Math.min(1, analysis.confidence)) * 100)
      : null;

  return (
    <div className="analysis-plan-view">
      {analysis ? (
        <section className="analysis-card" aria-label={t("Ticket analysis")}>
          <header className="analysis-card-header">
            <h4>{t("Analysis")}</h4>
            <div className="analysis-card-badges">
              <span className={`issue-badge tone-${badgeToneForRisk(analysis.risk_assessment)}`}>
                {t("risk:")} {analysis.risk_assessment ? t(analysis.risk_assessment) : t("unknown")}
              </span>
              {confidencePercent !== null ? (
                <span className="issue-badge tone-default">{t("confidence:")} {confidencePercent}%</span>
              ) : null}
              {analysis.recommended_policy ? (
                <span className="issue-badge tone-default">{t("policy:")} {analysis.recommended_policy}</span>
              ) : null}
            </div>
          </header>
          {analysis.intent ? (
            <p className="analysis-intent">
              <strong>{t("Intent:")}</strong> {analysis.intent}
            </p>
          ) : null}
          {analysis.recommended_workflow ? (
            <p className="analysis-workflow">
              <strong>{t("Recommended workflow:")}</strong> <code>{analysis.recommended_workflow}</code>
            </p>
          ) : null}
          <dl className="analysis-plan-facts">
            <div>
              <dt>{t("Target components")}</dt>
              <dd>
                <AnalysisPlanChipList items={analysis.target_components ?? []} />
              </dd>
            </div>
            <div>
              <dt>{t("Target paths")}</dt>
              <dd>
                <AnalysisPlanChipList items={analysis.target_paths ?? []} />
              </dd>
            </div>
            <div>
              <dt>{t("Constraints")}</dt>
              <dd>
                <AnalysisPlanChipList items={analysis.constraints ?? []} />
              </dd>
            </div>
            <div>
              <dt>{t("Acceptance criteria")}</dt>
              <dd>
                <AnalysisPlanChipList items={analysis.acceptance_criteria ?? []} />
              </dd>
            </div>
            <div>
              <dt>{t("Missing context")}</dt>
              <dd>
                <AnalysisPlanChipList items={analysis.missing_context ?? []} />
              </dd>
            </div>
          </dl>
        </section>
      ) : null}
      {plan ? (
        <section className="plan-card" aria-label={t("Ticket plan")}>
          <header className="plan-card-header">
            <h4>{t("Plan")}</h4>
            <span
              className={`issue-badge tone-${plan.requires_approval_before_execute ? "warn" : "default"}`}
            >
              {plan.requires_approval_before_execute ? t("approval required") : t("no gate")}
            </span>
          </header>
          {plan.summary ? <p className="plan-summary">{plan.summary}</p> : null}
          {plan.steps && plan.steps.length > 0 ? (
            <ol className="plan-step-list">
              {plan.steps.map((step, index) => (
                <li className="plan-step" key={step.id || `step-${index}`}>
                  <strong>{step.title || `Step ${index + 1}`}</strong>
                  {step.description ? <p>{step.description}</p> : null}
                </li>
              ))}
            </ol>
          ) : (
            <p className="analysis-plan-empty-value">{t("No plan steps recorded.")}</p>
          )}
          <dl className="analysis-plan-facts">
            <div>
              <dt>{t("Required tools")}</dt>
              <dd>
                <AnalysisPlanChipList items={plan.required_tools ?? []} />
              </dd>
            </div>
            <div>
              <dt>{t("Expected artifacts")}</dt>
              <dd>
                <AnalysisPlanChipList items={plan.expected_artifacts ?? []} />
              </dd>
            </div>
            <div>
              <dt>{t("Validation strategy")}</dt>
              <dd>
                <AnalysisPlanChipList items={plan.validation_strategy ?? []} />
              </dd>
            </div>
            <div>
              <dt>{t("Rollback strategy")}</dt>
              <dd>
                <AnalysisPlanChipList items={plan.rollback_strategy ?? []} />
              </dd>
            </div>
          </dl>
        </section>
      ) : null}
    </div>
  );
});

function FocusedIssueSection({
  activeSection,
  analysis,
  busy,
  comments,
  reviewScope,
  events,
  exportDownloadBusy,
  exportPreview,
  onComment,
  onDownloadExport,
  onExport,
  onRetryRun,
  onStopRun,
  plan,
  runs,
  ticket,
}: {
  activeSection: RepoSection;
  analysis: TeaAnalysis | null;
  busy: boolean;
  comments: TeaComment[];
  reviewScope: ReviewDraftScope;
  events: TeaEvent[];
  exportDownloadBusy: boolean;
  exportPreview: string;
  onComment: (submission: ReviewDraftSubmission) => Promise<boolean>;
  onDownloadExport: (format: "json" | "markdown") => void;
  onExport: (format: "json" | "markdown") => void;
  onRetryRun: (runId: string) => void;
  onStopRun: (runId: string) => void;
  plan: TeaPlan | null;
  runs: TeaRun[];
  ticket: TeaTicket;
}) {
  const editor = (
    <CommentEditor
      scope={reviewScope}
      busy={busy}
      disabled={isClosedTicket(ticket)}
      onSubmit={onComment}
    />
  );

  if (activeSection === "comments") {
    return (
      <section className="section-focus-card" aria-label={t("Focused comments section")}>
        <header>
          <h3>{t("Comments and daemon events")}</h3>
          <span>{comments.length} {t("comments")}</span>
        </header>
        {comments.length === 0 && events.length === 0 ? (
          <div className="section-focus-empty">
            <strong>{t("No conversation yet.")}</strong>
            <span>{t("Add a review comment or run an action to populate the timeline.")}</span>
          </div>
        ) : (
          <ConversationStream comments={comments} events={events} />
        )}
        {editor}
      </section>
    );
  }

  if (activeSection === "plan") {
    return (
      <section className="section-focus-card" aria-label={t("Focused analysis and plan section")}>
        <header>
          <h3>{t("AI analysis and plan")}</h3>
          <span>{plan ? t("Plan ready") : analysis ? t("Analyzed") : t("Not analyzed")}</span>
        </header>
        {!analysis && !plan ? (
          <div className="section-focus-empty">
            <strong>{t("No analysis yet.")}</strong>
            <span>{t("Use the Review actions to analyze or decompose this work order into a plan.")}</span>
          </div>
        ) : (
          <AnalysisPlanView analysis={analysis} plan={plan} />
        )}
      </section>
    );
  }

  if (activeSection === "runs") {
    return (
      <section className="section-focus-card" aria-label={t("Focused runs section")}>
        <header>
          <h3>{t("Run records")}</h3>
          <span>{runs.length} {t("runs")}</span>
        </header>
        {runs.length === 0 ? (
          <div className="section-focus-empty">
            <strong>{t("No runs yet.")}</strong>
            <span>{t("Approve and launch an AI task to see execution attempts here.")}</span>
          </div>
        ) : (
          <Runs
            busy={busy}
            disabled={isClosedTicket(ticket)}
            onRetryRun={onRetryRun}
            onStopRun={onStopRun}
            runs={runs}
          />
        )}
      </section>
    );
  }

  if (activeSection === "exports") {
    return (
      <section className="section-focus-card" aria-label={t("Focused export section")}>
        <header>
          <h3>{t("Export this work order")}</h3>
          <span>{issueNumber(ticket)}</span>
        </header>
        <div className="focused-actions">
          <button onClick={() => onExport("json")}>{t("Preview JSON export")}</button>
          <button onClick={() => onExport("markdown")}>{t("Preview Markdown export")}</button>
        </div>
        <div className="focused-actions export-download-actions">
          <button disabled={exportDownloadBusy} onClick={() => onDownloadExport("json")}>
            {t("Download JSON export")}
          </button>
          <button disabled={exportDownloadBusy} onClick={() => onDownloadExport("markdown")}>
            {t("Download Markdown export")}
          </button>
        </div>
        <ExportPreview exportPreview={exportPreview} />
      </section>
    );
  }


  return (
    <>
      <ConversationStream comments={comments} events={events} />
      {editor}
      <Runs
        busy={busy}
        disabled={isClosedTicket(ticket)}
        onRetryRun={onRetryRun}
        onStopRun={onStopRun}
        runs={runs}
      />
      <ExportPreview exportPreview={exportPreview} />
    </>
  );
}

const ExportPreview = memo(function ExportPreview({ exportPreview }: { exportPreview: string }) {
  // memo'd component: subscribe to locale changes so a language toggle
  // re-renders the t() output even when the preview prop is unchanged.
  useLocale();
  if (!exportPreview) return null;

  return (
    <section className="issue-comment">
      <header className="issue-comment-header">
        <strong>{t("Export preview")}</strong>
        <span>{t("JSON / Markdown")}</span>
      </header>
      <pre className="export-preview">{exportPreview}</pre>
    </section>
  );
});

const ConversationStream = memo(function ConversationStream({
  comments,
  events,
}: {
  comments: TeaComment[];
  events: TeaEvent[];
}) {
  // memo'd component: subscribe to locale changes so a language toggle
  // re-renders the t() output even when comments/events props are unchanged.
  useLocale();
  const [copiedEntryId, setCopiedEntryId] = useState<string | null>(null);
  const [conversationFilter, setConversationFilter] = useState<ConversationFilter>("all");
  const [activeEntryHash, setActiveEntryHash] = useState(() => {
    if (typeof window === "undefined") return "";
    return decodeTimelineEntryHash(window.location.hash);
  });
  // Derive the timeline via useMemo so the map/sort/group work only re-runs when the
  // underlying comments/events (or the active filter) change — not on every parent
  // poll, copy-button click, or hashchange re-render.
  const entries = useMemo(() => buildConversationEntries(comments, events), [comments, events]);
  const filteredConversationEntries = useMemo(
    () =>
      entries.filter((entry) => {
        if (conversationFilter === "comments") return entry.kind === "comment";
        if (conversationFilter === "events") return entry.kind === "event";
        return true;
      }),
    [entries, conversationFilter],
  );
  const filteredConversationTimelineItems = useMemo(
    () => buildConversationTimelineItems(filteredConversationEntries),
    [filteredConversationEntries],
  );

  useEffect(() => {
    if (!copiedEntryId) return undefined;
    const timer = window.setTimeout(() => setCopiedEntryId(null), 1800);
    return () => window.clearTimeout(timer);
  }, [copiedEntryId]);

  useEffect(() => {
    if (typeof window === "undefined") return undefined;
    const syncHash = () => setActiveEntryHash(decodeTimelineEntryHash(window.location.hash));
    syncHash();
    window.addEventListener("hashchange", syncHash);
    return () => window.removeEventListener("hashchange", syncHash);
  }, []);

  return (
    <section className="conversation-stream" aria-label={t("Conversation timeline")}>
      <div className="conversation-stream-header">
        <div>
          <h3>{t("Conversation timeline")}</h3>
          <p>{t("Human review comments and daemon state changes in one issue thread.")}</p>
        </div>
        <div className="conversation-stats" aria-label={t("Timeline activity summary")}>
          <span>{comments.length} {t("comments")}</span>
          <span>{events.length} {t("events")}</span>
          <span>{entries.length} {t("entries")}</span>
        </div>
      </div>
      <div className="conversation-filter-tabs" role="tablist" aria-label={t("Timeline filters")}>
        <button
          aria-selected={conversationFilter === "all"}
          className={conversationFilter === "all" ? "active" : ""}
          onClick={() => setConversationFilter("all")}
          role="tab"
          type="button"
        >
          {t("All")} <span>{entries.length}</span>
        </button>
        <button
          aria-selected={conversationFilter === "comments"}
          className={conversationFilter === "comments" ? "active" : ""}
          onClick={() => setConversationFilter("comments")}
          role="tab"
          type="button"
        >
          {t("Comments")} <span>{comments.length}</span>
        </button>
        <button
          aria-selected={conversationFilter === "events"}
          className={conversationFilter === "events" ? "active" : ""}
          onClick={() => setConversationFilter("events")}
          role="tab"
          type="button"
        >
          {t("Events")} <span>{events.length}</span>
        </button>
      </div>
      {entries.length === 0 ? <p className="empty">{t("No comments or events yet.")}</p> : null}
      {entries.length > 0 && filteredConversationTimelineItems.length === 0 ? (
        <p className="empty">{t("No timeline entries match this filter.")}</p>
      ) : null}
      <div className="timeline">
        <div className="comment-list" role="list">
          {filteredConversationTimelineItems.map((item) => {
            if (item.kind === "system-event-group") {
              const firstEntry = item.entries[0];
              const lastEntry = item.entries[item.entries.length - 1];
              const isLinkedEntry = item.entries.some((entry) => activeEntryHash === entry.id);
              return (
                <article
                  aria-current={isLinkedEntry ? "location" : undefined}
                  className={`conversation-entry event timeline-event-group ${isLinkedEntry ? "linked" : ""}`}
                  data-entry-group="system"
                  data-entry-kind="event"
                  id={item.id}
                  key={item.id}
                  role="listitem"
                >
                  <span className="timeline-marker" />
                  <div className="comment-with-avatar">
                    <span className="comment-avatar small">{t("SY")}</span>
                    <div className="issue-comment">
                      <header className="issue-comment-header">
                        <span className="conversation-entry-title">
                          <strong>{t("System event batch")}</strong>
                          <span className="conversation-entry-kind event">{t("Daemon events")}</span>
                          <span className="conversation-entry-group system">{t("System event")}</span>
                        </span>
                        <span className="conversation-entry-actions">
                          <span className="conversation-entry-meta">
                            {item.entries.length} {t("daemon events")} · {formatTime(firstEntry?.createdAt)}
                            {lastEntry && lastEntry.id !== firstEntry?.id ? ` ${t("to")} ${formatTime(lastEntry.createdAt)}` : ""}
                          </span>
                          <a className="timeline-entry-anchor" href={`#${item.id}`}>
                            {timelineEntryReference(item.id)}
                          </a>
                          <button
                            className={`timeline-entry-link ${copiedEntryId === item.id ? "copied" : ""}`}
                            onClick={() => {
                              void copyTimelineEntryLink(item.id).then((copied) => {
                                if (copied) setCopiedEntryId(item.id);
                              });
                            }}
                            type="button"
                          >
                            {copiedEntryId === item.id ? t("Copied entry link") : t("Copy entry link")}
                          </button>
                        </span>
                      </header>
                      <p className="timeline-event-group-summary">
                        {t("Consecutive low-level daemon events folded to keep the work-order discussion readable.")}
                      </p>
                      <ul className="timeline-event-group-list">
                        {item.entries.map((entry) => {
                          const isLinkedGroupMember = activeEntryHash === entry.id;
                          return (
                            <li className={isLinkedGroupMember ? "linked" : ""} id={entry.id} key={entry.id}>
                              <div className="timeline-event-group-member">
                                <span>
                                  <strong>{entry.title}</strong>
                                  <small>{formatTime(entry.createdAt)}</small>
                                </span>
                                <p>{conversationEntrySummary(entry, "system")}</p>
                              </div>
                              {entry.payload ? (
                                <details className="timeline-payload-collapsed">
                                  <summary className="timeline-payload-toggle">
                                    <span>{t("Show event payload")}</span>
                                    <span className="timeline-payload-summary">{payloadSummary(entry.payload)}</span>
                                  </summary>
                                  <pre className="event-payload">{pretty(entry.payload)}</pre>
                                </details>
                              ) : null}
                            </li>
                          );
                        })}
                      </ul>
                    </div>
                  </div>
                </article>
              );
            }
            const { entry } = item;
            const isLinkedEntry = activeEntryHash === entry.id;
            const entryGroup = conversationEntryGroup(entry);
            return (
              <article
                aria-current={isLinkedEntry ? "location" : undefined}
                className={`conversation-entry ${entry.kind} ${isLinkedEntry ? "linked" : ""}`}
                data-entry-group={entryGroup}
                data-entry-kind={entry.kind}
                id={entry.id}
                key={entry.id}
                role="listitem"
              >
                <span className="timeline-marker" />
                <div className="comment-with-avatar">
                  <span className={`comment-avatar ${entry.kind === "event" ? "small" : ""}`}>
                    {entry.avatar}
                  </span>
                  <div className="issue-comment">
                    <header className="issue-comment-header">
                      <span className="conversation-entry-title">
                        <strong>{entry.title}</strong>
                        <span className={`conversation-entry-kind ${entry.kind}`}>
                          {entry.kind === "comment" ? t("Comment") : t("Daemon event")}
                        </span>
                        <span className={`conversation-entry-group ${entryGroup}`}>
                          {conversationEntryGroupLabel(entryGroup)}
                        </span>
                      </span>
                      <span className="conversation-entry-actions">
                        <span className="conversation-entry-meta">
                          {entry.kind === "comment" ? t("review comment") : t("daemon event")} -{" "}
                          {formatTime(entry.createdAt)}
                        </span>
                        <a className="timeline-entry-anchor" href={`#${entry.id}`}>
                          {timelineEntryReference(entry.id)}
                        </a>
                        <button
                          className={`timeline-entry-link ${copiedEntryId === entry.id ? "copied" : ""}`}
                          onClick={() => {
                            void copyTimelineEntryLink(entry.id).then((copied) => {
                              if (copied) setCopiedEntryId(entry.id);
                            });
                          }}
                          type="button"
                        >
                          {copiedEntryId === entry.id ? t("Copied entry link") : t("Copy entry link")}
                        </button>
                      </span>
                    </header>
                    <div className="markdown-body">
                      {(entry.body ?? "").split(/\n{2,}/).map((paragraph, index) => (
                        <p key={`${entry.id}-paragraph-${index}`}>{paragraph || t("Event payload")}</p>
                      ))}
                    </div>
                    <p className="conversation-entry-summary">{conversationEntrySummary(entry, entryGroup)}</p>
                    {entry.payload ? (
                      <details className="timeline-payload-collapsed">
                        <summary className="timeline-payload-toggle">
                          <span>{t("Show event payload")}</span>
                          <span className="timeline-payload-summary">{payloadSummary(entry.payload)}</span>
                        </summary>
                        <pre className="event-payload">{pretty(entry.payload)}</pre>
                      </details>
                    ) : null}
                  </div>
                </div>
              </article>
            );
          })}
        </div>
      </div>
    </section>
  );
});


function Runs({
  busy = false,
  disabled = false,
  onRetryRun,
  onStopRun,
  runs,
}: {
  busy?: boolean;
  disabled?: boolean;
  onRetryRun?: (runId: string) => void;
  onStopRun?: (runId: string) => void;
  runs: TeaRun[];
}) {
  const actionsEnabled = Boolean(onStopRun || onRetryRun);
  return (
    <section className="runs-panel" aria-label={t("Run history")}>
      <h3>{t("Run history")}</h3>
      {runs.length === 0 ? <p className="empty">{t("No runs yet.")}</p> : null}
      {runs.map((run) => {
        const stopEnabled = canStopRun(run);
        const retryEnabled = canRetryRun(run);
        return (
          <article className="run-card" key={run.id}>
            <div>
              <strong>{run.id}</strong>
              <span aria-label={`${t("Status")}: ${run.status ?? "unknown"}`}>
                {run.status ?? "unknown"}
              </span>
            </div>
            <small>
              {formatTime(run.created_at)} - {formatTime(run.updated_at)}
            </small>
            {run.evidence ? <pre>{pretty(run.evidence)}</pre> : null}
            {actionsEnabled ? (
              <div className="run-actions">
                {onStopRun ? (
                  <button
                    aria-label={
                      stopEnabled
                        ? t("Stop run")
                        : t("Stop is available only for queued, running, or retrying runs.")
                    }
                    className="run-action-stop action-danger"
                    disabled={busy || disabled || !stopEnabled}
                    onClick={() => onStopRun(run.id)}
                    title={
                      stopEnabled
                        ? undefined
                        : t("Stop is available only for queued, running, or retrying runs.")
                    }
                    type="button"
                  >
                    {t("Stop run")}
                  </button>
                ) : null}
                {onRetryRun ? (
                  <button
                    aria-label={
                      retryEnabled
                        ? t("Retry run")
                        : t("Retry is available only for failed or stopped runs.")
                    }
                    className="run-action-retry"
                    disabled={busy || disabled || !retryEnabled}
                    onClick={() => onRetryRun(run.id)}
                    title={
                      retryEnabled ? undefined : t("Retry is available only for failed or stopped runs.")
                    }
                    type="button"
                  >
                    {t("Retry run")}
                  </button>
                ) : null}
              </div>
            ) : null}
            {disabled && actionsEnabled ? (
              <small className="run-actions-note">
                {t("Run actions are disabled for terminal work orders.")}
              </small>
            ) : null}
          </article>
        );
      })}
    </section>
  );
}
