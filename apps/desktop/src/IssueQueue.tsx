import { memo, type Dispatch, type ReactNode, type SetStateAction } from "react";

import { t, useLocale } from "./i18n";
import { IssueListRow, IssueSearchInput } from "./IssueList";
import type { IssueFilter, IssueListDensity, IssueMetrics, IssueSort } from "./issueTypes";
import type { IssueActionHint, IssueSignal, IssueSignalFilter } from "./issueSignals";
import type { LocalNotes, WatchStates } from "./localMetadata";
import type { TeaTicket } from "./teaClient";

type ActiveIssueFilterChip = {
  key: "author" | "label" | "priority" | "risk" | "search" | "signal" | "state" | "watch";
  label: string;
  value: string;
};

type IssuePresetQueue = {
  description: string;
  issueFilter: IssueFilter;
  key: "active" | "default" | "queued" | "resolved" | "review" | "stale";
  label: string;
  signalFilter: IssueSignalFilter;
};

type IssueSignalFilterOption = {
  label: string;
  tone: IssueSignal["tone"] | "default";
  value: IssueSignalFilter;
};

type IssuePriorityFilter = "all" | "high";
type IssueRiskFilter = "all" | "high";
type IssueWatchFilter = "all" | "watched";

// Memoized: App re-renders on every auto-refresh poll (the refresh timestamp and
// spinner flag change each time) while the queue's own inputs usually do not, so
// without this the whole toolbar/filter/list tree was rebuilt on every poll.
export const IssueQueue = memo(function IssueQueue({
  activeIssueFilterChips,
  activeIssuePresetQueue,
  activeIssuePresetQueueKey,
  authorFilter,
  availableAuthors,
  availableLabels,
  canCollapseIssueList,
  children,
  closedCount,
  filteredTickets,
  hasActiveIssueFilters,
  hasMoreVisibleTickets,
  issueFilter,
  issueFilterLabel,
  issueListDensity,
  issueMetrics,
  issuePresetQueueCounts,
  issuePresetQueues,
  issuePriorityFilter,
  issueQueueSummaryItems,
  issueRiskFilter,
  issueSignalCountForOption,
  issueSignalFilter,
  issueSignalFilterOptions,
  issueSignalFilterSummary,
  issueSort,
  issueTriageFilterCounts,
  issueViewPreferenceStatus,
  issueViewPreferencesAreDefault,
  issueWatchFilter,
  localNotes,
  onApplyIssuePresetQueue,
  onClearAuthorFilter,
  onClearIssueFilters,
  onClearLabelFilter,
  onClearWatchedIssueFilter,
  onCollapseIssueList,
  onRemoveIssueFilterChip,
  onResetIssueViewPreferences,
  onSearchChange,
  onSelectIssue,
  onShowMoreIssues,
  openCount,
  searchQuery,
  selectedId,
  selectedLabelFilter,
  setAuthorFilter,
  setIssueFilter,
  setIssueListDensity,
  setIssuePriorityFilter,
  setIssueRiskFilter,
  setIssueSignalFilter,
  setIssueSort,
  setIssueWatchFilter,
  setSelectedLabelFilter,
  setShowAdvancedFilters,
  setShowAuthorFilterPanel,
  setShowLabelFilterPanel,
  showAdvancedFilters,
  showAuthorFilterPanel,
  showLabelFilterPanel,
  sortedTicketsLength,
  ticketActionHints,
  ticketCount,
  ticketSignals,
  visibleTickets,
  watchStates,
}: {
  activeIssueFilterChips: ActiveIssueFilterChip[];
  activeIssuePresetQueue: { description: string; label: string } | null;
  activeIssuePresetQueueKey: string | null;
  authorFilter: string | null;
  availableAuthors: string[];
  availableLabels: string[];
  canCollapseIssueList: boolean;
  children?: ReactNode;
  closedCount: number;
  filteredTickets: TeaTicket[];
  hasActiveIssueFilters: boolean;
  hasMoreVisibleTickets: boolean;
  issueFilter: IssueFilter;
  issueFilterLabel: string;
  issueListDensity: IssueListDensity;
  issueMetrics: Record<string, IssueMetrics>;
  issuePresetQueueCounts: Record<string, number>;
  issuePresetQueues: IssuePresetQueue[];
  issuePriorityFilter: IssuePriorityFilter;
  issueQueueSummaryItems: Array<{ label: string; value: string }>;
  issueRiskFilter: IssueRiskFilter;
  issueSignalCountForOption: (value: IssueSignalFilter) => number;
  issueSignalFilter: IssueSignalFilter;
  issueSignalFilterOptions: IssueSignalFilterOption[];
  issueSignalFilterSummary: string;
  issueSort: IssueSort;
  issueTriageFilterCounts: { highPriority: number; highRisk: number; watched: number };
  issueViewPreferenceStatus: string;
  issueViewPreferencesAreDefault: boolean;
  issueWatchFilter: IssueWatchFilter;
  localNotes: LocalNotes;
  onApplyIssuePresetQueue: (queue: IssuePresetQueue) => void;
  onClearAuthorFilter: () => void;
  onClearIssueFilters: () => void;
  onClearLabelFilter: () => void;
  onClearWatchedIssueFilter: () => void;
  onCollapseIssueList: () => void;
  onRemoveIssueFilterChip: (key: ActiveIssueFilterChip["key"]) => void;
  onResetIssueViewPreferences: () => void;
  onSearchChange: (query: string) => void;
  onSelectIssue: (ticketId: string) => void;
  onShowMoreIssues: () => void;
  openCount: number;
  searchQuery: string;
  selectedId: string | null;
  selectedLabelFilter: string | null;
  setAuthorFilter: Dispatch<SetStateAction<string | null>>;
  setIssueFilter: Dispatch<SetStateAction<IssueFilter>>;
  setIssueListDensity: Dispatch<SetStateAction<IssueListDensity>>;
  setIssuePriorityFilter: Dispatch<SetStateAction<IssuePriorityFilter>>;
  setIssueRiskFilter: Dispatch<SetStateAction<IssueRiskFilter>>;
  setIssueSignalFilter: Dispatch<SetStateAction<IssueSignalFilter>>;
  setIssueSort: Dispatch<SetStateAction<IssueSort>>;
  setIssueWatchFilter: Dispatch<SetStateAction<IssueWatchFilter>>;
  setSelectedLabelFilter: Dispatch<SetStateAction<string | null>>;
  setShowAdvancedFilters: Dispatch<SetStateAction<boolean>>;
  setShowAuthorFilterPanel: Dispatch<SetStateAction<boolean>>;
  setShowLabelFilterPanel: Dispatch<SetStateAction<boolean>>;
  showAdvancedFilters: boolean;
  showAuthorFilterPanel: boolean;
  showLabelFilterPanel: boolean;
  sortedTicketsLength: number;
  ticketActionHints: Record<string, IssueActionHint>;
  ticketCount: number;
  ticketSignals: Record<string, IssueSignal>;
  visibleTickets: TeaTicket[];
  watchStates: WatchStates;
}) {
  // Subscribe to locale changes: this component is memo'd, so without the
  // subscription a language toggle would leave the toolbar in the old language
  // until one of its props changed.
  useLocale();

  return (
    <section className="issue-index" aria-label={t("Work order index")}>
      <div className="issue-toolbar">
        <div className="issue-preset-queue-lane" aria-label={t("Preset work-order queues")}>
          {issuePresetQueues.map((queue) => (
            <button
              aria-pressed={activeIssuePresetQueueKey === queue.key}
              className="issue-preset-queue-card"
              key={queue.key}
              onClick={() => onApplyIssuePresetQueue(queue)}
              type="button"
            >
              <span>
                <strong>{issuePresetQueueCounts[queue.key]}</strong>
                <span>{t(queue.label)}</span>
              </span>
              <small>{t(queue.description)}</small>
            </button>
          ))}
        </div>
        <div className="issue-queue-summary-card">
          <div>
            <span>{t("Current queue")}</span>
            <strong>{activeIssuePresetQueue ? t(activeIssuePresetQueue.label) : t("Custom filtered queue")}</strong>
            <small>
              {activeIssuePresetQueue
                ? t(activeIssuePresetQueue.description)
                : t("A custom combination of state, signal, search, author, label, priority, and risk filters.")}
            </small>
            <span className={`issue-view-preference-status ${issueViewPreferencesAreDefault ? "default" : "saved"}`}>
              {issueViewPreferenceStatus}
            </span>
            <div className="issue-queue-summary-actions">
              <button disabled={issueViewPreferencesAreDefault} onClick={onResetIssueViewPreferences} type="button">
                {t("Reset view")}
              </button>
            </div>
          </div>
          <div className="issue-queue-summary-grid" aria-label={t("Current queue summary")}>
            {issueQueueSummaryItems.map((item) => (
              <span key={t(item.label)}>
                <small>{t(item.label)}</small>
                <strong>{item.value}</strong>
              </span>
            ))}
          </div>
        </div>
        <div className="issue-filter-tabs" role="tablist" aria-label={t("Issue state filters")}>
          <button
            aria-selected={issueFilter === "open"}
            className={issueFilter === "open" ? "active" : ""}
            onClick={() => setIssueFilter("open")}
            role="tab"
          >
            {t("Open")} <span>{openCount}</span>
          </button>
          <button
            aria-selected={issueFilter === "closed"}
            className={issueFilter === "closed" ? "active" : ""}
            onClick={() => setIssueFilter("closed")}
            role="tab"
          >
            {t("Closed")} <span>{closedCount}</span>
          </button>
          <button
            aria-selected={issueFilter === "all"}
            className={issueFilter === "all" ? "active" : ""}
            onClick={() => setIssueFilter("all")}
            role="tab"
          >
            {t("All")} <span>{ticketCount}</span>
          </button>
          <button
            aria-expanded={showAdvancedFilters}
            aria-pressed={showAdvancedFilters}
            className={`issue-advanced-filter-toggle ${showAdvancedFilters ? "active" : ""}`}
            onClick={() => setShowAdvancedFilters((value) => !value)}
            type="button"
          >
            {t("Filters")}
            {activeIssueFilterChips.length > 0 ? <span>{activeIssueFilterChips.length}</span> : null}
          </button>
        </div>
        <div className="issue-query-bar">
          <button disabled={!hasActiveIssueFilters} onClick={onClearIssueFilters} type="button">
            {t("Clear filters")}
          </button>
          <IssueSearchInput onChange={onSearchChange} value={searchQuery} />
          <button type="button" onClick={() => setShowAuthorFilterPanel((value) => !value)}>
            {t("Author")} {authorFilter ? `(${authorFilter})` : ""}
          </button>
          <button
            aria-expanded={showLabelFilterPanel}
            data-testid="label-filter-toggle"
            type="button"
            onClick={() => setShowLabelFilterPanel((value) => !value)}
          >
            {t("Labels")} {selectedLabelFilter ? `(${selectedLabelFilter})` : ""}
          </button>
          <label className="issue-signal-filter-control">
            <span>{t("Signal")}</span>
            <select
              aria-label={t("Filter work orders by signal")}
              onChange={(event) => setIssueSignalFilter(event.target.value as IssueSignalFilter)}
              value={issueSignalFilter}
            >
              {issueSignalFilterOptions.map((option) => (
                <option key={option.value} value={option.value}>
                  {t(option.label)}
                </option>
              ))}
            </select>
          </label>
          <label className="issue-sort-control">
            <span>{t("Sort")}</span>
            <select
              aria-label={t("Sort work orders")}
              onChange={(event) => setIssueSort(event.target.value as IssueSort)}
              value={issueSort}
            >
              <option value="updated">{t("Recently updated")}</option>
              <option value="created">{t("Newest created")}</option>
              <option value="activity">{t("Most activity")}</option>
              <option value="touch">{t("Latest touch")}</option>
            </select>
          </label>
          <label className="issue-density-control">
            <span>{t("Density")}</span>
            <select
              aria-label={t("Issue list density")}
              onChange={(event) => setIssueListDensity(event.target.value as IssueListDensity)}
              value={issueListDensity}
            >
              <option value="comfortable">{t("Comfortable")}</option>
              <option value="compact">{t("Compact")}</option>
            </select>
          </label>
        </div>
        {showAdvancedFilters ? (
          <>
            <div className="issue-signal-filter-lane" aria-label={t("Signal queue filters")}>
              {issueSignalFilterOptions.map((option) => (
                <button
                  aria-pressed={issueSignalFilter === option.value}
                  className={`issue-signal-filter-pill tone-${option.tone}`}
                  key={option.value}
                  onClick={() => setIssueSignalFilter(option.value)}
                  type="button"
                >
                  <span>{t(option.label)}</span>
                  <strong>{issueSignalCountForOption(option.value)}</strong>
                </button>
              ))}
            </div>
            <div className="issue-triage-filter-lane" aria-label={t("Priority and risk quick filters")}>
              <button
                aria-pressed={issuePriorityFilter === "high"}
                className="issue-triage-filter-button priority"
                onClick={() => setIssuePriorityFilter((current) => (current === "high" ? "all" : "high"))}
                type="button"
              >
                <span>{t("High priority")}</span>
                <strong>{issueTriageFilterCounts.highPriority}</strong>
              </button>
              <button
                aria-pressed={issueRiskFilter === "high"}
                className="issue-triage-filter-button risk"
                onClick={() => setIssueRiskFilter((current) => (current === "high" ? "all" : "high"))}
                type="button"
              >
                <span>{t("High risk")}</span>
                <strong>{issueTriageFilterCounts.highRisk}</strong>
              </button>
              <button
                aria-pressed={issueWatchFilter === "watched"}
                className="issue-triage-filter-button watch"
                onClick={() => setIssueWatchFilter((current) => (current === "watched" ? "all" : "watched"))}
                type="button"
              >
                <span>{t("Watched")}</span>
                <strong>{issueTriageFilterCounts.watched}</strong>
              </button>
            </div>
          </>
        ) : null}
        {hasActiveIssueFilters ? (
          <div className="active-filter-chip-row" aria-label={t("Active issue filters")}>
            <span className="active-filter-chip-label">{t("Active filters")}</span>
            {activeIssueFilterChips.map((chip) => (
              <button
                aria-label={`Remove filter ${t(chip.label)}: ${chip.value}`}
                className="active-filter-chip"
                key={chip.key}
                onClick={() => onRemoveIssueFilterChip(chip.key)}
                type="button"
              >
                <span>{t(chip.label)}</span>
                <strong>{chip.value}</strong>
                <span aria-hidden="true">×</span>
              </button>
            ))}
            <button className="clear-filter-link" onClick={onClearIssueFilters} type="button">
              {t("Clear all filters")}
            </button>
          </div>
        ) : null}
        <div className="author-filter-panel">
          {showAuthorFilterPanel ? (
            <>
              <div className="label-filter-panel-header">
                <strong>{t("Author filters")}</strong>
                <button type="button" onClick={onClearAuthorFilter}>
                  {t("Clear author")}
                </button>
              </div>
              <div className="label-filter-list">
                {availableAuthors.length > 0 ? (
                  availableAuthors.map((author) => (
                    <button
                      aria-pressed={authorFilter === author}
                      className={authorFilter === author ? "active" : ""}
                      key={author}
                      onClick={() => setAuthorFilter((current) => (current === author ? null : author))}
                      type="button"
                    >
                      {author}
                    </button>
                  ))
                ) : (
                  <span className="label-filter-empty">{t("No authors available for filtering.")}</span>
                )}
              </div>
            </>
          ) : authorFilter ? (
            <div className="label-filter-summary">
              <span>
                {t("Filtering by author:")} <strong>{authorFilter}</strong>
              </span>
              <button type="button" onClick={onClearAuthorFilter}>
                {t("Clear author")}
              </button>
            </div>
          ) : null}
        </div>
        <div className="label-filter-panel" data-testid="label-filter-panel">
          {showLabelFilterPanel ? (
            <>
              <div className="label-filter-panel-header">
                <strong>{t("Label filters")}</strong>
                <button data-testid="label-filter-clear" type="button" onClick={onClearLabelFilter}>
                  {t("Clear filter")}
                </button>
              </div>
              <div className="label-filter-list">
                {availableLabels.length > 0 ? (
                  availableLabels.map((label) => (
                    <button
                      aria-pressed={selectedLabelFilter === label}
                      className={selectedLabelFilter === label ? "active" : ""}
                      data-testid={`label-filter-option-${label}`}
                      key={label}
                      onClick={() =>
                        setSelectedLabelFilter((current) => (current === label ? null : label))
                      }
                      type="button"
                    >
                      {label}
                    </button>
                  ))
                ) : (
                  <span className="label-filter-empty">{t("No labels available for filtering.")}</span>
                )}
              </div>
            </>
          ) : selectedLabelFilter ? (
            <div className="label-filter-summary">
              <span>
                {t("Filtering by label:")} <strong>{selectedLabelFilter}</strong>
              </span>
              <button type="button" onClick={onClearLabelFilter}>
                {t("Clear filter")}
              </button>
            </div>
          ) : null}
        </div>
        <p className="issue-count-summary">
          {t("Showing")} {visibleTickets.length} / {filteredTickets.length} {issueFilterLabel}
          {"，"}
          {t("Signal focus:")} {issueSignalFilterSummary}
          {"，"}
          {openCount} {t("open")}
          {"，"}
          {closedCount} {t("closed")}
        </p>
      </div>

      {children}

      <div className="issue-list" data-density={issueListDensity}>
        {visibleTickets.map((ticket) => (
          <IssueListRow
            actionHint={ticketActionHints[ticket.id]}
            density={issueListDensity}
            isWatched={Boolean(watchStates[ticket.id])}
            key={ticket.id}
            localNotes={localNotes[ticket.id]}
            metrics={issueMetrics[ticket.id]}
            onSelect={onSelectIssue}
            selected={selectedId === ticket.id}
            signal={ticketSignals[ticket.id]}
            ticket={ticket}
          />
        ))}
        {filteredTickets.length === 0 ? (
          issueWatchFilter === "watched" ? (
            <div className="watched-empty-state">
              <strong>{t("No watched work orders match this queue.")}</strong>
              <span>{t("Watch a work order locally, or show all work orders for the current filters.")}</span>
              <button type="button" onClick={onClearWatchedIssueFilter}>
                {t("Show all work orders")}
              </button>
            </div>
          ) : (
            <p className="empty">{t("No work orders match this filter.")}</p>
          )
        ) : null}
        {filteredTickets.length > 0 ? (
          <div className="issue-list-pagination">
            <span>
              {t("Showing")} {visibleTickets.length} / {sortedTicketsLength}{" "}
              {t("work orders in the current queue.")}
            </span>
            <div className="issue-list-pagination-actions">
              {hasMoreVisibleTickets ? (
                <button type="button" onClick={onShowMoreIssues}>
                  {t("Show more work orders")}
                </button>
              ) : null}
              {canCollapseIssueList ? (
                <button type="button" onClick={onCollapseIssueList}>
                  {t("Collapse list")}
                </button>
              ) : null}
            </div>
          </div>
        ) : null}
      </div>
    </section>
  );
});
