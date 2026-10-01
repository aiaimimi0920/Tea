[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PackageDir,
    [int]$Port = 0,
    [int]$DebugPort = 0,
    [string]$AuthToken = "tea-ui-smoke-token",
    [int]$TimeoutSec = 90,
    [ValidateRange(320, 640)]
    [int]$NarrowViewportWidth = 375,
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
$ProxyPort = Get-FreeTcpPort
while ($ProxyPort -eq $Port -or $ProxyPort -eq $DebugPort) {
    $ProxyPort = Get-FreeTcpPort
}

Assert-NoPreexistingPortListeners -Ports @($Port, $DebugPort, $ProxyPort)

$baseUrl = "http://127.0.0.1:$Port"
$cdpUrl = "http://127.0.0.1:$DebugPort"
$proxyUrl = "http://127.0.0.1:$ProxyPort"
$runId = Get-Date -Format "yyyyMMdd-HHmmss"
$tempRoot = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd("\", "/")
$artifactRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $tempRoot "tea-ui-smoke-$runId-$Port")
)
$tempPrefix = $tempRoot + [System.IO.Path]::DirectorySeparatorChar
if (-not $artifactRoot.StartsWith($tempPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to create UI smoke artifacts outside the system temp directory: $artifactRoot"
}
New-Item -ItemType Directory -Force -Path $artifactRoot | Out-Null
$storePath = Join-Path $artifactRoot "tea.sqlite"
$configPath = Join-Path $artifactRoot "config.json"
$daemonOut = Join-Path $artifactRoot "tea-daemon.out.log"
$daemonErr = Join-Path $artifactRoot "tea-daemon.err.log"
$uiOut = Join-Path $artifactRoot "tea-ui.out.log"
$uiErr = Join-Path $artifactRoot "tea-ui.err.log"
$browserOut = Join-Path $artifactRoot "tea-ui-browser.out.log"
$browserErr = Join-Path $artifactRoot "tea-ui-browser.err.log"
$proxyOut = Join-Path $artifactRoot "tea-ui-proxy.out.log"
$proxyErr = Join-Path $artifactRoot "tea-ui-proxy.err.log"
$uiSmokeScript = Join-Path $artifactRoot "tea-ui-smoke.mjs"
$uiProxyScript = Join-Path $artifactRoot "tea-ui-proxy.mjs"
$resultPath = Join-Path $artifactRoot "tea-ui-smoke-result.json"
$webview2UserDataDir = Join-Path $artifactRoot "webview2-user-data"
New-Item -ItemType Directory -Force -Path $webview2UserDataDir | Out-Null

$oldServerUrl = $env:TEA_SERVER_URL
$oldAuthToken = $env:TEA_AUTH_TOKEN
$oldStorePath = $env:TEA_STORE_PATH
$oldConfigPath = $env:TEA_CONFIG_PATH
$oldAdditionalArgs = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
$oldUserDataFolder = $env:WEBVIEW2_USER_DATA_FOLDER
# WebView2 Runtime 150+ intentionally ignores WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
# (and HKCU policy) when the host process is elevated. Elevated smoke runs must
# mirror the arguments through the HKLM policy value, which elevated hosts honor.
$webviewPolicyKeyPath = "HKLM:\SOFTWARE\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments"
$webviewPolicyValueName = "tea.exe"
$webviewPolicyApplied = $false
$webviewPolicyKeyCreated = $false
$webviewPolicyPreviousValue = $null
$oldCdpUrl = $env:TEA_UI_TAURI_CDP_URL
$oldResultPath = $env:TEA_UI_TAURI_RESULT_PATH
$oldTimeoutMs = $env:TEA_UI_TAURI_TIMEOUT_MS
$oldNarrowViewportWidth = $env:TEA_UI_TAURI_NARROW_WIDTH
$oldProxyUrl = $env:TEA_UI_TAURI_PROXY_URL
$oldProxyPort = $env:TEA_UI_PROXY_PORT
$oldProxyUpstream = $env:TEA_UI_PROXY_UPSTREAM
$oldPlaywrightPackageRoot = $env:PLAYWRIGHT_PACKAGE_ROOT
$daemonProcess = $null
$uiProcess = $null
$browserProcess = $null
$proxyProcess = $null
$smokePassed = $false

$uiProxySource = @'
import http from "node:http";

const host = "127.0.0.1";
const port = Number.parseInt(process.env.TEA_UI_PROXY_PORT ?? "", 10);
const upstream = process.env.TEA_UI_PROXY_UPSTREAM;
const targetTitle = "Tea UI uncertain create retry";
const bundlePathPattern = /^\/v1\/tickets\/[^/]+\/bundle$/;
const state = {
  requestKeys: [],
  responseDropped: false,
  failNextBundle: false,
  bundleFailuresInjected: 0,
  bundleFailuresServed: 0,
};
if (!Number.isInteger(port) || port < 1 || port > 65535 || !upstream) {
  throw new Error("TEA_UI_PROXY_PORT and TEA_UI_PROXY_UPSTREAM are required");
}

const readBody = (request) => new Promise((resolve, reject) => {
  const chunks = [];
  let length = 0;
  request.on("data", (chunk) => {
    length += chunk.length;
    if (length > 2 * 1024 * 1024) {
      reject(new Error("proxy request body exceeded 2 MiB"));
      request.destroy();
      return;
    }
    chunks.push(chunk);
  });
  request.on("end", () => resolve(Buffer.concat(chunks)));
  request.on("error", reject);
});

const sendJson = (response, status, payload) => {
  const body = JSON.stringify(payload);
  response.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(body),
    "connection": "close",
  });
  response.end(body);
};

const server = http.createServer(async (request, response) => {
  try {
    const url = new URL(request.url ?? "/", `http://${host}:${port}`);
    if (request.method === "GET" && url.pathname === "/__smoke_state") {
      sendJson(response, 200, state);
      return;
    }
    if (request.method === "POST" && url.pathname === "/__fail_next_bundle") {
      state.failNextBundle = true;
      state.bundleFailuresInjected += 1;
      sendJson(response, 200, state);
      return;
    }
    if (
      request.method === "GET" &&
      state.failNextBundle &&
      bundlePathPattern.test(url.pathname)
    ) {
      state.failNextBundle = false;
      state.bundleFailuresServed += 1;
      sendJson(response, 503, { error: "injected transient bundle failure" });
      return;
    }

    const body = await readBody(request);
    const headers = {};
    for (const [name, value] of Object.entries(request.headers)) {
      if (value == null || ["host", "connection", "content-length", "accept-encoding"].includes(name)) continue;
      headers[name] = Array.isArray(value) ? value.join(", ") : value;
    }
    const upstreamResponse = await fetch(new URL(url.pathname + url.search, upstream), {
      method: request.method,
      headers,
      body: body.length > 0 ? body : undefined,
      redirect: "manual",
    });
    const responseBody = Buffer.from(await upstreamResponse.arrayBuffer());

    let isTargetCreate = false;
    if (request.method === "POST" && url.pathname === "/v1/tickets") {
      const parsedBody = JSON.parse(body.toString("utf8"));
      isTargetCreate = parsedBody?.title === targetTitle;
      if (isTargetCreate) {
        state.requestKeys.push(request.headers["idempotency-key"] ?? null);
      }
    }
    if (isTargetCreate && !state.responseDropped) {
      state.responseDropped = true;
      response.destroy();
      return;
    }

    const responseHeaders = {};
    const contentType = upstreamResponse.headers.get("content-type");
    if (contentType) responseHeaders["content-type"] = contentType;
    responseHeaders["content-length"] = responseBody.length;
    responseHeaders["connection"] = "close";
    response.writeHead(upstreamResponse.status, responseHeaders);
    response.end(responseBody);
  } catch (error) {
    if (!response.headersSent && !response.destroyed) {
      sendJson(response, 502, { error: error instanceof Error ? error.message : String(error) });
    } else {
      response.destroy();
    }
  }
});

server.listen(port, host, () => {
  process.stdout.write(`Tea UI response-loss proxy listening on http://${host}:${port}\n`);
});
'@

$uiSmokeSource = @'
import { createRequire } from "node:module";
import { writeFile } from "node:fs/promises";
import path from "node:path";
import { verifyCompletionReview } from "./tea-completion-review-smoke.mjs";

const playwrightRoot = process.env.PLAYWRIGHT_PACKAGE_ROOT;
if (!playwrightRoot) throw new Error("PLAYWRIGHT_PACKAGE_ROOT is required");
const requireFromPlaywrightRoot = createRequire(path.join(playwrightRoot, "package.json"));
const { chromium } = requireFromPlaywrightRoot("playwright-core");

const cdpUrl = process.env.TEA_UI_TAURI_CDP_URL;
const resultPath = process.env.TEA_UI_TAURI_RESULT_PATH;
const proxyUrl = process.env.TEA_UI_TAURI_PROXY_URL;
const timeoutMs = Number(process.env.TEA_UI_TAURI_TIMEOUT_MS || "90000");
const narrowViewportWidth = Number(process.env.TEA_UI_TAURI_NARROW_WIDTH || "375");

if (!cdpUrl || !resultPath || !proxyUrl) {
  throw new Error("TEA_UI_TAURI_CDP_URL, TEA_UI_TAURI_RESULT_PATH, and TEA_UI_TAURI_PROXY_URL are required");
}
if (!Number.isInteger(narrowViewportWidth) || narrowViewportWidth < 320 || narrowViewportWidth > 640) {
  throw new Error(`Invalid Tea narrow viewport width: ${narrowViewportWidth}`);
}

const screenshotRoot = path.dirname(resultPath);
const wideScreenshotPath = path.join(screenshotRoot, "tea-ui-wide.png");
const narrowScreenshotPath = path.join(screenshotRoot, "tea-ui-narrow.png");

const consoleMessages = [];
const pageErrors = [];
const pageStates = [];
const progress = [];
const seenPages = new Set();

const writeResult = async (result) => {
  await writeFile(resultPath, JSON.stringify(result, null, 2), "utf8");
};

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const markProgress = (phase) => {
  const record = { at: new Date().toISOString(), phase };
  progress.push(record);
  console.log(`[tea-ui-smoke] ${record.at} ${phase}`);
};

const inspectNarrowLayout = async (page) => page.evaluate(() => {
  const clientWidth = document.documentElement.clientWidth;
  const describe = (element) => {
    const rect = element.getBoundingClientRect();
    return {
      tag: element.tagName.toLowerCase(),
      id: element.id,
      className: typeof element.className === "string" ? element.className : "",
      left: Math.round(rect.left),
      right: Math.round(rect.right),
      width: Math.round(rect.width),
    };
  };
  const isInsideHorizontalScroller = (element) => {
    for (let ancestor = element.parentElement; ancestor && ancestor !== document.body; ancestor = ancestor.parentElement) {
      const overflowX = getComputedStyle(ancestor).overflowX;
      if (
        (overflowX === "auto" || overflowX === "scroll") &&
        ancestor.scrollWidth > ancestor.clientWidth + 1
      ) {
        return true;
      }
    }
    return false;
  };
  const overflowingElements = [...document.querySelectorAll("body *")]
    .filter((element) => !isInsideHorizontalScroller(element))
    .map(describe)
    .filter(({ left, right, width }) => width > 0 && (left < -1 || right > clientWidth + 1))
    .slice(0, 20);
  const detailElements = [
    ...document.querySelectorAll(
      ".conversation-header > *, .conversation-stream-header > *, .conversation-filter-tabs, " +
      ".issue-comment-header > *, .issue-comment-header a, .issue-comment-header button",
    ),
  ];
  const clippedDetailElements = detailElements
    .map((element) => {
      let container = element.parentElement;
      if (element.matches(".conversation-filter-tabs")) {
        container = element.closest(".conversation-stream");
      } else if (element.matches(".issue-comment-header > *, .issue-comment-header a, .issue-comment-header button")) {
        container = element.closest(".issue-comment");
      }
      if (!container) return null;
      const rect = element.getBoundingClientRect();
      const containerRect = container.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) return null;
      if (rect.left >= containerRect.left - 1 && rect.right <= containerRect.right + 1) return null;
      return {
        ...describe(element),
        containerClassName: typeof container.className === "string" ? container.className : "",
        containerLeft: Math.round(containerRect.left),
        containerRight: Math.round(containerRect.right),
      };
    })
    .filter(Boolean)
    .slice(0, 20);
  return {
    clientWidth,
    scrollWidth: document.documentElement.scrollWidth,
    overflowingElements,
    clippedDetailElements,
  };
});

const inspectAccessibilityContracts = async (page) => page.evaluate(() => {
  const activitySummaryInteractiveDescendants = document.querySelectorAll(
    ".activity-log-summary button, .activity-log-summary a[href], .activity-log-summary input, " +
    ".activity-log-summary select, .activity-log-summary textarea, " +
    ".activity-log-summary [tabindex]:not([tabindex='-1'])",
  ).length;
  const issueItems = [...document.querySelectorAll(".issue-item")];
  const issueItemsWithOverriddenRole = issueItems.filter((element) => {
    const role = element.getAttribute("role");
    return element.tagName !== "BUTTON" || (role !== null && role !== "button");
  }).length;
  const issueItemContextInvalidTags = issueItems.filter((element) =>
    [...element.querySelectorAll(".issue-item-context")].some((context) => context.tagName !== "SPAN")
  ).length;
  return {
    activitySummaryInteractiveDescendants,
    issueItemCount: issueItems.length,
    issueItemsWithOverriddenRole,
    issueItemContextInvalidTags,
  };
});

const assertNarrowLayout = (layout, phase) => {
  if (layout.scrollWidth > layout.clientWidth + 1) {
    throw new Error(
      `Tea UI overflows a narrow viewport during ${phase}: clientWidth=${layout.clientWidth}, ` +
      `scrollWidth=${layout.scrollWidth}, elements=${JSON.stringify(layout.overflowingElements)}`,
    );
  }
  if (layout.clippedDetailElements.length > 0) {
    throw new Error(
      `Tea detail actions are clipped during ${phase}: ${JSON.stringify(layout.clippedDetailElements)}`,
    );
  }
};

const connectToTauri = async () => {
  const deadline = Date.now() + timeoutMs;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      return await chromium.connectOverCDP(cdpUrl);
    } catch (error) {
      lastError = error;
      await delay(300);
    }
  }
  throw new Error(
    `Timed out connecting to Tea WebView CDP at ${cdpUrl}: ${lastError instanceof Error ? lastError.message : String(lastError)}`,
  );
};

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
  markProgress("connect-cdp:start");
  browser = await connectToTauri();
  markProgress("connect-cdp:complete");
  for (const context of browser.contexts()) {
    context.on("page", trackPage);
    for (const page of context.pages()) {
      trackPage(page);
    }
  }

  markProgress("attach-main:start");
  const { page } = await attachedLocatorOnAnyPage(browser, 'main.issue-shell', timeoutMs);
  markProgress("attach-main:complete");
  const nativeTauriRuntime = await page.evaluate(() => Boolean(window.__TAURI_INTERNALS__));
  if (!nativeTauriRuntime) {
    throw new Error("Tea WebView did not expose the native Tauri runtime");
  }
  const accessibility = await inspectAccessibilityContracts(page);
  if (accessibility.activitySummaryInteractiveDescendants !== 0) {
    throw new Error("Activity log summary contains a nested interactive control");
  }
  if (accessibility.issueItemCount === 0 || accessibility.issueItemsWithOverriddenRole !== 0) {
    throw new Error(`Tea issue buttons have invalid overridden roles: ${JSON.stringify(accessibility)}`);
  }
  if (accessibility.issueItemContextInvalidTags !== 0) {
    throw new Error(`Tea issue buttons contain non-phrasing context elements: ${JSON.stringify(accessibility)}`);
  }

  const initialIssueButton = page.locator(".issue-item").filter({ hasText: "Tea UI smoke" }).first();
  await initialIssueButton.focus();
  await initialIssueButton.press("Enter");
  await page.locator(".timeline-entry-link").first().waitFor({ state: "visible", timeout: timeoutMs });
  markProgress("timeline:ready");

  // Mutation guards must reject same-turn duplicate submissions before React
  // has rendered the disabled state back into the WebView.
  const duplicateCommentBody = "Tea UI mutation gate comment";
  const commentEditor = page.locator("form.comment-editor");
  await commentEditor.locator("textarea").fill(duplicateCommentBody);
  await commentEditor.locator("button[type='submit']").evaluate((button) => {
    button.click();
    button.click();
  });
  const duplicateCommentDeadline = Date.now() + timeoutMs;
  let duplicateCommentCount = 0;
  while (Date.now() < duplicateCommentDeadline) {
    duplicateCommentCount = await page.evaluate(async (body) => {
      const ticketsResponse = await window.__TAURI_INTERNALS__.invoke("tea_request", {
        method: "GET",
        path: "/v1/tickets",
        body: null,
        baseUrl: null,
        authToken: null,
      });
      const tickets = Array.isArray(ticketsResponse) ? ticketsResponse : (ticketsResponse?.items ?? []);
      const ticket = tickets.find((candidate) => candidate?.title === "Tea UI smoke");
      if (!ticket?.id) return 0;
      const comments = await window.__TAURI_INTERNALS__.invoke("tea_request", {
        method: "GET",
        path: `/v1/tickets/${encodeURIComponent(ticket.id)}/comments`,
        body: null,
        baseUrl: null,
        authToken: null,
      });
      return Array.isArray(comments) ? comments.filter((comment) => comment?.body === body).length : 0;
    }, duplicateCommentBody);
    if (duplicateCommentCount > 0) break;
    await delay(100);
  }
  if (duplicateCommentCount !== 1) {
    throw new Error(`Same-turn Tea comment submitted ${duplicateCommentCount} records instead of exactly one`);
  }
  markProgress("mutation-guard:comment-complete");

  // A transient bundle failure during auto/manual refresh must not erase the
  // last valid conversation, runs, analysis, or plan. The error remains visible
  // until the next successful refresh, which then clears it without reselecting
  // the ticket or rebuilding the page layout.
  const autoRefreshToggle = page.locator(".auto-refresh-toggle");
  if ((await autoRefreshToggle.getAttribute("aria-pressed")) === "true") {
    await autoRefreshToggle.click();
  }
  const refreshButton = page.locator(".refresh-control .ghost-button");
  const refreshReadyDeadline = Date.now() + timeoutMs;
  while (await refreshButton.isDisabled()) {
    if (Date.now() >= refreshReadyDeadline) {
      throw new Error("Tea refresh button did not become ready for the bundle failure probe");
    }
    await delay(50);
  }
  const preservedComment = page.getByText("Tea UI smoke comment for timeline coverage.", { exact: true });
  await preservedComment.waitFor({ state: "visible", timeout: timeoutMs });
  const entryCountBeforeFailure = await page.locator(".conversation-entry").count();
  const injectResponse = await fetch(`${proxyUrl}/__fail_next_bundle`, { method: "POST" });
  if (!injectResponse.ok) {
    throw new Error(`Tea proxy rejected the bundle failure probe with HTTP ${injectResponse.status}`);
  }
  await refreshButton.click();
  const detailErrorBanner = page.locator(".runtime-error-banner").filter({
    hasText: "Failed to read ticket",
  });
  await detailErrorBanner.waitFor({ state: "visible", timeout: timeoutMs });
  const bundleFailureState = await fetch(`${proxyUrl}/__smoke_state`).then((response) => response.json());
  const entryCountAfterFailure = await page.locator(".conversation-entry").count();
  const staleConversationPreserved = await preservedComment.isVisible().catch(() => false);
  if (
    bundleFailureState.bundleFailuresInjected !== 1 ||
    bundleFailureState.bundleFailuresServed !== 1
  ) {
    throw new Error(`Tea proxy did not inject exactly one bundle failure: ${JSON.stringify(bundleFailureState)}`);
  }
  if (!staleConversationPreserved || entryCountAfterFailure < entryCountBeforeFailure) {
    throw new Error(
      `Tea erased valid detail after a transient bundle failure: before=${entryCountBeforeFailure} ` +
      `after=${entryCountAfterFailure} preserved=${staleConversationPreserved}`,
    );
  }
  await refreshButton.click();
  await detailErrorBanner.waitFor({ state: "hidden", timeout: timeoutMs });
  await preservedComment.waitFor({ state: "visible", timeout: timeoutMs });
  const detailRefreshFailure = {
    bundleFailuresInjected: bundleFailureState.bundleFailuresInjected,
    bundleFailuresServed: bundleFailureState.bundleFailuresServed,
    entryCountBeforeFailure,
    entryCountAfterFailure,
    staleConversationPreserved,
    recoveredWithoutReselection: true,
  };
  markProgress("detail-refresh:transient-failure-recovered");

  await page.setViewportSize({ width: 1440, height: 900 });
  await page.locator(".issue-detail").evaluate((element) => element.scrollIntoView({ block: "start" }));
  await page.screenshot({ path: wideScreenshotPath });
  markProgress("wide-screenshot:complete");

  await page.setViewportSize({ width: narrowViewportWidth, height: 812 });
  await delay(150);
  await page.locator(".issue-detail").evaluate((element) => element.scrollIntoView({ block: "start" }));
  const narrowViewport = await inspectNarrowLayout(page);
  assertNarrowLayout(narrowViewport, "initial timeline rendering");
  await page.screenshot({ path: narrowScreenshotPath });
  markProgress("narrow-layout:initial-complete");

  await page.locator(".timeline-entry-link").first().click();
  await page.locator(".timeline-entry-link.copied").first().waitFor({ state: "visible", timeout: timeoutMs });
  await delay(50);
  const copiedLinkNarrowViewport = await inspectNarrowLayout(page);
  assertNarrowLayout(copiedLinkNarrowViewport, "copied-link rendering");
  markProgress("narrow-layout:copied-link-complete");

  await page.setViewportSize({ width: 1180, height: 780 });

  const localeToggle = page.getByTestId("locale-toggle");
  const initialLocaleLabel = (await localeToggle.textContent())?.trim();
  await localeToggle.click();
  const switchedLocaleLabel = (await localeToggle.textContent())?.trim();
  if (!initialLocaleLabel || !switchedLocaleLabel || initialLocaleLabel === switchedLocaleLabel) {
    throw new Error("locale toggle did not change the active language");
  }
  await localeToggle.click();

  await page.getByTestId("execution-provider").evaluate((element) => {
    if (element.getAttribute("data-provider") !== "mock") {
      throw new Error("standalone UI smoke did not report the mock execution provider");
    }
  });

  // Local notes are a desktop-only, additive overlay (never sent to the daemon).
  await page.getByTestId("local-notes-toggle").click();
  await page.getByTestId("local-notes-input").fill("smoke-ui-label");
  await page.getByTestId("local-notes-add").click();
  await page.getByText("smoke-ui-label").first().waitFor({ state: "visible", timeout: timeoutMs });

  // Filters use the union of daemon labels and local notes, so a local note is
  // still a selectable label filter option.
  await page.getByTestId("label-filter-toggle").click();
  const labelFilter = page.getByTestId("label-filter-option-smoke-ui-label");
  await labelFilter.waitFor({ state: "visible", timeout: timeoutMs });
  await labelFilter.click();
  await labelFilter.evaluate((element) => {
    if (element.getAttribute("aria-pressed") !== "true") {
      throw new Error("smoke-ui-label filter button was not pressed");
    }
  });
  await page.getByTestId("label-filter-clear").click();
  await page.getByTestId("label-filter-toggle").evaluate((element) => {
    if (element.getAttribute("aria-expanded") !== "false") {
      throw new Error("label filter panel did not close after clearing the filter");
    }
  });

  // Reopen the notes editor and remove its final note. Empty ticket entries are
  // compacted immediately, so the redundant clear action must become disabled.
  await page.getByTestId("local-notes-toggle").click();
  await page.getByTestId("local-notes-toggle").click();
  await page.getByTestId("local-note-remove-smoke-ui-label").click();
  const clearNotesButton = page.getByTestId("local-notes-clear");
  await clearNotesButton.evaluate((element) => {
    if (!(element instanceof HTMLButtonElement) || !element.disabled) {
      throw new Error("local notes clear action remained enabled after removing the final note");
    }
  });
  await page.getByTestId("local-notes-toggle").click();
  await page.getByTestId("local-notes-toggle").evaluate((element) => {
    if (element.getAttribute("aria-expanded") !== "false") {
      throw new Error("local notes editor did not close after removing the final note");
    }
  });

  const labelSummary = await page.locator(".label-stack").first().innerText().catch(() => "");

  // Trigger two submissions in one JavaScript turn. React state alone cannot
  // prevent this; the synchronous in-flight gate must allow exactly one POST.
  const duplicateSubmitTitle = "Tea UI duplicate submit guard";
  await page.locator(".repo-actions > button.new-issue-button").click();
  const newIssueForm = page.locator("form.new-issue-panel");
  await newIssueForm.locator("input").first().fill(duplicateSubmitTitle);
  await newIssueForm.locator("textarea").fill(
    "Same-tick double activation must create exactly one Tea work order.",
  );
  await newIssueForm.locator("button.new-issue-button").evaluate((button) => {
    button.click();
    button.click();
  });
  let duplicateSubmitTicketCount = 0;
  const duplicateSubmitDeadline = Date.now() + timeoutMs;
  while (Date.now() < duplicateSubmitDeadline) {
    duplicateSubmitTicketCount = await page.evaluate(async (title) => {
      const response = await window.__TAURI_INTERNALS__.invoke("tea_request", {
        method: "GET",
        path: "/v1/tickets",
        body: null,
        baseUrl: null,
        authToken: null,
      });
      const tickets = Array.isArray(response) ? response : (response?.items ?? []);
      return tickets.filter((ticket) => ticket?.title === title).length;
    }, duplicateSubmitTitle);
    if (duplicateSubmitTicketCount > 0) break;
    await delay(100);
  }
  if (duplicateSubmitTicketCount !== 1) {
    throw new Error(
      `Same-tick Tea create submitted ${duplicateSubmitTicketCount} tickets instead of exactly one`,
    );
  }

  // Let the daemon persist the first create, then make the WebView observe a
  // transport failure. Retrying the unchanged draft must reuse the same key and
  // receive the server's durable replay instead of creating another ticket.
  const responseLossTitle = "Tea UI uncertain create retry";
  await page.locator(".repo-actions > button.new-issue-button").click();
  const retryForm = page.locator("form.new-issue-panel");
  await retryForm.locator("input").first().fill(responseLossTitle);
  await retryForm.locator("textarea").fill(
    "The server persists this ticket before the first desktop response is lost.",
  );
  const retrySubmit = retryForm.locator("button.new-issue-button");
  await retrySubmit.click();
  const firstAttemptDeadline = Date.now() + timeoutMs;
  let responseLossRetry = null;
  while (Date.now() < firstAttemptDeadline) {
    responseLossRetry = await fetch(`${proxyUrl}/__smoke_state`).then((response) => response.json());
    if (responseLossRetry.requestKeys.length === 1 && responseLossRetry.responseDropped) break;
    await delay(100);
  }
  if (!responseLossRetry || responseLossRetry.requestKeys.length !== 1 || !responseLossRetry.responseDropped) {
    throw new Error(`Tea response-loss proxy did not observe the first create: ${JSON.stringify(responseLossRetry)}`);
  }
  await retrySubmit.click();
  await retryForm.waitFor({ state: "hidden", timeout: timeoutMs });

  responseLossRetry = await fetch(`${proxyUrl}/__smoke_state`).then((response) => response.json());
  responseLossRetry.ticketCount = await page.evaluate(async (title) => {
    const response = await window.__TAURI_INTERNALS__.invoke("tea_request", {
      method: "GET",
      path: "/v1/tickets",
      body: null,
      baseUrl: null,
      authToken: null,
    });
    const tickets = Array.isArray(response) ? response : (response?.items ?? []);
    return tickets.filter((ticket) => ticket?.title === title).length;
  }, responseLossTitle);
  if (!responseLossRetry.responseDropped || responseLossRetry.requestKeys.length !== 2) {
    throw new Error(`Tea uncertain create retry did not execute twice: ${JSON.stringify(responseLossRetry)}`);
  }
  if (
    typeof responseLossRetry.requestKeys[0] !== "string" ||
    !responseLossRetry.requestKeys[0].startsWith("tea-desktop-") ||
    responseLossRetry.requestKeys[0] !== responseLossRetry.requestKeys[1]
  ) {
    throw new Error(`Tea uncertain create retry did not reuse one stable key: ${JSON.stringify(responseLossRetry)}`);
  }
  if (responseLossRetry.ticketCount !== 1) {
    throw new Error(
      `Tea uncertain create retry persisted ${responseLossRetry.ticketCount} tickets instead of exactly one`,
    );
  }

  markProgress("completion-review:start");
  const completionReview = await verifyCompletionReview(
    page, timeoutMs, path.join(screenshotRoot, "tea-completion-review.png"),
  );
  markProgress("completion-review:complete");
  markProgress("accepted-approval:start");
  const acceptedApprovalReview = await verifyCompletionReview(
    page, timeoutMs, path.join(screenshotRoot, "tea-accepted-approval.png"), false,
  );
  markProgress("accepted-approval:complete");

  await writeResult({
    status: "passed",
    native_tauri_runtime: nativeTauriRuntime,
    accessibility,
    narrowViewport,
    copiedLinkNarrowViewport,
    screenshots: {
      wide: wideScreenshotPath,
      narrow: narrowScreenshotPath,
    },
    labelSummary,
    duplicateSubmitTicketCount,
    duplicateCommentCount,
    detailRefreshFailure,
    responseLossRetry,
    completionReview,
    acceptedApprovalReview,
    pageUrl: page.url(),
    pageTitle: await page.title().catch(() => ""),
    pageStates,
    progress,
    consoleMessages,
    pageErrors,
  });
} catch (error) {
  await writeResult({
    status: "failed",
    error: error instanceof Error ? error.message : String(error),
    pageStates,
    progress,
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

Write-Utf8NoBom -Path $uiProxyScript -Content $uiProxySource
Write-Utf8NoBom -Path $uiSmokeScript -Content $uiSmokeSource
Copy-Item -LiteralPath (Join-Path $PSScriptRoot "tea-completion-review-smoke.mjs") `
    -Destination (Join-Path $artifactRoot "tea-completion-review-smoke.mjs")
$nodeExe = (Get-Command node -ErrorAction Stop).Source

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

    $env:TEA_UI_PROXY_PORT = [string]$ProxyPort
    $env:TEA_UI_PROXY_UPSTREAM = $baseUrl
    $proxyProcess = Start-Process -FilePath $nodeExe `
        -ArgumentList @($uiProxyScript) `
        -PassThru `
        -WindowStyle Hidden `
        -RedirectStandardOutput $proxyOut `
        -RedirectStandardError $proxyErr
    Wait-TeaHealth -BaseUrl $proxyUrl -Process $proxyProcess -TimeoutSec $TimeoutSec

    $env:TEA_SERVER_URL = $proxyUrl
    $env:TEA_AUTH_TOKEN = $AuthToken
    $env:TEA_STORE_PATH = $storePath
    $env:TEA_CONFIG_PATH = $configPath
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$DebugPort --remote-allow-origins=*"
    $env:WEBVIEW2_USER_DATA_FOLDER = $webview2UserDataDir
    $currentPrincipal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if ($currentPrincipal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        if (-not (Test-Path -LiteralPath $webviewPolicyKeyPath)) {
            New-Item -Path $webviewPolicyKeyPath -Force | Out-Null
            $webviewPolicyKeyCreated = $true
        }
        $existingPolicy = Get-ItemProperty -LiteralPath $webviewPolicyKeyPath -Name $webviewPolicyValueName -ErrorAction SilentlyContinue
        if ($null -ne $existingPolicy) {
            $webviewPolicyPreviousValue = $existingPolicy.$webviewPolicyValueName
        }
        Set-ItemProperty -LiteralPath $webviewPolicyKeyPath -Name $webviewPolicyValueName -Value $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -Type String
        $webviewPolicyApplied = $true
    }
    $env:TEA_UI_TAURI_CDP_URL = $cdpUrl
    $env:TEA_UI_TAURI_RESULT_PATH = $resultPath
    $env:TEA_UI_TAURI_TIMEOUT_MS = [string]($TimeoutSec * 1000)
    $env:TEA_UI_TAURI_NARROW_WIDTH = [string]$NarrowViewportWidth
    $env:TEA_UI_TAURI_PROXY_URL = $proxyUrl

    $uiProcess = Start-Process -FilePath $uiExe `
        -PassThru `
        -RedirectStandardOutput $uiOut `
        -RedirectStandardError $uiErr
    Start-Sleep -Seconds 1
    $uiProcess.Refresh()
    if ($uiProcess.HasExited) {
        throw "tea.exe UI process exited early with code $($uiProcess.ExitCode)."
    }

    if ([string]::IsNullOrWhiteSpace($PlaywrightPackageRoot)) {
        $repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
        $localDesktopRoot = Join-Path $repoRoot "apps\desktop"
        if (-not [string]::IsNullOrWhiteSpace($env:PLAYWRIGHT_PACKAGE_ROOT)) {
            $PlaywrightPackageRoot = $env:PLAYWRIGHT_PACKAGE_ROOT
        } elseif (Test-Path -LiteralPath (Join-Path $localDesktopRoot "node_modules\playwright-core\package.json") -PathType Leaf) {
            $PlaywrightPackageRoot = $localDesktopRoot
        }
    }
    if ([string]::IsNullOrWhiteSpace($PlaywrightPackageRoot)) {
        throw "Tea UI smoke requires playwright-core. Run npm install in apps\desktop or pass -PlaywrightPackageRoot <directory containing package.json and node_modules\playwright-core>."
    }
    $env:PLAYWRIGHT_PACKAGE_ROOT = (Resolve-Path -LiteralPath $PlaywrightPackageRoot).Path
    $browserProcess = Start-Process -FilePath $nodeExe `
        -ArgumentList @($uiSmokeScript) `
        -PassThru `
        -WindowStyle Hidden `
        -RedirectStandardOutput $browserOut `
        -RedirectStandardError $browserErr
    $browserWaitTimeoutSec = $TimeoutSec + 15
    if (-not $browserProcess.WaitForExit($browserWaitTimeoutSec * 1000)) {
        throw "Tea Tauri UI smoke browser script did not finish within the $TimeoutSec-second test budget plus a 15-second result-flush grace period."
    }
    [void]$browserProcess.WaitForExit()
    $browserProcess.Refresh()
    $browserExitCode = $browserProcess.ExitCode
    if (-not (Test-Path -LiteralPath $resultPath -PathType Leaf)) {
        $browserStdout = if (Test-Path -LiteralPath $browserOut -PathType Leaf) { Get-Content -Raw -LiteralPath $browserOut } else { "" }
        $browserStderr = if (Test-Path -LiteralPath $browserErr -PathType Leaf) { Get-Content -Raw -LiteralPath $browserErr } else { "" }
        $exitCodeText = if ($null -eq $browserExitCode) { "unavailable" } else { [string]$browserExitCode }
        throw "Tea Tauri UI smoke result was not written (browser exit code $exitCodeText): $resultPath. stdout=$browserStdout stderr=$browserStderr"
    }

    $result = Get-Content -Raw -LiteralPath $resultPath | ConvertFrom-Json
    if ($result.status -ne "passed") {
        $browserStderr = if (Test-Path -LiteralPath $browserErr -PathType Leaf) { Get-Content -Raw -LiteralPath $browserErr } else { "" }
        throw "Tea Tauri UI smoke failed: $($result.error). stderr=$browserStderr"
    }
    if (($null -ne $browserExitCode) -and ($browserExitCode -ne 0)) {
        throw "Tea Tauri UI smoke browser script returned exit code $browserExitCode after writing a passed result."
    }

    $smokePassed = $true
    [ordered]@{
        status = "passed"
        packageDir = $packagePath
        baseUrl = $baseUrl
        proxyUrl = $proxyUrl
        cdpUrl = $cdpUrl
        daemonPid = $daemonProcess.Id
        proxyPid = $proxyProcess.Id
        uiPid = $uiProcess.Id
        ticketId = $ticket.id
        storeBackend = $status.store.backend
        native_tauri_runtime = [bool]$result.native_tauri_runtime
        duplicateSubmitTicketCount = [int]$result.duplicateSubmitTicketCount
        responseLossRetryTicketCount = [int]$result.responseLossRetry.ticketCount
        responseLossRetryStableKey = (
            @($result.responseLossRetry.requestKeys).Count -eq 2 -and
            [string]$result.responseLossRetry.requestKeys[0] -eq [string]$result.responseLossRetry.requestKeys[1]
        )
        transientDetailFailurePreserved = [bool]$result.detailRefreshFailure.staleConversationPreserved
        transientDetailFailureRecovered = [bool]$result.detailRefreshFailure.recoveredWithoutReselection
        issueItemContextInvalidTags = [int]$result.accessibility.issueItemContextInvalidTags
        narrowViewportWidth = $NarrowViewportWidth
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
    if ($webviewPolicyApplied) {
        if ($null -ne $webviewPolicyPreviousValue) {
            Set-ItemProperty -LiteralPath $webviewPolicyKeyPath -Name $webviewPolicyValueName -Value $webviewPolicyPreviousValue -Type String
        } else {
            Remove-ItemProperty -LiteralPath $webviewPolicyKeyPath -Name $webviewPolicyValueName -ErrorAction SilentlyContinue
        }
        if ($webviewPolicyKeyCreated) {
            Remove-Item -LiteralPath $webviewPolicyKeyPath -ErrorAction SilentlyContinue
        }
    }
    $env:TEA_UI_TAURI_CDP_URL = $oldCdpUrl
    $env:TEA_UI_TAURI_RESULT_PATH = $oldResultPath
    $env:TEA_UI_TAURI_TIMEOUT_MS = $oldTimeoutMs
    $env:TEA_UI_TAURI_NARROW_WIDTH = $oldNarrowViewportWidth
    $env:TEA_UI_TAURI_PROXY_URL = $oldProxyUrl
    $env:TEA_UI_PROXY_PORT = $oldProxyPort
    $env:TEA_UI_PROXY_UPSTREAM = $oldProxyUpstream
    $env:PLAYWRIGHT_PACKAGE_ROOT = $oldPlaywrightPackageRoot

    $browserStopped = $true
    $uiStopped = $true
    $proxyStopped = $true
    $daemonStopped = $true
    if ($browserProcess -ne $null) {
        try { $browserStopped = Stop-ProcessTree -Process $browserProcess -Name "tea-ui-browser" }
        catch {
            Stop-Process -Id $browserProcess.Id -Force -ErrorAction SilentlyContinue
            $browserProcess.Refresh()
            $browserStopped = [bool]$browserProcess.HasExited
        }
    }
    if ($uiProcess -ne $null) {
        try { $uiStopped = Stop-ProcessTree -Process $uiProcess -Name "tea-ui" }
        catch {
            Stop-Process -Id $uiProcess.Id -Force -ErrorAction SilentlyContinue
            $uiProcess.Refresh()
            $uiStopped = [bool]$uiProcess.HasExited
        }
    }
    if ($proxyProcess -ne $null) {
        try { $proxyStopped = Stop-ProcessTree -Process $proxyProcess -Name "tea-ui-proxy" }
        catch {
            Stop-Process -Id $proxyProcess.Id -Force -ErrorAction SilentlyContinue
            $proxyProcess.Refresh()
            $proxyStopped = [bool]$proxyProcess.HasExited
        }
    }
    if ($daemonProcess -ne $null) {
        try { $daemonStopped = Stop-ProcessTree -Process $daemonProcess -Name "tea-daemon" }
        catch {
            Stop-Process -Id $daemonProcess.Id -Force -ErrorAction SilentlyContinue
            $daemonProcess.Refresh()
            $daemonStopped = [bool]$daemonProcess.HasExited
        }
    }

    $cleanupDeadline = (Get-Date).AddSeconds(5)
    do {
        $daemonListeners = @(Get-PortListeners -Port $Port)
        $debugListeners = @(Get-PortListeners -Port $DebugPort)
        $proxyListeners = @(Get-PortListeners -Port $ProxyPort)
        if ($daemonListeners.Count -eq 0 -and $debugListeners.Count -eq 0 -and $proxyListeners.Count -eq 0) { break }
        Start-Sleep -Milliseconds 100
    } while ((Get-Date) -lt $cleanupDeadline)

    $cleanupPassed = $browserStopped -and $uiStopped -and $proxyStopped -and $daemonStopped -and `
        $daemonListeners.Count -eq 0 -and $debugListeners.Count -eq 0 -and $proxyListeners.Count -eq 0
    if (-not $KeepArtifacts -and $cleanupPassed) {
        $resolvedArtifactRoot = [System.IO.Path]::GetFullPath($artifactRoot)
        if (-not $resolvedArtifactRoot.StartsWith(
            $tempPrefix,
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
            throw "Refusing to clean UI smoke artifacts outside the system temp directory: $resolvedArtifactRoot"
        }
        Remove-Item -LiteralPath $resolvedArtifactRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
    if ($smokePassed -and -not $cleanupPassed) {
        throw "Tea UI smoke cleanup failed: browser_stopped=$browserStopped ui_stopped=$uiStopped proxy_stopped=$proxyStopped daemon_stopped=$daemonStopped daemon_listeners=$($daemonListeners.Count) debug_listeners=$($debugListeners.Count) proxy_listeners=$($proxyListeners.Count) artifacts=$artifactRoot"
    }
}
