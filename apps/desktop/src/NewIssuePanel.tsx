import { useState } from "react";

import { t } from "./i18n";
import { approvalPolicyOptions, createPriorityOptions } from "./issueFormat";
import type { TicketDraft } from "./issueTypes";

export function NewIssuePanel({
  createNotice,
  creating,
  onCancel,
  onSubmit,
}: {
  createNotice: string;
  creating: boolean;
  onCancel: () => void;
  onSubmit: (draft: TicketDraft) => void;
}) {
  // The new-issue draft is owned here so typing re-renders only this panel,
  // not the whole App. Submission passes the composed draft up to App's
  // submitTicket, which keeps the validation, in-flight gate, and
  // fingerprint/idempotency-key logic. The panel unmounts when it is hidden
  // (successful create or cancel), which clears the draft.
  const [draft, setDraft] = useState<TicketDraft>({
    title: "",
    description: "",
    approvalPolicy: "",
    priority: "",
    labels: "",
  });

  const applyTemplate = (template: "investigation" | "implementation" | "release") => {
    const templates: Record<typeof template, TicketDraft> = {
      investigation: {
        title: "Investigate AI workflow failure",
        description:
          "## Background\nDescribe the failed workflow, current evidence, and expected behavior.\n\n## Acceptance criteria\n- Root cause is identified from logs or runtime data.\n- A minimal fix or follow-up plan is proposed.\n- Verification evidence is attached.",
        approvalPolicy: "plan_only",
        priority: "high",
        labels: "kind:investigation",
      },
      implementation: {
        title: "Implement AI work-order change",
        description:
          "## Goal\nDescribe the product or workflow change.\n\n## Constraints\n- Keep Tea standalone.\n- Preserve UI/headless dual mode.\n\n## Acceptance criteria\n- Code is implemented.\n- Typecheck/build/tests pass.\n- Release smoke is updated if needed.",
        approvalPolicy: "human_before_execute",
        priority: "normal",
        labels: "kind:implementation",
      },
      release: {
        title: "Prepare Tea release validation",
        description:
          "## Release target\nDescribe the package or runtime to validate.\n\n## Checks\n- tea.exe UI launches.\n- tea-daemon.exe headless mode works.\n- tea-cli.exe lifecycle smoke passes.\n- Manifest and checksums are generated.",
        approvalPolicy: "manual_only",
        priority: "normal",
        labels: "kind:release",
      },
    };
    setDraft(templates[template]);
  };

  return (
    <form
      className="new-issue-panel"
      onSubmit={(event) => {
        event.preventDefault();
        onSubmit(draft);
      }}
    >
      <div className="new-issue-header">
        <div>
          <h2>{t("New Work Order")}</h2>
          <p>{t("Create an AI issue with a goal, context, and acceptance criteria.")}</p>
        </div>
        <button className="link-button" onClick={onCancel} type="button">
          {t("Cancel")}
        </button>
      </div>
      <input
        aria-label={t("Work order title")}
        autoFocus
        onChange={(event) => setDraft({ ...draft, title: event.target.value })}
        placeholder={t("Title: e.g. Investigate provider failure in login flow")}
        value={draft.title}
      />
      <textarea
        aria-label={t("Work order description")}
        onChange={(event) => setDraft({ ...draft, description: event.target.value })}
        placeholder={t("Write a markdown description: background, expected result, constraints, and evidence needed.")}
        value={draft.description}
      />
      <section className="template-panel" aria-label={t("Suggested work-order templates")}>
        <h3>{t("Suggested work-order templates")}</h3>
        <div className="quick-template-grid">
          <button onClick={() => applyTemplate("investigation")} type="button">
            {t("Investigation")}
          </button>
          <button onClick={() => applyTemplate("implementation")} type="button">
            {t("Implementation")}
          </button>
          <button onClick={() => applyTemplate("release")} type="button">
            {t("Release validation")}
          </button>
        </div>
      </section>
      <div className="new-issue-metadata" aria-label={t("New work order metadata")}>
        <label className="new-issue-priority">
          <span>{t("Priority")}</span>
          <select
            onChange={(event) => setDraft({ ...draft, priority: event.target.value })}
            value={draft.priority}
          >
            <option value="">{t("Default priority (normal)")}</option>
            {createPriorityOptions.map((option) => (
              <option key={option.value} value={option.value}>
                {t(option.label)}
              </option>
            ))}
          </select>
        </label>
        <label className="new-issue-labels">
          <span>{t("Initial labels")}</span>
          <input
            onChange={(event) => setDraft({ ...draft, labels: event.target.value })}
            placeholder={t("Comma-separated, e.g. area:auth, needs-triage")}
            value={draft.labels}
          />
        </label>
      </div>
      <div className="form-row">
        <select
          onChange={(event) => setDraft({ ...draft, approvalPolicy: event.target.value })}
          value={draft.approvalPolicy}
        >
          <option value="">{t("Default approval policy")}</option>
          {approvalPolicyOptions.map((option) => (
            <option key={option.value} value={option.value}>
              {t(option.label)}
            </option>
          ))}
        </select>
        <button
          className="new-issue-button"
          disabled={creating}
          type="button"
          onClick={() => onSubmit(draft)}
        >
          {creating ? t("Creating...") : t("Submit work order")}
        </button>
      </div>
      {createNotice ? (
        <p className="new-issue-notice" role="status">
          {createNotice}
        </p>
      ) : null}
    </form>
  );
}
