import { memo, useEffect, useRef, useState } from "react";

import { conversationEntryGroupLabel } from "./conversation";
import { t, useLocale } from "./i18n";
import {
  daemonLabelsForTicket,
  formatTime,
  isClosedTicket,
  issueAgeLabel,
  issueNumber,
  issueStateLabel,
  issueSummary,
  latestTouchLabel,
} from "./issueFormat";
import {
  badgeToneForPriority,
  badgeToneForRisk,
  issueActionHintForTicket,
  issueSignalForTicket,
  type IssueActionHint,
  type IssueSignal,
} from "./issueSignals";
import type { IssueListDensity, IssueMetrics } from "./issueTypes";
import type { TeaTicket } from "./teaClient";

const searchCommitDelayMs = 150;

export function IssueSearchInput({
  onChange,
  value,
}: {
  onChange: (query: string) => void;
  value: string;
}) {
  // Local echo of the search box. Typing re-renders only this input; App's
  // searchQuery (and the filtered queue) update on blur, Enter, or a short
  // debounce — matching the settings URL/token commit pattern.
  const [localValue, setLocalValue] = useState(value);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  useEffect(() => {
    setLocalValue(value);
  }, [value]);

  useEffect(() => {
    if (localValue === value) return;
    const timer = window.setTimeout(() => onChangeRef.current(localValue), searchCommitDelayMs);
    return () => window.clearTimeout(timer);
  }, [localValue, value]);

  const commit = () => {
    if (localValue === value) return;
    onChangeRef.current(localValue);
  };

  return (
    <input
      aria-label={t("Search work orders")}
      className="issue-search"
      onBlur={commit}
      onChange={(event) => setLocalValue(event.target.value)}
      onKeyDown={(event) => {
        if (event.key === "Enter") commit();
      }}
      placeholder={t("Search issues...")}
      value={localValue}
    />
  );
}

export const IssueListRow = memo(function IssueListRow({
  actionHint: actionHintProp,
  density,
  isWatched,
  localNotes,
  metrics,
  onSelect,
  selected,
  signal,
  ticket,
}: {
  actionHint?: IssueActionHint;
  density: IssueListDensity;
  isWatched: boolean;
  localNotes: string[] | undefined;
  metrics: IssueMetrics | undefined;
  onSelect: (ticketId: string) => void;
  selected: boolean;
  signal?: IssueSignal;
  ticket: TeaTicket;
}) {
  // Subscribe to locale changes: this component is memo'd, so without the
  // subscription a language toggle would leave rows in the old language until
  // their data changed.
  useLocale();
  const latestTouch = metrics?.latestTouch;
  const issueSignal = signal ?? issueSignalForTicket(ticket, metrics);
  const actionHint = actionHintProp ?? issueActionHintForTicket(ticket, metrics, issueSignal);
  const ownerLabel = `${t("Owner")} ${ticket.owner_human_id ?? t("Tea local operator")}`;
  const agentLabel = `${t("Agent")} ${ticket.delegated_agent_id ?? t("not delegated")}`;
  const ticketLocalNotes = (localNotes ?? []).filter(Boolean);
  return (
    <button
      aria-current={selected ? "true" : undefined}
      className={`issue-item ${selected ? "selected issue-item-selected" : ""}`}
      onClick={() => onSelect(ticket.id)}
      type="button"
    >
      <span className={`issue-state ${isClosedTicket(ticket) ? "closed" : "open"}`}>
        {t(issueStateLabel(ticket))}
      </span>
      <span className="issue-item-main">
        <span className="issue-item-topline">
          <strong>{ticket.title}</strong>
          <small>{issueNumber(ticket)}</small>
        </span>
        <small>{t("opened")} {formatTime(ticket.created_at)} · Tea</small>
      </span>
      <span className="issue-item-summary">{issueSummary(ticket)}</span>
      <span className="issue-item-context">
        <span className="issue-item-meta">
          <span>{issueAgeLabel(ticket)}</span>
          {metrics ? (
            <span>
              {metrics.comments} {t("comments")} · {metrics.runs} {t("runs")}
            </span>
          ) : (
            <span>{t("Activity loading")}</span>
          )}
        </span>
        {latestTouch ? (
          <span className="issue-item-latest-touch">
            <span className={`issue-touch-badge ${latestTouch.group}`}>
              {conversationEntryGroupLabel(latestTouch.group)}
            </span>
            <span>{latestTouchLabel(latestTouch)}</span>
          </span>
        ) : null}
        <span className="issue-item-routing">
          <span>{ownerLabel}</span>
          <span>{agentLabel}</span>
        </span>
      </span>
      {density === "compact" ? (
        <span className="issue-item-compact-scanline" aria-label={`Compact scanline for ${ticket.title}`}>
          <span className="issue-signal-with-reason">
            <span className={`issue-signal-chip tone-${issueSignal.tone}`} title={issueSignal.description}>
              {t(issueSignal.label)}
            </span>
            <span className="issue-signal-reason">{issueSignal.reason}</span>
          </span>
          <span className={`issue-action-hint tone-${actionHint.tone}`} title={actionHint.description}>
            {t(actionHint.label)}
          </span>
          {latestTouch ? (
            <span className="issue-item-latest-touch">
              <span className={`issue-touch-badge ${latestTouch.group}`}>
                {conversationEntryGroupLabel(latestTouch.group)}
              </span>
              <span>{latestTouchLabel(latestTouch)}</span>
            </span>
          ) : (
            <span className="issue-item-latest-touch muted">
              <span className="issue-touch-badge system">{t("Idle")}</span>
              <span>{latestTouchLabel(undefined)}</span>
            </span>
          )}
          <span className="issue-item-routing">
            <span>{ownerLabel}</span>
            <span>{agentLabel}</span>
          </span>
        </span>
      ) : (
        <span className="issue-item-badges">
          <span className="issue-signal-with-reason">
            <span className={`issue-signal-chip tone-${issueSignal.tone}`} title={issueSignal.description}>
              {t(issueSignal.label)}
            </span>
            <span className="issue-signal-reason">{issueSignal.reason}</span>
          </span>
          <span className={`issue-action-hint tone-${actionHint.tone}`} title={actionHint.description}>
            {t(actionHint.label)}
          </span>
          <span className={`issue-badge tone-${badgeToneForPriority(ticket.priority)}`}>
            {t("Priority")} {t(ticket.priority ?? "normal")}
          </span>
          <span className={`issue-badge tone-${badgeToneForRisk(ticket.risk_level)}`}>
            {t("Risk")} {t(ticket.risk_level ?? "medium")}
          </span>
          {isWatched ? (
            <span className="issue-badge tone-watch">{t("Watched")}</span>
          ) : null}
        </span>
      )}
      <span className="issue-item-footer">
        <span className="issue-item-labels">
          {daemonLabelsForTicket(ticket).map((label) => (
            <span key={label}>{label}</span>
          ))}
          {ticketLocalNotes.map((note) => (
            <span className="issue-item-note" key={`note-${note}`}>
              {note}
            </span>
          ))}
        </span>
        <span className="issue-metrics" aria-label={`Activity for ${ticket.title}`}>
          {metrics ? (
            <>
              <span>{metrics.comments} {t("comments")}</span>
              <span>{metrics.runs} {t("runs")}</span>
            </>
          ) : (
            <span>{t("Activity loading")}</span>
          )}
        </span>
      </span>
    </button>
  );
});
