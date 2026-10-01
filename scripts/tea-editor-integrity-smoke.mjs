import assert from "node:assert/strict";

// Synthetic work orders only, inside the native smoke's isolated mock daemon.
export async function verifyEditorSessions(page, timeoutMs, screenshotPath) {
  const request = (method, path, body = null) => page.evaluate(
    (args) => window.__TAURI_INTERNALS__.invoke("tea_request", args),
    { method, path, body, baseUrl: null, authToken: null, timeoutMs: 15000 },
  );
  assert.equal((await request("GET", "/v1/status")).execution_provider, "mock");
  const tickets = [];
  for (const suffix of ["A", "B"]) {
    tickets.push(await request("POST", "/v1/tickets", {
      title: `Tea editor session smoke ${suffix}`, description: `Original ${suffix}`,
      priority: "normal", labels: ["editor-initial"],
    }));
  }
  const [a, b] = tickets;
  const ticketPath = (id) => `/v1/tickets/${encodeURIComponent(id)}`;
  const row = (title) => page.locator(".issue-item").filter({ hasText: title });
  const button = (name) => page.getByRole("button", { name, exact: true });
  const editor = page.locator("form.issue-edit-form");
  // React serializes a textarea's initial value inside the wrapping label;
  // exact label text then includes that value. Its unique placeholder is stable.
  const field = (label) => label === "Description"
    ? editor.getByPlaceholder("Work order description", { exact: true })
    : editor.getByLabel(label, { exact: true });
  const fieldValue = async (label, phase) => {
    try { return await field(label).inputValue(); }
    catch (error) {
      const state = await page.evaluate(() => ({
        title: document.querySelector(".issue-title-heading")?.textContent,
        editors: document.querySelectorAll("form.issue-edit-form").length,
        fields: [...document.querySelectorAll("form.issue-edit-form label")].map((node) => node.textContent?.trim()),
        editButtons: [...document.querySelectorAll(".issue-detail-actions button")].map((node) => node.textContent?.trim()),
      }));
      throw new Error(`Editor fixture ${phase}: ${JSON.stringify(state)}; ${String(error).split("\n")[0]}`);
    }
  };
  await page.locator(".issue-filter-tabs > button").nth(0).click();
  await button("Refresh").click();
  await row(a.title).click({ timeout: timeoutMs });
  await button("Edit issue").click();
  await field("Title").fill("Unsubmitted A draft");
  await row(b.title).click();
  await editor.waitFor({ state: "hidden", timeout: timeoutMs });
  await button("Edit issue").click();
  assert.equal(await fieldValue("Title", "open-B-after-switch"), b.title);
  await field("Title").fill("Tea editor intentional B title");
  await button("Save changes").click();
  await editor.waitFor({ state: "hidden", timeout: timeoutMs });
  assert.equal((await request("GET", ticketPath(b.id))).title, "Tea editor intentional B title");
  assert.equal((await request("GET", ticketPath(a.id))).title, a.title);

  await row(a.title).click();
  await button("Edit issue").click();
  await field("Title").fill("Tea editor intentional A title");
  await request("PATCH", ticketPath(a.id), {
    description: "New external description", priority: "high", labels: ["editor-external"],
  });
  await button("Refresh").click();
  await page.locator(".issue-description").getByText("New external description", { exact: true })
    .waitFor({ state: "visible", timeout: timeoutMs });
  await button("Save changes").click();
  await editor.waitFor({ state: "hidden", timeout: timeoutMs });
  const saved = await request("GET", ticketPath(a.id));
  assert.equal(saved.title, "Tea editor intentional A title");
  assert.equal(saved.description, "New external description");
  assert.equal(saved.priority, "high");
  assert.ok(saved.labels.includes("editor-external"));
  assert.ok(!saved.labels.includes("editor-initial"));

  // Saving an untouched draft after a refresh must not manufacture an edit.
  await button("Edit issue").click();
  await request("PATCH", ticketPath(a.id), { description: "Newest external description" });
  await button("Refresh").click();
  await page.locator(".issue-description").getByText("Newest external description", { exact: true })
    .waitFor({ state: "visible", timeout: timeoutMs });
  const before = (await request("GET", `${ticketPath(a.id)}/events`)).length;
  await button("Save changes").click();
  await editor.waitFor({ state: "hidden", timeout: timeoutMs });
  assert.equal((await request("GET", `${ticketPath(a.id)}/events`)).length, before);
  assert.equal((await request("GET", ticketPath(a.id))).description, "Newest external description");
  await button("Edit issue").click();
  assert.equal(await fieldValue("Description", "reopen-A-after-noop"), "Newest external description");
  await button("Cancel").click();
  await page.screenshot({ path: screenshotPath });
  return { ticketIds: tickets.map((ticket) => ticket.id), untouchedFieldsPreserved: true, screenshotPath };
}
