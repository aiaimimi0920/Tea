# Tea

Tea is Neuro's API-first AI ticket/work-order control plane.

Repository: <https://github.com/aiaimimi0920/Tea>

Tea is maintained as an independent GitHub repository and is consumed by the
top-level Neuro workspace as a submodule, matching the Hook repository model.

Tea owns:

- ticket intake;
- ticket comments and event timelines;
- AI ticket analysis and plan records;
- approval policy;
- Loom run dispatch records;
- run evidence and human review state.

Tea does not own:

- Git hosting;
- model/provider routing;
- Gateway credentials or relay internals;
- Loom workflow execution internals;
- Hook foreground capture.

## Runtime modes

- UI: run `start-tea.bat` from a Windows release package. It starts
  `tea-daemon.exe` with package-local data and then opens `tea.exe`, Tea's own
  GUI program. The GUI is a Gitea-style issue/work-order tracker with
  Open/Closed/All filters, search, new work-order creation, issue detail,
  repository tabs, query controls, suggested work-order templates, issue
  conversation cards, durable review comments, a combined chronological
  comment/event conversation stream, comment preview, issue-row activity
  metrics, issue summary/recency metadata, interactive repository tabs for
  Issues/Runs/Comments/Exports/Settings focus views, compact title metadata,
  quick issue actions, local watch state, clipboard copy-link support, local label overlays with inline add/remove/reset controls, local label filter controls, issue-row priority/risk
  badges, label filter controls, searchable owner/agent/label metadata, focused
  empty-state guidance, focused panel hints, timeline avatars, progress
  metadata, run history, real daemon ticket labels/priority/risk metadata,
  workflow actions, and export preview.
- Headless: run `start-tea-daemon.bat`, direct `tea-daemon.exe`, or
  `tea-daemon` from source and call it with the `tea` CLI or HTTP clients.
- Platform-managed: Platform calls Tea HTTP APIs and renders Tea records.

## Release package contract

Tea release packages are standalone local-app artifacts in the same product family as Hook and Talk.
A user must be able to install and run Tea without
installing Platform, Hook, or Talk; integrations are additive rather than
required for the package to start.

The Windows release package is one Tea app with both UI and no-UI launch modes.
It contains `tea.exe`, `tea-daemon.exe`, `tea-cli.exe`, `tea-mcp.exe`, and launchers:

- `tea.exe` is Tea's first-party UI program. Double-clicking the Tea program
  opens this GUI.
- `tea-daemon.exe` is Tea's local HTTP service and owns ticket state, approval
  policy, events, SQLite persistence, Loom run records, and evidence. This is
  Tea's no-UI/headless mode.
- `tea-cli.exe` is the operator CLI and must use Tea's HTTP API instead of
  mutating Tea stores directly.
- `tea-mcp.exe` is Tea's Model Context Protocol server (stdio). It exposes Tea
  ticket operations (create, edit, comment, analyze/decompose/plan, approve,
  run, accept, close, cancel, export, and reads) as MCP tools so MCP-capable
  agents can drive Tea work orders. It is a thin adapter over Tea's HTTP API and
  reads `TEA_SERVER_URL` / `TEA_AUTH_TOKEN` from its environment.
- `start-tea.bat` is the double-click UI launcher. It generates or reuses a
  package-local auth token, starts `tea-daemon.exe`, waits for `/health`, and
  opens `tea.exe`.
- `start-tea-daemon.bat` starts the same local daemon without opening the GUI.
- `stop-tea.bat` stops only the daemon/UI processes launched from the same
  package directory.

Release package configuration follows the same independent-program rule as Hook
and Talk. Without Loom, or when Loom is present but has not claimed Tea
configuration, Tea may expose and write Tea-local settings through its own local
settings UI or equivalent CLI/API surface. When Loom claims Tea configuration,
Tea settings buttons and configuration UI entries must open Loom's Tea
configuration panel, while Tea-local configuration UI becomes read-only,
fallback-only, or jump-to-Loom only.

Formal clean release acceptance requires `gitDirty: false` in the generated
release manifest plus a passing release-profile CLI lifecycle smoke.
After building a package, run the package itself with
`scripts\smoke-tea-cli-real.ps1 -PackageDir <release\Tea\versionId>` and
`scripts\smoke-tea-ui-real.ps1 -PackageDir <release\Tea\versionId>`; this
verifies the copied `tea.exe` UI program, `tea-daemon.exe` no-UI daemon,
`tea-cli.exe` CLI, and rejects package manifests that do not report
`gitDirty: false` for a formal release.

## Configuration ownership

Tea is an independent program, so it must be able to configure Tea-specific
options through its own local settings surface or equivalent CLI/API entry when
Loom is not available. This includes settings such as hotkeys, ticket intake
defaults, approval defaults, notification/UI behavior, integration switches,
auth, persistence, and Loom endpoint preferences.

When a usable Loom is present and declares that it manages Tea configuration,
Tea settings buttons, preferences entries, hotkey configuration entries, UI
configuration entries, and equivalent configuration actions must open or jump
to Loom's Tea configuration panel instead of writing the same settings locally.
In that state, Tea-local settings UI is limited to read-only status, failure
fallback, or a jump-to-Loom action. Tea status/health surfaces must expose the
active configuration source as `local`, `loom-managed`, or `fallback`.

This preserves standalone Tea operation while centralizing settings in Loom when
the full local suite is installed.

Runtime ownership behavior:

- `TEA_CONFIG_PATH` points to Tea's local JSON config file. If unset, Windows
  uses `%APPDATA%\Neuro\tea\config.json`; non-Windows/dev fallback uses
  `.runtime/neuro/tea/config.json`.
- `TEA_LOOM_BASE_URL` enables Loom configuration ownership discovery through
  `GET /v1/configuration/claims?app=tea`.
- If Loom returns `managed=true` with a non-empty `panel_url`, Tea reports
  `configuration_source: "loom-managed"` and rejects local config writes with
  `409 Conflict`.
- If Loom is configured but unavailable, unauthorized, or returns invalid claim
  JSON, Tea reports `configuration_source: "fallback"` and exposes the reason in
  status/configuration responses.
- `GET /v1/configuration` returns sanitized ownership and local config data.
`PUT /v1/configuration` writes the Tea local config only when the active
source is `local` or `fallback`.

The current local config schema version is `1` and stores:

```json
{
  "schema_version": 1,
  "notifications_enabled": true,
  "human_ticket_default_approval_policy": "human_before_execute",
  "hook_ticket_default_approval_policy": "plan_only"
}
```

The configured default approval policies are applied when Tea creates new
human tickets through `POST /v1/tickets` and Hook tickets through
`POST /v1/intake/hook`. Unknown approval policy values are rejected by
`PUT /v1/configuration` instead of being saved for a later runtime failure.

## Standalone mode

```powershell
$env:TEA_AUTH_TOKEN = "replace-with-a-strong-local-token"
cargo run --manifest-path Tea/Cargo.toml -p tea-daemon
```

In another shell:

```powershell
$env:TEA_AUTH_TOKEN = "replace-with-a-strong-local-token"
cargo run --manifest-path Tea/Cargo.toml -p tea-cli -- status
cargo run --manifest-path Tea/Cargo.toml -p tea-cli -- ticket create --title "Smoke" --body "Create a safe plan."
```

`tea-daemon` supports `--help` and `--version` and accepts CLI overrides for
the environment-driven runtime settings:

```powershell
cargo run --manifest-path Tea/Cargo.toml -p tea-daemon -- --help
cargo run --manifest-path Tea/Cargo.toml -p tea-daemon -- `
  --bind-addr 127.0.0.1:48910 `
  --auth-token "replace-with-a-strong-local-token" `
  --store-path ".runtime\tea.sqlite"
```

For development compatibility, the daemon and CLI still default to
`dev-token` when `TEA_AUTH_TOKEN` is unset. Do not use that default for a
shared workstation, container port mapping, or formal release package runtime.

`/health` and `/settings` are local unauthenticated surfaces. All `/v1/*`
endpoints, including read-only status, ticket, comment, run, and export routes,
require `Authorization: Bearer <TEA_AUTH_TOKEN>`.

Ticket approval policy can be overridden explicitly when a human wants to
tighten or relax the run gate for a specific ticket:

```powershell
cargo run --manifest-path Tea/Cargo.toml -p tea-cli -- ticket policy <ticket-id> --mode manual_only
```

Run records can be inspected and controlled directly when a run id is known:

```powershell
cargo run --manifest-path Tea/Cargo.toml -p tea-cli -- run show <run-id>
cargo run --manifest-path Tea/Cargo.toml -p tea-cli -- run stop <run-id>
cargo run --manifest-path Tea/Cargo.toml -p tea-cli -- run retry <run-id>
```

## Platform mode

Platform should call Tea through HTTP APIs. Platform owns account, identity,
entitlement, and UI rendering. Tea remains the source of truth for ticket state,
approval policy, event timeline, Loom run records, and evidence.

The local Platform compose stack includes Tea as an independent service:

```powershell
$env:TEA_AUTH_TOKEN = "local-internal-token"
docker compose -f Platform/deploy/docker-compose.local.yml up tea
```

Tea listens on `http://localhost:48910` by default in compose. Platform services
should call the service over HTTP and should not mutate Tea stores directly.
The compose service sets `TEA_STORE_PATH=/data/tea.sqlite` and persists data in
the `tea-data` volume. The same compose stack also starts a local `loom` service
and sets `TEA_LOOM_BASE_URL=http://loom:8765` by default, so approved Tea runs
are dispatched to Loom instead of the in-process mock.

## SQLite schema metadata

When `TEA_STORE_PATH` is set, `tea-daemon` opens a SQLite-backed store. On open,
Tea creates a `schema_migrations` table and records the current schema version.
The current version is `1`, matching the ticket, comment, event, analysis, plan,
approval, and run tables used by the MVP. Reopening the same store is
idempotent, and legacy v1 stores that predate migration metadata are marked as
version 1 during startup. If a store records a schema version newer than the
current binary supports, Tea refuses to open it instead of writing through an
unknown future schema. Business schema creation and migration version recording
are committed in one SQLite transaction, so a failed version record cannot leave
behind partially-created Tea tables without matching migration metadata.

`GET /v1/status` includes the active store backend and schema compatibility
metadata plus the active configuration ownership. In memory mode the schema
fields are `null`; in SQLite mode `schema_version` reports the highest recorded
migration version and `supported_schema_version` reports the newest version the
current binary can write. `configuration_source` is `local`, `loom-managed`, or
`fallback`; the nested `configuration` object exposes the current owner, local
config path, Loom base URL, Loom panel URL when present, and any fallback reason.

Review comments are durable records, not fire-and-forget form submissions. Tea
stores them in memory or SQLite, exposes them through
`GET /v1/tickets/{ticket_id}/comments`, and includes them in JSON and Markdown
exports. Platform Core mirrors that read path under
`/internal/tea/tickets/{ticket_id}/comments`, and Platform Web renders the
comments on `/tea/[ticketId]`.

Platform Web's Tea entry is `/tea`, with a detail/review page at
`/tea/[ticketId]`. It is intentionally a backend-mediated surface: browser
requests hit Platform Web routes such as `/api/tea/tickets`,
`/api/tea/tickets/{ticket_id}/comments`,
`/api/tea/tickets/{ticket_id}/reject`,
`/api/tea/tickets/{ticket_id}/stop`,
`/api/tea/tickets/{ticket_id}/retry`,
`/api/tea/tickets/{ticket_id}/cancel`,
`/api/tea/tickets/{ticket_id}/runs`,
`/api/tea/tickets/{ticket_id}/export/json`,
`/api/tea/tickets/{ticket_id}/export/json/download`,
`/api/tea/tickets/{ticket_id}/export/markdown`, and
`/api/tea/tickets/{ticket_id}/export/markdown/download`; Platform Web calls
Platform Core `/internal/tea/*`; and only Platform Core holds the Tea daemon
bearer token. This preserves Tea as an independently runnable service while
keeping credentials out of the browser.

## Ticket lifecycle contract

Closed and cancelled tickets are read-only terminal records. Tea still allows
read-only endpoints such as ticket show/list, comments, events, runs, and export
after a ticket is closed, because those endpoints are needed for audit and
review.

Mutating endpoints reject terminal tickets with `409 Conflict`:

- `POST /v1/tickets/{ticket_id}/comments`
- `POST /v1/tickets/{ticket_id}/analyze`
- `POST /v1/tickets/{ticket_id}/plan`
- `POST /v1/tickets/{ticket_id}/decompose`
- `POST /v1/tickets/{ticket_id}/policy`
- `POST /v1/tickets/{ticket_id}/approve`
- `POST /v1/tickets/{ticket_id}/reject`
- `POST /v1/tickets/{ticket_id}/run`
- `POST /v1/tickets/{ticket_id}/stop`
- `POST /v1/tickets/{ticket_id}/retry`
- `POST /v1/runs/{run_id}/stop`
- `POST /v1/runs/{run_id}/retry`
- `POST /v1/tickets/{ticket_id}/accept`
- `POST /v1/tickets/{ticket_id}/close`
- `POST /v1/tickets/{ticket_id}/cancel`

This guard is enforced in both the in-memory store and the SQLite-backed store,
so standalone, test, and Platform-managed runtime modes share the same state
machine behavior.

Human acceptance is also evidence-gated: `POST /v1/tickets/{ticket_id}/accept`
requires at least one run with attached evidence, so an empty or only-planned
ticket cannot be marked accepted.

Rejected approval keeps a ticket in `Blocked`; blocked tickets reject new run
attempts before Tea calls Loom, even if the ticket policy would otherwise allow
automatic execution.

## BrainProvider and Loom integration

Tea owns decomposition records and lifecycle state. Loom owns strong reasoning
that generates decomposition proposals. Gateway is not part of Tea ticket
decomposition business logic.

Tea uses a deterministic in-process template BrainProvider and mock Loom client
unless a Loom endpoint is configured. This keeps standalone and Platform-local
development usable without embedding Loom's agent runtime in Tea. When
`TEA_LOOM_BASE_URL` is set, Tea uses Loom's local capability API for advanced
decomposition through `tea.ticket.decompose.v1` and still records the resulting
analysis/plan in Tea.

Runtime configuration:

| Variable | Default | Behavior |
|---|---|---|
| `TEA_LOOM_BASE_URL` | empty outside compose; `http://loom:8765` in compose | When set, Tea sends strong decomposition and run/stop/retry requests to this Loom HTTP service. |
| `TEA_LOOM_AUTH_TOKEN` | empty | Optional bearer token for Loom capability and run requests. |

Current Loom capability contract used by Tea:

```http
POST /v1/invoke
Authorization: Bearer <TEA_LOOM_AUTH_TOKEN>

{
  "requestId": "<uuid>",
  "caller": "tea",
  "capability": "tea.ticket.decompose.v1",
  "input": {
    "schema_version": 1,
    "request_id": "<uuid>",
    "ticket": <Ticket>,
    "comments": [<TicketComment>],
    "policy": {
      "approval_policy": "human_before_execute",
      "terminal_state_guard": true
    },
    "context": {
      "workspace_root": "C:\\Users\\Public\\nas_home\\AI\\GameEditor\\Neuro",
      "platform_mode": "standalone",
      "requested_by": "tea-api"
    }
  }
}
```

Loom returns a decomposition proposal containing `analysis` and `plan`. Tea
validates the proposal and then stores both records in Tea. Loom must not mutate
Tea ticket state directly.

Current Tea-side decomposition APIs:

```http
POST /v1/tickets/{ticket_id}/decompose
Authorization: Bearer <TEA_AUTH_TOKEN>
```

Returns provider metadata, `proposal_id`, `analysis`, `plan`,
`requires_human_review`, and `notes`; Tea stores the analysis and plan as one
accepted decomposition proposal.

```http
POST /v1/tickets/{ticket_id}/analyze
POST /v1/tickets/{ticket_id}/plan
Authorization: Bearer <TEA_AUTH_TOKEN>
```

These compatibility endpoints delegate to the same BrainProvider proposal path.
`analyze` stores only the analysis. `plan` stores the analysis used by the plan
and then stores the plan.

```http
POST /v1/runs
Authorization: Bearer <TEA_LOOM_AUTH_TOKEN>

{ "ticket": <Ticket> }
```

Returns a `Run` with `loom_session_id`, `status`, and optional `evidence`.

```http
POST /v1/runs/{run_id}/stop
POST /v1/runs/{run_id}/retry
Authorization: Bearer <TEA_LOOM_AUTH_TOKEN>

{ "run": <Run> }
```

Each returns the updated `Run`. BrainProvider/Loom failures are surfaced by Tea
as `502 Bad Gateway` responses so the ticket timeline does not record false
success. Tea also validates that any Loom `Run` belongs to the addressed ticket
before recording it, so a mismatched run cannot pollute another ticket's
timeline. Stop/retry responses must also refer to the exact run being acted on,
so Loom cannot redirect a run action to another run in the same ticket.

## Container image

```powershell
docker build -f Tea/Dockerfile -t neuro-tea:local Tea
docker volume create neuro-tea-data
docker run --rm -p 48910:48910 `
  -e TEA_AUTH_TOKEN=replace-with-a-strong-local-token `
  -e TEA_STORE_PATH=/data/tea.sqlite `
  -v neuro-tea-data:/data `
  neuro-tea:local
```

## Local smoke

```powershell
$env:TEA_AUTH_TOKEN = "replace-with-a-strong-local-token"
cargo run --manifest-path Tea/Cargo.toml -p tea-daemon
cargo run --manifest-path Tea/Cargo.toml -p tea-cli -- status
cargo run --manifest-path Tea/Cargo.toml -p tea-cli -- ticket create --title "Smoke" --body "Body"
cargo run --manifest-path Tea/Cargo.toml -p tea-cli -- ticket list
```

For a repeatable real local acceptance smoke, run the repository-level harness:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-tea-cli-real.ps1
```

This proves the self-contained path `tea CLI -> tea-daemon -> HTTP API -> SQLite store -> template BrainProvider/mock Loom`.
The harness builds the debug `tea-daemon.exe` and `tea.exe`, starts an isolated
daemon on a free loopback port, uses temporary `TEA_STORE_PATH` and
`TEA_CONFIG_PATH` values, then drives the full CLI lifecycle:
`status/config/create/comment/edit/decompose/approve/run/stop/retry/accept/close/cancel/export/events`.
It also verifies Markdown export evidence, JSON export events, daemon shutdown,
and zero listeners left on the selected port after cleanup.

Smoke artifacts are written under `.tmp/tea-smoke/tea-cli-real-<timestamp>/`.
By default the temporary SQLite store and config file are removed after a
passing run; pass `-KeepArtifacts` to preserve them for inspection. Pass
`-Release` to run the same lifecycle against `Tea\target\release` binaries.

For a repeatable Tea -> Loom decompose acceptance smoke, run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-tea-loom-decompose-real.ps1
```

This starts isolated `loom-daemon.exe` and `tea-daemon.exe` processes on free
loopback ports, sets `TEA_LOOM_BASE_URL`, creates a Tea ticket, calls
`tea ticket decompose <ticket_id>`, and verifies that Tea used Loom capability
`tea.ticket.decompose.v1`. The expected Loom-generated workflow is
`loom.tea_ticket_decompose.v1`. The smoke also verifies that Tea stored both
the analysis and plan records by checking for `ticket_analyzed` and
`plan_proposed` events in the Tea timeline. Smoke artifacts are written under
`.tmp/tea-smoke/tea-loom-decompose-real-<timestamp>/`.

The `tea-cli status` command renders the status response as an operator summary,
including the active store backend, SQLite schema compatibility, and
configuration ownership when available:

```text
Service: tea
Status: ok
Store: sqlite
SQLite schema: 1 (supported: 1)
Configuration source: local
Configuration owner: tea
```

Configuration can also be inspected or changed through the CLI:

```powershell
cargo run --manifest-path Tea/Cargo.toml -p tea-cli -- config show
cargo run --manifest-path Tea/Cargo.toml -p tea-cli -- config set --notifications-enabled false
cargo run --manifest-path Tea/Cargo.toml -p tea-cli -- config set --human-ticket-default-approval-policy human_before_completion
cargo run --manifest-path Tea/Cargo.toml -p tea-cli -- config set --hook-ticket-default-approval-policy manual_only
```

Platform Web exposes the same ownership rule through `/tea/settings`: when
configuration is `local` or `fallback`, the page can edit the v1 local settings
fields; when configuration is `loom-managed`, the page stays read-only and
offers the Loom settings jump target.

Tea daemon also exposes a standalone settings page at
`http://127.0.0.1:48910/settings` by default. This gives the Tea release package
its own local configuration UI when Loom is absent or has not claimed Tea
configuration. When Loom owns Tea configuration, the same page becomes read-only
and shows an `Open Loom Tea settings` jump target instead of saving local
settings.

## Hook integration smoke

To prove Hook can create a ticket in a real Tea daemon, run the root smoke
harness:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-hook-tea-real.ps1
```

The harness starts an isolated `tea-daemon` on a free loopback port with a
temporary SQLite store, runs Hook's ignored Rust test
`tea_real_daemon_smoke`, then verifies:

- Hook posted to Tea `POST /v1/intake/hook` through its real Rust client;
- the created ticket has `source:hook`, `policy:plan-only`, and
  `context:untrusted`;
- Tea can return the ticket, events, and Markdown export through HTTP.

Smoke artifacts are written to `.tmp/tea-smoke/hook-tea-real-<timestamp>/`.
Use `-KeepArtifacts` if you want to preserve the temporary SQLite store for
manual inspection.

To prove the Hook frontend panel can create a Tea ticket through the Tauri
invoke bridge and the real Tea daemon, run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-hook-tea-ui-real.ps1
```

The UI smoke builds Hook's static frontend, starts an isolated Hook preview and
`tea-daemon`, injects a Tauri-compatible invoke bridge into headless Chromium,
clicks the `Create Tea Ticket` panel button, and then verifies the created
ticket through Tea's HTTP ticket/events/Markdown export endpoints. Artifacts are
written to `.tmp/tea-smoke/hook-tea-ui-real-<timestamp>-<8 hex>/`.

## Platform integration smoke

To prove Platform Core can operate against a real Tea daemon through its
internal `/internal/tea/*` proxy routes, run the root smoke harness:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-platform-tea-real.ps1
```

The harness starts an isolated `tea-daemon` on a free loopback port with a
temporary SQLite store, runs Platform's skipped-by-default Node test
`core/src/modules/tea/real-daemon-smoke.test.ts` with
`TEA_PLATFORM_REAL_SMOKE=1`, then verifies:

- Platform Core creates and reads a Tea ticket through `/internal/tea/tickets`;
- approval, run, events, runs, Markdown export, cancel, and close flow through the
  Platform proxy;
- closed tickets still reject mutating operations with `409 Conflict`.

Smoke artifacts are written to
`.tmp/tea-smoke/platform-tea-real-<timestamp>/`. Use `-KeepArtifacts` if you
want to preserve the temporary SQLite store for manual inspection.

## Platform Web integration smoke

To prove Platform Web's browser-facing Tea handlers can operate through
Platform Core HTTP against a real Tea daemon, run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-platform-web-tea-real.ps1
```

The harness starts an isolated `tea-daemon`, starts an in-process Platform Core
HTTP server for `/internal/tea/*`, then drives the Platform Web Tea handlers for
create, comment, reject, cancel, approve, run, detail/comments/events, comments, runs,
JSON export, Markdown export, raw JSON/Markdown downloads, stop, retry, close,
and terminal-ticket conflict checks. It verifies that persisted review comments
are readable and present in both export formats. It also verifies the credential
boundary: Platform Web does not send `authorization` to Core, while Platform
Core does send the Tea bearer token to the Tea daemon. Smoke artifacts are
written to `.tmp/tea-smoke/platform-web-tea-real-<timestamp>/`.

To prove Platform Web's actual Next.js Tea work-order desk still works through
Local Dev auth, forms, redirects, downloads, and real browser interaction, run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-platform-web-tea-ui-real.ps1 -KeepArtifacts
```

The UI harness starts an isolated `tea-daemon`, starts a minimal Platform Core
helper for `/internal/tea/*` plus the feature/public-surface and Local Dev
identity routes needed by the Next layout/auth path, starts `next dev`, and uses
real Chrome or Edge through `playwright-core` to click `/tea`. It creates a
ticket, opens the detail page, submits a durable human comment, approves, runs,
downloads Markdown/JSON evidence, stops and retries the latest run, and verifies
captured Web -> Core / Core -> Tea credential-boundary evidence. Artifacts are
written to `.tmp/tea-smoke/platform-web-tea-ui-real-<guid>/`. If another
`Platform/web` Next dev instance already holds `.next/dev/lock`, stop it first;
the harness waits briefly and then fails rather than killing unknown processes.

## Configuration ownership smoke

To prove Tea follows the centralized configuration rule with and without Loom,
run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-tea-configuration-ownership.ps1 -KeepArtifacts
```

The harness builds debug `tea-daemon` and `loom-daemon`, starts isolated
loopback processes, and verifies:

- Tea without Loom reports `configuration_source: local` and accepts local
  config writes;
- the Tea standalone settings page at `/settings` exposes local configuration
  UI when Tea owns configuration;
- Loom present but not claiming Tea keeps Tea in `local`;
- Loom claiming Tea through `/v1/configuration/claims?app=tea` moves Tea to
  `loom-managed`, local writes return conflict, and `/settings` shows the
  `Open Loom Tea settings` jump target;
- configured but unreachable Loom moves Tea to `fallback` with a visible reason.

Smoke artifacts are written to `.tmp/tea-smoke/tea-configuration-ownership-*`.

## Validation

```powershell
cargo fmt --manifest-path Tea/Cargo.toml --all -- --check
cargo check --manifest-path Tea/Cargo.toml --workspace --all-targets
cargo test --manifest-path Tea/Cargo.toml --workspace
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\tests\test-smoke-tea-cli-real-contract.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-tea-cli-real.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-tea-cli-real.ps1 -Release
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-tea-cli-real.ps1 -PackageDir .\release\Tea\<versionId>
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-tea-release-package.ps1 -PackageDir .\release\Tea\<versionId> -RunSmoke
```

Platform Web's backend-mediated Tea entry can be validated from `Platform/`:

```powershell
cd Platform
node --test --import tsx web/src/lib/tea-client.test.ts web/src/lib/tea-route-utils.test.ts web/src/lib/tea-api-handlers.test.ts web/src/lib/tea-detail-controls.test.ts web/src/lib/tea-real-core-smoke.test.ts web/src/lib/tea-web-ui-smoke-harness-contract.test.ts
```
