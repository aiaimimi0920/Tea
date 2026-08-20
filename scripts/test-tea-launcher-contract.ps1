[CmdletBinding()]
param(
    [string]$DaemonExe = "",
    [switch]$KeepArtifacts
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-FreeTcpPort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $listener.Start()
    try { return $listener.LocalEndpoint.Port }
    finally { $listener.Stop() }
}

function Write-Utf8NoBom {
    param(
        [string]$Path,
        [string]$Content
    )

    [System.IO.File]::WriteAllText($Path, $Content, [System.Text.UTF8Encoding]::new($false))
}

function Wait-TeaProcesses {
    param(
        [System.Diagnostics.Process[]]$Processes,
        [int]$TimeoutSec,
        [string]$Description
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    do {
        $running = @($Processes | Where-Object {
            $_.Refresh()
            -not $_.HasExited
        })
        if ($running.Count -eq 0) { return }
        Start-Sleep -Milliseconds 50
    } while ((Get-Date) -lt $deadline)

    $details = @($running | ForEach-Object { "pid=$($_.Id)" }) -join ", "
    throw "$Description did not finish within $TimeoutSec seconds: $details"
}

function Wait-AuthenticatedTeaStatus {
    param(
        [string]$Url,
        [string]$Token,
        [System.Diagnostics.Process]$Process,
        [int]$TimeoutSec,
        [string]$Description
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    do {
        $Process.Refresh()
        if ($Process.HasExited) { throw "$Description exited before becoming ready" }
        try {
            Invoke-RestMethod `
                -Uri ($Url.TrimEnd("/") + "/v1/status") `
                -Headers @{ Authorization = "Bearer $Token" } `
                -TimeoutSec 1 | Out-Null
            return
        } catch {
            Start-Sleep -Milliseconds 100
        }
    } while ((Get-Date) -lt $deadline)
    throw "$Description did not become ready within $TimeoutSec seconds"
}

$teaRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$daemonSource = if ([string]::IsNullOrWhiteSpace($DaemonExe)) {
    Join-Path $teaRoot "target\debug\tea-daemon.exe"
} else {
    [System.IO.Path]::GetFullPath($DaemonExe)
}
if (-not (Test-Path -LiteralPath $daemonSource -PathType Leaf)) {
    throw "Missing tea-daemon executable for launcher contract: $daemonSource"
}

$artifactBase = [System.IO.Path]::GetFullPath((Join-Path $teaRoot ".tmp\launcher-contract"))
$stage = Join-Path $artifactBase ([Guid]::NewGuid().ToString("N"))
$stagePrefix = $artifactBase.TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
if (-not $stage.StartsWith($stagePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Launcher contract artifact path escaped its Tea temp root: $stage"
}
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item -LiteralPath $daemonSource -Destination (Join-Path $stage "tea-daemon.exe")

$environmentNames = @(
    "TEA_HOME",
    "TEA_DATA_DIR",
    "TEA_LOG_DIR",
    "TEA_BIND_ADDR",
    "TEA_SERVER_URL",
    "TEA_STORE_PATH",
    "TEA_CONFIG_PATH",
    "TEA_AUTH_TOKEN",
    "TEA_LOOM_BASE_URL",
    "TEA_LOOM_AUTH_TOKEN"
)
$oldEnvironment = @{}
foreach ($name in $environmentNames) {
    $oldEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
}

$port = Get-FreeTcpPort
$token = $null
$daemonPid = $null
$sameEndpointDecoyProcess = $null
$decoyProcess = $null
$result = $null
$workerProcesses = @()
try {
    $env:TEA_HOME = $stage
    $env:TEA_DATA_DIR = Join-Path $stage "data"
    $env:TEA_LOG_DIR = Join-Path $stage "logs"
    $env:TEA_BIND_ADDR = "127.0.0.1:$port"
    $env:TEA_SERVER_URL = "http://127.0.0.1:$port"
    $env:TEA_STORE_PATH = Join-Path $env:TEA_DATA_DIR "tea.sqlite"
    $env:TEA_CONFIG_PATH = Join-Path $env:TEA_DATA_DIR "config.json"
    $env:TEA_LOOM_BASE_URL = ""
    $env:TEA_LOOM_AUTH_TOKEN = ""

    New-Item -ItemType Directory -Force -Path $env:TEA_DATA_DIR, $env:TEA_LOG_DIR | Out-Null
    $resolverPath = Join-Path $PSScriptRoot "resolve-tea-token.ps1"
    $tokenPath = Join-Path $env:TEA_DATA_DIR "auth-token.txt"
    $tokenGate = Join-Path $stage "token-jobs.go"
    $tokenWorkerPath = Join-Path $stage "token-worker.ps1"
    Write-Utf8NoBom -Path $tokenWorkerPath -Content @'
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ResolverPath,
    [Parameter(Mandatory = $true)]
    [string]$TokenPath,
    [Parameter(Mandatory = $true)]
    [string]$GatePath
)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
while (-not (Test-Path -LiteralPath $GatePath)) { Start-Sleep -Milliseconds 10 }
& $ResolverPath -TokenPath $TokenPath
'@
    $tokenWorkerResults = @(1..8 | ForEach-Object {
        $stdoutPath = Join-Path $stage "token-worker-$_.out.log"
        $stderrPath = Join-Path $stage "token-worker-$_.err.log"
        $process = Start-Process -FilePath "powershell.exe" `
            -ArgumentList @(
                "-NoProfile",
                "-ExecutionPolicy", "Bypass",
                "-File", "`"$tokenWorkerPath`"",
                "-ResolverPath", "`"$resolverPath`"",
                "-TokenPath", "`"$tokenPath`"",
                "-GatePath", "`"$tokenGate`""
            ) `
            -PassThru `
            -WindowStyle Hidden `
            -RedirectStandardOutput $stdoutPath `
            -RedirectStandardError $stderrPath
        [pscustomobject]@{
            process = $process
            stdoutPath = $stdoutPath
            stderrPath = $stderrPath
        }
    })
    $tokenProcesses = @($tokenWorkerResults | ForEach-Object { $_.process })
    $workerProcesses += $tokenProcesses
    Set-Content -LiteralPath $tokenGate -Value "go" -Encoding ASCII
    Wait-TeaProcesses -Processes $tokenProcesses -TimeoutSec 30 -Description "Concurrent token resolver processes"
    foreach ($worker in $tokenWorkerResults) {
        [void]$worker.process.WaitForExit()
        $worker.process.Refresh()
        $exitCode = $worker.process.ExitCode
        if (($null -ne $exitCode) -and ([int]$exitCode -ne 0)) {
            $stderr = if (Test-Path -LiteralPath $worker.stderrPath -PathType Leaf) { [System.IO.File]::ReadAllText($worker.stderrPath).Trim() } else { "" }
            throw "Concurrent token resolver process $($worker.process.Id) failed with exit code ${exitCode}: $stderr"
        }
    }
    $tokenValues = @($tokenWorkerResults | ForEach-Object { [System.IO.File]::ReadAllText($_.stdoutPath).Trim() } | Where-Object { $_ -ne "" })
    $uniqueTokenValues = @($tokenValues | Sort-Object -Unique)
    if ($tokenValues.Count -ne 8 -or $uniqueTokenValues.Count -ne 1) {
        throw "Concurrent token resolvers did not return one shared value"
    }
    $token = $uniqueTokenValues[0]
    if ($token -notmatch '^[0-9a-fA-F]{32}$') { throw "Resolver returned an invalid token format" }
    if ((Get-Content -Raw -LiteralPath $tokenPath).Trim() -ne $token) {
        throw "Resolver output does not match the persisted token"
    }

    $malformedTokenPath = Join-Path $stage "malformed-auth-token.txt"
    Set-Content -LiteralPath $malformedTokenPath -Value "dev-token" -Encoding ASCII
    $malformedTokenRejected = $false
    try {
        & $resolverPath -TokenPath $malformedTokenPath | Out-Null
    } catch {
        $malformedTokenRejected = $true
    }
    if (-not $malformedTokenRejected) { throw "Resolver accepted a malformed persisted token" }
    if ((Get-Content -Raw -LiteralPath $malformedTokenPath).Trim() -ne "dev-token") {
        throw "Resolver replaced a malformed persisted token"
    }

    $directoryTokenPath = Join-Path $stage "token-as-directory"
    New-Item -ItemType Directory -Path $directoryTokenPath | Out-Null
    $directoryTokenRejected = $false
    try {
        & $resolverPath -TokenPath $directoryTokenPath | Out-Null
    } catch {
        $directoryTokenRejected = $true
    }
    if (-not $directoryTokenRejected) { throw "Resolver accepted a directory as the token file" }
    $env:TEA_AUTH_TOKEN = $token

    $startScript = Join-Path $PSScriptRoot "start-tea-daemon.ps1"
    $launchGate = Join-Path $stage "launcher-jobs.go"
    $launcherWorkerPath = Join-Path $stage "launcher-worker.ps1"
    Write-Utf8NoBom -Path $launcherWorkerPath -Content @'
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$LauncherPath,
    [Parameter(Mandatory = $true)]
    [string]$GatePath,
    [int]$TimeoutSec = 20
)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
while (-not (Test-Path -LiteralPath $GatePath)) { Start-Sleep -Milliseconds 10 }
& $LauncherPath -TimeoutSec $TimeoutSec
'@
    $launcherWorkerResults = @(1..4 | ForEach-Object {
        $stdoutPath = Join-Path $stage "launcher-worker-$_.out.log"
        $stderrPath = Join-Path $stage "launcher-worker-$_.err.log"
        $process = Start-Process -FilePath "powershell.exe" `
            -ArgumentList @(
                "-NoProfile",
                "-ExecutionPolicy", "Bypass",
                "-File", "`"$launcherWorkerPath`"",
                "-LauncherPath", "`"$startScript`"",
                "-GatePath", "`"$launchGate`"",
                "-TimeoutSec", "20"
            ) `
            -PassThru `
            -WindowStyle Hidden `
            -RedirectStandardOutput $stdoutPath `
            -RedirectStandardError $stderrPath
        [pscustomobject]@{
            process = $process
            stdoutPath = $stdoutPath
            stderrPath = $stderrPath
        }
    })
    $launchProcesses = @($launcherWorkerResults | ForEach-Object { $_.process })
    $workerProcesses += $launchProcesses
    Set-Content -LiteralPath $launchGate -Value "go" -Encoding ASCII
    Wait-TeaProcesses -Processes $launchProcesses -TimeoutSec 45 -Description "Concurrent launcher processes"
    foreach ($worker in $launcherWorkerResults) {
        [void]$worker.process.WaitForExit()
        $worker.process.Refresh()
        $exitCode = $worker.process.ExitCode
        if (($null -ne $exitCode) -and ([int]$exitCode -ne 0)) {
            $stderr = if (Test-Path -LiteralPath $worker.stderrPath -PathType Leaf) { [System.IO.File]::ReadAllText($worker.stderrPath).Trim() } else { "" }
            throw "Concurrent launcher process $($worker.process.Id) failed with exit code ${exitCode}: $stderr"
        }
    }

    $pidFile = Join-Path $env:TEA_DATA_DIR "tea-daemon.pid"
    $firstPid = [int](Get-Content -Raw -LiteralPath $pidFile)
    $daemonPid = $firstPid

    $ownedDaemons = @(Get-Process -Name "tea-daemon" -ErrorAction SilentlyContinue | Where-Object {
        $processPath = try { $_.Path } catch { $null }
        -not [string]::IsNullOrWhiteSpace($processPath) -and
            [string]::Equals(
                [System.IO.Path]::GetFullPath($processPath),
                [System.IO.Path]::GetFullPath((Join-Path $stage "tea-daemon.exe")),
                [System.StringComparison]::OrdinalIgnoreCase
            )
    })
    if ($ownedDaemons.Count -ne 1 -or $ownedDaemons[0].Id -ne $firstPid) {
        throw "Concurrent launcher contract expected one owned daemon, found $($ownedDaemons.Count)"
    }

    $daemonCommandLine = (Get-CimInstance Win32_Process -Filter "ProcessId = $firstPid").CommandLine
    if ([string]::IsNullOrWhiteSpace($daemonCommandLine)) {
        throw "Launcher contract could not inspect the owned daemon command line"
    }
    if ($daemonCommandLine.Contains($token) -or $daemonCommandLine -match '(?i)--auth-token') {
        throw "Launcher exposed the Tea auth token through the daemon command line"
    }

    & $startScript -TimeoutSec 15
    $secondPid = [int](Get-Content -Raw -LiteralPath $pidFile)
    if ($firstPid -ne $secondPid) {
        throw "Repeated launcher invocation changed daemon PID: $firstPid -> $secondPid"
    }

    $env:TEA_AUTH_TOKEN = "wrong-token"
    $wrongTokenRejected = $false
    try {
        & (Join-Path $PSScriptRoot "start-tea-daemon.ps1") -TimeoutSec 3
    } catch {
        $wrongTokenRejected = $true
    }
    if (-not $wrongTokenRejected) { throw "Launcher accepted a daemon with the wrong token" }
    if ([int](Get-Content -Raw -LiteralPath $pidFile) -ne $firstPid) {
        throw "Wrong-token probe changed the owned daemon PID"
    }
    if ($null -eq (Get-Process -Id $firstPid -ErrorAction SilentlyContinue)) {
        throw "Owned daemon exited during launcher contract"
    }

    $env:TEA_AUTH_TOKEN = $token
    $stopScript = Join-Path $PSScriptRoot "stop-tea.ps1"
    & $stopScript
    if ($null -ne (Get-Process -Id $firstPid -ErrorAction SilentlyContinue)) {
        throw "Stop launcher did not stop the original matching Tea daemon"
    }
    $daemonPid = $null

    $decoyDataDir = Join-Path $stage "decoy-data"
    New-Item -ItemType Directory -Force -Path $decoyDataDir | Out-Null
    $decoyStorePath = Join-Path $decoyDataDir "tea.sqlite"
    $decoyConfigPath = Join-Path $decoyDataDir "config.json"

    $sameEndpointDecoyProcess = Start-Process `
        -FilePath (Join-Path $stage "tea-daemon.exe") `
        -ArgumentList @(
            "--bind-addr", $env:TEA_BIND_ADDR,
            "--store-path", $decoyStorePath,
            "--config-path", $decoyConfigPath
        ) `
        -PassThru `
        -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $stage "same-endpoint-decoy.out.log") `
        -RedirectStandardError (Join-Path $stage "same-endpoint-decoy.err.log")
    Wait-AuthenticatedTeaStatus `
        -Url $env:TEA_SERVER_URL `
        -Token $token `
        -Process $sameEndpointDecoyProcess `
        -TimeoutSec 15 `
        -Description "Same-endpoint decoy daemon"
    Set-Content -LiteralPath $pidFile -Value $sameEndpointDecoyProcess.Id -Encoding ASCII
    $sameEndpointDecoyRejected = $false
    try {
        & $startScript -TimeoutSec 3
    } catch {
        $sameEndpointDecoyRejected = $true
    }
    if (-not $sameEndpointDecoyRejected) {
        throw "Launcher accepted an authenticated same-endpoint daemon from another profile"
    }
    if ($null -eq (Get-Process -Id $sameEndpointDecoyProcess.Id -ErrorAction SilentlyContinue)) {
        throw "Launcher terminated the authenticated same-endpoint daemon from another profile"
    }
    Stop-Process -Id $sameEndpointDecoyProcess.Id -Force -ErrorAction Stop
    try { $sameEndpointDecoyProcess.WaitForExit(5000) | Out-Null } catch {}
    if ($null -ne (Get-Process -Id $sameEndpointDecoyProcess.Id -ErrorAction SilentlyContinue)) {
        throw "Same-endpoint decoy daemon did not stop during contract setup"
    }

    $decoyPort = Get-FreeTcpPort
    $decoyProcess = Start-Process `
        -FilePath (Join-Path $stage "tea-daemon.exe") `
        -ArgumentList @(
            "--bind-addr", "127.0.0.1:$decoyPort",
            "--store-path", $decoyStorePath,
            "--config-path", $decoyConfigPath
        ) `
        -PassThru `
        -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $stage "decoy-daemon.out.log") `
        -RedirectStandardError (Join-Path $stage "decoy-daemon.err.log")

    Wait-AuthenticatedTeaStatus `
        -Url "http://127.0.0.1:$decoyPort" `
        -Token $token `
        -Process $decoyProcess `
        -TimeoutSec 15 `
        -Description "Same-executable decoy daemon"

    Set-Content -LiteralPath $pidFile -Value $decoyProcess.Id -Encoding ASCII
    & $startScript -TimeoutSec 15
    $replacementPid = [int](Get-Content -Raw -LiteralPath $pidFile)
    $daemonPid = $replacementPid
    if ($replacementPid -eq $decoyProcess.Id) {
        throw "Launcher accepted a same-executable daemon with mismatched profile arguments"
    }
    if ($null -eq (Get-Process -Id $replacementPid -ErrorAction SilentlyContinue)) {
        throw "Launcher did not start a replacement daemon for the expected profile"
    }
    if ($null -eq (Get-Process -Id $decoyProcess.Id -ErrorAction SilentlyContinue)) {
        throw "Launcher terminated the mismatched same-executable daemon"
    }

    Set-Content -LiteralPath $pidFile -Value $decoyProcess.Id -Encoding ASCII
    & $stopScript
    if ($null -eq (Get-Process -Id $decoyProcess.Id -ErrorAction SilentlyContinue)) {
        throw "Stop launcher terminated a same-executable daemon from another profile"
    }
    if ($null -eq (Get-Process -Id $replacementPid -ErrorAction SilentlyContinue)) {
        throw "Stop launcher terminated the matching daemon while a forged PID was present"
    }

    Set-Content -LiteralPath $pidFile -Value $replacementPid -Encoding ASCII
    & $stopScript
    if ($null -ne (Get-Process -Id $replacementPid -ErrorAction SilentlyContinue)) {
        throw "Stop launcher did not stop the daemon whose profile arguments matched"
    }
    if ($null -eq (Get-Process -Id $decoyProcess.Id -ErrorAction SilentlyContinue)) {
        throw "Matching-profile stop also terminated the other Tea profile"
    }
    $daemonPid = $null

    $result = [ordered]@{
        status = "passed"
        artifactRoot = $stage
        port = $port
        daemonPid = $firstPid
        concurrentLaunchers = $launchProcesses.Count
        concurrentTokenResolvers = $tokenProcesses.Count
        concurrentTokenCreationSharedValue = $true
        duplicateLaunchReusedPid = $true
        malformedTokenRejectedWithoutReplacement = $true
        nonFileTokenPathRejected = $true
        tokenOmittedFromCommandLine = $true
        wrongTokenRejected = $true
        sameEndpointMismatchedProfileRejected = $true
        mismatchedProfilePidRejectedByLauncher = $true
        mismatchedProfilePidNotStopped = $true
        matchingProfileStopped = $true
    }
} finally {
    if (-not [string]::IsNullOrWhiteSpace($token)) { $env:TEA_AUTH_TOKEN = $token }
    foreach ($process in $workerProcesses) {
        try {
            $process.Refresh()
            if (-not $process.HasExited) {
                Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
                try { $process.WaitForExit(5000) | Out-Null } catch {}
            }
        } catch {
        }
    }
    try { & (Join-Path $PSScriptRoot "stop-tea.ps1") } catch { Write-Warning $_ }
    $leakedDaemon = $null -ne $daemonPid -and $null -ne (Get-Process -Id $daemonPid -ErrorAction SilentlyContinue)
    if ($leakedDaemon) {
        Stop-Process -Id $daemonPid -Force -ErrorAction SilentlyContinue
    }
    $leakedSameEndpointDecoy = $null -ne $sameEndpointDecoyProcess -and $null -ne (Get-Process -Id $sameEndpointDecoyProcess.Id -ErrorAction SilentlyContinue)
    if ($leakedSameEndpointDecoy) {
        Stop-Process -Id $sameEndpointDecoyProcess.Id -Force -ErrorAction SilentlyContinue
        try { $sameEndpointDecoyProcess.WaitForExit(5000) | Out-Null } catch {}
    }
    $leakedDecoy = $null -ne $decoyProcess -and $null -ne (Get-Process -Id $decoyProcess.Id -ErrorAction SilentlyContinue)
    if ($leakedDecoy) {
        Stop-Process -Id $decoyProcess.Id -Force -ErrorAction SilentlyContinue
        try { $decoyProcess.WaitForExit(5000) | Out-Null } catch {}
    }
    foreach ($name in $environmentNames) {
        $value = $oldEnvironment[$name]
        if ($null -eq $value) {
            Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
        } else {
            Set-Item -LiteralPath "Env:$name" -Value $value
        }
    }
    if (-not $KeepArtifacts) {
        Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
    }
    if ($leakedDaemon) {
        throw "Launcher stop contract left tea-daemon running: pid=$daemonPid"
    }
}

$result | ConvertTo-Json -Depth 5
