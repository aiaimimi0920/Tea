[CmdletBinding()]
param(
    [int]$Port = 0,
    [string]$AuthToken = "tea-mcp-smoke-token",
    [int]$TimeoutSec = 60
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))
$teaRoot = Join-Path $repoRoot "Tea"

function Get-FreeTcpPort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Parse("127.0.0.1"), 0)
    $listener.Start()
    try { return $listener.LocalEndpoint.Port } finally { $listener.Stop() }
}

if ($Port -eq 0) { $Port = Get-FreeTcpPort }
$baseUrl = "http://127.0.0.1:$Port"

Write-Host ">> building tea-daemon and tea-mcp"
& cargo build --manifest-path (Join-Path $teaRoot "Cargo.toml") -p tea-daemon -p tea-mcp | Out-Null
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$daemonExe = Join-Path $teaRoot "target\debug\tea-daemon.exe"
$mcpExe = Join-Path $teaRoot "target\debug\tea-mcp.exe"

$storeDir = Join-Path ([System.IO.Path]::GetTempPath()) ("tea-mcp-smoke-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $storeDir | Out-Null
$storePath = Join-Path $storeDir "tea.sqlite"

$env:TEA_BIND_ADDR = "127.0.0.1:$Port"
$env:TEA_AUTH_TOKEN = $AuthToken
$env:TEA_STORE_PATH = $storePath
$env:TEA_CONFIG_PATH = (Join-Path $storeDir "config.json")
$env:TEA_LOOM_BASE_URL = ""
$env:TEA_LOOM_AUTH_TOKEN = ""
$env:TEA_SERVER_URL = $baseUrl

Write-Host ">> starting tea-daemon at $baseUrl"
$daemon = Start-Process -FilePath $daemonExe -WorkingDirectory $teaRoot -PassThru -WindowStyle Hidden

try {
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    do {
        try { Invoke-RestMethod -Uri "$baseUrl/health" -TimeoutSec 1 | Out-Null; break } catch { Start-Sleep -Milliseconds 300 }
    } while ((Get-Date) -lt $deadline)

    # Build a batch of JSON-RPC requests to drive over stdio.
    $requests = @(
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
        '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
        '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tea_create_ticket","arguments":{"title":"MCP smoke ticket","description":"Created through the Tea MCP server over stdio."}}}'
        '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"tea_list_tickets","arguments":{}}}'
    )
    $input = ($requests -join "`n") + "`n"

    Write-Host ">> driving tea-mcp over stdio"
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $mcpExe
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.WorkingDirectory = $teaRoot
    $psi.EnvironmentVariables["TEA_SERVER_URL"] = $baseUrl
    $psi.EnvironmentVariables["TEA_AUTH_TOKEN"] = $AuthToken

    $proc = [System.Diagnostics.Process]::Start($psi)
    # Write stdin as UTF-8 without a BOM so the first JSON-RPC line parses cleanly.
    $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
    $stdinWriter = [System.IO.StreamWriter]::new($proc.StandardInput.BaseStream, $utf8NoBom)
    $stdinWriter.Write($input)
    $stdinWriter.Flush()
    $stdinWriter.Close()
    $stdout = $proc.StandardOutput.ReadToEnd()
    $proc.WaitForExit(10000) | Out-Null

    Write-Host "--- MCP stdout ---"
    Write-Host $stdout

    $lines = @($stdout -split "`n" | Where-Object { $_.Trim() -ne "" })
    $responses = @($lines | ForEach-Object { $_ | ConvertFrom-Json })

    function Get-ResponseById([int]$id) {
        return $responses | Where-Object { $null -ne $_.id -and [int]$_.id -eq $id } | Select-Object -First 1
    }

    $init = Get-ResponseById 1
    if (-not $init) { throw "no response for initialize (id 1)" }
    if (-not $init.PSObject.Properties['result'] -or -not $init.result.serverInfo) {
        throw "initialize did not return serverInfo"
    }

    $toolsList = Get-ResponseById 2
    $toolCount = @($toolsList.result.tools).Count
    if ($toolCount -lt 10) { throw "tools/list returned too few tools: $toolCount" }

    $create = Get-ResponseById 3
    if ($create.result.isError) { throw "tea_create_ticket reported an error: $($create.result.content[0].text)" }
    $createdText = $create.result.content[0].text
    $created = $createdText | ConvertFrom-Json
    if (-not $created.id) { throw "create did not return a ticket id" }

    $list = Get-ResponseById 4
    $listText = $list.result.content[0].text
    if (-not $listText.Contains($created.id)) { throw "created ticket id not present in tea_list_tickets output" }

    Write-Host "Tea MCP real smoke passed"
    Write-Host "tool_count=$toolCount"
    Write-Host "ticket_id=$($created.id)"
} finally {
    if ($daemon -and -not $daemon.HasExited) {
        Stop-Process -Id $daemon.Id -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -Recurse -Force $storeDir -ErrorAction SilentlyContinue
}
