import assert from "node:assert/strict";

// Only synthetic records in the existing native smoke's isolated mock daemon.
export async function verifyReviewDrafts(page, timeoutMs, screenshotPath) {
  const request = (method, path, body = null) => page.evaluate(
    (args) => window.__TAURI_INTERNALS__.invoke("tea_request", args),
    { method, path, body, baseUrl: null, authToken: null, timeoutMs: 15000 },
  );
  assert.equal((await request("GET", "/v1/status")).execution_provider, "mock");
  const clear = async (selector, phase) => {
    try {
      await page.waitForFunction((target) => document.querySelector(target)?.value === "", selector, { timeout: timeoutMs });
    } catch { throw new Error(`Review draft did not clear: ${phase}`); }
  };
  const results = [];
  for (const kind of ["comment", "rejection"]) {
    const tickets = [];
    for (const letter of ["A", "B"]) {
      tickets.push(await request("POST", "/v1/tickets", {
        title: `Tea ${kind} ownership smoke ${letter}`, description: "Synthetic review draft ownership fixture.",
      }));
    }
    const [a, b] = tickets;
    const path = (id) => `/v1/tickets/${encodeURIComponent(id)}`;
    const row = (title) => page.locator(".issue-item").filter({ hasText: title });
    const formSelector = kind === "comment" ? "form.comment-editor" : "form.reject-reason-form";
    const form = page.locator(formSelector);
    const input = form.locator("textarea");
    await page.locator(".issue-filter-tabs > button").nth(0).click();
    await page.locator(".refresh-control").getByRole("button", { name: "Refresh", exact: true }).click();
    await row(a.title).click({ timeout: timeoutMs });
    await page.locator(".repo-tabs").getByRole("tab", { name: /^Comments/ }).click();
    await input.fill(`Unsubmitted private ${kind} for A`);
    if (kind === "comment") {
      await form.getByRole("tab", { name: "Preview comment", exact: true }).click();
      assert.ok((await form.locator(".comment-preview").textContent()).includes("for A"));
    }
    await row(b.title).click();
    await input.waitFor({ state: "visible", timeout: timeoutMs });
    await clear(`${formSelector} textarea`, `${kind} switch A to B`);
    await row(a.title).click();
    await clear(`${formSelector} textarea`, `${kind} return to A`);
    await row(b.title).click();
    const text = `Intentional ${kind} for B`;
    await input.fill(text);
    await form.getByRole("button", { name: kind === "comment" ? "Comment" : "Reject approval", exact: true }).click();
    await clear(`${formSelector} textarea`, `${kind} successful submit`);
    if (kind === "comment") {
      const comments = await request("GET", `${path(b.id)}/comments`);
      assert.equal(comments.filter((item) => item.body === text).length, 1);
      assert.equal((await request("GET", `${path(a.id)}/comments`)).length, 0);
    } else {
      assert.equal((await request("GET", path(b.id))).status, "blocked");
      assert.equal((await request("GET", path(a.id))).status, "open");
      assert.equal((await request("GET", `${path(b.id)}/events`)).filter((item) => item.kind === "approval_rejected").length, 1);
    }
    results.push({ kind, ticketIds: tickets.map((ticket) => ticket.id), isolated: true });
  }
  await page.screenshot({ path: screenshotPath });
  return { results, screenshotPath };
}
