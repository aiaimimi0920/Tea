[CmdletBinding()]
param(
    [string]$PackageDir = "",
    [ValidateRange(1, 100)]
    [int]$Iterations = 16,
    [switch]$KeepArtifacts
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-FreeTcpPort {
    $listener = [System.Net.Sockets.TcpListener]::new(
        [System.Net.IPAddress]::Parse("127.0.0.1"),
        0
    )
    $listener.Start()
    try { return $listener.LocalEndpoint.Port }
    finally { $listener.Stop() }
}

function Get-PortListeners {
    param([int]$Port)

    $listeners = @()
    foreach ($line in @(netstat -ano -p TCP 2>$null)) {
        $parts = @($line -split "\s+" | Where-Object { $_ -ne "" })
        if ($parts.Count -lt 5 -or $parts[0] -ne "TCP") { continue }
        if ($parts[$parts.Count - 2] -ne "LISTENING") { continue }
        $localEndpoint = $parts[1]
        $lastColon = $localEndpoint.LastIndexOf(":")
        if ($lastColon -lt 0) { continue }
        $localPort = 0
        if (![int]::TryParse($localEndpoint.Substring($lastColon + 1), [ref]$localPort)) { continue }
        if ($localPort -ne $Port) { continue }
        $listeners += $line
    }
    return $listeners
}

function Start-TeaDaemon {
    param(
        [string]$DaemonExe,
        [int]$Port,
        [string]$AuthToken,
        [string]$ConfigPath,
        [string]$StorePath
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $DaemonExe
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.EnvironmentVariables["TEA_BIND_ADDR"] = "127.0.0.1:$Port"
    $startInfo.EnvironmentVariables["TEA_AUTH_TOKEN"] = $AuthToken
    $startInfo.EnvironmentVariables["TEA_CONFIG_PATH"] = $ConfigPath
    $startInfo.EnvironmentVariables["TEA_STORE_PATH"] = $StorePath

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        throw "Unable to start Tea daemon on port $Port"
    }
    return $process
}

function Wait-TeaHealth {
    param(
        [int]$Port,
        [System.Diagnostics.Process]$Process,
        [int]$TimeoutSec = 30
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    do {
        $Process.Refresh()
        if ($Process.HasExited) {
            $stderr = $Process.StandardError.ReadToEnd()
            throw "Tea daemon on port $Port exited early with code $($Process.ExitCode): $stderr"
        }
        try {
            $response = Invoke-WebRequest `
                -Uri "http://127.0.0.1:$Port/health" `
                -UseBasicParsing `
                -TimeoutSec 2
            if ($response.StatusCode -eq 200) { return }
        } catch {
            Start-Sleep -Milliseconds 100
        }
    } while ((Get-Date) -lt $deadline)
    throw "Timed out waiting for Tea daemon health on port $Port"
}

function Stop-TeaDaemon {
    param([System.Diagnostics.Process]$Process)

    if ($null -eq $Process) { return $true }
    try {
        $Process.Refresh()
        if (-not $Process.HasExited) {
            $Process.Kill()
            [void]$Process.WaitForExit(5000)
        }
        $Process.Refresh()
        return [bool]$Process.HasExited
    } finally {
        $Process.Dispose()
    }
}

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
if ([string]::IsNullOrWhiteSpace($PackageDir)) {
    & cargo build --manifest-path (Join-Path $repoRoot "Cargo.toml") --locked -p tea-daemon
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to build tea-daemon for config concurrency smoke"
    }
    $daemonExe = Join-Path $repoRoot "target\debug\tea-daemon.exe"
} else {
    $packagePath = (Resolve-Path -LiteralPath $PackageDir).Path
    $daemonExe = Join-Path $packagePath "tea-daemon.exe"
}
if (-not (Test-Path -LiteralPath $daemonExe -PathType Leaf)) {
    throw "Missing Tea daemon executable: $daemonExe"
}

$artifactParent = [System.IO.Path]::GetFullPath(
    (Join-Path $repoRoot ".tmp\tea-config-concurrency")
)
$artifactRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $artifactParent ([System.Guid]::NewGuid().ToString("N")))
)
if (-not $artifactRoot.StartsWith(
    $artifactParent + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase
)) {
    throw "Config concurrency artifact root escaped the Tea .tmp boundary: $artifactRoot"
}
[void][System.IO.Directory]::CreateDirectory($artifactRoot)

$configPath = Join-Path $artifactRoot "config.json"
$portA = Get-FreeTcpPort
$portB = Get-FreeTcpPort
while ($portB -eq $portA) { $portB = Get-FreeTcpPort }
$authToken = "tea-config-concurrency-smoke-token"
$daemonA = $null
$daemonB = $null
$client = $null
$smokePassed = $false

try {
    $daemonA = Start-TeaDaemon `
        -DaemonExe $daemonExe `
        -Port $portA `
        -AuthToken $authToken `
        -ConfigPath $configPath `
        -StorePath (Join-Path $artifactRoot "store-a.sqlite")
    $daemonB = Start-TeaDaemon `
        -DaemonExe $daemonExe `
        -Port $portB `
        -AuthToken $authToken `
        -ConfigPath $configPath `
        -StorePath (Join-Path $artifactRoot "store-b.sqlite")
    Wait-TeaHealth -Port $portA -Process $daemonA
    Wait-TeaHealth -Port $portB -Process $daemonB

    Add-Type -AssemblyName System.Net.Http
    $client = [System.Net.Http.HttpClient]::new()
    $client.Timeout = [System.TimeSpan]::FromSeconds(15)
    $client.DefaultRequestHeaders.Authorization = `
        [System.Net.Http.Headers.AuthenticationHeaderValue]::new("Bearer", $authToken)
    $urlA = "http://127.0.0.1:$portA/v1/configuration"
    $urlB = "http://127.0.0.1:$portB/v1/configuration"

    for ($iteration = 0; $iteration -lt $Iterations; $iteration++) {
        $payloadA = [ordered]@{
            notifications_enabled = ($iteration % 2 -eq 0)
        } | ConvertTo-Json -Compress
        $payloadB = [ordered]@{
            human_ticket_default_approval_policy = if ($iteration % 2 -eq 0) {
                "manual_only"
            } else {
                "human_before_completion"
            }
            hook_ticket_default_approval_policy = if ($iteration % 2 -eq 0) {
                "manual_only"
            } else {
                "plan_only"
            }
        } | ConvertTo-Json -Compress
        $requestA = [System.Net.Http.HttpRequestMessage]::new(
            [System.Net.Http.HttpMethod]::new("PATCH"),
            $urlA
        )
        $requestB = [System.Net.Http.HttpRequestMessage]::new(
            [System.Net.Http.HttpMethod]::new("PATCH"),
            $urlB
        )
        $requestA.Content = [System.Net.Http.StringContent]::new(
            $payloadA,
            [System.Text.Encoding]::UTF8,
            "application/json"
        )
        $requestB.Content = [System.Net.Http.StringContent]::new(
            $payloadB,
            [System.Text.Encoding]::UTF8,
            "application/json"
        )
        try {
            $taskA = $client.SendAsync($requestA)
            $taskB = $client.SendAsync($requestB)
            $responseA = $taskA.GetAwaiter().GetResult()
            $responseB = $taskB.GetAwaiter().GetResult()
            try {
                if (-not $responseA.IsSuccessStatusCode -or -not $responseB.IsSuccessStatusCode) {
                    throw "Concurrent config patch failed: A=$($responseA.StatusCode), B=$($responseB.StatusCode)"
                }
            } finally {
                $responseA.Dispose()
                $responseB.Dispose()
            }
        } finally {
            $requestA.Dispose()
            $requestB.Dispose()
        }
    }

    $lastIteration = $Iterations - 1
    $expectedNotifications = ($lastIteration % 2 -eq 0)
    $expectedHumanPolicy = if ($lastIteration % 2 -eq 0) {
        "manual_only"
    } else {
        "human_before_completion"
    }
    $expectedHookPolicy = if ($lastIteration % 2 -eq 0) {
        "manual_only"
    } else {
        "plan_only"
    }
    $document = Get-Content -LiteralPath $configPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([int]$document.schema_version -ne 1) {
        throw "Concurrent config smoke wrote an unexpected schema version"
    }
    if ([bool]$document.notifications_enabled -ne $expectedNotifications) {
        throw "Concurrent config smoke lost the notification field update"
    }
    if ([string]$document.human_ticket_default_approval_policy -ne $expectedHumanPolicy) {
        throw "Concurrent config smoke lost the human approval policy update"
    }
    if ([string]$document.hook_ticket_default_approval_policy -ne $expectedHookPolicy) {
        throw "Concurrent config smoke lost the Hook approval policy update"
    }

    foreach ($url in @($urlA, $urlB)) {
        $response = $client.GetAsync($url).GetAwaiter().GetResult()
        try {
            if (-not $response.IsSuccessStatusCode) {
                throw "Config convergence read failed for $url with $($response.StatusCode)"
            }
            $runtimeConfig = (
                $response.Content.ReadAsStringAsync().GetAwaiter().GetResult() |
                    ConvertFrom-Json
            ).config
            if ([bool]$runtimeConfig.notifications_enabled -ne $expectedNotifications -or
                [string]$runtimeConfig.human_ticket_default_approval_policy -ne $expectedHumanPolicy -or
                [string]$runtimeConfig.hook_ticket_default_approval_policy -ne $expectedHookPolicy) {
                throw "Tea daemon did not refresh the shared config after a peer update: $url"
            }
        } finally {
            $response.Dispose()
        }
    }
    if (Test-Path -LiteralPath "$configPath.bak") {
        throw "Concurrent config smoke left a backup file after successful writes"
    }
    $temporaryFiles = @(Get-ChildItem -LiteralPath $artifactRoot -File -Filter "config.json.*.tmp")
    if ($temporaryFiles.Count -ne 0) {
        throw "Concurrent config smoke left temporary config files: $($temporaryFiles.FullName -join ', ')"
    }
    if (-not (Test-Path -LiteralPath "$configPath.lock" -PathType Leaf)) {
        throw "Concurrent config smoke did not retain the stable config lock file"
    }

    $smokePassed = $true
    [ordered]@{
        status = "passed"
        daemonExe = $daemonExe
        daemonPids = @($daemonA.Id, $daemonB.Id)
        ports = @($portA, $portB)
        iterationsPerDaemon = $Iterations
        successfulPatches = $Iterations * 2
        bothRuntimeSnapshotsConverged = $true
        configPath = $configPath
        finalSchemaVersion = [int]$document.schema_version
        backupPresent = $false
        temporaryFileCount = 0
        lockFilePresent = $true
        artifactRoot = $artifactRoot
    } | ConvertTo-Json -Depth 6
} finally {
    if ($null -ne $client) { $client.Dispose() }
    $daemonAStopped = Stop-TeaDaemon -Process $daemonA
    $daemonBStopped = Stop-TeaDaemon -Process $daemonB
    $cleanupDeadline = (Get-Date).AddSeconds(5)
    do {
        $listenersA = @(Get-PortListeners -Port $portA)
        $listenersB = @(Get-PortListeners -Port $portB)
        if ($listenersA.Count -eq 0 -and $listenersB.Count -eq 0) { break }
        Start-Sleep -Milliseconds 100
    } while ((Get-Date) -lt $cleanupDeadline)
    $cleanupPassed = $daemonAStopped -and $daemonBStopped -and `
        $listenersA.Count -eq 0 -and $listenersB.Count -eq 0
    if (-not $KeepArtifacts -and $cleanupPassed -and (Test-Path -LiteralPath $artifactRoot)) {
        $resolvedArtifactRoot = [System.IO.Path]::GetFullPath($artifactRoot)
        if (-not $resolvedArtifactRoot.StartsWith(
            $artifactParent + [System.IO.Path]::DirectorySeparatorChar,
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
            throw "Refusing to clean config smoke artifacts outside Tea .tmp: $resolvedArtifactRoot"
        }
        Remove-Item -LiteralPath $resolvedArtifactRoot -Recurse -Force
    }
    if ($smokePassed -and -not $cleanupPassed) {
        throw "Config concurrency smoke cleanup failed: daemon_a_stopped=$daemonAStopped daemon_b_stopped=$daemonBStopped listeners_a=$($listenersA.Count) listeners_b=$($listenersB.Count) artifacts=$artifactRoot"
    }
}
