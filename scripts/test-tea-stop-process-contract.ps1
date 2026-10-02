[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Load only definitions, never the launcher's top-level discovery/stop routine.
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $PSScriptRoot "stop-tea.ps1"), [ref]$tokens, [ref]$parseErrors
)
if ($parseErrors.Count -ne 0) { throw "Stop launcher has PowerShell parse errors" }
foreach ($name in @(
    "Test-SamePath", "ConvertFrom-WindowsCommandLine", "Get-UniqueCommandLineOption",
    "Test-TeaDaemonCommandLine", "Stop-OwnedProcess", "Stop-OwnedDaemon"
)) {
    $definition = $ast.Find({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
    }, $false)
    if ($null -eq $definition) { throw "Missing launcher function $name" }
    . ([scriptblock]::Create($definition.Extent.Text))
}

$children = @()
$lookups = 0
# Simulate a post-stop PID lookup observing another process. Waiting must use
# the pinned original object; the unrelated process must remain alive.
function Get-Process {
    param([int]$Id, $ErrorAction)
    $script:lookups++
    return $script:decoy
}
try {
    $shell = Join-Path $PSHOME "powershell.exe"
    for ($index = 0; $index -lt 3; $index++) {
        $children += Start-Process -FilePath $shell -ArgumentList @(
            "-NoProfile", "-Command", "Start-Sleep -Seconds 60"
        ) -PassThru -WindowStyle Hidden
    }
    $decoy = $children[2]
    # Retain a second handle, as another launcher can while its child exits.
    [void]$children[0].Handle
    $target = [System.Diagnostics.Process]::GetProcessById($children[0].Id)
    try {
        if (Stop-OwnedProcess -Process $target -ExpectedPath ($shell + ".other") -Label "fixture") {
            throw "Mismatched executable was accepted"
        }
        if ($target.HasExited) { throw "Mismatched executable was terminated" }
        if (-not (Stop-OwnedProcess -Process $target -ExpectedPath $shell -Label "fixture")) {
            throw "Verified process was not stopped"
        }
        if (-not $children[0].WaitForExit(5000)) { throw "Owned child is still running" }
        if ($decoy.HasExited) { throw "Unrelated process was terminated" }
        if ($lookups -ne 0) { throw "Stop result was decided by a fresh PID lookup" }
    } finally {
        $target.Dispose()
    }
    $daemon = [System.Diagnostics.Process]::GetProcessById($children[1].Id)
    [void]$children[1].Handle
    $fixtureStore = Join-Path ([System.IO.Path]::GetTempPath()) "tea-stop-contract.sqlite"
    $fixtureConfig = Join-Path ([System.IO.Path]::GetTempPath()) "tea-stop-contract.json"
    $fixtureBind = "127.0.0.1:48999"
    # Substitute only the OS command-line query; run the actual parser and all
    # profile checks. These paths are identity fixtures, not files we write.
    function Get-ProcessCommandLine {
        param([int]$ProcessId)
        if ($ProcessId -ne $daemon.Id) { throw "Queried a different daemon identity" }
        return ('"{0}" --bind-addr "{1}" --store-path "{2}" --config-path "{3}"' -f
            $shell, $fixtureBind, $fixtureStore, $fixtureConfig)
    }
    try {
        $expected = @{ Process = $daemon; ExpectedPath = $shell; BindAddr = $fixtureBind;
            StorePath = $fixtureStore; ConfigPath = $fixtureConfig }
        foreach ($field in @("ExpectedPath", "BindAddr", "StorePath", "ConfigPath")) {
            $mismatch = $expected.Clone()
            $mismatch[$field] += ".other"
            if (Stop-OwnedDaemon @mismatch) { throw "Mismatched daemon $field was accepted" }
            if ($daemon.HasExited) { throw "Mismatched daemon $field was terminated" }
        }
        if (-not (Stop-OwnedDaemon @expected)) { throw "Verified daemon was not stopped" }
        if (-not $children[1].WaitForExit(5000)) { throw "Owned daemon child is still running" }
        if ($decoy.HasExited) { throw "Daemon stop terminated the unrelated child" }
        if ($lookups -ne 0) { throw "Daemon stop reacquired a process by PID" }
    } finally { $daemon.Dispose() }

    # A disappeared/unavailable process association must fail before mutation,
    # rather than suppressing the handle failure and reporting success.
    $unavailable = [System.Diagnostics.Process]::new()
    try {
        foreach ($stop in @("Stop-OwnedProcess", "Stop-OwnedDaemon")) {
            $rejected = $false
            try { & $stop -Process $unavailable -ExpectedPath $shell | Out-Null }
            catch { $rejected = $true }
            if (-not $rejected) { throw "$stop accepted an unavailable process association" }
        }
    } finally { $unavailable.Dispose() }
    Write-Output "Tea stop contract passed: both stop paths, profile guards, pinned identity, decoy preserved"
} finally {
    foreach ($child in $children) {
        try {
            if (-not $child.HasExited) { $child.Kill() }
            if (-not $child.WaitForExit(5000)) { throw "Fixture child failed to exit" }
        } finally { $child.Dispose() }
    }
}
