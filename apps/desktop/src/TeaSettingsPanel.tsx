import { useEffect, useState } from "react";
import { safeLoomPanelUrl } from "./externalLinks";
import { t } from "./i18n";
import {
  approvalPolicyOptions, configurationDetailsOf, configurationSourceOf,
  executionProviderOf, localConfigOf, pretty, statusText, storeBackendOf,
} from "./issueFormat";
import type { TeaClientOptions, TeaLocalConfig, TeaSnapshot } from "./teaClient";

// Configuration belongs to the daemon, independently of any selected work order.
export function TeaSettingsPanel({ busy, connection, snapshot, onSaveConfiguration }: {
  busy: boolean;
  connection: TeaClientOptions;
  snapshot: TeaSnapshot | null;
  onSaveConfiguration: (config: Partial<TeaLocalConfig>, connection: TeaClientOptions) => void;
}) {
  const configurationSource = configurationSourceOf(snapshot);
  const loomManaged = configurationSource === "loom-managed";
  const details = configurationDetailsOf(snapshot);
  const localConfig = localConfigOf(snapshot);
  const loomPanelUrl = safeLoomPanelUrl(details?.loom_panel_url);
  const fallbackReason = typeof details?.reason === "string" ? details.reason : null;

  return (
    <section className="section-focus-card" aria-label={t("Focused settings section")}>
      <header>
        <h3>{t("Connection and ownership")}</h3>
        <span>{statusText(snapshot)}</span>
      </header>
      <dl className="focused-settings-list">
        <div>
          <dt>{t("Configuration source")}</dt>
          <dd className={`config-source-badge ${configurationSource}`}>{configurationSource}</dd>
        </div>
        <div>
          <dt>{t("Daemon mode")}</dt>
          <dd>{snapshot?.status ? t("HTTP API online") : snapshot?.health ? t("Health endpoint only") : t("Offline")}</dd>
        </div>
        <div>
          <dt>{t("Execution provider")}</dt>
          <dd>{executionProviderOf(snapshot)}</dd>
        </div>
        <div>
          <dt>{t("Store backend")}</dt>
          <dd>{storeBackendOf(snapshot)}</dd>
        </div>
      </dl>
      {loomManaged ? (
        <div className="loom-managed-settings">
          <strong>{t("Loom manages Tea configuration.")}</strong>
          <p>{t("Tea-local settings are read-only while Loom owns Tea configuration. Change these settings from Loom instead.")}</p>
          {loomPanelUrl ? (
            <a className="loom-settings-link" href={loomPanelUrl} rel="noreferrer" target="_blank">
              {t("Open Loom Tea settings")}
            </a>
          ) : (
            <span className="loom-settings-missing">
              {t("Loom did not provide a Tea configuration panel URL.")}
            </span>
          )}
        </div>
      ) : (
        <SettingsConfigEditor
          busy={busy}
          connection={connection}
          config={localConfig}
          fallbackReason={configurationSource === "fallback" ? fallbackReason : null}
          onSave={(patch) => onSaveConfiguration(patch, connection)}
        />
      )}
      <details className="raw-details">
        <summary>{t("Configuration JSON")}</summary>
        <pre>{pretty(snapshot?.configuration ?? null)}</pre>
      </details>
    </section>
  );
}

function SettingsConfigEditor({
  busy,
  connection,
  config,
  fallbackReason,
  onSave,
}: {
  busy: boolean;
  connection: TeaClientOptions;
  config: TeaLocalConfig;
  fallbackReason: string | null;
  onSave: (config: Partial<TeaLocalConfig>) => void;
}) {
  const [draft, setDraft] = useState<TeaLocalConfig>(config);
  const [draftConnection, setDraftConnection] = useState(connection);
  // Discard an unsaved draft before it can be committed under another daemon.
  if (draftConnection !== connection) {
    setDraftConnection(connection);
    setDraft(config);
  }

  useEffect(() => {
    setDraft(config);
    // Intentionally reset the draft only when a persisted config field we edit changes,
    // not on every `config` object identity change (which would discard in-progress edits).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    config.notifications_enabled,
    config.human_ticket_default_approval_policy,
    config.hook_ticket_default_approval_policy,
  ]);

  const dirty =
    draft.notifications_enabled !== config.notifications_enabled ||
    draft.human_ticket_default_approval_policy !== config.human_ticket_default_approval_policy ||
    draft.hook_ticket_default_approval_policy !== config.hook_ticket_default_approval_policy;

  return (
    <form
      className="settings-config-editor"
      onSubmit={(event) => {
        event.preventDefault();
        const patch: Partial<TeaLocalConfig> = {};
        if (draft.notifications_enabled !== config.notifications_enabled) {
          patch.notifications_enabled = draft.notifications_enabled;
        }
        if (
          draft.human_ticket_default_approval_policy !==
          config.human_ticket_default_approval_policy
        ) {
          patch.human_ticket_default_approval_policy =
            draft.human_ticket_default_approval_policy;
        }
        if (
          draft.hook_ticket_default_approval_policy !== config.hook_ticket_default_approval_policy
        ) {
          patch.hook_ticket_default_approval_policy = draft.hook_ticket_default_approval_policy;
        }
        onSave(patch);
      }}
    >
      <div className="settings-config-intro">
        <strong>{t("Tea local settings")}</strong>
        <span>{t("Tea owns these settings until Loom claims Tea configuration.")}</span>
      </div>
      {fallbackReason ? (
        <p className="settings-fallback-reason">{t("Fallback")}: {fallbackReason}</p>
      ) : null}
      <label className="settings-config-toggle">
        <input
          checked={draft.notifications_enabled}
          disabled={busy}
          onChange={(event) =>
            setDraft((current) => ({ ...current, notifications_enabled: event.target.checked }))
          }
          type="checkbox"
        />
        <span>{t("Enable notifications")}</span>
      </label>
      <label className="settings-config-field">
        <span>{t("Human ticket default approval policy")}</span>
        <select
          disabled={busy}
          onChange={(event) =>
            setDraft((current) => ({
              ...current,
              human_ticket_default_approval_policy: event.target.value,
            }))
          }
          value={draft.human_ticket_default_approval_policy}
        >
          {approvalPolicyOptions.map((option) => (
            <option key={option.value} value={option.value}>
              {t(option.label)}
            </option>
          ))}
        </select>
      </label>
      <label className="settings-config-field">
        <span>{t("Hook ticket default approval policy")}</span>
        <select
          disabled={busy}
          onChange={(event) =>
            setDraft((current) => ({
              ...current,
              hook_ticket_default_approval_policy: event.target.value,
            }))
          }
          value={draft.hook_ticket_default_approval_policy}
        >
          {approvalPolicyOptions.map((option) => (
            <option key={option.value} value={option.value}>
              {t(option.label)}
            </option>
          ))}
        </select>
      </label>
      <div className="settings-config-actions">
        <button className="action-primary" disabled={busy || !dirty} type="submit">
          {t("Save Tea settings")}
        </button>
        <button
          disabled={busy || !dirty}
          onClick={() => setDraft(config)}
          type="button"
        >
          {t("Reset changes")}
        </button>
      </div>
    </form>
  );
}

