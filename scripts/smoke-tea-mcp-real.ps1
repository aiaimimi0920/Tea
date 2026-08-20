[CmdletBinding()]
param(
    [int]$Port = 0,
    [string]$AuthToken = "tea-mcp-smoke-token",
    [int]$TimeoutSec = 60,
    [string]$PackageDir = ""
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

function Get-PortListeners {
    param([int]$Port)

    $listeners = @()
    $lines = @(netstat -ano -p TCP 2>$null)
    foreach ($line in $lines) {
        $parts = @($line -split "\s+" | Where-Object { $_ -ne "" })
        if ($parts.Count -lt 5 -or $parts[0] -ne "TCP") { continue }
        $state = $parts[$parts.Count - 2]
        if ($state -ne "LISTENING") { continue }
        $localEndpoint = $parts[1]
        $lastColon = $localEndpoint.LastIndexOf(":")
        if ($lastColon -lt 0) { continue }
        $localPortText = $localEndpoint.Substring($lastColon + 1)
        $localPort = 0
        if (![int]::TryParse($localPortText, [ref]$localPort)) { continue }
        if ($localPort -ne $Port) { continue }
        $processId = 0
        [void][int]::TryParse($parts[$parts.Count - 1], [ref]$processId)
        $listeners += [pscustomobject]@{
            local_endpoint = $localEndpoint
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

function Stop-SmokeProcess {
    param(
        [AllowNull()]
        [System.Diagnostics.Process]$Process,
        [string]$Name
    )

    if ($null -eq $Process) { return }
    $Process.Refresh()
    if (-not $Process.HasExited) {
        Stop-Process -Id $Process.Id -Force -ErrorAction Stop
    }
    [void]$Process.WaitForExit(5000)
    $Process.Refresh()
    if (-not $Process.HasExited) { throw "$Name did not exit during smoke cleanup" }
}

function Test-TcpPortOpen {
    param([int]$Port)

    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $task = $client.ConnectAsync("127.0.0.1", $Port)
        if (-not $task.Wait(300)) { return $false }
        return $client.Connected
    } catch {
        return $false
    } finally {
        $client.Dispose()
    }
}

function Wait-TcpPortReleased {
    param([int]$Port)

    $deadline = (Get-Date).AddSeconds(5)
    while ((Get-Date) -lt $deadline) {
        if (-not (Test-TcpPortOpen -Port $Port)) { return }
        Start-Sleep -Milliseconds 100
    }
    throw "Tea MCP smoke listener remained open after cleanup: 127.0.0.1:$Port"
}

if ($Port -eq 0) { $Port = Get-FreeTcpPort }
Assert-NoPreexistingPortListeners -Ports @($Port)
$baseUrl = "http://127.0.0.1:$Port"

if ([string]::IsNullOrWhiteSpace($PackageDir)) {
    Write-Host ">> building tea-daemon and tea-mcp"
    & cargo build --manifest-path (Join-Path $teaRoot "Cargo.toml") -p tea-daemon -p tea-mcp | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

    $daemonExe = Join-Path $teaRoot "target\debug\tea-daemon.exe"
    $mcpExe = Join-Path $teaRoot "target\debug\tea-mcp.exe"
    $workingDirectory = $teaRoot
} else {
    $packagePath = [System.IO.Path]::GetFullPath((Resolve-Path -LiteralPath $PackageDir).Path)
    $daemonExe = Join-Path $packagePath "tea-daemon.exe"
    $mcpExe = Join-Path $packagePath "tea-mcp.exe"
    $workingDirectory = $packagePath
    if (-not (Test-Path -LiteralPath $daemonExe -PathType Leaf)) { throw "missing packaged tea-daemon.exe: $daemonExe" }
    if (-not (Test-Path -LiteralPath $mcpExe -PathType Leaf)) { throw "missing packaged tea-mcp.exe: $mcpExe" }
    Write-Host ">> using packaged tea-daemon and tea-mcp from $packagePath"
}

$storeDir = Join-Path ([System.IO.Path]::GetTempPath()) ("tea-mcp-smoke-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $storeDir | Out-Null
$storePath = Join-Path $storeDir "tea.sqlite"
$daemonOut = Join-Path $storeDir "tea-daemon.out.log"
$daemonErr = Join-Path $storeDir "tea-daemon.err.log"

$environmentNames = @(
    "TEA_BIND_ADDR",
    "TEA_AUTH_TOKEN",
    "TEA_STORE_PATH",
    "TEA_CONFIG_PATH",
    "TEA_LOOM_BASE_URL",
    "TEA_LOOM_AUTH_TOKEN",
    "TEA_SERVER_URL"
)
$oldEnvironment = @{}
foreach ($name in $environmentNames) {
    $oldEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
}
$daemon = $null
$proc = $null

try {
    $env:TEA_BIND_ADDR = "127.0.0.1:$Port"
    $env:TEA_AUTH_TOKEN = $AuthToken
    $env:TEA_STORE_PATH = $storePath
    $env:TEA_CONFIG_PATH = (Join-Path $storeDir "config.json")
    $env:TEA_LOOM_BASE_URL = ""
    $env:TEA_LOOM_AUTH_TOKEN = ""
    $env:TEA_SERVER_URL = $baseUrl

    Write-Host ">> starting tea-daemon at $baseUrl"
    $daemon = Start-Process `
        -FilePath $daemonExe `
        -WorkingDirectory $workingDirectory `
        -PassThru `
        -WindowStyle Hidden `
        -RedirectStandardOutput $daemonOut `
        -RedirectStandardError $daemonErr

    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    $healthy = $false
    do {
        $daemon.Refresh()
        if ($daemon.HasExited) {
            throw "tea-daemon exited before becoming healthy (exit code $($daemon.ExitCode))"
        }
        try {
            Invoke-RestMethod -Uri "$baseUrl/health" -TimeoutSec 1 | Out-Null
            $healthy = $true
            break
        } catch {
            Start-Sleep -Milliseconds 300
        }
    } while ((Get-Date) -lt $deadline)
    if (-not $healthy) { throw "tea-daemon did not become healthy at $baseUrl within $TimeoutSec seconds" }

    # Build a batch of JSON-RPC requests to drive over stdio.
    $requests = @(
        '[]'
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
        '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
        '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tea_create_ticket","arguments":{"title":"MCP smoke ticket","description":"Created through the Tea MCP server over stdio.","idempotency_key":"mcp-smoke-create-1"}}}'
        '{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"tea_create_ticket","arguments":{"title":"MCP smoke ticket","description":"Created through the Tea MCP server over stdio.","idempotency_key":"mcp-smoke-create-1"}}}'
        '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"tea_list_tickets","arguments":{"source":"human","limit":1}}}'
        '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"tea_edit_ticket","arguments":{"ticket_id":"not-called","description":123}}}'
    )
    $input = ($requests -join "`n") + "`n"

    Write-Host ">> driving tea-mcp over stdio"
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $mcpExe
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.WorkingDirectory = $workingDirectory
    $psi.EnvironmentVariables["TEA_SERVER_URL"] = $baseUrl
    $psi.EnvironmentVariables["TEA_AUTH_TOKEN"] = $AuthToken

    $proc = [System.Diagnostics.Process]::Start($psi)
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()
    # Write stdin as UTF-8 without a BOM so the first JSON-RPC line parses cleanly.
    $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
    $stdinWriter = [System.IO.StreamWriter]::new($proc.StandardInput.BaseStream, $utf8NoBom)
    $stdinWriter.Write($input)
    $stdinWriter.Flush()
    $stdinWriter.Close()
    if (-not $proc.WaitForExit($TimeoutSec * 1000)) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        throw "tea-mcp did not exit within $TimeoutSec seconds after stdin closed"
    }
    $stdout = $stdoutTask.GetAwaiter().GetResult()
    $stderr = $stderrTask.GetAwaiter().GetResult()
    if ($proc.ExitCode -ne 0) {
        throw "tea-mcp exited with code $($proc.ExitCode): $stderr"
    }

    Write-Host "--- MCP stdout ---"
    Write-Host $stdout

    $lines = @($stdout -split "`n" | Where-Object { $_.Trim() -ne "" })
    $responses = @($lines | ForEach-Object { $_ | ConvertFrom-Json })

    function Get-ResponseById([int]$id) {
        return $responses | Where-Object { $null -ne $_.id -and [int]$_.id -eq $id } | Select-Object -First 1
    }

    $invalidRequests = @($responses | Where-Object {
        $null -eq $_.id -and $_.error.code -eq -32600
    })
    if ($invalidRequests.Count -ne 1) {
        throw "malformed JSON-RPC shape did not return exactly one -32600 Invalid Request"
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
    $createReplay = Get-ResponseById 6
    if ($createReplay.result.isError) { throw "tea_create_ticket replay reported an error: $($createReplay.result.content[0].text)" }
    $replayed = $createReplay.result.content[0].text | ConvertFrom-Json
    if ([string]$replayed.id -ne [string]$created.id) {
        throw "tea_create_ticket idempotent replay returned another ticket id"
    }

    $list = Get-ResponseById 4
    $listText = $list.result.content[0].text
    $listPage = $listText | ConvertFrom-Json
    if (@($listPage.items).Count -ne 1) { throw "tea_list_tickets paged output did not contain exactly one item" }
    if ([string]$listPage.items[0].id -ne [string]$created.id) { throw "created ticket id not present in tea_list_tickets paged output" }
    if ($null -ne $listPage.next_cursor) { throw "single-ticket MCP page unexpectedly returned a continuation cursor" }

    $invalidParams = Get-ResponseById 5
    if (-not $invalidParams -or [int]$invalidParams.error.code -ne -32602) {
        throw "mistyped optional MCP argument did not return -32602 Invalid Params"
    }

    Write-Host "Tea MCP real smoke passed"
    Write-Host "tool_count=$toolCount"
    Write-Host "ticket_id=$($created.id)"
    Write-Host "idempotent_create_replayed=true"
    Write-Host "invalid_request_rejected=true"
    Write-Host "invalid_params_rejected=true"
} finally {
    Stop-SmokeProcess -Process $proc -Name "tea-mcp"
    Stop-SmokeProcess -Process $daemon -Name "tea-daemon"
    Wait-TcpPortReleased -Port $Port
    foreach ($name in $environmentNames) {
        $value = $oldEnvironment[$name]
        if ($null -eq $value) {
            Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
        } else {
            Set-Item -LiteralPath "Env:$name" -Value $value
        }
    }
    Remove-Item -Recurse -Force $storeDir -ErrorAction SilentlyContinue
}
