import { useState } from "react";
import { t } from "./i18n";
import type { TicketEditDraft } from "./issueTypes";
import type { TeaTicket } from "./teaClient";
import { ticketEditDraft, type TicketEditSubmission } from "./ticketEditing";

export function IssueEditForm({
  busy,
  onCancel,
  onSubmit,
  ticket,
}: {
  busy: boolean;
  onCancel: () => void;
  onSubmit: (submission: TicketEditSubmission) => void;
  ticket: TeaTicket;
}) {
  // The edit draft is owned here so typing re-renders only this form. The form
  // mounts when editing opens, seeding the draft from the ticket at that
  // moment. Labels seed from the ticket's authoritative daemon labels, minus
  // the system-derived ones the daemon manages (source:/policy:/context:).
  const [session] = useState(() => ({ ticketId: ticket.id, baseline: ticketEditDraft(ticket) }));
  const [draft, setDraft] = useState<TicketEditDraft>(session.baseline);

  return (
    <form
      className="issue-edit-form"
      onSubmit={(event) => {
        event.preventDefault();
        onSubmit({ ...session, draft });
      }}
    >
      <label>
        <span>{t("Title")}</span>
        <input
          value={draft.title}
          onChange={(event) => setDraft({ ...draft, title: event.target.value })}
          placeholder={t("Work order title")}
        />
      </label>
      <label>
        <span>{t("Description")}</span>
        <textarea
          value={draft.description}
          onChange={(event) => setDraft({ ...draft, description: event.target.value })}
          placeholder={t("Work order description")}
          rows={4}
        />
      </label>
      <label>
        <span>{t("Priority")}</span>
        <input
          value={draft.priority}
          onChange={(event) => setDraft({ ...draft, priority: event.target.value })}
          placeholder={t("e.g. high, normal, low")}
        />
      </label>
      <label>
        <span>{t("Labels")}</span>
        <input
          value={draft.labels}
          onChange={(event) => setDraft({ ...draft, labels: event.target.value })}
          placeholder={t("comma-separated operator labels")}
        />
      </label>
      <div className="issue-edit-actions">
        <button type="submit" disabled={busy}>
          {t("Save changes")}
        </button>
        <button type="button" onClick={onCancel} disabled={busy}>
          {t("Cancel")}
        </button>
      </div>
    </form>
  );
}

