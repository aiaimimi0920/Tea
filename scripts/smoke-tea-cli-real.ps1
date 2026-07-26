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

        $pid = 0
        [void][int]::TryParse($parts[$parts.Count - 1], [ref]$pid)
        $listeners += [pscustomobject]@{
            local_endpoint = $localEndpoint
            local_address = $localEndpoint.Substring(0, $lastColon)
            local_port = $localPort
            pid = $pid
        }
    }

    return $listeners
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
$packageManifestPath = if ($usingPackage) { Join-Path $resolvedPackageDir "manifest.json" } else { $null }

New-Item -ItemType Directory -Force -Path $artifactRoot | Out-Null

if ($Port -eq 0) {
    $Port = Get-FreeTcpPort
}

$baseUrl = "http://127.0.0.1:$Port"
$daemon = $null
$summary = [ordered]@{
    status = "running"
    base_url = $baseUrl
    ticket_id = $null
    run_id = $null
    cancelled_ticket_id = $null
    cancelled_ticket_status = $null
    decomposition_provider_mode = $null
    decomposition_recommended_workflow = $null
    decomposition_step_count = $null
    stopped_run_status = $null
    retried_run_status = $null
    accepted_ticket_status = $null
    closed_ticket_status = $null
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
    daemon_pid = $null
    build_profile = $buildProfile
    package_dir = $resolvedPackageDir
    package_manifest_path = $packageManifestPath
    package_git_dirty = $null
    allow_dirty_manifest = [bool]$AllowDirtyManifest
    keep_artifacts = [bool]$KeepArtifacts
    cleanup_checked_at = $null
    daemon_stopped = $false
    port_listener_count_after_stop = $null
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
            "-p", "tea-cli"
        )

        Invoke-Checked -FilePath "cargo" -WorkingDirectory $repoRoot -Arguments $buildArguments | Out-Null
    }

    Assert-True (Test-Path -LiteralPath $teaDaemonExe) "tea-daemon.exe was not built at $teaDaemonExe"
    Assert-True (Test-Path -LiteralPath $script:teaExe) "tea-cli.exe was not built at $script:teaExe"

    # In package mode, also verify the copied Tea UI program is present. The
    # release package ships tea.exe (the GUI) alongside tea-cli.exe and
    # tea-daemon.exe, so a package that dropped the UI binary must fail here.
    if ($usingPackage) {
        $teaUiExe = Join-Path $targetDir "tea.exe"
        Assert-True (Test-Path -LiteralPath $teaUiExe) "tea.exe UI program was not found in package at $teaUiExe"
        $summary["package_ui_exe_present"] = $true
    }

    $env:TEA_BIND_ADDR = "127.0.0.1:$Port"
    $env:TEA_AUTH_TOKEN = $AuthToken
    $env:TEA_STORE_PATH = $storePath
    $env:TEA_CONFIG_PATH = $configPath
    $env:TEA_LOOM_BASE_URL = ""
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
    $ticket = Invoke-TeaJson -Arguments @(
        "ticket", "create",
        "--title", "Tea CLI real smoke",
        "--description", "Exercise daemon and CLI lifecycle against an isolated store."
    )
    Assert-True (-not [string]::IsNullOrWhiteSpace($ticket.id)) "ticket create did not return ticket id."
    $ticketId = [string]$ticket.id
    $summary["ticket_id"] = $ticketId

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
    Assert-Equal "template" ([string]$decomposition.provider.mode) "standalone decompose should use the template BrainProvider."
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
    Assert-Equal "succeeded" $run.status "mock Loom run should succeed."
    $runId = [string]$run.id
    $summary["run_id"] = $runId

    # smoke step: run stop
    $stopped = Invoke-TeaJson -Arguments @("run", "stop", $runId)
    Assert-Equal $runId ([string]$stopped.id) "run stop returned another run id."
    Assert-Equal "stopped" $stopped.status "run stop did not return stopped status."
    $summary["stopped_run_status"] = [string]$stopped.status

    # smoke step: run retry
    $retried = Invoke-TeaJson -Arguments @("run", "retry", $runId)
    Assert-Equal $runId ([string]$retried.id) "run retry returned another run id."
    Assert-Equal "retrying" $retried.status "run retry did not return retrying status."
    $summary["retried_run_status"] = [string]$retried.status

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
    Assert-True $markdown.Contains("mock loom run completed") "Markdown export did not include Loom evidence summary."
    Assert-True $markdown.Contains("CLI smoke review comment") "Markdown export did not include review comment."
    $summary["markdown_contains_evidence"] = $true

    $events = Invoke-TeaJson -Arguments @("ticket", "events", $ticketId)
    Assert-True (@($events).Count -ge 7) "ticket events did not include the expected lifecycle events."

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

    $listenersAfterStop = @(Get-PortListeners -Port $Port)

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
    $summary["cleanup_checked_at"] = (Get-Date).ToString("o")
    $summary["daemon_stopped"] = $daemonStopped
    $summary["port_listener_count_after_stop"] = $listenersAfterStop.Count
    $summary["listeners_after_stop"] = $listenersAfterStop
    $summary["store_preserved"] = [bool](Test-Path -LiteralPath $storePath)
    $summary["finished_at"] = (Get-Date).ToString("o")
    $summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding UTF8

    if ($summary["status"] -eq "passed" -and (!$daemonStopped -or $listenersAfterStop.Count -ne 0)) {
        throw "tea-daemon cleanup failed: daemon_stopped=$daemonStopped port_listener_count_after_stop=$($listenersAfterStop.Count)"
    }
}
