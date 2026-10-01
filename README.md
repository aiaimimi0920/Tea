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
It contains `tea.exe`, `tea-daemon.exe`, `tea-cli.exe`, `tea-mcp.exe`,
`tea-sync.exe`, and launchers:

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
  package-local auth token, starts `tea-daemon.exe`, waits for authenticated
  `/v1/status`, and opens `tea.exe`.
- `start-tea-daemon.bat` starts the same local daemon without opening the GUI.
- `stop-tea.bat` stops only the daemon/UI processes launched from the same
  package directory.
- `resolve-tea-token.ps1`, `start-tea-daemon.ps1`, and `stop-tea.ps1` are
  required launcher helpers. They are part of the release payload and must be
  listed in the manifest and checksum inventory alongside the BAT entrypoints.
  On Windows, PowerShell and direct GUI startup share a per-data-directory named
  mutex; non-Windows GUI startup uses a persistent file lock in the data
  directory. Concurrent launchers therefore cannot start duplicate daemons on
  either platform. Launchers pass the auth
  token through the child environment instead of exposing it on the daemon
  command line. Cleanup removes a PID file only when it still identifies the
  process being cleaned up. A PID file identifies an owned daemon only when the
  process executable and its unique `--bind-addr`, `--store-path`, and
  `--config-path` arguments match the requested Tea profile. The stop launcher
  likewise refuses to terminate a same-executable daemon whose profile arguments
  differ, and a healthy authenticated endpoint is not reused unless that same
  ownership check succeeds. Generated token files contain exactly 32 ASCII hex
  characters; malformed files are rejected without replacement, and the token
  file or its direct parent directory must not be a symlink/reparse point. Direct
  GUI startup also requires the sibling `tea-daemon.exe` to be a regular file and
  rejects symbolic links, junctions, and other reparse points before spawning it.

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
- If Loom returns `app="tea"`, `managed=true`, and a safe `http://`, `https://`,
  or `loom://` `panel_url`, Tea reports `configuration_source: "loom-managed"`
  and rejects local config writes with `409 Conflict`. Claims for another app
  enter fallback mode. URLs with executable or local-file schemes, credentials,
  surrounding whitespace, or invalid syntax are discarded and never rendered
  as links on the unauthenticated settings page. The desktop applies the same
  allowlist again before rendering a daemon-provided Loom link, so a remote or
  incompatible Tea endpoint cannot inject an executable link into the WebView.
- If Loom is configured but unavailable, unauthorized, or returns invalid claim
  JSON, Tea reports `configuration_source: "fallback"` and exposes the reason in
  status/configuration responses.
- `GET /v1/configuration` returns sanitized ownership and local config data.
- `PUT /v1/configuration` replaces the full Tea local config only when the
  active source is `local` or `fallback`.
- `PATCH /v1/configuration` accepts one or more config fields and applies them
  under the process-shared config lock. CLI and first-party settings surfaces
  use this endpoint so independent concurrent field changes are merged instead
  of overwriting a stale full-document snapshot.

The current local config schema version is `1` and stores:

```json
{
  "schema_version": 1,
  "notifications_enabled": true,
  "human_ticket_default_approval_policy": "human_before_execute",
  "hook_ticket_default_approval_policy": "plan_only"
}
```

Tea serializes local-config reads, backup recovery, and atomic replacement with
a persistent sibling `<config>.lock` file. The lock is process-shared on Windows
and Unix, is released automatically if a process exits, and times out after five
seconds instead of leaving a request blocked indefinitely. The lock file is
intentionally retained: deleting and recreating it could let concurrent Tea
processes lock different file objects while they both manipulate the shared
`<config>.bak` recovery file. File-backed Tea runtimes refresh this shared config
by reading the protected file before returning configuration state or choosing a
default approval policy. They do not trust only file length and modification time,
so same-size external edits with a preserved/coarse timestamp are still observed,
and separate daemon processes converge after either one applies a patch. If the
primary file contains malformed JSON or invalid UTF-8 while a valid backup is
present, Tea promotes that backup under the same lock and removes the corrupt
primary. If the primary file is externally deleted, the next field PATCH rebuilds
it from that runtime's last-known configuration before applying the requested
field, rather than resetting untouched fields to defaults. An explicitly newer
unsupported schema is never hidden by fallback.

The configured default approval policies are applied when Tea creates new
human tickets through `POST /v1/tickets` and Hook tickets through
`POST /v1/intake/hook`. Unknown approval policy values are rejected by
both `PUT /v1/configuration` and `PATCH /v1/configuration` instead of being saved
for a later runtime failure.

Both create endpoints accept an optional `Idempotency-Key` header. A key must be
supplied exactly once, contain 1-255 visible ASCII bytes, and contain no
whitespace. Keys are scoped independently to the human and Hook create routes.
Repeating the same semantic request with the same key returns the original `200`
response and does not append another ticket or creation event. Reusing the key
with a different request returns `409 Conflict`. Requests without the header keep
the historical create-on-every-call behavior. SQLite stores persist the key,
SHA-256 request fingerprint, ticket identity, and original response in the same
Immediate transaction as ticket/event creation, so retries remain idempotent
across daemon restarts and competing Tea processes.

First-party create clients expose that contract without changing legacy calls:

- `tea-cli ticket create` and `tea-cli hook intake` accept an optional explicit
  `--idempotency-key`; operators and scripts reuse it when retrying one logical
  request.
- MCP `tea_create_ticket` accepts optional `idempotency_key` tool metadata and
  sends it as the HTTP header, never as part of the ticket JSON body.
- `tea-sync` automatically derives a stable visible-ASCII key from the provider
  slug and a SHA-256 digest of the provider/external issue identity. A later sync
  pass therefore reuses the same create key even when the external identifier
  contains Unicode or whitespace.
- Tea desktop generates one UUID-backed key per normalized create draft. An
  uncertain failure leaves that key attached to the unchanged draft for retry;
  editing the request creates a new key, while success or explicit cancellation
  clears it.

SQLite idempotency records currently have no automatic expiry. This preserves
late-retry at-most-once behavior; a retention policy must not be introduced until
Tea defines a maximum supported retry window and the post-expiry semantics.

## Standalone mode

```powershell
$env:TEA_AUTH_TOKEN = "replace-with-a-strong-local-token"
cargo run -p tea-daemon
```

In another shell:

```powershell
$env:TEA_AUTH_TOKEN = "replace-with-a-strong-local-token"
cargo run -p tea-cli -- status
cargo run -p tea-cli -- ticket create --title "Smoke" --body "Create a safe plan."
cargo run -p tea-cli -- ticket list --limit 100
cargo run -p tea-cli -- ticket list --limit 100 --cursor <next_cursor>
```

`tea-daemon` supports `--help` and `--version` and accepts CLI overrides for
the environment-driven runtime settings:

```powershell
cargo run -p tea-daemon -- --help
cargo run -p tea-daemon -- `
  --bind-addr 127.0.0.1:48910 `
  --auth-token "replace-with-a-strong-local-token" `
  --store-path ".runtime\tea.sqlite"
```

For loopback-only development compatibility, the daemon and CLI still default
to `dev-token` when `TEA_AUTH_TOKEN` is unset. The daemon refuses to bind a
non-loopback address while that default is active, so containers and shared
hosts must set a non-empty, non-default `TEA_AUTH_TOKEN` explicitly. Leading and
trailing whitespace is removed once during daemon startup; validation and bearer
authentication therefore use the same canonical token value.

`/health` and `/settings` are unauthenticated surfaces and should remain behind
local or otherwise trusted network boundaries. The settings page exposes only
configuration state and redacts local paths, upstream base URLs, and raw discovery
errors; those details remain available through the authenticated configuration API. All `/v1/*`
endpoints, including read-only status, ticket, comment, run, and export routes,
require `Authorization: Bearer <TEA_AUTH_TOKEN>`.

All production reqwest response consumers use an 8 MiB streaming limit,
including the desktop bridge, CLI, MCP server, `tea-sync`, daemon Loom ownership
discovery, Loom run/configuration client, and BrainProvider capability client.
Both declared `Content-Length` and chunked bodies are checked. Oversized
responses fail explicitly instead of being accumulated without a memory bound.
CLI, MCP, daemon ownership-discovery, and BrainProvider error-body previews are
single-line and limited to 512 characters. Tea's reqwest clients do not
automatically follow redirects, so a configured bearer token is never forwarded
through a server-directed redirect; operators must configure the final Tea,
Loom, or tracker endpoint directly.

The desktop bridge accepts cleartext `http` only for literal loopback hosts
(`localhost` or a loopback IP). Remote Tea endpoints must use `https`; server URLs
with embedded credentials, query strings, fragments, or a non-HTTP scheme are
rejected before a bearer token is selected. The production WebView also applies a
deny-by-default CSP: scripts and resources are package-local, Tauri IPC is the only
connect target, and objects, forms, framing, and base-URL rewriting are disabled.

Incoming JSON request bodies are explicitly limited to 2 MiB. Persisted input
has tighter UTF-8 byte limits: ticket titles 1 KiB, descriptions 512 KiB,
comments 256 KiB, rejection reasons 64 KiB, priorities 128 bytes, and at most 64
labels of 256 bytes each. Hook intake additionally limits its source, text,
context fields, attachment count, attachment metadata, and final normalized
description. Attachment kind/reference pairs are retained in the normalized
description under an explicitly untrusted-reference heading instead of being
silently dropped. `tea-cli hook intake --file` reads at most 2 MiB plus one byte
before JSON parsing and rejects larger local files. Oversized semantic fields
return `400`; HTTP bodies above the transport limit return `413` before JSON
deserialization can allocate an unbounded value.

`tea-mcp` accepts newline-delimited JSON-RPC messages up to 2 MiB per physical
line. It reads with a bounded buffer instead of `BufRead::lines`; an oversized or
invalid UTF-8 line is drained, receives one JSON-RPC parse error, and does not
prevent the next valid request from being processed. Parsed requests must be JSON
objects with `jsonrpc: "2.0"`, a string method, and a string, numeric, null, or
absent id; malformed shapes return `-32600 Invalid Request` instead of being
silently mistaken for notifications. Optional tool arguments also reject the
wrong JSON type with `-32602 Invalid Params` rather than dropping the field.
Runtime tool dispatch enforces each advertised `additionalProperties: false`
schema too: misspelled or unknown argument names return `-32602` instead of being
silently ignored.

Ticket lists and metrics support opt-in keyset pagination:

```http
GET /v1/tickets?limit=50&cursor=<opaque-cursor>&status=open&source=hook
GET /v1/tickets/metrics?limit=50&cursor=<opaque-cursor>&status=open&source=hook
```

`limit` must be between 1 and 200. When a filter or pagination parameter is
present, the response is `{ "items": [...], "next_cursor": "..." | null }`.
The cursor is an opaque, versioned representation of Tea's unique zero-based
ticket ordinal and is exclusive: the next page starts strictly after the last
delivered ticket. It is bound to the original `status` and `source` filters, so
reusing it with different filters returns `400` instead of silently skipping
records. SQLite executes `LIMIT + 1` keyset queries and computes metrics only for
the current ticket page. Its bounded correlated metric lookups intentionally use
the ticket ordinal plus ticket-scoped comment, run, and event indexes; a query-plan
regression test prevents an apparently batched rewrite from degrading into full
child-table scans. The no-query response remains the historical bare array for
compatibility with already released clients; new clients should use the paged
contract rather than the legacy full-list path.

The desktop client and `tea-sync` traverse pages in 200-ticket chunks. Both
reject repeated cursors, empty continuation pages, oversized pages, conflicting
duplicate ticket records, and more than 1,000 pages instead of returning a
silently truncated snapshot. `tea-sync` also accepts a bare array from an older
Tea daemon as one terminal compatibility page.

Ticket approval policy can be overridden explicitly when a human wants to
tighten or relax the run gate for a specific ticket:

```powershell
cargo run -p tea-cli -- ticket policy <ticket-id> --mode manual_only
```

Approval decisions are bound to the policy under which they were granted. When
an operator policy update or a new analysis recommendation changes that policy,
both memory and SQLite stores atomically clear the earlier approval/rejection.
Reapplying the same policy preserves its approval, avoiding unnecessary prompts.

Run-policy semantics are explicit rather than label-only:

- `auto_if_low_risk` runs without approval only for low-risk tickets; medium and
  high risk require human approval;
- `auto_if_validation_passes` requires a persisted successful validation result
  or human approval. Tea does not yet persist such a pre-execution result, so the
  current API conservatively requires approval;
- `human_before_completion` permits execution but still gates close/accept at the
  completion boundary;
- `manual_only` never starts an automatic run, even after an approval record.

Tickets whose analysis reports missing context remain `needs_info` when a plan is
stored, and both API and store run boundaries reject them. A later plan therefore
cannot accidentally turn incomplete context into an executable work order.

Run records can be inspected and controlled directly when a run id is known:

```powershell
cargo run -p tea-cli -- run show <run-id>
cargo run -p tea-cli -- run stop <run-id>
cargo run -p tea-cli -- run retry <run-id>
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
The current version is `4`. Version 2 added database-enforced unique ticket and
per-ticket child ordinals so concurrent Tea processes cannot create ambiguous
event/run ordering. Version 3 adds durable create-idempotency records keyed by
route scope and caller-supplied key. Version 4 adds generated `status` and
`source` columns plus `(status, ordinal)` and `(source, ordinal)` indexes so
filtered ticket list and metrics pages stay bounded by page size. Reopening the same store is
idempotent, and legacy v1 stores that predate migration metadata are marked as
version 1 during startup. If a store records a schema version newer than the
current binary supports, Tea refuses to open it instead of writing through an
unknown future schema. Business schema creation and migration version recording
are committed in one SQLite transaction, so a failed version record cannot leave
behind partially-created Tea tables without matching migration metadata.
On every open, Tea also repairs the required unique ordinal indexes and removes
obsolete non-unique indexes on the same columns, avoiding duplicate B-tree writes
without changing the ordinal data format.

SQLite read paths materialize raw JSON while holding the single connection mutex,
then release the mutex before deserializing tickets, metrics pages, bundles,
comments, events, analyses, plans, and runs. Snapshot reads still gather all raw
rows in one transaction, while large JSON payloads no longer extend connection-lock
hold time and block unrelated database operations.

`GET /v1/status` includes the active store backend and schema compatibility
metadata plus the active configuration ownership. In memory mode the schema
and SQLite capacity fields are `null`; in SQLite mode `schema_version` reports
the highest recorded migration version and `supported_schema_version` reports
the newest version the current binary can write. `idempotency_key_count` reports
the durable create-idempotency record count, while `sqlite_page_count` and
`sqlite_freelist_count` expose the database page allocation and reusable-page
counts without changing retention. `configuration_source` is `local`,
`loom-managed`, or `fallback`; the nested `configuration` object exposes the
current owner, local config path, Loom base URL, Loom panel URL when present,
and any fallback reason.

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

## Desktop editing sessions

The work-order editor belongs to the selected ticket and daemon connection.
Switching either closes the old draft. Saving compares fields with the snapshot
from when editing opened, so background refreshes cannot turn untouched values
into edits that overwrite another update. No-op saves send no PATCH; failed saves
keep the draft, and an older save response cannot close a newly opened editor.
The API still uses last-write behavior when two clients deliberately edit the
same field; this is not a server-side optimistic-concurrency contract.

## Ticket lifecycle contract

Closed and cancelled tickets are read-only terminal records. Tea still allows
read-only endpoints such as ticket show/list, comments, events, runs, and export
after a ticket is closed, because those endpoints are needed for audit and
review. Completed and accepted tickets remain in the desktop's Open / Needs
review queue until explicitly closed. Their review comments and completion
approval controls remain available; Accept is enabled for completed tickets,
and Close for completed or accepted tickets. The daemon still enforces evidence
and approval requirements before accepting these actions.

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

Close validation is repeated inside the store's atomic boundary. In-memory
stores check evidence and completion approval under one mutex guard; SQLite
stores perform the same checks and state transition in one immediate
transaction. A concurrent approval change therefore cannot bypass a
`human_before_completion` close gate between API policy evaluation and commit.
The ticket must currently be `completed` or `accepted`; evidence from an older
successful run cannot close a ticket whose latest execution left it `failed` or
`needs_review`. Granting a completion approval records the approval without
regressing an already-completed or accepted ticket to the pre-execution
`approved` state. An accepted ticket can therefore receive completion approval
and then close without repeating its execution or human acceptance.

Rejected approval keeps a ticket in `Blocked`; blocked tickets reject new run
attempts before Tea calls Loom, even if the ticket policy would otherwise allow
automatic execution.

Run actions have a separate state guard:

- stop is valid only from `queued`, `running`, or `retrying`, and must return
  `stopped`;
- retry is valid only from `failed` or `stopped`, and must return `retrying`;
  retries re-evaluate the current ticket status, risk and approval policy before
  dispatch, so an earlier run does not bypass a later rejection, missing context,
  or tightened approval requirement. Stop remains available for active runs even
  when execution permission has since been restricted, and preserves blocked or
  missing-context states rather than clearing those restrictions;
- `succeeded` is a terminal run outcome: its status, Loom session, and evidence
  snapshot are immutable and cannot be overwritten by later updates, stop, or
  retry;
- normal Loom progress may move `queued` to `running`, `succeeded`, `failed`, or
  `stopped`; `running` to `succeeded`, `failed`, or `stopped`; and `retrying` to
  `running`, `succeeded`, `failed`, or `stopped`.

Tea rejects an invalid action with `409 Conflict` before calling Loom, validates
the status returned by Loom, and repeats the transition check in both store
backends before persistence. Successful stop/retry updates append
`run_stopped`/`run_retrying` events, so these operator-visible actions remain in
the ticket audit timeline.

One API process also keeps a per-ticket in-flight action gate around run, stop,
and retry dispatch. A second overlapping operator action for the same ticket is
rejected with `409 Conflict` before another Loom side effect is issued; actions on
different tickets remain independent. Store compare-and-set remains the durable
last line of defense against external run updates. This process-local gate does
not claim cross-daemon or crash-recovery idempotency; that requires a future Loom
operation-key/reservation protocol.
A retry response is also checked against current authorization under the store's
mutex/write transaction before it can change a failed or stopped run to retrying.
This preserves a newer rejection or policy change in Tea, but cannot undo an
external Loom call already issued before that decision changed.

After Loom returns, Tea also compare-and-sets the complete Run snapshot read
before the remote call. A response is rejected with `409 Conflict` if another
action changed that Run while the request was in flight. Ticket-level stop/retry
additionally require the addressed Run to still be the ticket's latest Run.
These checks execute under the Memory mutex or SQLite immediate transaction, so
stale remote responses cannot overwrite the winning local state or append
duplicate audit events.

## BrainProvider and Loom integration

Tea owns decomposition records and lifecycle state. Loom owns strong reasoning
that generates decomposition proposals. Gateway is not part of Tea ticket
decomposition business logic.

Tea uses a deterministic in-process template BrainProvider and mock Loom client
unless a Loom endpoint is configured. This keeps standalone and Platform-local
development usable without embedding Loom's agent runtime in Tea, but mock runs
are simulation results rather than evidence that external work was performed.
`GET /v1/status` reports `execution_provider: "mock"` in this mode and
`execution_provider: "loom"` when real Loom dispatch is configured. When
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
Tea ticket state directly. A provider proposal may tighten, but cannot remove, an
existing pre-execution or completion approval gate. Tea also rejects a proposal
whose plan says no execution approval is required while its recommended policy
would require one.

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
so Loom cannot redirect a run action to another run in the same ticket, and the
response status must be the requested `stopped` or `retrying` state.

## Container image

```powershell
docker build -f Dockerfile -t neuro-tea:local .
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
cargo run -p tea-daemon
cargo run -p tea-cli -- status
cargo run -p tea-cli -- ticket create --title "Smoke" --body "Body"
cargo run -p tea-cli -- ticket list
```

For a repeatable real local acceptance smoke, run the repository-level harness:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-tea-cli-real.ps1
```

This proves the self-contained path `tea CLI -> tea-daemon -> HTTP API -> SQLite store -> stateful Loom HTTP stub`.
The harness builds the debug `tea-daemon.exe`, `tea-cli.exe`, and `tea-sync.exe`, starts an isolated
daemon and Loom stub on separate free loopback ports, uses temporary
`TEA_STORE_PATH` and `TEA_CONFIG_PATH` values, then drives the full CLI lifecycle:
`status/config/create/comment/edit/decompose/approve/run/stop/retry/accept/close/cancel/export/events`.
The main run returns completed evidence; a separate control run genuinely moves
from `running` to `stopped` to `retrying`. The harness verifies the matching
audit events, Markdown evidence, JSON export events, daemon/stub shutdown, and
zero listeners left on both selected ports after cleanup.
It also drives a closed external issue through `tea-sync`, requiring the first
sync pass to create the mirror with a deterministic idempotency key and
immediately apply the mapped `cancel` lifecycle action to the returned ticket ID.

Smoke artifacts are written under `.tmp/tea-smoke/tea-cli-real-<timestamp>/`.
By default the temporary SQLite store and config file are removed after a
passing run; pass `-KeepArtifacts` to preserve them for inspection. Pass
`-Release` to run the same lifecycle against `target\release` binaries.

For a repeatable Tea -> Loom decompose acceptance smoke, run:

The Tea -> Loom, Hook -> Tea, and Platform -> Tea harnesses below are maintained
in the parent Neuro workspace because they span independently versioned
repositories. Run those commands from the Neuro repository root; they are not
part of a standalone Tea clone.

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
SQLite schema: 4 (supported: 4)
Configuration source: local
Configuration owner: tea
```

Configuration can also be inspected or changed through the CLI:

```powershell
cargo run -p tea-cli -- config show
cargo run -p tea-cli -- config set --notifications-enabled false
cargo run -p tea-cli -- config set --human-ticket-default-approval-policy human_before_completion
cargo run -p tea-cli -- config set --hook-ticket-default-approval-policy manual_only
```

`tea-sync` preflights every external issue before applying the first write. If
multiple distinct Tea tickets carry the same `sync-id:<provider>:<external-id>`
label, the pass fails with the conflicting ticket IDs instead of silently
updating an arbitrary mirror or partially applying earlier issues.
When a previously unseen external issue is already closed, the same pass uses
the created Tea ticket ID to apply the mapped terminal lifecycle action instead
of leaving the mirror open until the next sync.
If a create returns `409` because the stable key belongs to an earlier uncertain
request with older issue content, sync does not change keys or create again. It
refreshes the complete Tea ticket snapshot, requires exactly one matching
`sync-id` mirror, and PATCHes that ticket with the current external content.
Zero or multiple matches fail the pass for operator reconciliation.

The initial Tea mirror snapshot is fetched through cursor pagination with a
200-ticket page size. Cursor loops, malformed envelopes, conflicting duplicate
ticket IDs, and the 1,000-page safety ceiling fail the pass before planning or
the first write, so sync never proceeds from a partial Tea snapshot.

Tracker issue reads and the initial Tea ticket-list read use a bounded GET-only
retry policy: at most three attempts within the existing 300-second request
budget, with capped backoff for transport failures, `408`, `425`, `429`, and
selected `5xx` responses. `Retry-After` delta seconds are honored up to two
seconds. Ticket create/edit/lifecycle `POST` and `PATCH` requests are never
automatically replayed. The create-conflict recovery above performs a fresh GET
and a distinct PATCH only after Tea confirms the stable create key conflicts.

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

Run Tea-local validation from the Tea repository root:

Pull requests targeting `main` run the same read-only build, dependency audit,
test, and Windows package verification gates as main-branch builds. PR runs do
not upload release artifacts; publishing GitHub releases remains restricted to
the separate tag/manual release workflow.

The release workflows pin third-party GitHub Actions to immutable commit SHAs;
the adjacent comments retain the audited major-version/toolchain intent.
Manual releases pass the requested tag through an environment variable, validate
the complete value as `Vx.x.x`, and only then check out the exact `refs/tags/*`
reference. Checkout does not persist the workflow token in Git configuration.
Published release assets are immutable (`overwrite_files: false`). The package
verifier requires one ZIP artifact, exact 64-hex SHA-256 values, unique canonical
checksum records, and a single canonical ZIP sidecar whose file name matches the
artifact. Executable manifest paths must exactly match their canonical names, so
hash and reproducibility records cannot be redirected to alternate payload files.
Release manifest schema 3 records two separate contracts. ZIP payload
paths are written in ordinal order, every entry uses the fixed, timezone-free
ZIP/DOS timestamp `1980-01-01T00:00:00` and zero external attributes, and the
builder must produce the same SHA-256 twice from the same payload. Windows MSVC
builds also use `/Brepro`; the builder runs the complete release command sequence
twice and requires matching size and SHA-256 for all five executables. The
manifest deliberately records this as `same-worktree-repeat-build` with
`cleanBuilds: false`, so it is not misrepresented as an independent clean-build
reproducibility guarantee. The verifier checks both contracts and independently
validates the stored ZIP entry order and metadata.

The builder and verifier enumerate package files without following filesystem
links and reject symbolic links, directory junctions, and other reparse points in
the package tree or a manifest path. Every manifest hash record is resolved to a
canonical path below the package root before it is read. The manifest contract
uses a correctly hashed `..\outside-build-info.txt` record and requires rejection,
so a package cannot redirect build-info, checksum, or ZIP reads outside its
declared directory. The Windows reparse containment contract constructs real
package and build-output junctions and requires both verification and the
builder's output/destination checks to fail before `-Force` can remove or create
release content.
The standalone GitHub-asset packager applies the same component-by-component
check from the Tea release root through its output ZIP and sidecar paths. Its
contract points the requested output directory at a real junction and requires
rejection before any file appears in the junction target.

The tag workflow keeps `contents: read` while compiling and running smoke tests.
Only the separate publish job receives `contents: write`; it downloads the two
verified release assets, revalidates the canonical sidecar and ZIP hash, and then
creates the GitHub release. This prevents build tools and test processes from
inheriting repository write permission.

The native UI smoke runs the issue timeline at a 375-pixel viewport. It rejects
global horizontal overflow and detail controls that extend outside their card,
while excluding descendants of explicitly scrollable horizontal tab strips from
the diagnostic overflow list. It then repeats the containment check after `Copy
entry link` changes to the longer `Copied entry link` state. The same smoke also
rejects interactive controls nested inside the activity-log disclosure summary
and issue-row buttons whose native button role has been overridden or whose
contents use non-phrasing container elements. It also fires two create-button
activations in one JavaScript turn and requires exactly one work order, verifying
the desktop client's synchronous in-flight create gate. It also lets the daemon
persist a create before injecting a lost WebView response, then resubmits the
unchanged draft and requires both attempts to use the same idempotency key and
leave exactly one durable ticket.
Timeline deep links preserve malformed percent-encoded hashes instead of
throwing during render or `hashchange`. Clipboard permission failures resolve
without an unhandled promise rejection and fall back to the entry hash; the
copied state is shown only after an actual clipboard write.

```powershell
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo fmt --check --manifest-path .\apps\desktop\src-tauri\Cargo.toml
cargo clippy --locked --manifest-path .\apps\desktop\src-tauri\Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path .\apps\desktop\src-tauri\Cargo.toml
cargo audit
cargo audit --file .\apps\desktop\src-tauri\Cargo.lock
Push-Location .\apps\desktop
npm ci --no-audit --no-fund
npm audit --audit-level=high
npm run typecheck
npm run lint
npm test
Pop-Location
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\test-tea-launcher-contract.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\test-tea-release-reparse-contract.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\test-tea-release-manifest-contract.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\test-tea-release-asset-copy-contract.ps1 -PackageDir <absolute-release-Tea-path>\<versionId>
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\test-tea-release-asset-reparse-contract.ps1 -PackageDir <absolute-release-Tea-path>\<versionId>
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-tea-config-concurrency-real.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-tea-config-recovery-real.ps1 -PackageDir <absolute-release-Tea-path>\<versionId>
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-tea-cli-real.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-tea-cli-real.ps1 -Release
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-tea-mcp-real.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-local-tea-release.ps1 -OutputDir <absolute-release-Tea-path> -VersionId <versionId>
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-tea-ui-real.ps1 -PackageDir <absolute-release-Tea-path>\<versionId>
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-tea-release-package.ps1 -PackageDir <absolute-release-Tea-path>\<versionId> -RunSmoke
```

When Tea is checked out as the Neuro submodule, the parent repository also owns
cross-project harness contracts such as
`scripts\tests\test-smoke-tea-cli-real-contract.ps1`. Run those from the Neuro
repository root rather than expecting them in a standalone Tea checkout.

Platform Web's backend-mediated Tea entry can be validated from `Platform/`:

```powershell
cd Platform
node --test --import tsx web/src/lib/tea-client.test.ts web/src/lib/tea-route-utils.test.ts web/src/lib/tea-api-handlers.test.ts web/src/lib/tea-detail-controls.test.ts web/src/lib/tea-real-core-smoke.test.ts web/src/lib/tea-web-ui-smoke-harness-contract.test.ts
```
