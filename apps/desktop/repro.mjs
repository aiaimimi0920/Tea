import { chromium } from "playwright";
const DAEMON = process.env.DAEMON_URL || "http://127.0.0.1:48910";
const TOKEN = process.env.TOKEN || "dev-token";
const APP = process.env.APP_URL || "http://127.0.0.1:1424";
const init = `
window.__TAURI_INTERNALS__ = {
  transformCallback: (cb) => cb,
  invoke: async (cmd, args) => {
    if (cmd === "resolve_tea_runtime_config") return { serverUrl: ${JSON.stringify(DAEMON)}, authConfigured: true };
    if (cmd === "tea_request") {
      const { method, path, body } = args;
      const res = await fetch(${JSON.stringify(DAEMON)} + path, { method, headers: { "content-type": "application/json", authorization: "Bearer ${TOKEN}" }, body: body == null ? undefined : JSON.stringify(body) });
      const text = await res.text();
      if (!res.ok) throw "Tea returned HTTP " + res.status + ": " + text;
      if (!text.trim()) return null;
      try { return JSON.parse(text); } catch { return text; }
    }
    if (cmd === "save_tea_export") return "mock-path";
    return null;
  },
};
`;
const browser = await chromium.launch();
const page = await browser.newPage();
const errors = [];
page.on("console", (m) => { if (m.type() === "error") errors.push("CONSOLE: " + m.text()); });
page.on("pageerror", (e) => errors.push("PAGEERROR: " + (e.stack || e.message)));
await page.addInitScript(init);
await page.goto(APP, { waitUntil: "networkidle" });
await page.waitForTimeout(1500);
await page.locator('button', { hasText: /新建工单|New Work Order/ }).first().click();
await page.waitForTimeout(500);
await page.locator('input').first().fill("黑屏复现工单标题");
await page.locator('textarea').first().fill("这是一个用于复现新建工单后黑屏问题的描述，长度足够。");
await page.locator('button[type="submit"]').first().click();
await page.waitForTimeout(2500);
const rootHtml = await page.locator('#root').innerHTML();
const boundaryText = await page.locator('body').innerText().catch(() => "");
console.log("=== ERRORS ==="); console.log(errors.join("\n---\n") || "(none)");
console.log("=== ROOT LEN ===", rootHtml.trim().length, "BLANK?", rootHtml.trim().length < 200);
console.log("=== VISIBLE TEXT ==="); console.log(boundaryText.slice(0, 600));
await browser.close();
