import assert from "node:assert/strict";

// Runs before seeding the isolated native smoke daemon with its first work order.
export async function verifyEmptySettings(page, timeoutMs, screenshotPath) {
  const request = (method, path, body = null) => page.evaluate(
    (args) => window.__TAURI_INTERNALS__.invoke("tea_request", args),
    { method, path, body, baseUrl: null, authToken: null, timeoutMs: 15000 },
  );
  assert.equal((await request("GET", "/v1/status")).execution_provider, "mock");
  const tickets = await request("GET", "/v1/tickets");
  assert.equal((Array.isArray(tickets) ? tickets : tickets.items).length, 0);
  const original = await request("GET", "/v1/configuration");
  assert.equal(original.configuration_source, "local");
  // The fresh profile defaults to Chinese; reuse the public language control.
  const localeToggle = page.getByTestId("locale-toggle");
  const restoreChinese = (await localeToggle.textContent())?.trim() === "EN";
  if (restoreChinese) await localeToggle.click();
  const tabs = page.locator(".repo-tabs");
  const settings = () => tabs.getByRole("tab", { name: "Settings", exact: true }).click();
  const issues = () => tabs.getByRole("tab", { name: /^Issues/ }).click();
  const form = page.locator("form.settings-config-editor");
  const notifications = form.getByRole("checkbox", { name: "Enable notifications", exact: true });
  let phase = "open empty settings";
  try {
    await settings();
    await form.waitFor({ state: "visible", timeout: timeoutMs });
    assert.equal(await notifications.isChecked(), original.config.notifications_enabled);
    assert.equal(await form.getByRole("button", { name: "Save Tea settings", exact: true }).isDisabled(), true);
    await notifications.setChecked(!original.config.notifications_enabled);
    phase = "leave settings with unsaved draft";
    await issues();
    await form.waitFor({ state: "hidden", timeout: timeoutMs });
    phase = "return to settings";
    await settings();
    assert.equal(await notifications.isChecked(), original.config.notifications_enabled);
    await notifications.setChecked(!original.config.notifications_enabled);
    phase = "reset unsaved changes";
    await form.getByRole("button", { name: "Reset changes", exact: true }).click();
    assert.equal(await notifications.isChecked(), original.config.notifications_enabled);
    assert.deepEqual(await request("GET", "/v1/configuration"), original);
    await page.screenshot({ path: screenshotPath });

    // Seed the same records used by the existing lifecycle/accessibility assertions.
    const ticket = await request("POST", "/v1/tickets", {
      title: "Tea UI smoke", description: "Created after verifying empty-workspace settings in tea.exe.",
    });
    await request("POST", `/v1/tickets/${encodeURIComponent(ticket.id)}/comments`, {
      body: "Tea UI smoke comment for timeline coverage.",
    });
    phase = "refresh first work order";
    await page.locator(".refresh-control").getByRole("button", { name: "Refresh", exact: true }).click();
    await page.locator(".issue-item").filter({ hasText: ticket.title }).waitFor({ state: "visible", timeout: timeoutMs });
    assert.equal(await notifications.isChecked(), original.config.notifications_enabled);
    phase = "return to issues with first work order";
    await issues();
    if (restoreChinese) await localeToggle.click();
    return { ticketId: ticket.id, emptyWorkspaceVerified: true, screenshotPath };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`Empty settings smoke failed during ${phase}: ${message.replace(/\s+/g, " ")}`);
  }
}
