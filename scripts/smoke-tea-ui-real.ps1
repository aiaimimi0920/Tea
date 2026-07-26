[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PackageDir,
    [int]$Port = 0,
    [int]$DebugPort = 0,
    [string]$AuthToken = "tea-ui-smoke-token",
    [int]$TimeoutSec = 90,
    [string]$PlaywrightPackageRoot = "",
    [switch]$KeepArtifacts
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-FreeTcpPort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Parse("127.0.0.1"), 0)
    $listener.Start()
    try { return $listener.LocalEndpoint.Port }
    finally { $listener.Stop() }
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
        $pid = 0
        [void][int]::TryParse($parts[$parts.Count - 1], [ref]$pid)
        $listeners += [pscustomobject]@{
            local_endpoint = $localEndpoint
            local_port = $localPort
            pid = $pid
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

function Wait-TeaHealth {
    param(
        [string]$BaseUrl,
        [System.Diagnostics.Process]$Process,
        [int]$TimeoutSec
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    do {
        if ($null -ne $Process) {
            $Process.Refresh()
            if ($Process.HasExited) {
                throw "Tea daemon exited early with code $($Process.ExitCode) while waiting for $BaseUrl/health"
            }
        }
        try {
            Invoke-RestMethod -Uri "$BaseUrl/health" -TimeoutSec 1 | Out-Null
            return
        } catch {
            Start-Sleep -Milliseconds 300
        }
    } while ((Get-Date) -lt $deadline)

    throw "Tea daemon did not become healthy at $BaseUrl within $TimeoutSec seconds."
}

function Invoke-TeaApi {
    param(
        [string]$BaseUrl,
        [string]$Token,
        [string]$Path,
        [string]$Method = "GET",
        [object]$Body = $null
    )

    $headers = @{ Authorization = "Bearer $Token" }
    $args = @{
        Uri = "$BaseUrl$Path"
        Method = $Method
        Headers = $headers
        TimeoutSec = 5
    }
    if ($null -ne $Body) {
        $args["ContentType"] = "application/json"
        $args["Body"] = ($Body | ConvertTo-Json -Depth 12)
    }
    return Invoke-RestMethod @args
}

function Stop-ProcessTree {
    param(
        [AllowNull()]
        [System.Diagnostics.Process]$Process,
        [string]$Name
    )

    if ($null -eq $Process) { return $false }
    $Process.Refresh()
    if ($Process.HasExited) { return $true }

    $children = @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $($Process.Id)" -ErrorAction SilentlyContinue)
    foreach ($child in $children) {
        try {
            $childProcess = Get-Process -Id $child.ProcessId -ErrorAction SilentlyContinue
            if ($null -ne $childProcess) {
                [void](Stop-ProcessTree -Process $childProcess -Name "$Name-child")
            }
        } catch {}
    }

    Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
    try { $Process.WaitForExit(5000) | Out-Null } catch {}
    $Process.Refresh()
    return [bool]$Process.HasExited
}

function Write-Utf8NoBom {
    param(
        [string]$Path,
        [string]$Content
    )

    $encoding = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText($Path, $Content, $encoding)
}

$packagePath = (Resolve-Path -LiteralPath $PackageDir).Path
$daemonExe = Join-Path $packagePath "tea-daemon.exe"
$uiExe = Join-Path $packagePath "tea.exe"
if (-not (Test-Path -LiteralPath $daemonExe -PathType Leaf)) { throw "Missing tea-daemon.exe in $packagePath" }
if (-not (Test-Path -LiteralPath $uiExe -PathType Leaf)) { throw "Missing tea.exe UI executable in $packagePath" }

if ($Port -le 0) { $Port = Get-FreeTcpPort }
if ($DebugPort -le 0) {
    do {
        $DebugPort = Get-FreeTcpPort
    } while ($DebugPort -eq $Port)
}

Assert-NoPreexistingPortListeners -Ports @($Port, $DebugPort)

$baseUrl = "http://127.0.0.1:$Port"
$cdpUrl = "http://127.0.0.1:$DebugPort"
$runId = Get-Date -Format "yyyyMMdd-HHmmss"
$artifactRoot = Join-Path $env:TEMP "tea-ui-smoke-$runId-$Port"
New-Item -ItemType Directory -Force -Path $artifactRoot | Out-Null
$storePath = Join-Path $artifactRoot "tea.sqlite"
$configPath = Join-Path $artifactRoot "config.json"
$daemonOut = Join-Path $artifactRoot "tea-daemon.out.log"
$daemonErr = Join-Path $artifactRoot "tea-daemon.err.log"
$uiSmokeScript = Join-Path $artifactRoot "tea-ui-smoke.mjs"
$resultPath = Join-Path $artifactRoot "tea-ui-smoke-result.json"
$webview2UserDataDir = Join-Path $artifactRoot "webview2-user-data"
New-Item -ItemType Directory -Force -Path $webview2UserDataDir | Out-Null

$oldServerUrl = $env:TEA_SERVER_URL
$oldAuthToken = $env:TEA_AUTH_TOKEN
$oldStorePath = $env:TEA_STORE_PATH
$oldConfigPath = $env:TEA_CONFIG_PATH
$oldAdditionalArgs = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
$oldUserDataFolder = $env:WEBVIEW2_USER_DATA_FOLDER
$oldCdpUrl = $env:TEA_UI_TAURI_CDP_URL
$oldResultPath = $env:TEA_UI_TAURI_RESULT_PATH
$oldTimeoutMs = $env:TEA_UI_TAURI_TIMEOUT_MS
$oldPlaywrightPackageRoot = $env:PLAYWRIGHT_PACKAGE_ROOT
$daemonProcess = $null
$uiProcess = $null

$uiSmokeSource = @'
import { createRequire } from "node:module";
import { writeFile } from "node:fs/promises";
import path from "node:path";

const playwrightRoot = process.env.PLAYWRIGHT_PACKAGE_ROOT;
if (!playwrightRoot) throw new Error("PLAYWRIGHT_PACKAGE_ROOT is required");
const requireFromPlaywrightRoot = createRequire(path.join(playwrightRoot, "package.json"));
const { chromium } = requireFromPlaywrightRoot("playwright-core");

const cdpUrl = process.env.TEA_UI_TAURI_CDP_URL;
const resultPath = process.env.TEA_UI_TAURI_RESULT_PATH;
const timeoutMs = Number(process.env.TEA_UI_TAURI_TIMEOUT_MS || "90000");

if (!cdpUrl || !resultPath) {
  throw new Error("TEA_UI_TAURI_CDP_URL and TEA_UI_TAURI_RESULT_PATH are required");
}

const consoleMessages = [];
const pageErrors = [];
const pageStates = [];
const seenPages = new Set();

const writeResult = async (result) => {
  await writeFile(resultPath, JSON.stringify(result, null, 2), "utf8");
};

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const trackPage = (page) => {
  if (seenPages.has(page)) return;
  seenPages.add(page);
  page.on("console", (message) => {
    consoleMessages.push({ type: message.type(), text: message.text() });
  });
  page.on("pageerror", (error) => {
    pageErrors.push(error instanceof Error ? error.message : String(error));
  });
};

const attachedLocatorOnAnyPage = async (browser, selector, timeout) => {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const pages = browser.contexts().flatMap((context) => context.pages());
    for (const page of pages) {
      if (page.isClosed()) continue;
      trackPage(page);
      pageStates.push({
        at: new Date().toISOString(),
        url: page.url(),
        title: await page.title().catch(() => ""),
      });
      try {
        const locator = page.locator(selector);
        await locator.waitFor({ state: "attached", timeout: 500 });
        return { page, locator };
      } catch {
      }
    }
    await delay(300);
  }
  throw new Error(`Timed out waiting for attached selector ${selector}`);
};

let browser = null;
try {
  browser = await chromium.connectOverCDP(cdpUrl);
  for (const context of browser.contexts()) {
    context.on("page", trackPage);
    for (const page of context.pages()) {
      trackPage(page);
    }
  }

  const { page } = await attachedLocatorOnAnyPage(browser, 'main.issue-shell', timeoutMs);
  const nativeTauriRuntime = await page.evaluate(() => Boolean(window.__TAURI_INTERNALS__));
  if (!nativeTauriRuntime) {
    throw new Error("Tea WebView did not expose the native Tauri runtime");
  }

  await page.getByText("Tea UI smoke").first().click();
  // Local notes are a desktop-only, additive overlay (never sent to the daemon).
  // The toggle button reads "Local notes"; the editor submit is "Add note".
  await page.getByRole("button", { name: "Local notes", exact: true }).click();
  await page.getByPlaceholder("Add a local note").fill("smoke-ui-label");
  await page.getByRole("button", { name: "Add note", exact: true }).click();
  await page.getByText("smoke-ui-label").first().waitFor({ state: "visible", timeout: timeoutMs });

  // Filters use the union of daemon labels and local notes, so a local note is
  // still a selectable label filter option.
  await page.getByRole("button", { name: /Labels/ }).click();
  await page.getByText("Label filters").waitFor({ state: "visible", timeout: timeoutMs });
  await page.getByRole("button", { name: "smoke-ui-label", exact: true }).click();
  await page.getByRole("button", { name: /Labels \(smoke-ui-label\)/ }).waitFor({ state: "visible", timeout: timeoutMs });
  await page.getByRole("button", { name: "smoke-ui-label", exact: true }).evaluate((element) => {
    if (element.getAttribute("aria-pressed") !== "true") {
      throw new Error("smoke-ui-label filter button was not pressed");
    }
  });
  await page.getByRole("button", { name: "Clear filter", exact: true }).click();
  await page.getByRole("button", { name: /^Labels\s*$/ }).waitFor({ state: "visible", timeout: timeoutMs });

  // Reopen the notes editor and remove the note, then clear all notes.
  await page.getByRole("button", { name: "Hide local notes" }).click();
  await page.getByRole("button", { name: "Local notes", exact: true }).click();
  await page.getByRole("button", { name: /Remove local note smoke-ui-label/ }).click();
  await page.getByRole("button", { name: "Clear notes" }).click();

  const labelSummary = await page.locator(".label-stack").first().innerText().catch(() => "");

  await writeResult({
    status: "passed",
    native_tauri_runtime: nativeTauriRuntime,
    labelSummary,
    pageUrl: page.url(),
    pageTitle: await page.title().catch(() => ""),
    pageStates,
    consoleMessages,
    pageErrors,
  });
} catch (error) {
  await writeResult({
    status: "failed",
    error: error instanceof Error ? error.message : String(error),
    pageStates,
    consoleMessages,
    pageErrors,
  });
  throw error;
} finally {
  if (browser) {
    await browser.close().catch(() => {});
  }
}
'@

Write-Utf8NoBom -Path $uiSmokeScript -Content $uiSmokeSource

try {
    $daemonProcess = Start-Process -FilePath $daemonExe `
        -ArgumentList @("--bind-addr", "127.0.0.1:$Port", "--auth-token", $AuthToken, "--store-path", $storePath, "--config-path", $configPath) `
        -PassThru `
        -WindowStyle Hidden `
        -RedirectStandardOutput $daemonOut `
        -RedirectStandardError $daemonErr

    Wait-TeaHealth -BaseUrl $baseUrl -Process $daemonProcess -TimeoutSec $TimeoutSec
    $status = Invoke-TeaApi -BaseUrl $baseUrl -Token $AuthToken -Path "/v1/status"
    $ticket = Invoke-TeaApi -BaseUrl $baseUrl -Token $AuthToken -Path "/v1/tickets" -Method "POST" -Body @{
        title = "Tea UI smoke"
        description = "Created before launching tea.exe UI mode so the UI has real local data."
    }
    Invoke-TeaApi -BaseUrl $baseUrl -Token $AuthToken -Path "/v1/tickets/$($ticket.id)/comments" -Method "POST" -Body @{
        body = "Tea UI smoke comment for timeline coverage."
    } | Out-Null

    $env:TEA_SERVER_URL = $baseUrl
    $env:TEA_AUTH_TOKEN = $AuthToken
    $env:TEA_STORE_PATH = $storePath
    $env:TEA_CONFIG_PATH = $configPath
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$DebugPort --remote-allow-origins=*"
    $env:WEBVIEW2_USER_DATA_FOLDER = $webview2UserDataDir
    $env:TEA_UI_TAURI_CDP_URL = $cdpUrl
    $env:TEA_UI_TAURI_RESULT_PATH = $resultPath
    $env:TEA_UI_TAURI_TIMEOUT_MS = [string]($TimeoutSec * 1000)

    $uiProcess = Start-Process -FilePath $uiExe -PassThru
    Start-Sleep -Seconds 6
    $uiProcess.Refresh()
    if ($uiProcess.HasExited) {
        throw "tea.exe UI process exited early with code $($uiProcess.ExitCode)."
    }

    if ([string]::IsNullOrWhiteSpace($PlaywrightPackageRoot)) {
        $repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
        $localDesktopRoot = Join-Path $repoRoot "apps\desktop"
        $legacyGatewayScriptsRoot = "C:\Users\Public\nas_home\AI\GameEditor\Neuro\Gateway\scripts"
        if (-not [string]::IsNullOrWhiteSpace($env:PLAYWRIGHT_PACKAGE_ROOT)) {
            $PlaywrightPackageRoot = $env:PLAYWRIGHT_PACKAGE_ROOT
        } elseif (Test-Path -LiteralPath (Join-Path $localDesktopRoot "node_modules\playwright-core\package.json") -PathType Leaf) {
            $PlaywrightPackageRoot = $localDesktopRoot
        } elseif (Test-Path -LiteralPath (Join-Path $legacyGatewayScriptsRoot "node_modules\playwright-core\package.json") -PathType Leaf) {
            $PlaywrightPackageRoot = $legacyGatewayScriptsRoot
        }
    }
    if ([string]::IsNullOrWhiteSpace($PlaywrightPackageRoot)) {
        throw "Tea UI smoke requires playwright-core. Run npm install in apps\desktop or pass -PlaywrightPackageRoot <directory containing package.json and node_modules\playwright-core>."
    }
    $env:PLAYWRIGHT_PACKAGE_ROOT = (Resolve-Path -LiteralPath $PlaywrightPackageRoot).Path
    node $uiSmokeScript
    if ($LASTEXITCODE -ne 0) {
        throw "Tea Tauri UI smoke browser script failed with exit code $LASTEXITCODE"
    }

    if (-not (Test-Path -LiteralPath $resultPath -PathType Leaf)) {
        throw "Tea Tauri UI smoke result was not written: $resultPath"
    }

    $result = Get-Content -Raw -LiteralPath $resultPath | ConvertFrom-Json
    if ($result.status -ne "passed") {
        throw "Tea Tauri UI smoke failed: $($result.error)"
    }

    [ordered]@{
        status = "passed"
        packageDir = $packagePath
        baseUrl = $baseUrl
        cdpUrl = $cdpUrl
        daemonPid = $daemonProcess.Id
        uiPid = $uiProcess.Id
        ticketId = $ticket.id
        storeBackend = $status.store.backend
        native_tauri_runtime = [bool]$result.native_tauri_runtime
        artifactRoot = $artifactRoot
        resultPath = $resultPath
    } | ConvertTo-Json -Depth 8
}
finally {
    $env:TEA_SERVER_URL = $oldServerUrl
    $env:TEA_AUTH_TOKEN = $oldAuthToken
    $env:TEA_STORE_PATH = $oldStorePath
    $env:TEA_CONFIG_PATH = $oldConfigPath
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $oldAdditionalArgs
    $env:WEBVIEW2_USER_DATA_FOLDER = $oldUserDataFolder
    $env:TEA_UI_TAURI_CDP_URL = $oldCdpUrl
    $env:TEA_UI_TAURI_RESULT_PATH = $oldResultPath
    $env:TEA_UI_TAURI_TIMEOUT_MS = $oldTimeoutMs
    $env:PLAYWRIGHT_PACKAGE_ROOT = $oldPlaywrightPackageRoot

    if ($uiProcess -ne $null) {
        [void](Stop-ProcessTree -Process $uiProcess -Name "tea-ui")
    }
    if ($daemonProcess -ne $null) {
        [void](Stop-ProcessTree -Process $daemonProcess -Name "tea-daemon")
    }
    if (-not $KeepArtifacts) {
        Remove-Item -LiteralPath $artifactRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
