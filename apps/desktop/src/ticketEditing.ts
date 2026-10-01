import { isSystemLabel, operatorLabelsForTicket } from "./issueFormat";
import type { TicketEditDraft } from "./issueTypes";
import type { TeaTicket, UpdateTicketInput } from "./teaClient";

export interface TicketEditSubmission {
  ticketId: string;
  baseline: TicketEditDraft;
  draft: TicketEditDraft;
}

export function ticketEditDraft(ticket: TeaTicket): TicketEditDraft {
  return {
    title: ticket.title ?? "",
    description: ticket.description ?? "",
    priority: ticket.priority ?? "",
    labels: operatorLabelsForTicket(ticket).join(", "),
  };
}

const operatorLabels = (value: string): string[] => Array.from(new Set(value
  .split(/[,\n]/).map((label) => label.trim()).filter(Boolean)
  .filter((label) => !isSystemLabel(label))));

// Compare with the immutable opening baseline. Comparing against a refreshed
// ticket would misclassify stale, untouched form values as intentional edits.
// Same-field concurrent writes retain the API's existing last-write behavior.
export function ticketEditPatch(baseline: TicketEditDraft, draft: TicketEditDraft): UpdateTicketInput {
  const patch: UpdateTicketInput = {};
  if (draft.title.trim() !== baseline.title.trim()) patch.title = draft.title.trim();
  if (draft.description !== baseline.description) patch.description = draft.description;
  if (draft.priority.trim() !== baseline.priority.trim()) patch.priority = draft.priority.trim();
  const labels = operatorLabels(draft.labels);
  const originalLabels = operatorLabels(baseline.labels);
  if (labels.length !== originalLabels.length || labels.some((label, index) => label !== originalLabels[index])) {
    patch.labels = labels;
  }
  return patch;
}
