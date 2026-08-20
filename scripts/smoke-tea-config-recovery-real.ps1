[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PackageDir,
    [int]$TimeoutSec = 30,
    [switch]$KeepArtifacts
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-FreeTcpPort {
    $listener = [System.Net.Sockets.TcpListener]::new(
        [System.Net.IPAddress]::Loopback,
        0
    )
    $listener.Start()
    try { return $listener.LocalEndpoint.Port }
    finally { $listener.Stop() }
}

function Get-PortListeners {
    param([int]$Port)

    return @(netstat -ano -p TCP 2>$null | Where-Object {
        $parts = @($_ -split "\s+" | Where-Object { $_ -ne "" })
        if ($parts.Count -lt 5 -or $parts[0] -ne "TCP" -or $parts[$parts.Count - 2] -ne "LISTENING") {
            return $false
        }
        $localEndpoint = $parts[1]
        $lastColon = $localEndpoint.LastIndexOf(":")
        if ($lastColon -lt 0) { return $false }
        $localPort = 0
        return [int]::TryParse($localEndpoint.Substring($lastColon + 1), [ref]$localPort) -and $localPort -eq $Port
    })
}

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$packagePath = (Resolve-Path -LiteralPath $PackageDir).Path
$daemonExe = Join-Path $packagePath "tea-daemon.exe"
if (-not (Test-Path -LiteralPath $daemonExe -PathType Leaf)) {
    throw "Missing packaged Tea daemon: $daemonExe"
}

$artifactParent = [System.IO.Path]::GetFullPath(
    (Join-Path $repoRoot ".tmp\tea-config-recovery")
)
$artifactRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $artifactParent ([System.Guid]::NewGuid().ToString("N")))
)
if (-not $artifactRoot.StartsWith(
    $artifactParent + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase
)) {
    throw "Config recovery artifact root escaped Tea .tmp: $artifactRoot"
}
[void][System.IO.Directory]::CreateDirectory($artifactRoot)

$configPath = Join-Path $artifactRoot "config.json"
$backupPath = "$configPath.bak"
$storePath = Join-Path $artifactRoot "store.sqlite"
$port = Get-FreeTcpPort
$baseUrl = "http://127.0.0.1:$port"
$canonicalToken = "round18-config-recovery-token"
$daemon = $null
$smokePassed = $false

try {
    [System.IO.File]::WriteAllText(
        $configPath,
        '{"schema_version":1,',
        [System.Text.UTF8Encoding]::new($false)
    )
    [System.IO.File]::WriteAllText(
        $backupPath,
        '{"schema_version":1,"notifications_enabled":false,"human_ticket_default_approval_policy":"manual_only","hook_ticket_default_approval_policy":"plan_only"}',
        [System.Text.UTF8Encoding]::new($false)
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $daemonExe
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.EnvironmentVariables["TEA_BIND_ADDR"] = "127.0.0.1:$port"
    $startInfo.EnvironmentVariables["TEA_AUTH_TOKEN"] = "  $canonicalToken  "
    $startInfo.EnvironmentVariables["TEA_CONFIG_PATH"] = $configPath
    $startInfo.EnvironmentVariables["TEA_STORE_PATH"] = $storePath
    $daemon = [System.Diagnostics.Process]::Start($startInfo)

    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    $configuration = $null
    do {
        $daemon.Refresh()
        if ($daemon.HasExited) {
            throw "Packaged Tea daemon exited during config recovery with code $($daemon.ExitCode): $($daemon.StandardError.ReadToEnd())"
        }
        try {
            $configuration = Invoke-RestMethod `
                -Uri "$baseUrl/v1/configuration" `
                -Headers @{ Authorization = "Bearer $canonicalToken" } `
                -TimeoutSec 2
        } catch {
            Start-Sleep -Milliseconds 100
        }
    } while ($null -eq $configuration -and (Get-Date) -lt $deadline)
    if ($null -eq $configuration) {
        throw "Timed out waiting for packaged Tea config recovery at $baseUrl"
    }

    if ([bool]$configuration.config.notifications_enabled) {
        throw "Recovered Tea config did not preserve notifications_enabled=false"
    }
    if ([string]$configuration.config.human_ticket_default_approval_policy -ne "manual_only") {
        throw "Recovered Tea config did not preserve the backup approval policy"
    }
    if (Test-Path -LiteralPath $backupPath) {
        throw "Tea did not promote and remove the valid config backup"
    }
    $diskConfig = Get-Content -LiteralPath $configPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([int]$diskConfig.schema_version -ne 1 -or
        [string]$diskConfig.human_ticket_default_approval_policy -ne "manual_only") {
        throw "Recovered Tea config on disk does not match the valid backup"
    }

    Remove-Item -LiteralPath $configPath -Force
    $configuration = Invoke-RestMethod `
        -Uri "$baseUrl/v1/configuration" `
        -Method Patch `
        -Headers @{ Authorization = "Bearer $canonicalToken" } `
        -ContentType "application/json" `
        -Body '{"hook_ticket_default_approval_policy":"human_before_execute"}' `
        -TimeoutSec 5
    if ([bool]$configuration.config.notifications_enabled) {
        throw "Missing-file PATCH reset notifications_enabled instead of preserving the runtime snapshot"
    }
    if ([string]$configuration.config.human_ticket_default_approval_policy -ne "manual_only") {
        throw "Missing-file PATCH reset the human approval policy instead of preserving the runtime snapshot"
    }
    if ([string]$configuration.config.hook_ticket_default_approval_policy -ne "human_before_execute") {
        throw "Missing-file PATCH did not apply the requested hook approval policy"
    }
    if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) {
        throw "Missing-file PATCH did not recreate the Tea config file"
    }
    $rebuiltDiskConfig = Get-Content -LiteralPath $configPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([bool]$rebuiltDiskConfig.notifications_enabled -or
        [string]$rebuiltDiskConfig.human_ticket_default_approval_policy -ne "manual_only" -or
        [string]$rebuiltDiskConfig.hook_ticket_default_approval_policy -ne "human_before_execute") {
        throw "Recreated Tea config does not match the preserved runtime snapshot plus PATCH"
    }

    $idempotencyHeaders = @{
        Authorization = "Bearer $canonicalToken"
        "Idempotency-Key" = "config-recovery-restart-idempotency"
    }
    $idempotencyBody = '{"title":"Tea restart idempotency smoke","description":"The original create response must survive a packaged daemon restart."}'
    $idempotentFirst = Invoke-RestMethod `
        -Uri "$baseUrl/v1/tickets" `
        -Method Post `
        -Headers $idempotencyHeaders `
        -ContentType "application/json" `
        -Body $idempotencyBody `
        -TimeoutSec 5

    $daemon.Refresh()
    if (-not $daemon.HasExited) {
        $daemon.Kill()
        [void]$daemon.WaitForExit(5000)
    }
    $daemon.Dispose()
    $daemon = $null
    $listenerDeadline = (Get-Date).AddSeconds(5)
    do {
        $listeners = @(Get-PortListeners -Port $port)
        if ($listeners.Count -eq 0) { break }
        Start-Sleep -Milliseconds 100
    } while ((Get-Date) -lt $listenerDeadline)
    if ($listeners.Count -ne 0) {
        throw "Tea daemon port remained in use before idempotency restart: $port"
    }

    $daemon = [System.Diagnostics.Process]::Start($startInfo)
    $restartDeadline = (Get-Date).AddSeconds($TimeoutSec)
    $restartReady = $false
    do {
        $daemon.Refresh()
        if ($daemon.HasExited) {
            throw "Packaged Tea daemon exited during idempotency restart with code $($daemon.ExitCode): $($daemon.StandardError.ReadToEnd())"
        }
        try {
            Invoke-RestMethod `
                -Uri "$baseUrl/v1/status" `
                -Headers @{ Authorization = "Bearer $canonicalToken" } `
                -TimeoutSec 2 | Out-Null
            $restartReady = $true
        } catch {
            Start-Sleep -Milliseconds 100
        }
    } while (-not $restartReady -and (Get-Date) -lt $restartDeadline)
    if (-not $restartReady) {
        throw "Timed out waiting for packaged Tea daemon idempotency restart at $baseUrl"
    }

    $idempotentReplay = Invoke-RestMethod `
        -Uri "$baseUrl/v1/tickets" `
        -Method Post `
        -Headers $idempotencyHeaders `
        -ContentType "application/json" `
        -Body $idempotencyBody `
        -TimeoutSec 5
    if ([string]$idempotentReplay.id -ne [string]$idempotentFirst.id) {
        throw "Idempotency replay after restart returned another ticket"
    }
    $tickets = @(Invoke-RestMethod `
        -Uri "$baseUrl/v1/tickets" `
        -Headers @{ Authorization = "Bearer $canonicalToken" } `
        -TimeoutSec 5)
    $idempotentTicketCount = @($tickets | Where-Object { $_.title -eq "Tea restart idempotency smoke" }).Count
    if ($idempotentTicketCount -ne 1) {
        throw "Idempotency replay after restart persisted $idempotentTicketCount tickets"
    }
    $status = Invoke-RestMethod `
        -Uri "$baseUrl/v1/status" `
        -Headers @{ Authorization = "Bearer $canonicalToken" } `
        -TimeoutSec 5
    if ([int]$status.store.schema_version -ne 4 -or [int]$status.store.supported_schema_version -ne 4) {
        throw "Packaged Tea daemon did not report SQLite schema v4 after restart"
    }

    $smokePassed = $true
    [ordered]@{
        status = "passed"
        packageDir = $packagePath
        daemonExe = $daemonExe
        daemonPid = $daemon.Id
        port = $port
        corruptPrimaryRecovered = $true
        backupPromoted = $true
        trimmedBearerTokenAuthenticated = $true
        missingFilePatchPreservedSnapshot = $true
        missingFilePatchRecreatedConfig = $true
        idempotencyReplaySurvivedRestart = $true
        idempotencyRestartTicketCount = $idempotentTicketCount
        sqliteSchemaVersion = [int]$status.store.schema_version
        recoveredPolicy = [string]$configuration.config.human_ticket_default_approval_policy
        artifactRoot = $artifactRoot
    } | ConvertTo-Json -Depth 4
} finally {
    $daemonStopped = $true
    if ($null -ne $daemon) {
        $daemon.Refresh()
        if (-not $daemon.HasExited) {
            $daemon.Kill()
            [void]$daemon.WaitForExit(5000)
        }
        $daemon.Refresh()
        $daemonStopped = [bool]$daemon.HasExited
        $daemon.Dispose()
    }
    $deadline = (Get-Date).AddSeconds(5)
    do {
        $listeners = @(Get-PortListeners -Port $port)
        if ($listeners.Count -eq 0) { break }
        Start-Sleep -Milliseconds 100
    } while ((Get-Date) -lt $deadline)
    $cleanupPassed = $daemonStopped -and $listeners.Count -eq 0
    if (-not $KeepArtifacts -and $cleanupPassed -and (Test-Path -LiteralPath $artifactRoot)) {
        Remove-Item -LiteralPath $artifactRoot -Recurse -Force
    }
    if ($smokePassed -and -not $cleanupPassed) {
        throw "Config recovery smoke cleanup failed: daemon_stopped=$daemonStopped listeners=$($listeners.Count) artifacts=$artifactRoot"
    }
}
