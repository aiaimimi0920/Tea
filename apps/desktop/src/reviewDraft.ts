import { useState } from "react";
import type { TeaClientOptions } from "./teaClient";

export interface ReviewDraftScope {
  readonly ticketId: string;
  // Existing immutable connection options; kept in memory, never logged.
  readonly connection: TeaClientOptions;
}
export interface ReviewDraftSubmission {
  scope: ReviewDraftScope;
  text: string;
}
interface Draft extends ReviewDraftSubmission {
  mode: "write" | "preview";
}
const empty = (scope: ReviewDraftScope): Draft => ({ scope, text: "", mode: "write" });

export function useReviewDraft(scope: ReviewDraftScope) {
  const [draft, setDraft] = useState(() => empty(scope));
  // Reset during the render that changes ownership, before stale text can be
  // committed under a different task/connection. A passive effect is too late.
  if (draft.scope !== scope) setDraft(empty(scope));
  const setText = (text: string) => setDraft((current) => ({ ...current, text }));
  const setMode = (mode: Draft["mode"]) => setDraft((current) => ({ ...current, mode }));
  const submit = async (onSubmit: (value: ReviewDraftSubmission) => Promise<boolean>) => {
    const submitted = draft;
    if (submitted.scope !== scope) return;
    if (await onSubmit({ scope, text: submitted.text })) {
      // A response from an unmounted/older owner cannot erase a newer draft.
      setDraft((current) => current.scope === scope && current.text === submitted.text
        ? { ...current, text: "" } : current);
    }
  };
  return { draft, setText, setMode, submit };
}
