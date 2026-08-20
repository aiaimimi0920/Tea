use tea_config::{is_safe_loom_panel_url, ConfigurationOwner, ConfigurationSource};

use crate::ConfigurationResponse;

pub(crate) fn render_settings_page(configuration: &ConfigurationResponse) -> String {
    let source = configuration_source_label(configuration.configuration_source);
    let owner = configuration_owner_label(configuration.configuration.owner);
    let disabled = configuration.configuration_source == ConfigurationSource::LoomManaged;
    let disabled_attr = if disabled { " disabled" } else { "" };
    let loom_panel_link = configuration
        .configuration
        .loom_panel_url
        .as_deref()
        .filter(|value| is_safe_loom_panel_url(value))
        .map(|value| {
            format!(
                r#"<a class="primary-link" href="{}">Open Loom Tea settings</a>"#,
                escape_html(value)
            )
        })
        .unwrap_or_else(|| {
            r#"<p class="muted">Loom did not provide a safe settings link.</p>"#.to_string()
        });
    let loom_panel = if disabled {
        format!(
            r#"<section class="loom-callout">
                <h2>This Tea configuration is managed by Loom</h2>
                <p>Tea is running as an independent app, but Loom owns this configuration. Use Loom's Tea configuration panel for changes.</p>
                {loom_panel_link}
            </section>"#,
        )
    } else {
        r#"<section class="local-callout">
                <h2>Tea local settings</h2>
                <p>Loom is not managing Tea configuration, so this standalone Tea daemon can edit its local settings.</p>
            </section>"#
            .to_string()
    };
    // `/settings` is intentionally reachable without auth for the standalone
    // loopback UI. Keep operational paths, upstream addresses and raw failure
    // reasons on the authenticated `/v1/configuration` response only.
    let reason = configuration
        .configuration
        .reason
        .as_ref()
        .map(|_| {
            r#"<p class="muted"><strong>Status:</strong> Loom configuration discovery failed; Tea is using its local fallback.</p>"#
                .to_string()
        })
        .unwrap_or_default();
    let local_path = if configuration.configuration.local_config_path.is_some() {
        "configured"
    } else {
        "default"
    };
    let loom_base_url = if configuration.configuration.loom_base_url.is_some() {
        "configured"
    } else {
        "not configured"
    };
    let notifications_checked = if configuration.config.notifications_enabled {
        " checked"
    } else {
        ""
    };
    let initial_config_json =
        serde_json::to_string(&configuration.config).expect("serialize Tea configuration");

    format!(
        r#"<!doctype html>
<html lang="en" data-configuration-source="{source}">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Tea Settings</title>
  <style>
    :root {{
      color-scheme: dark;
      font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      background: #090d1a;
      color: #edf3ff;
    }}
    body {{
      margin: 0;
      min-height: 100vh;
      background:
        radial-gradient(circle at 20% 20%, rgba(113, 96, 255, 0.30), transparent 34rem),
        radial-gradient(circle at 80% 10%, rgba(45, 212, 191, 0.18), transparent 26rem),
        linear-gradient(135deg, #070b16 0%, #101827 100%);
    }}
    main {{
      box-sizing: border-box;
      width: min(960px, calc(100% - 32px));
      margin: 0 auto;
      padding: 48px 0 64px;
    }}
    .panel, .loom-callout, .local-callout {{
      border: 1px solid rgba(148, 163, 184, 0.22);
      border-radius: 24px;
      background: rgba(15, 23, 42, 0.72);
      box-shadow: 0 24px 80px rgba(0, 0, 0, 0.28);
      backdrop-filter: blur(18px);
      padding: 24px;
      margin-top: 20px;
    }}
    h1 {{
      font-size: clamp(2.1rem, 6vw, 4rem);
      margin: 0 0 10px;
      letter-spacing: -0.05em;
    }}
    h2 {{
      margin-top: 0;
    }}
    .muted {{
      color: #aab8d4;
    }}
    .grid {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
      gap: 14px;
    }}
    label {{
      display: grid;
      gap: 8px;
      margin: 16px 0;
      color: #cbd5e1;
    }}
    input, select {{
      border: 1px solid rgba(148, 163, 184, 0.28);
      border-radius: 14px;
      background: rgba(2, 6, 23, 0.72);
      color: #f8fafc;
      padding: 12px 14px;
      font: inherit;
    }}
    input[disabled], select[disabled] {{
      color: #94a3b8;
      cursor: not-allowed;
      opacity: 0.65;
    }}
    button, .primary-link {{
      border: 0;
      border-radius: 999px;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      background: linear-gradient(135deg, #7c3aed, #06b6d4);
      color: white;
      cursor: pointer;
      font-weight: 700;
      min-height: 44px;
      padding: 0 20px;
      text-decoration: none;
    }}
    button[disabled] {{
      cursor: not-allowed;
      filter: grayscale(0.7);
      opacity: 0.55;
    }}
    code {{
      color: #bfdbfe;
      overflow-wrap: anywhere;
    }}
    #message {{
      min-height: 1.4em;
    }}
  </style>
</head>
<body>
  <main>
    <p class="muted">Standalone Tea configuration</p>
    <h1>Tea Settings</h1>
    <p class="muted">Tea can run as an independent local app. When Loom claims Tea configuration, this page becomes a read-only jump surface.</p>

    <section class="panel">
      <div class="grid">
        <p><strong>Configuration source:</strong><br><code>{source}</code></p>
        <p><strong>Owner:</strong><br><code>{owner}</code></p>
        <p><strong>Local config:</strong><br><code>{local_path}</code></p>
        <p><strong>Loom base URL:</strong><br><code>{loom_base_url}</code></p>
      </div>
      {reason}
    </section>

    {loom_panel}

    <form class="panel" id="settings-form">
      <h2>Editable local config</h2>
      <label>
        <span>Auth token for saving</span>
        <input name="auth_token" type="password" autocomplete="current-password" placeholder="Bearer token required for PATCH /v1/configuration"{disabled_attr}>
      </label>
      <label>
        <span>Notifications enabled</span>
        <input name="notifications_enabled" type="checkbox"{notifications_checked}{disabled_attr}>
      </label>
      <label>
        <span>Human ticket default approval policy</span>
        <select name="human_ticket_default_approval_policy"{disabled_attr}>
          {human_options}
        </select>
      </label>
      <label>
        <span>Hook ticket default approval policy</span>
        <select name="hook_ticket_default_approval_policy"{disabled_attr}>
          {hook_options}
        </select>
      </label>
      <button type="submit"{disabled_attr}>Save Tea local settings</button>
      <p id="message" class="muted"></p>
    </form>
  </main>
  <script>
    const form = document.getElementById('settings-form');
    const message = document.getElementById('message');
    let savedConfig = {initial_config_json};
    form.addEventListener('submit', async (event) => {{
      event.preventDefault();
      if ({disabled_js}) {{
        message.textContent = 'Tea configuration is managed by Loom. Open Loom Tea settings instead.';
        return;
      }}
      const data = new FormData(form);
      const token = String(data.get('auth_token') || '').trim();
      const nextConfig = {{
        notifications_enabled: data.get('notifications_enabled') === 'on',
        human_ticket_default_approval_policy: data.get('human_ticket_default_approval_policy'),
        hook_ticket_default_approval_policy: data.get('hook_ticket_default_approval_policy')
      }};
      const patch = Object.fromEntries(
        Object.entries(nextConfig).filter(([key, value]) => savedConfig[key] !== value)
      );
      if (Object.keys(patch).length === 0) {{
        message.textContent = 'No Tea local settings changed.';
        return;
      }}
      const response = await fetch('/v1/configuration', {{
        method: 'PATCH',
        headers: {{
          'content-type': 'application/json',
          ...(token ? {{ authorization: `Bearer ${{token}}` }} : {{}})
        }},
        body: JSON.stringify(patch)
      }});
      if (response.ok) {{
        savedConfig = nextConfig;
      }}
      message.textContent = response.ok ? 'Saved Tea local settings.' : `Save failed: ${{await response.text()}}`;
    }});
  </script>
</body>
</html>"#,
        source = source,
        owner = owner,
        local_path = escape_html(local_path),
        loom_base_url = escape_html(loom_base_url),
        reason = reason,
        loom_panel = loom_panel,
        disabled_attr = disabled_attr,
        disabled_js = if disabled { "true" } else { "false" },
        notifications_checked = notifications_checked,
        initial_config_json = initial_config_json,
        human_options =
            approval_policy_options(&configuration.config.human_ticket_default_approval_policy),
        hook_options =
            approval_policy_options(&configuration.config.hook_ticket_default_approval_policy),
    )
}

fn approval_policy_options(selected: &str) -> String {
    [
        ("human_before_execute", "Human before execute"),
        ("human_before_completion", "Human before completion"),
        ("manual_only", "Manual only"),
        ("plan_only", "Plan only"),
    ]
    .into_iter()
    .map(|(value, label)| {
        let selected_attr = if selected == value { " selected" } else { "" };
        format!(
            r#"<option value="{}"{}>{}</option>"#,
            escape_html(value),
            selected_attr,
            escape_html(label)
        )
    })
    .collect::<Vec<_>>()
    .join("\n")
}

fn configuration_source_label(source: ConfigurationSource) -> &'static str {
    match source {
        ConfigurationSource::Local => "local",
        ConfigurationSource::LoomManaged => "loom-managed",
        ConfigurationSource::Fallback => "fallback",
    }
}

fn configuration_owner_label(owner: ConfigurationOwner) -> &'static str {
    match owner {
        ConfigurationOwner::Tea => "tea",
        ConfigurationOwner::Loom => "loom",
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
