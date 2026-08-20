[CmdletBinding()]
param(
    [int]$TimeoutSec = 10
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Test-SamePath {
    param([string]$Left, [string]$Right)

    if ([string]::IsNullOrWhiteSpace($Left) -or [string]::IsNullOrWhiteSpace($Right)) {
        return $false
    }
    try {
        $leftFull = [System.IO.Path]::GetFullPath($Left).TrimEnd("\", "/")
        $rightFull = [System.IO.Path]::GetFullPath($Right).TrimEnd("\", "/")
        return [string]::Equals($leftFull, $rightFull, [System.StringComparison]::OrdinalIgnoreCase)
    } catch {
        return $false
    }
}

function ConvertFrom-WindowsCommandLine {
    param([string]$CommandLine)

    if ([string]::IsNullOrWhiteSpace($CommandLine)) { return @() }
    if (-not ("TeaLauncher.CommandLineNative" -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

namespace TeaLauncher {
    public static class CommandLineNative {
        [DllImport("shell32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        public static extern IntPtr CommandLineToArgvW(string commandLine, out int argumentCount);

        [DllImport("kernel32.dll")]
        public static extern IntPtr LocalFree(IntPtr memory);
    }
}
'@
    }

    $argumentCount = 0
    $argumentPointer = [TeaLauncher.CommandLineNative]::CommandLineToArgvW(
        $CommandLine,
        [ref]$argumentCount
    )
    if ($argumentPointer -eq [IntPtr]::Zero) { return @() }
    try {
        $arguments = @()
        for ($index = 0; $index -lt $argumentCount; $index++) {
            $valuePointer = [Runtime.InteropServices.Marshal]::ReadIntPtr(
                $argumentPointer,
                $index * [IntPtr]::Size
            )
            $arguments += [Runtime.InteropServices.Marshal]::PtrToStringUni($valuePointer)
        }
        return $arguments
    } finally {
        [void][TeaLauncher.CommandLineNative]::LocalFree($argumentPointer)
    }
}

function Get-ProcessCommandLine {
    param([int]$ProcessId)

    try {
        $record = Get-CimInstance Win32_Process -Filter "ProcessId = $ProcessId" -ErrorAction Stop
        if ($null -eq $record) { return $null }
        return [string]$record.CommandLine
    } catch {
        return $null
    }
}

function Get-UniqueCommandLineOption {
    param(
        [string[]]$Arguments,
        [string]$Name
    )

    $values = @()
    for ($index = 1; $index -lt $Arguments.Count; $index++) {
        if (-not [string]::Equals($Arguments[$index], $Name, [System.StringComparison]::Ordinal)) {
            continue
        }
        if (($index + 1) -ge $Arguments.Count) { return $null }
        $values += $Arguments[$index + 1]
        $index += 1
    }
    if ($values.Count -ne 1) { return $null }
    return [string]$values[0]
}

function Test-TeaDaemonCommandLine {
    param(
        [string]$CommandLine,
        [string]$DaemonExe,
        [string]$BindAddr,
        [string]$StorePath,
        [string]$ConfigPath
    )

    $arguments = @(ConvertFrom-WindowsCommandLine -CommandLine $CommandLine)
    if ($arguments.Count -lt 1 -or -not (Test-SamePath -Left $arguments[0] -Right $DaemonExe)) {
        return $false
    }
    $actualBindAddr = Get-UniqueCommandLineOption -Arguments $arguments -Name "--bind-addr"
    $actualStorePath = Get-UniqueCommandLineOption -Arguments $arguments -Name "--store-path"
    $actualConfigPath = Get-UniqueCommandLineOption -Arguments $arguments -Name "--config-path"
    return [string]::Equals($actualBindAddr, $BindAddr, [System.StringComparison]::OrdinalIgnoreCase) -and
        (Test-SamePath -Left $actualStorePath -Right $StorePath) -and
        (Test-SamePath -Left $actualConfigPath -Right $ConfigPath)
}

function Get-TeaDaemonMutexName {
    param([string]$DataDir)

    $normalized = [System.IO.Path]::GetFullPath($DataDir).TrimEnd("\", "/").ToUpperInvariant()
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hash = $sha256.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($normalized))
    } finally {
        $sha256.Dispose()
    }
    $suffix = ([System.BitConverter]::ToString($hash)).Replace("-", "")
    return "Local\Neuro.Tea.Daemon.$suffix"
}

function Enter-TeaDaemonMutex {
    param(
        [string]$DataDir,
        [int]$TimeoutSec
    )

    $mutex = [System.Threading.Mutex]::new($false, (Get-TeaDaemonMutexName -DataDir $DataDir))
    $acquired = $false
    try {
        try {
            $acquired = $mutex.WaitOne([TimeSpan]::FromSeconds($TimeoutSec))
        } catch [System.Threading.AbandonedMutexException] {
            $acquired = $true
        }
        if (-not $acquired) {
            throw "Timed out waiting for another Tea launcher to finish after $TimeoutSec seconds."
        }
        return $mutex
    } catch {
        $mutex.Dispose()
        throw
    }
}

function Exit-TeaDaemonMutex {
    param([System.Threading.Mutex]$Mutex)

    try { $Mutex.ReleaseMutex() }
    finally { $Mutex.Dispose() }
}

function Remove-PidFileIfValueMatches {
    param(
        [string]$PidFile,
        [string]$ExpectedValue
    )

    try {
        if (-not (Test-Path -LiteralPath $PidFile -PathType Leaf)) { return $false }
        $currentValue = ([System.IO.File]::ReadAllText($PidFile)).Trim()
        if (-not [string]::Equals($currentValue, $ExpectedValue, [System.StringComparison]::Ordinal)) {
            return $false
        }
        [System.IO.File]::Delete($PidFile)
        return $true
    } catch {
        return $false
    }
}

function Test-TeaHealth {
    try {
        Invoke-RestMethod -Uri ($env:TEA_SERVER_URL.TrimEnd("/") + "/health") -TimeoutSec 1 | Out-Null
        return $true
    } catch {
        return $false
    }
}

function Test-TeaStatus {
    try {
        Invoke-RestMethod `
            -Uri ($env:TEA_SERVER_URL.TrimEnd("/") + "/v1/status") `
            -Headers @{ Authorization = "Bearer $env:TEA_AUTH_TOKEN" } `
            -TimeoutSec 1 | Out-Null
        return $true
    } catch {
        if (Test-TeaHealth) {
            throw "A process is listening at $env:TEA_SERVER_URL but rejected the configured Tea token or API contract. Stop that instance or use matching TEA_SERVER_URL/TEA_AUTH_TOKEN values."
        }
        return $false
    }
}

function Get-OwnedDaemon {
    param(
        [string]$PidFile,
        [string]$DaemonExe,
        [string]$BindAddr,
        [string]$StorePath,
        [string]$ConfigPath
    )

    if (-not (Test-Path -LiteralPath $PidFile -PathType Leaf)) { return $null }
    $pidValue = (Get-Content -Raw -LiteralPath $PidFile).Trim()
    if ($pidValue -notmatch '^\d+$') {
        [void](Remove-PidFileIfValueMatches -PidFile $PidFile -ExpectedValue $pidValue)
        return $null
    }
    $process = Get-Process -Id ([int]$pidValue) -ErrorAction SilentlyContinue
    $processPath = if ($null -eq $process) { $null } else { try { $process.Path } catch { $null } }
    $commandLine = if ($null -eq $process) { $null } else { Get-ProcessCommandLine -ProcessId $process.Id }
    if ($null -eq $process -or
        -not (Test-SamePath -Left $processPath -Right $DaemonExe) -or
        -not (Test-TeaDaemonCommandLine `
            -CommandLine $commandLine `
            -DaemonExe $DaemonExe `
            -BindAddr $BindAddr `
            -StorePath $StorePath `
            -ConfigPath $ConfigPath)) {
        [void](Remove-PidFileIfValueMatches -PidFile $PidFile -ExpectedValue $pidValue)
        return $null
    }
    return $process
}

function Wait-TeaReady {
    param(
        [System.Diagnostics.Process]$Process,
        [int]$TimeoutSec
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    do {
        $Process.Refresh()
        if ($Process.HasExited) {
            throw "tea-daemon.exe exited during startup with code $($Process.ExitCode)."
        }
        if (Test-TeaStatus) { return }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)
    throw "tea-daemon.exe did not expose an authenticated status endpoint within $TimeoutSec seconds."
}

$teaHome = if ([string]::IsNullOrWhiteSpace($env:TEA_HOME)) {
    (Resolve-Path -LiteralPath $PSScriptRoot).Path
} else {
    [System.IO.Path]::GetFullPath($env:TEA_HOME)
}
$dataDir = if ([string]::IsNullOrWhiteSpace($env:TEA_DATA_DIR)) { Join-Path $teaHome "data" } else { $env:TEA_DATA_DIR }
$logDir = if ([string]::IsNullOrWhiteSpace($env:TEA_LOG_DIR)) { Join-Path $teaHome "logs" } else { $env:TEA_LOG_DIR }
$daemonExe = [System.IO.Path]::GetFullPath((Join-Path $teaHome "tea-daemon.exe"))
$pidFile = Join-Path $dataDir "tea-daemon.pid"

if (-not (Test-Path -LiteralPath $daemonExe -PathType Leaf)) { throw "Missing tea-daemon.exe: $daemonExe" }
foreach ($requiredName in @("TEA_BIND_ADDR", "TEA_SERVER_URL", "TEA_STORE_PATH", "TEA_CONFIG_PATH", "TEA_AUTH_TOKEN")) {
    if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($requiredName, "Process"))) {
        throw "$requiredName is required"
    }
}
New-Item -ItemType Directory -Force -Path $dataDir, $logDir | Out-Null
$storePath = [System.IO.Path]::GetFullPath($env:TEA_STORE_PATH)
$configPath = [System.IO.Path]::GetFullPath($env:TEA_CONFIG_PATH)

$startupMutex = Enter-TeaDaemonMutex -DataDir $dataDir -TimeoutSec $TimeoutSec
try {
    $ownedDaemon = Get-OwnedDaemon `
        -PidFile $pidFile `
        -DaemonExe $daemonExe `
        -BindAddr $env:TEA_BIND_ADDR `
        -StorePath $storePath `
        -ConfigPath $configPath
    if (Test-TeaHealth) {
        if (Test-TeaStatus) {
            if ($null -eq $ownedDaemon) {
                throw "An authenticated Tea daemon is listening at $env:TEA_SERVER_URL but its PID, executable, or profile arguments do not match this launcher. Stop that instance or choose a different Tea endpoint."
            }
            Write-Host "tea-daemon already ready: pid=$($ownedDaemon.Id) url=$env:TEA_SERVER_URL"
            return
        }
    }

    if ($null -ne $ownedDaemon) {
        Wait-TeaReady -Process $ownedDaemon -TimeoutSec $TimeoutSec
        Write-Host "tea-daemon ready: pid=$($ownedDaemon.Id) url=$env:TEA_SERVER_URL"
        return
    }

    $outLog = Join-Path $logDir "tea-daemon.out.log"
    $errLog = Join-Path $logDir "tea-daemon.err.log"
    $arguments = @(
        "--bind-addr", $env:TEA_BIND_ADDR,
        "--store-path", $storePath,
        "--config-path", $configPath
    )
    $started = $null
    try {
        $started = Start-Process `
            -FilePath $daemonExe `
            -ArgumentList $arguments `
            -PassThru `
            -WindowStyle Hidden `
            -RedirectStandardOutput $outLog `
            -RedirectStandardError $errLog
        Set-Content -LiteralPath $pidFile -Value $started.Id -Encoding ASCII
        Wait-TeaReady -Process $started -TimeoutSec $TimeoutSec
        Write-Host "tea-daemon ready: pid=$($started.Id) url=$env:TEA_SERVER_URL"
    } catch {
        if ($null -ne $started) {
            Stop-Process -Id $started.Id -Force -ErrorAction SilentlyContinue
            try { $started.WaitForExit(5000) | Out-Null } catch {}
            [void](Remove-PidFileIfValueMatches -PidFile $pidFile -ExpectedValue ([string]$started.Id))
        }
        throw
    }
} finally {
    Exit-TeaDaemonMutex -Mutex $startupMutex
}
