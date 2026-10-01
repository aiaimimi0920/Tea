import assert from "node:assert/strict";

// Run only inside the existing smoke harness's isolated daemon/profile. Never
// dispatch against a configured external executor or the user's real work.
export async function verifyCompletionReview(page, timeoutMs, screenshotPath, approveBeforeAccept = true) {
  const request = (method, path, body = null) => page.evaluate(
    (args) => window.__TAURI_INTERNALS__.invoke("tea_request", args),
    { method, path, body, baseUrl: null, authToken: null, timeoutMs: 15000 },
  );
  const until = async (check, message) => {
    const deadline = Date.now() + timeoutMs;
    do {
      if (await check()) return;
      await new Promise((resolve) => setTimeout(resolve, 100));
    } while (Date.now() < deadline);
    throw new Error(message);
  };
  const status = await request("GET", "/v1/status");
  assert.equal(status.execution_provider, "mock", "completion smoke requires the mock executor");

  const localeToggle = page.getByTestId("locale-toggle");
  if ((await localeToggle.textContent())?.trim() === "EN") await localeToggle.click();
  const title = `Tea completion review smoke (${approveBeforeAccept ? "approval-first" : "acceptance-first"})`;
  const ticket = await request("POST", "/v1/tickets", {
    title,
    description: "Synthetic local completion evidence for the desktop lifecycle contract.",
    approval_policy: "human_before_completion",
  });
  const ticketPath = `/v1/tickets/${encodeURIComponent(ticket.id)}`;
  const run = await request("POST", `${ticketPath}/run`, {});
  assert.equal(run.status, "succeeded");
  assert.ok(run.evidence, "mock execution must provide reviewable evidence");

  // Closing must still be denied by the daemon until completion approval exists.
  await assert.rejects(request("POST", `${ticketPath}/close`, {}), /approval/i);
  await page.locator(".issue-filter-tabs > button").nth(0).click();
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  const row = page.locator(".issue-item").filter({ hasText: title });
  await row.click({ timeout: timeoutMs });
  await page.locator(".repo-tabs").getByRole("tab", { name: /^Comments/ }).click();

  const waitStatus = async (expected) => {
    await until(async () => (await request("GET", ticketPath)).status === expected,
      `Completion fixture did not reach ${expected}`);
    await page.locator(".issue-title-topline").getByText(expected, { exact: true })
      .waitFor({ state: "visible", timeout: timeoutMs });
  };
  await waitStatus("completed");
  assert.equal(await page.locator(".issue-detail .issue-state.open").count(), 1);

  const commentBody = "Human reviewed the synthetic completion evidence.";
  const comments = page.locator("form.comment-editor");
  await comments.locator("textarea").fill(commentBody);
  await comments.locator("button[type='submit']").click();
  await until(async () => (await request("GET", `${ticketPath}/comments`))
    .filter((comment) => comment.body === commentBody).length === 1,
  "Review comment was not persisted for the completed ticket");

  const action = (name) => page.locator(".workflow-actions")
    .getByRole("button", { name, exact: true });
  const approve = async () => {
    await action("Approve").click();
    await until(async () => (await request("GET", `${ticketPath}/events`))
      .some((event) => event.kind === "approval_granted"), "Completion approval was not persisted");
    await waitStatus(approveBeforeAccept ? "completed" : "accepted");
  };
  if (approveBeforeAccept) await approve();
  await action("Accept").click();
  await waitStatus("accepted");
  if (!approveBeforeAccept) {
    await assert.rejects(request("POST", `${ticketPath}/close`, {}), /approval/i);
    await approve();
  }
  assert.equal(await action("Accept").isDisabled(), true);
  assert.equal(await comments.locator("textarea").isEnabled(), true);
  assert.equal(await row.count(), 1, "accepted work must remain in the open queue");
  await page.screenshot({ path: screenshotPath });

  await action("Close").click();
  await until(async () => (await request("GET", ticketPath)).status === "closed",
    "Completion fixture did not close");
  await row.waitFor({ state: "hidden", timeout: timeoutMs });
  await page.locator(".issue-filter-tabs > button").nth(1).click();
  await row.click();
  await waitStatus("closed");
  assert.equal(await comments.locator("textarea").isDisabled(), true);
  assert.equal(await page.locator("#policy-editor-select").isDisabled(), true);
  assert.equal(await page.locator("#reject-reason-input").isDisabled(), true);
  for (const label of ["Accept", "Approve", "Close", "Run"]) {
    assert.equal(await action(label).isDisabled(), true, `${label} must remain disabled after closure`);
  }
  return { ticketId: ticket.id, finalStatus: "closed", approveBeforeAccept, commentCount: 1, screenshotPath };
}
