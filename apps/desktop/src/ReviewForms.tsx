import { t } from "./i18n";
import { useReviewDraft, type ReviewDraftScope, type ReviewDraftSubmission } from "./reviewDraft";

export function RejectReasonForm({
  scope,
  busy,
  onReject,
}: {
  scope: ReviewDraftScope;
  busy: boolean;
  onReject: (submission: ReviewDraftSubmission) => Promise<boolean>;
}) {
  const { draft, setText, submit } = useReviewDraft(scope);

  return (
    <form
      className="reject-reason-form"
      onSubmit={(event) => {
        event.preventDefault();
        void submit(onReject);
      }}
    >
      <label htmlFor="reject-reason-input">{t("Reject with reason")}</label>
      <textarea
        disabled={busy}
        id="reject-reason-input"
        onChange={(event) => setText(event.target.value)}
        placeholder={t("Explain why this approval is rejected.")}
        rows={2}
        value={draft.text}
      />
      <button className="action-danger" disabled={busy} type="submit">
        {t("Reject approval")}
      </button>
    </form>
  );
}


export function CommentEditor({
  scope,
  busy,
  disabled,
  onSubmit,
}: {
  scope: ReviewDraftScope;
  busy: boolean;
  disabled: boolean;
  onSubmit: (submission: ReviewDraftSubmission) => Promise<boolean>;
}) {
  const { draft, setText, setMode, submit } = useReviewDraft(scope);
  const { text: value, mode } = draft;

  return (
    <form className="comment-editor" onSubmit={(event) => { event.preventDefault(); void submit(onSubmit); }}>
      <div className="comment-with-avatar">
        <span className="comment-avatar">{t("Me")}</span>
        <section className="issue-comment">
          <header className="issue-comment-header">
            <strong>{t("Leave a review comment")}</strong>
            <span>{disabled ? t("terminal ticket") : t("markdown supported")}</span>
          </header>
          <div className="comment-editor-tabs" role="tablist" aria-label={t("Comment editor mode")}>
            <button
              aria-selected={mode === "write"}
              className={mode === "write" ? "active" : ""}
              onClick={() => setMode("write")}
              role="tab"
              type="button"
            >
              {t("Write")}
            </button>
            <button
              aria-selected={mode === "preview"}
              className={mode === "preview" ? "active" : ""}
              onClick={() => setMode("preview")}
              role="tab"
              type="button"
            >
              {t("Preview comment")}
            </button>
          </div>
          {mode === "write" ? (
            <textarea
              disabled={disabled || busy}
              onChange={(event) => setText(event.target.value)}
              placeholder={
                disabled
                  ? "Closed and cancelled tickets are read-only."
                  : "Write context, decisions, review notes, or acceptance evidence."
              }
              value={value}
            />
          ) : (
            <div className="comment-preview">
              <div className="markdown-body">
                {value.trim() ? (
                  value.split(/\n{2,}/).map((paragraph, index) => (
                    <p key={`comment-preview-${index}`}>{paragraph}</p>
                  ))
                ) : (
                  <p>{t("Nothing to preview yet.")}</p>
                )}
              </div>
            </div>
          )}
          <footer className="comment-editor-footer">
            <span>{t("Comments are durable and included in JSON/Markdown exports.")}</span>
            <button disabled={disabled || busy || !value.trim()} type="submit">
              {t("Comment")}
            </button>
          </footer>
        </section>
      </div>
    </form>
  );
}

