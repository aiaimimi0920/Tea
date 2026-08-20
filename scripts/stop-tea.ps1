[CmdletBinding()]
param()

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
    $bindMatches = [string]::IsNullOrWhiteSpace($BindAddr) -or
        [string]::Equals($actualBindAddr, $BindAddr, [System.StringComparison]::OrdinalIgnoreCase)
    return $bindMatches -and
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
    param([string]$DataDir)

    $mutex = [System.Threading.Mutex]::new($false, (Get-TeaDaemonMutexName -DataDir $DataDir))
    $acquired = $false
    try {
        try {
            $acquired = $mutex.WaitOne([TimeSpan]::FromSeconds(10))
        } catch [System.Threading.AbandonedMutexException] {
            $acquired = $true
        }
        if (-not $acquired) { throw "Timed out waiting for the Tea launcher to finish." }
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

function Stop-OwnedProcess {
    param(
        [System.Diagnostics.Process]$Process,
        [string]$ExpectedPath,
        [string]$Label
    )

    $processPath = try { $Process.Path } catch { $null }
    if (-not (Test-SamePath -Left $processPath -Right $ExpectedPath)) { return $false }
    $processId = $Process.Id
    Stop-Process -Id $Process.Id -Force -ErrorAction Stop
    try { [void]$Process.WaitForExit(5000) } catch {}
    if ($null -ne (Get-Process -Id $processId -ErrorAction SilentlyContinue)) {
        throw "failed to stop $Label within 5 seconds: pid=$processId"
    }
    Write-Host "stopped $Label pid=$processId"
    return $true
}

function Stop-OwnedDaemon {
    param(
        [System.Diagnostics.Process]$Process,
        [string]$ExpectedPath,
        [string]$BindAddr,
        [string]$StorePath,
        [string]$ConfigPath
    )

    $processPath = try { $Process.Path } catch { $null }
    $commandLine = Get-ProcessCommandLine -ProcessId $Process.Id
    if (-not (Test-SamePath -Left $processPath -Right $ExpectedPath) -or
        -not (Test-TeaDaemonCommandLine `
            -CommandLine $commandLine `
            -DaemonExe $ExpectedPath `
            -BindAddr $BindAddr `
            -StorePath $StorePath `
            -ConfigPath $ConfigPath)) {
        return $false
    }
    $processId = $Process.Id
    Stop-Process -Id $Process.Id -Force -ErrorAction Stop
    try { [void]$Process.WaitForExit(5000) } catch {}
    if ($null -ne (Get-Process -Id $processId -ErrorAction SilentlyContinue)) {
        throw "failed to stop tea-daemon within 5 seconds: pid=$processId"
    }
    Write-Host "stopped tea-daemon pid=$processId"
    return $true
}

$teaHome = if ([string]::IsNullOrWhiteSpace($env:TEA_HOME)) {
    (Resolve-Path -LiteralPath $PSScriptRoot).Path
} else {
    [System.IO.Path]::GetFullPath($env:TEA_HOME)
}
$daemonExe = [System.IO.Path]::GetFullPath((Join-Path $teaHome "tea-daemon.exe"))
$uiExe = [System.IO.Path]::GetFullPath((Join-Path $teaHome "tea.exe"))
$dataDirs = @(
    $env:TEA_DATA_DIR,
    (Join-Path $teaHome "data"),
    $(if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA "Neuro\tea" }),
    $(if ($env:APPDATA) { Join-Path $env:APPDATA "Neuro\tea" })
) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique

foreach ($dataDir in $dataDirs) {
    $pidFile = Join-Path $dataDir "tea-daemon.pid"
    if (-not (Test-Path -LiteralPath $pidFile -PathType Leaf)) { continue }
    $startupMutex = Enter-TeaDaemonMutex -DataDir $dataDir
    try {
        if (-not (Test-Path -LiteralPath $pidFile -PathType Leaf)) { continue }
        $pidValue = (Get-Content -Raw -LiteralPath $pidFile).Trim()
        $removePidFile = $true
        if ($pidValue -match '^\d+$') {
            $process = Get-Process -Id ([int]$pidValue) -ErrorAction SilentlyContinue
            if ($null -ne $process) {
                $processPath = try { $process.Path } catch { $null }
                if (-not (Test-SamePath -Left $processPath -Right $daemonExe)) {
                    $removePidFile = $false
                    continue
                }
                $explicitDataDir = -not [string]::IsNullOrWhiteSpace($env:TEA_DATA_DIR) -and
                    (Test-SamePath -Left $dataDir -Right $env:TEA_DATA_DIR)
                $storePath = if ($explicitDataDir -and -not [string]::IsNullOrWhiteSpace($env:TEA_STORE_PATH)) {
                    $env:TEA_STORE_PATH
                } else {
                    Join-Path $dataDir "tea.sqlite"
                }
                $configPath = if ($explicitDataDir -and -not [string]::IsNullOrWhiteSpace($env:TEA_CONFIG_PATH)) {
                    $env:TEA_CONFIG_PATH
                } else {
                    Join-Path $dataDir "config.json"
                }
                $bindAddr = if ($explicitDataDir) { $env:TEA_BIND_ADDR } else { $null }
                [void](Stop-OwnedDaemon `
                    -Process $process `
                    -ExpectedPath $daemonExe `
                    -BindAddr $bindAddr `
                    -StorePath $storePath `
                    -ConfigPath $configPath)
            }
        }
        if ($removePidFile) {
            [void](Remove-PidFileIfValueMatches -PidFile $pidFile -ExpectedValue $pidValue)
        }
    } finally {
        Exit-TeaDaemonMutex -Mutex $startupMutex
    }
}

Get-Process -Name "tea" -ErrorAction SilentlyContinue | ForEach-Object {
    [void](Stop-OwnedProcess -Process $_ -ExpectedPath $uiExe -Label "tea ui")
}
