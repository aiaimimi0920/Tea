[CmdletBinding()]
param(
    [int]$Port = 0,
    [string]$AuthToken = "tea-cli-smoke-token",
    [int]$TimeoutSec = 45,
    [switch]$Release,
    [string]$PackageDir = "",
    [switch]$AllowDirtyManifest,
    [switch]$KeepArtifacts
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-FreeTcpPort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Parse("127.0.0.1"), 0)
    $listener.Start()
    try {
        return $listener.LocalEndpoint.Port
    }
    finally {
        $listener.Stop()
    }
}

function Get-PortListeners {
    param(
        [int]$Port
    )

    $listeners = @()
    $lines = @(netstat -ano -p TCP 2>$null)
    foreach ($line in $lines) {
        $parts = @($line -split "\s+" | Where-Object { $_ -ne "" })
        if ($parts.Count -lt 5 -or $parts[0] -ne "TCP") {
            continue
        }

        $state = $parts[$parts.Count - 2]
        if ($state -ne "LISTENING") {
            continue
        }

        $localEndpoint = $parts[1]
        $lastColon = $localEndpoint.LastIndexOf(":")
        if ($lastColon -lt 0) {
            continue
        }

        $localPortText = $localEndpoint.Substring($lastColon + 1)
        $localPort = 0
        if (![int]::TryParse($localPortText, [ref]$localPort)) {
            continue
        }
        if ($localPort -ne $Port) {
            continue
        }

        $processId = 0
        [void][int]::TryParse($parts[$parts.Count - 1], [ref]$processId)
        $listeners += [pscustomobject]@{
            local_endpoint = $localEndpoint
            local_address = $localEndpoint.Substring(0, $lastColon)
            local_port = $localPort
            pid = $processId
        }
    }

    return $listeners
}

function Assert-NoPreexistingPortListeners {
    param([int[]]$Ports)

    foreach ($port in $Ports) {
        $listeners = @(Get-PortListeners -Port $port)
        if ($listeners.Count -gt 0) {
            $payload = $listeners | ConvertTo-Json -Depth 8 -Compress
            throw "blocked_preexisting_listener port=$port listeners=$payload"
        }
    }
}

function Assert-True {
    param(
        [bool]$Condition,
        [string]$Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

function Assert-Equal {
    param(
        [object]$Expected,
        [object]$Actual,
        [string]$Message
    )

    if ($Expected -ne $Actual) {
        throw "$Message Expected=[$Expected] Actual=[$Actual]"
    }
}

function Restore-EnvValue {
    param(
        [string]$Name,
        [AllowNull()]
        [string]$Value
    )

    if ($null -eq $Value) {
        Remove-Item -Path "Env:$Name" -ErrorAction SilentlyContinue
    }
    else {
        Set-Item -Path "Env:$Name" -Value $Value
    }
}

function Invoke-Checked {
    param(
        [string]$FilePath,
        [string[]]$Arguments,
        [string]$WorkingDirectory
    )

    Write-Host ">> $FilePath $($Arguments -join ' ')"
    Push-Location $WorkingDirectory
    $previousErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        $output = & $FilePath @Arguments 2>&1
        $exitCode = $LASTEXITCODE
        if ($exitCode -ne 0) {
            throw "Command failed with exit code $exitCode`: $FilePath $($Arguments -join ' ')`n$($output -join [Environment]::NewLine)"
        }
        return $output
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
        Pop-Location
    }
}

function Invoke-TeaRaw {
    param(
        [string[]]$Arguments
    )

    Write-Host ">> tea $($Arguments -join ' ')"
    $previousErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        $output = & $script:teaExe @Arguments 2>&1
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($exitCode -ne 0) {
        throw "tea command failed with exit code $exitCode`: tea $($Arguments -join ' ')`n$($output -join [Environment]::NewLine)"
    }
    return ($output -join [Environment]::NewLine)
}

function Invoke-TeaJson {
    param(
        [string[]]$Arguments
    )

    return (Invoke-TeaRaw -Arguments $Arguments | ConvertFrom-Json)
}

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptRoot
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$artifactRoot = Join-Path $repoRoot ".tmp\tea-smoke\tea-cli-real-$timestamp"
$storePath = Join-Path $artifactRoot "tea-cli-smoke.sqlite"
$configPath = Join-Path $artifactRoot "tea-config.json"
$stdoutPath = Join-Path $artifactRoot "tea-daemon.stdout.log"
$stderrPath = Join-Path $artifactRoot "tea-daemon.stderr.log"
$loomStubPath = Join-Path $artifactRoot "loom-smoke-stub.mjs"
$loomStdoutPath = Join-Path $artifactRoot "loom-smoke-stub.stdout.log"
$loomStderrPath = Join-Path $artifactRoot "loom-smoke-stub.stderr.log"
$summaryPath = Join-Path $artifactRoot "summary.json"
$teaManifest = Join-Path $repoRoot "Cargo.toml"
$usingPackage = -not [string]::IsNullOrWhiteSpace($PackageDir)
if ($usingPackage -and $Release) {
    throw "-PackageDir already selects release package binaries; do not combine it with -Release."
}

$resolvedPackageDir = $null
if ($usingPackage) {
    $resolvedPackageDir = (Resolve-Path -LiteralPath $PackageDir).Path
}

$buildProfile = if ($usingPackage) { "package" } elseif ($Release) { "release" } else { "debug" }
$targetDir = if ($usingPackage) { $resolvedPackageDir } else { Join-Path $repoRoot "target\$buildProfile" }
$script:teaExe = Join-Path $targetDir "tea-cli.exe"
$teaDaemonExe = Join-Path $targetDir "tea-daemon.exe"
$teaSyncExe = Join-Path $targetDir "tea-sync.exe"
$packageManifestPath = if ($usingPackage) { Join-Path $resolvedPackageDir "manifest.json" } else { $null }

if ($Port -eq 0) {
    $Port = Get-FreeTcpPort
}

function Invoke-TeaExpectedFailure {
    param(
        [string[]]$Arguments,
        [string]$ExpectedText
    )

    Write-Host ">> tea $($Arguments -join ' ') (expect failure)"
    $previousErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        $output = & $script:teaExe @Arguments 2>&1
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    $text = $output -join [Environment]::NewLine
    if ($exitCode -eq 0) {
        throw "tea command unexpectedly succeeded: tea $($Arguments -join ' ')"
    }
    if (-not $text.Contains($ExpectedText)) {
        throw "tea command failed without expected text [$ExpectedText]: tea $($Arguments -join ' ')`n$text"
    }
    return $text
}
$loomPort = Get-FreeTcpPort
while ($loomPort -eq $Port) {
    $loomPort = Get-FreeTcpPort
}
Assert-NoPreexistingPortListeners -Ports @($Port, $loomPort)

New-Item -ItemType Directory -Force -Path $artifactRoot | Out-Null

$baseUrl = "http://127.0.0.1:$Port"
$loomBaseUrl = "http://127.0.0.1:$loomPort"
$daemon = $null
$loomStub = $null
$summary = [ordered]@{
    status = "running"
    base_url = $baseUrl
    ticket_id = $null
    run_id = $null
    control_ticket_id = $null
    control_run_id = $null
    cancelled_ticket_id = $null
    cancelled_ticket_status = $null
    decomposition_provider_mode = $null
    decomposition_recommended_workflow = $null
    decomposition_step_count = $null
    stopped_run_status = $null
    retried_run_status = $null
    control_events_include_stopped = $false
    control_events_include_retrying = $false
    accepted_ticket_status = $null
    closed_ticket_status = $null
    policy_change_ticket_id = $null
    policy_change_invalidated_approval = $false
    policy_change_close_status = $null
    cancel_events_include_cancelled = $false
    markdown_contains_evidence = $false
    json_export_contains_events = $false
    settings_page_contains_local_ui = $false
    package_ui_exe_present = $false
    artifact_root = $artifactRoot
    store_path = $storePath
    config_path = $configPath
    stdout_path = $stdoutPath
    stderr_path = $stderrPath
    loom_base_url = $loomBaseUrl
    loom_stub_path = $loomStubPath
    loom_stdout_path = $loomStdoutPath
    loom_stderr_path = $loomStderrPath
    loom_stub_pid = $null
    daemon_pid = $null
    build_profile = $buildProfile
    package_dir = $resolvedPackageDir
    package_manifest_path = $packageManifestPath
    package_git_dirty = $null
    allow_dirty_manifest = [bool]$AllowDirtyManifest
    keep_artifacts = [bool]$KeepArtifacts
    cleanup_checked_at = $null
    daemon_stopped = $false
    loom_stub_stopped = $false
    port_listener_count_after_stop = $null
    loom_port_listener_count_after_stop = $null
    listeners_after_stop = @()
    store_preserved = $null
    started_at = (Get-Date).ToString("o")
    finished_at = $null
    error = $null
}

$oldEnv = @{
    TEA_BIND_ADDR = $env:TEA_BIND_ADDR
    TEA_AUTH_TOKEN = $env:TEA_AUTH_TOKEN
    TEA_STORE_PATH = $env:TEA_STORE_PATH
    TEA_CONFIG_PATH = $env:TEA_CONFIG_PATH
    TEA_LOOM_BASE_URL = $env:TEA_LOOM_BASE_URL
    TEA_LOOM_AUTH_TOKEN = $env:TEA_LOOM_AUTH_TOKEN
    TEA_SERVER_URL = $env:TEA_SERVER_URL
    TEA_SMOKE_LOOM_PORT = $env:TEA_SMOKE_LOOM_PORT
}

try {
    if ($usingPackage) {
        Assert-True (Test-Path -LiteralPath $packageManifestPath) "clean release package manifest.json was not found at $packageManifestPath"
        $manifest = Get-Content -LiteralPath $packageManifestPath -Raw | ConvertFrom-Json
        Assert-Equal "Tea" ([string]$manifest.app) "clean release package manifest is not for Tea."
        if (-not $AllowDirtyManifest) {
            Assert-Equal $false ([bool]$manifest.gitDirty) "clean release package manifest must report gitDirty: false."
        }
        $summary["package_git_dirty"] = [bool]$manifest.gitDirty
    }
    else {
        $buildArguments = @(
            "build",
            "--manifest-path", $teaManifest
        )
        if ($Release) {
            $buildArguments += "--release"
        }
        $buildArguments += @(
            "-p", "tea-daemon",
            "-p", "tea-cli",
            "-p", "tea-sync"
        )

        Invoke-Checked -FilePath "cargo" -WorkingDirectory $repoRoot -Arguments $buildArguments | Out-Null
    }

    Assert-True (Test-Path -LiteralPath $teaDaemonExe) "tea-daemon.exe was not built at $teaDaemonExe"
    Assert-True (Test-Path -LiteralPath $script:teaExe) "tea-cli.exe was not built at $script:teaExe"
    Assert-True (Test-Path -LiteralPath $teaSyncExe) "tea-sync.exe was not built at $teaSyncExe"
    Assert-True ($null -ne (Get-Command "node" -ErrorAction SilentlyContinue)) "node is required for the stateful Loom smoke stub."

    # In package mode, also verify the copied Tea UI program is present. The
    # release package ships tea.exe (the GUI) alongside tea-cli.exe and
    # tea-daemon.exe, so a package that dropped the UI binary must fail here.
    if ($usingPackage) {
        $teaUiExe = Join-Path $targetDir "tea.exe"
        Assert-True (Test-Path -LiteralPath $teaUiExe) "tea.exe UI program was not found in package at $teaUiExe"
        $summary["package_ui_exe_present"] = $true
    }

    $loomStubSource = @'
import http from "node:http";
import { randomUUID } from "node:crypto";

const host = "127.0.0.1";
const port = Number.parseInt(process.env.TEA_SMOKE_LOOM_PORT ?? "", 10);
let syncObserved = {
  listCount: 0,
  createCount: 0,
  idempotencyKeys: [],
  editCount: 0,
  editTicketId: null,
  lifecycleCount: 0,
  lifecycleAction: null,
  lifecycleTicketId: null,
};
if (!Number.isInteger(port) || port < 1 || port > 65535) {
  throw new Error("TEA_SMOKE_LOOM_PORT must be a valid TCP port");
}

function send(response, status, payload) {
  const body = JSON.stringify(payload);
  response.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(body),
  });
  response.end(body);
}

function readJson(request) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let length = 0;
    request.on("data", (chunk) => {
      length += chunk.length;
      if (length > 2 * 1024 * 1024) {
        reject(new Error("request body exceeded 2 MiB"));
        request.destroy();
        return;
      }
      chunks.push(chunk);
    });
    request.on("end", () => {
      try {
        resolve(JSON.parse(Buffer.concat(chunks).toString("utf8")));
      } catch (error) {
        reject(error);
      }
    });
    request.on("error", reject);
  });
}

function proposalFor(request) {
  return {
    requestId: request.requestId,
    status: "succeeded",
    output: {
      proposal: {
        schema_version: 1,
        proposal_id: `smoke-proposal-${randomUUID()}`,
        analysis: {
          intent: "engineering_work_order",
          target_components: ["Tea"],
          target_paths: [],
          constraints: ["isolated smoke environment"],
          acceptance_criteria: ["run lifecycle completes"],
          missing_context: [],
          risk_assessment: "medium",
          confidence: 0.95,
          recommended_policy: "human_before_execute",
          recommended_workflow: "loom.tea_ticket_decompose.v1",
        },
        plan: {
          summary: "Exercise Tea through the stateful Loom smoke stub.",
          steps: [
            { id: "inspect", title: "Inspect", description: "Inspect the isolated request." },
            { id: "execute", title: "Execute", description: "Execute the requested smoke run." },
            { id: "verify", title: "Verify", description: "Verify evidence and lifecycle state." },
          ],
          required_tools: ["loom.run"],
          expected_artifacts: ["smoke-evidence.json"],
          validation_strategy: ["verify run status"],
          rollback_strategy: ["stop the run"],
          requires_approval_before_execute: true,
        },
        requires_human_review: true,
        notes: [],
      },
    },
  };
}

function startedRun(ticket) {
  const isControlRun = ticket.title === "Tea CLI run control smoke";
  return {
    id: randomUUID(),
    ticket_id: ticket.id,
    loom_session_id: `smoke-session-${randomUUID()}`,
    status: isControlRun ? "running" : "succeeded",
    evidence: isControlRun
      ? null
      : {
          summary: "smoke loom run completed",
          commands: ["tea smoke verify"],
          artifacts: ["smoke-evidence.json"],
          risks: [],
        },
  };
}

const server = http.createServer(async (request, response) => {
  try {
    const url = new URL(request.url ?? "/", `http://${host}:${port}`);
    if (request.method === "GET" && url.pathname === "/health") {
      send(response, 200, { status: "ok" });
      return;
    }
    if (request.method === "GET" && url.pathname === "/v1/configuration/claims") {
      send(response, 200, {
        app: "tea",
        managed: false,
        panel_url: null,
        reason: "stateful smoke stub leaves configuration under Tea ownership",
      });
      return;
    }
    if (request.method === "GET" && url.pathname === "/repos/smoke/tea/issues") {
      send(response, 200, [
        {
          number: "外部 工单:42",
          title: "tea-sync packaged idempotency smoke",
          body: "Verify the packaged sync adapter sends a deterministic create key.",
          state: "closed",
          labels: [{ name: "priority:high" }],
          html_url: "https://example.invalid/smoke/tea/issues/42",
        },
        {
          number: "外部 工单:43",
          title: "tea-sync changed payload recovery smoke",
          body: "Recover a mirror created by an earlier uncertain request, then refresh it.",
          state: "open",
          labels: [{ name: "area:sync-recovery" }],
          html_url: "https://example.invalid/smoke/tea/issues/43",
        },
      ]);
      return;
    }
    if (request.method === "GET" && url.pathname === "/v1/tickets") {
      syncObserved = { ...syncObserved, listCount: syncObserved.listCount + 1 };
      const items = syncObserved.listCount === 1
        ? []
        : [
            { id: "sync-smoke-ticket", labels: ["sync-id:github:外部 工单:42"] },
            { id: "sync-recovered-ticket", labels: ["sync-id:github:外部 工单:43"] },
          ];
      send(response, 200, { items, next_cursor: null });
      return;
    }
    if (request.method === "POST" && url.pathname === "/v1/tickets") {
      const key = request.headers["idempotency-key"] ?? null;
      const body = await readJson(request);
      syncObserved = {
        ...syncObserved,
        createCount: syncObserved.createCount + 1,
        idempotencyKeys: [...syncObserved.idempotencyKeys, key],
      };
      if (body.title === "tea-sync changed payload recovery smoke") {
        send(response, 409, { error: "idempotency key was already used with a different request" });
        return;
      }
      send(response, 200, { id: "sync-smoke-ticket", ...body });
      return;
    }
    const syncEdit = url.pathname.match(/^\/v1\/tickets\/([^/]+)$/);
    if (request.method === "PATCH" && syncEdit) {
      syncObserved = {
        ...syncObserved,
        editCount: syncObserved.editCount + 1,
        editTicketId: syncEdit[1],
      };
      send(response, 200, { id: syncEdit[1], ...(await readJson(request)) });
      return;
    }
    const syncLifecycle = url.pathname.match(/^\/v1\/tickets\/([^/]+)\/(cancel)$/);
    if (request.method === "POST" && syncLifecycle) {
      syncObserved = {
        ...syncObserved,
        lifecycleCount: syncObserved.lifecycleCount + 1,
        lifecycleAction: syncLifecycle[2],
        lifecycleTicketId: syncLifecycle[1],
      };
      send(response, 200, { id: syncLifecycle[1], status: "cancelled" });
      return;
    }
    if (request.method === "GET" && url.pathname === "/sync-observed") {
      send(response, 200, syncObserved);
      return;
    }
    if (request.method === "POST" && url.pathname === "/v1/invoke") {
      send(response, 200, proposalFor(await readJson(request)));
      return;
    }
    if (request.method === "POST" && url.pathname === "/v1/runs") {
      const body = await readJson(request);
      send(response, 200, startedRun(body.ticket));
      return;
    }

    const action = url.pathname.match(/^\/v1\/runs\/([^/]+)\/(stop|retry)$/);
    if (request.method === "POST" && action) {
      const body = await readJson(request);
      const run = body.run;
      if (!run || run.id !== action[1]) {
        send(response, 409, { error: "run identity mismatch" });
        return;
      }
      if (action[2] === "stop") {
        if (!["queued", "running", "retrying"].includes(run.status)) {
          send(response, 409, { error: `cannot stop run in ${run.status} status` });
          return;
        }
        send(response, 200, { ...run, status: "stopped" });
        return;
      }
      if (!["failed", "stopped"].includes(run.status)) {
        send(response, 409, { error: `cannot retry run in ${run.status} status` });
        return;
      }
      send(response, 200, { ...run, status: "retrying" });
      return;
    }

    send(response, 404, { error: `unsupported smoke route ${request.method} ${url.pathname}` });
  } catch (error) {
    send(response, 400, { error: error instanceof Error ? error.message : String(error) });
  }
});

server.listen(port, host, () => {
  process.stdout.write(`stateful Loom smoke stub listening on http://${host}:${port}\n`);
});
'@
    [IO.File]::WriteAllText($loomStubPath, $loomStubSource, [Text.UTF8Encoding]::new($false))

    Write-Host ">> starting stateful Loom smoke stub at $loomBaseUrl"
    $env:TEA_SMOKE_LOOM_PORT = [string]$loomPort
    $loomStub = Start-Process -FilePath "node" `
        -ArgumentList @($loomStubPath) `
        -WorkingDirectory $repoRoot `
        -RedirectStandardOutput $loomStdoutPath `
        -RedirectStandardError $loomStderrPath `
        -WindowStyle Hidden `
        -PassThru
    $summary["loom_stub_pid"] = $loomStub.Id

    $loomDeadline = (Get-Date).AddSeconds($TimeoutSec)
    $loomHealthy = $false
    while ((Get-Date) -lt $loomDeadline) {
        if ($loomStub.HasExited) {
            throw "stateful Loom smoke stub exited early with code $($loomStub.ExitCode). stdout=$loomStdoutPath stderr=$loomStderrPath"
        }
        try {
            $loomHealth = Invoke-RestMethod -Uri "$loomBaseUrl/health" -Method Get -TimeoutSec 2
            if ($loomHealth.status -eq "ok") {
                $loomHealthy = $true
                break
            }
        }
        catch {
            Start-Sleep -Milliseconds 200
        }
    }
    Assert-True $loomHealthy "stateful Loom smoke stub did not become healthy at $loomBaseUrl within $TimeoutSec seconds"

    Invoke-Checked `
        -FilePath $teaSyncExe `
        -WorkingDirectory $repoRoot `
        -Arguments @(
            "--provider", "github",
            "--owner", "smoke",
            "--repo", "tea",
            "--api-base", $loomBaseUrl,
            "--tea-url", $loomBaseUrl,
            "--tea-token", "sync-smoke-token",
            "--apply"
        ) | Out-Null
    $syncObserved = Invoke-RestMethod -Uri "$loomBaseUrl/sync-observed" -TimeoutSec 5
    Assert-Equal 2 ([int]$syncObserved.listCount) "tea-sync did not refresh Tea tickets after a create conflict."
    Assert-Equal 2 ([int]$syncObserved.createCount) "tea-sync did not send exactly two create requests."
    $syncIdempotencyKeys = @($syncObserved.idempotencyKeys)
    Assert-Equal 2 $syncIdempotencyKeys.Count "tea-sync did not report both deterministic create keys."
    foreach ($syncIdempotencyKey in $syncIdempotencyKeys) {
        if ([string]$syncIdempotencyKey -notmatch '^tea-sync-v1-github-[0-9a-f]{64}$') {
            throw "tea-sync did not send a deterministic header-safe Idempotency-Key: $syncIdempotencyKey"
        }
    }
    Assert-True ($syncIdempotencyKeys[0] -ne $syncIdempotencyKeys[1]) "tea-sync reused one key for distinct external issues."
    Assert-Equal 1 ([int]$syncObserved.editCount) "tea-sync did not update the mirror recovered after create conflict."
    Assert-Equal "sync-recovered-ticket" ([string]$syncObserved.editTicketId) "tea-sync updated the wrong recovered mirror."
    Assert-Equal 1 ([int]$syncObserved.lifecycleCount) "tea-sync did not apply lifecycle to a newly created closed issue."
    Assert-Equal "cancel" ([string]$syncObserved.lifecycleAction) "tea-sync mapped a closed issue to the wrong lifecycle action."
    Assert-Equal "sync-smoke-ticket" ([string]$syncObserved.lifecycleTicketId) "tea-sync did not apply lifecycle to the ticket returned by create."
    $summary["sync_create_idempotency_header"] = $true
    $summary["sync_closed_create_lifecycle_applied"] = $true
    $summary["sync_changed_payload_conflict_reconciled"] = $true

    $env:TEA_BIND_ADDR = "127.0.0.1:$Port"
    $env:TEA_AUTH_TOKEN = $AuthToken
    $env:TEA_STORE_PATH = $storePath
    $env:TEA_CONFIG_PATH = $configPath
    $env:TEA_LOOM_BASE_URL = $loomBaseUrl
    $env:TEA_LOOM_AUTH_TOKEN = ""
    $env:TEA_SERVER_URL = $baseUrl

    Write-Host ">> starting tea-daemon.exe at $baseUrl"
    $daemon = Start-Process -FilePath $teaDaemonExe `
        -WorkingDirectory $repoRoot `
        -RedirectStandardOutput $stdoutPath `
        -RedirectStandardError $stderrPath `
        -WindowStyle Hidden `
        -PassThru

    $summary["daemon_pid"] = $daemon.Id

    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    $healthy = $false
    while ((Get-Date) -lt $deadline) {
        if ($daemon.HasExited) {
            throw "tea-daemon exited early with code $($daemon.ExitCode). stdout=$stdoutPath stderr=$stderrPath"
        }

        try {
            $health = Invoke-RestMethod -Uri "$baseUrl/health" -Method Get -TimeoutSec 2
            if ($health.status -eq "ok") {
                $healthy = $true
                break
            }
        }
        catch {
            Start-Sleep -Milliseconds 300
        }
    }
    Assert-True $healthy "tea-daemon did not become healthy at $baseUrl within $TimeoutSec seconds"

    $statusText = Invoke-TeaRaw -Arguments @("status")
    Assert-True $statusText.Contains("Service: tea") "tea status output did not identify the service."
    Assert-True $statusText.Contains("Store: sqlite") "tea status output did not identify the isolated SQLite store."
    Assert-True $statusText.Contains("SQLite schema: 4 (supported: 4)") "tea status output did not report SQLite schema v4."

    $settingsPage = [string](Invoke-RestMethod -Uri "$baseUrl/settings" -Method Get -TimeoutSec 5)
    Assert-True $settingsPage.Contains("Tea Settings") "Tea standalone settings page did not render Tea Settings."
    Assert-True $settingsPage.Contains("data-configuration-source=""local""") "Tea standalone settings page did not report local configuration ownership."
    Assert-True $settingsPage.Contains("Save Tea local settings") "Tea standalone settings page did not expose local save action."
    Assert-True $settingsPage.Contains("notifications_enabled") "Tea standalone settings page did not expose notifications setting."
    $summary["settings_page_contains_local_ui"] = $true

    # smoke step: config set
    $config = Invoke-TeaJson -Arguments @(
        "config", "set",
        "--notifications-enabled", "false",
        "--human-ticket-default-approval-policy", "human_before_execute",
        "--hook-ticket-default-approval-policy", "plan_only"
    )
    Assert-Equal $false $config.config.notifications_enabled "config set did not update notifications_enabled."
    Assert-Equal "human_before_execute" $config.config.human_ticket_default_approval_policy "config set did not preserve human default policy."

    # smoke step: ticket create
    $ticketCreateArguments = @(
        "ticket", "create",
        "--title", "Tea CLI real smoke",
        "--description", "Exercise daemon and CLI lifecycle against an isolated store.",
        "--idempotency-key", "tea-cli-smoke-create-1"
    )
    $ticket = Invoke-TeaJson -Arguments $ticketCreateArguments
    $ticketReplay = Invoke-TeaJson -Arguments $ticketCreateArguments
    Assert-True (-not [string]::IsNullOrWhiteSpace($ticket.id)) "ticket create did not return ticket id."
    $ticketId = [string]$ticket.id
    Assert-Equal $ticketId ([string]$ticketReplay.id) "tea-cli idempotent replay returned another ticket."
    $summary["ticket_id"] = $ticketId
    $summary["cli_create_idempotency_replayed"] = $true

    # smoke step: cursor-paged ticket list
    $ticketPage = Invoke-TeaJson -Arguments @(
        "ticket", "list",
        "--source", "human",
        "--limit", "1"
    )
    Assert-Equal 1 @($ticketPage.items).Count "paged ticket list did not return one item."
    Assert-Equal $ticketId ([string]$ticketPage.items[0].id) "paged ticket list returned another ticket."
    Assert-True ($null -eq $ticketPage.next_cursor) "single-ticket page unexpectedly returned a continuation cursor."
    $summary["paged_ticket_list"] = $true

    $hookPayloadPath = Join-Path $artifactRoot "hook-idempotency.json"
    [IO.File]::WriteAllText(
        $hookPayloadPath,
        '{"source":"tea-cli-smoke","text":"Tea CLI Hook idempotency smoke","context":{"active_window":null,"selection_text":null,"ocr_text":null,"screenshot_ref":null,"cwd":null,"app":"Tea"},"attachments":[]}',
        [Text.UTF8Encoding]::new($false)
    )
    $hookCreateArguments = @(
        "hook", "intake",
        "--file", $hookPayloadPath,
        "--idempotency-key", "tea-cli-hook-smoke-1"
    )
    $hookCliFirst = Invoke-TeaJson -Arguments $hookCreateArguments
    $hookCliReplay = Invoke-TeaJson -Arguments $hookCreateArguments
    Assert-Equal ([string]$hookCliFirst.id) ([string]$hookCliReplay.id) "tea-cli Hook replay returned another ticket."
    $allTickets = @(Invoke-RestMethod `
        -Uri "$baseUrl/v1/tickets" `
        -Headers @{ Authorization = "Bearer $AuthToken" } `
        -TimeoutSec 5)
    Assert-Equal 1 @($allTickets | Where-Object { $_.title -eq "Tea CLI Hook idempotency smoke" }).Count "tea-cli Hook replay persisted duplicate tickets."
    $summary["cli_hook_idempotency_replayed"] = $true

    # The packaged daemon must provide durable create idempotency below every
    # first-party client. Missing headers preserve legacy behavior; a supplied
    # key replays the original response and cannot be rebound to another payload.
    $idempotencyHeaders = @{
        Authorization = "Bearer $AuthToken"
        "Idempotency-Key" = "tea-cli-real-idempotency"
    }
    $idempotencyBody = @{
        title = "Tea HTTP idempotency smoke"
        description = "Repeated HTTP create requests must persist exactly one Tea ticket."
    } | ConvertTo-Json -Compress
    $idempotentFirst = Invoke-RestMethod `
        -Uri "$baseUrl/v1/tickets" `
        -Method Post `
        -Headers $idempotencyHeaders `
        -ContentType "application/json" `
        -Body $idempotencyBody `
        -TimeoutSec 5
    $idempotentReplay = Invoke-RestMethod `
        -Uri "$baseUrl/v1/tickets" `
        -Method Post `
        -Headers $idempotencyHeaders `
        -ContentType "application/json" `
        -Body $idempotencyBody `
        -TimeoutSec 5
    Assert-Equal ([string]$idempotentFirst.id) ([string]$idempotentReplay.id) "Idempotent replay returned another ticket."
    $allTickets = @(Invoke-RestMethod `
        -Uri "$baseUrl/v1/tickets" `
        -Headers @{ Authorization = "Bearer $AuthToken" } `
        -TimeoutSec 5)
    Assert-Equal 1 @($allTickets | Where-Object { $_.title -eq "Tea HTTP idempotency smoke" }).Count "Idempotent create persisted duplicate tickets."

    $idempotencyConflictStatus = 0
    try {
        Invoke-RestMethod `
            -Uri "$baseUrl/v1/tickets" `
            -Method Post `
            -Headers $idempotencyHeaders `
            -ContentType "application/json" `
            -Body (@{
                title = "Changed Tea HTTP idempotency smoke"
                description = "A reused idempotency key must not identify a different request."
            } | ConvertTo-Json -Compress) `
            -TimeoutSec 5 | Out-Null
        throw "Reusing an idempotency key with another payload unexpectedly succeeded."
    }
    catch {
        if ($null -eq $_.Exception.Response) { throw }
        $idempotencyConflictStatus = [int]$_.Exception.Response.StatusCode
    }
    Assert-Equal 409 $idempotencyConflictStatus "Changed idempotency payload did not return HTTP 409."

    $hookIdempotencyHeaders = @{
        Authorization = "Bearer $AuthToken"
        "Idempotency-Key" = "tea-hook-real-idempotency"
    }
    $hookIdempotencyBody = @{
        source = "tea-cli-real-smoke"
        text = "Tea Hook idempotency smoke"
        context = @{
            active_window = $null
            selection_text = $null
            ocr_text = $null
            screenshot_ref = $null
            cwd = $null
            app = "Tea"
        }
        attachments = @()
    } | ConvertTo-Json -Depth 5 -Compress
    $hookIdempotentFirst = Invoke-RestMethod `
        -Uri "$baseUrl/v1/intake/hook" `
        -Method Post `
        -Headers $hookIdempotencyHeaders `
        -ContentType "application/json" `
        -Body $hookIdempotencyBody `
        -TimeoutSec 5
    $hookIdempotentReplay = Invoke-RestMethod `
        -Uri "$baseUrl/v1/intake/hook" `
        -Method Post `
        -Headers $hookIdempotencyHeaders `
        -ContentType "application/json" `
        -Body $hookIdempotencyBody `
        -TimeoutSec 5
    Assert-Equal ([string]$hookIdempotentFirst.id) ([string]$hookIdempotentReplay.id) "Hook idempotent replay returned another ticket."
    $allTickets = @(Invoke-RestMethod `
        -Uri "$baseUrl/v1/tickets" `
        -Headers @{ Authorization = "Bearer $AuthToken" } `
        -TimeoutSec 5)
    Assert-Equal 1 @($allTickets | Where-Object { $_.title -eq "Tea Hook idempotency smoke" }).Count "Hook idempotent create persisted duplicate tickets."
    $summary["human_create_idempotency_replayed"] = $true
    $summary["hook_create_idempotency_replayed"] = $true
    $summary["idempotency_payload_conflict_rejected"] = $true

    $comment = Invoke-TeaJson -Arguments @("ticket", "comment", $ticketId, "CLI smoke review comment")
    Assert-Equal "CLI smoke review comment" $comment.body "ticket comment did not round-trip the comment body."

    # smoke step: ticket edit
    $edited = Invoke-TeaJson -Arguments @(
        "ticket", "edit", $ticketId,
        "--title", "Tea CLI real smoke (edited)",
        "--priority", "high",
        "--label", "area:cli-smoke"
    )
    Assert-Equal "Tea CLI real smoke (edited)" ([string]$edited.title) "ticket edit did not update the title."
    Assert-Equal "high" ([string]$edited.priority) "ticket edit did not update the priority."
    Assert-True (@($edited.labels) -contains "area:cli-smoke") "ticket edit did not apply the operator label."
    Assert-True (@($edited.labels) -contains "source:human") "ticket edit dropped the system source label."
    Assert-True (@($edited.labels | Where-Object { $_ -like "policy:*" }).Count -ge 1) "ticket edit dropped the system policy label."
    $summary["edited_ticket_title"] = [string]$edited.title
    $summary["edited_ticket_priority"] = [string]$edited.priority

    # smoke step: ticket decompose
    $decomposition = Invoke-TeaJson -Arguments @("ticket", "decompose", $ticketId)
    Assert-Equal "loom" ([string]$decomposition.provider.mode) "external Loom smoke should use the Loom BrainProvider."
    Assert-Equal "tea.ticket.decompose.v1" ([string]$decomposition.provider.capability) "decompose provider capability mismatch."
    Assert-Equal "engineering_work_order" ([string]$decomposition.analysis.intent) "decompose did not return the expected analysis intent."
    Assert-Equal "loom.tea_ticket_decompose.v1" ([string]$decomposition.analysis.recommended_workflow) "decompose did not return the expected workflow."
    Assert-True (@($decomposition.plan.steps).Count -ge 3) "decompose plan did not include at least three steps."
    Assert-Equal $true ([bool]$decomposition.plan.requires_approval_before_execute) "decompose plan should require approval before execute."
    $summary["decomposition_provider_mode"] = [string]$decomposition.provider.mode
    $summary["decomposition_recommended_workflow"] = [string]$decomposition.analysis.recommended_workflow
    $summary["decomposition_step_count"] = @($decomposition.plan.steps).Count

    # smoke step: ticket approve
    $approved = Invoke-TeaJson -Arguments @("ticket", "approve", $ticketId)
    Assert-Equal "approved" $approved.status "ticket approve did not set approved status."

    # smoke step: ticket run
    $run = Invoke-TeaJson -Arguments @("ticket", "run", $ticketId)
    Assert-True (-not [string]::IsNullOrWhiteSpace($run.id)) "ticket run did not return run id."
    Assert-Equal $ticketId ([string]$run.ticket_id) "ticket run returned a run for another ticket."
    Assert-Equal "succeeded" $run.status "stateful Loom smoke run should succeed."
    $runId = [string]$run.id
    $summary["run_id"] = $runId

    # smoke step: ticket accept
    $accepted = Invoke-TeaJson -Arguments @("ticket", "accept", $ticketId)
    Assert-Equal "accepted" $accepted.status "ticket accept did not set accepted status."
    $summary["accepted_ticket_status"] = [string]$accepted.status

    # smoke step: ticket close
    $closed = Invoke-TeaJson -Arguments @("ticket", "close", $ticketId)
    Assert-Equal "closed" $closed.status "ticket close did not set closed status."
    $summary["closed_ticket_status"] = [string]$closed.status

    # smoke step: ticket export
    $jsonExport = Invoke-TeaJson -Arguments @("ticket", "export", $ticketId, "--format", "json")
    Assert-Equal $ticketId ([string]$jsonExport.ticket.id) "JSON export returned another ticket."
    Assert-True (@($jsonExport.events).Count -gt 0) "JSON export did not include ticket events."
    $summary["json_export_contains_events"] = $true

    $markdown = Invoke-TeaRaw -Arguments @("ticket", "export", $ticketId, "--format", "markdown")
    Assert-True $markdown.Contains("smoke loom run completed") "Markdown export did not include Loom evidence summary."
    Assert-True $markdown.Contains("CLI smoke review comment") "Markdown export did not include review comment."
    $summary["markdown_contains_evidence"] = $true

    $events = Invoke-TeaJson -Arguments @("ticket", "events", $ticketId)
    Assert-True (@($events).Count -ge 7) "ticket events did not include the expected lifecycle events."

    # An approval is valid only for the policy under which it was granted.
    $policyTicket = Invoke-TeaJson -Arguments @(
        "ticket", "create",
        "--title", "Tea CLI approval policy binding smoke",
        "--description", "Changing approval policy must invalidate an earlier decision."
    )
    $policyTicketId = [string]$policyTicket.id
    $summary["policy_change_ticket_id"] = $policyTicketId
    Invoke-TeaJson -Arguments @("ticket", "approve", $policyTicketId) | Out-Null
    $policyRun = Invoke-TeaJson -Arguments @("ticket", "run", $policyTicketId)
    Assert-Equal "succeeded" ([string]$policyRun.status) "policy-binding smoke run must produce evidence."
    Invoke-TeaJson -Arguments @(
        "ticket", "policy", $policyTicketId,
        "--mode", "human_before_completion"
    ) | Out-Null
    Invoke-TeaExpectedFailure `
        -Arguments @("ticket", "close", $policyTicketId) `
        -ExpectedText "human approval required before completion" | Out-Null
    $summary["policy_change_invalidated_approval"] = $true
    Invoke-TeaJson -Arguments @("ticket", "approve", $policyTicketId) | Out-Null
    $policyClosed = Invoke-TeaJson -Arguments @("ticket", "close", $policyTicketId)
    Assert-Equal "closed" ([string]$policyClosed.status) "fresh approval did not close policy-binding ticket."
    $summary["policy_change_close_status"] = [string]$policyClosed.status

    # Run control uses a separate ticket. A retried asynchronous run is not a
    # completed ticket and must never inherit the main ticket's acceptance path.
    $controlTicket = Invoke-TeaJson -Arguments @(
        "ticket", "create",
        "--title", "Tea CLI run control smoke",
        "--description", "Exercise stop and retry without accepting an in-progress run."
    )
    $controlTicketId = [string]$controlTicket.id
    $summary["control_ticket_id"] = $controlTicketId
    Invoke-TeaJson -Arguments @("ticket", "approve", $controlTicketId) | Out-Null
    $controlRun = Invoke-TeaJson -Arguments @("ticket", "run", $controlTicketId)
    $controlRunId = [string]$controlRun.id
    $summary["control_run_id"] = $controlRunId
    Assert-Equal "running" $controlRun.status "run-control ticket must begin with a genuinely running Loom run."

    $stopped = Invoke-TeaJson -Arguments @("run", "stop", $controlRunId)
    Assert-Equal $controlRunId ([string]$stopped.id) "run stop returned another run id."
    Assert-Equal "stopped" $stopped.status "run stop did not return stopped status."
    $summary["stopped_run_status"] = [string]$stopped.status

    $retried = Invoke-TeaJson -Arguments @("run", "retry", $controlRunId)
    Assert-Equal $controlRunId ([string]$retried.id) "run retry returned another run id."
    Assert-Equal "retrying" $retried.status "run retry did not return retrying status."
    $summary["retried_run_status"] = [string]$retried.status

    $controlEvents = Invoke-TeaJson -Arguments @("ticket", "events", $controlTicketId)
    Assert-True (@($controlEvents | Where-Object { $_.kind -eq "run_stopped" }).Count -eq 1) "run stop did not append exactly one run_stopped audit event."
    Assert-True (@($controlEvents | Where-Object { $_.kind -eq "run_retrying" }).Count -eq 1) "run retry did not append exactly one run_retrying audit event."
    $summary["control_events_include_stopped"] = $true
    $summary["control_events_include_retrying"] = $true

    # smoke step: ticket cancel
    $cancelTicket = Invoke-TeaJson -Arguments @(
        "ticket", "create",
        "--title", "Tea CLI cancel smoke",
        "--description", "Exercise cancelled terminal state through the CLI."
    )
    Assert-True (-not [string]::IsNullOrWhiteSpace($cancelTicket.id)) "cancel smoke ticket create did not return ticket id."
    $cancelTicketId = [string]$cancelTicket.id
    $summary["cancelled_ticket_id"] = $cancelTicketId

    $cancelled = Invoke-TeaJson -Arguments @("ticket", "cancel", $cancelTicketId)
    Assert-Equal "cancelled" $cancelled.status "ticket cancel did not set cancelled status."
    $summary["cancelled_ticket_status"] = [string]$cancelled.status

    $cancelEvents = Invoke-TeaJson -Arguments @("ticket", "events", $cancelTicketId)
    Assert-True (@($cancelEvents | Where-Object { $_.kind -eq "ticket_cancelled" }).Count -gt 0) "ticket cancel did not append ticket_cancelled event."
    $summary["cancel_events_include_cancelled"] = $true

    $summary["status"] = "passed"
    $summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding UTF8

    Write-Host "Tea CLI real smoke passed"
    Write-Host "ticket_id=$ticketId"
    Write-Host "run_id=$runId"
    Write-Host "cancelled_ticket_id=$cancelTicketId"
    Write-Host "human_create_idempotency_replayed=true"
    Write-Host "hook_create_idempotency_replayed=true"
    Write-Host "idempotency_payload_conflict_rejected=true"
    Write-Host "cli_create_idempotency_replayed=true"
    Write-Host "cli_hook_idempotency_replayed=true"
    Write-Host "sync_create_idempotency_header=true"
    Write-Host "sync_closed_create_lifecycle_applied=true"
    Write-Host "sync_changed_payload_conflict_reconciled=true"
    Write-Host "summary=$summaryPath"
}
catch {
    $summary["status"] = "failed"
    $summary["error"] = $_.Exception.Message
    $summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding UTF8
    throw
}
finally {
    if ($daemon -ne $null -and !$daemon.HasExited) {
        Write-Host ">> stopping tea-daemon pid=$($daemon.Id)"
        Stop-Process -Id $daemon.Id -Force -ErrorAction SilentlyContinue
        $daemon.WaitForExit(5000) | Out-Null
    }
    if ($daemon -ne $null) {
        $daemon.Refresh()
    }

    if ($loomStub -ne $null -and !$loomStub.HasExited) {
        Write-Host ">> stopping stateful Loom smoke stub pid=$($loomStub.Id)"
        Stop-Process -Id $loomStub.Id -Force -ErrorAction SilentlyContinue
        $loomStub.WaitForExit(5000) | Out-Null
    }
    if ($loomStub -ne $null) {
        $loomStub.Refresh()
    }

    $listenersAfterStop = @(Get-PortListeners -Port $Port)
    $loomListenersAfterStop = @(Get-PortListeners -Port $loomPort)

    foreach ($entry in $oldEnv.GetEnumerator()) {
        Restore-EnvValue -Name $entry.Key -Value $entry.Value
    }

    if (!$KeepArtifacts) {
        Remove-Item -LiteralPath $storePath -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath "$storePath-shm" -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath "$storePath-wal" -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $configPath -Force -ErrorAction SilentlyContinue
    }

    $daemonStopped = [bool]($daemon -eq $null -or $daemon.HasExited)
    $loomStubStopped = [bool]($loomStub -eq $null -or $loomStub.HasExited)
    $summary["cleanup_checked_at"] = (Get-Date).ToString("o")
    $summary["daemon_stopped"] = $daemonStopped
    $summary["loom_stub_stopped"] = $loomStubStopped
    $summary["port_listener_count_after_stop"] = $listenersAfterStop.Count
    $summary["loom_port_listener_count_after_stop"] = $loomListenersAfterStop.Count
    $summary["listeners_after_stop"] = $listenersAfterStop
    $summary["store_preserved"] = [bool](Test-Path -LiteralPath $storePath)
    $summary["finished_at"] = (Get-Date).ToString("o")
    $summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding UTF8

    if ($summary["status"] -eq "passed" -and (!$daemonStopped -or !$loomStubStopped -or $listenersAfterStop.Count -ne 0 -or $loomListenersAfterStop.Count -ne 0)) {
        throw "Tea CLI smoke cleanup failed: daemon_stopped=$daemonStopped loom_stub_stopped=$loomStubStopped port_listener_count_after_stop=$($listenersAfterStop.Count) loom_port_listener_count_after_stop=$($loomListenersAfterStop.Count)"
    }
}
