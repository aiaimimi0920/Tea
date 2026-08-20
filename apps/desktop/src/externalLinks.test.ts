import { describe, expect, it } from "vitest";

import { safeLoomPanelUrl } from "./externalLinks";

describe("safeLoomPanelUrl", () => {
  it.each([
    "loom://settings/tea",
    "https://loom.example/settings/apps/tea",
    "http://127.0.0.1:8765/settings/tea",
  ])("accepts an explicit Loom or web link: %s", (value) => {
    expect(safeLoomPanelUrl(value)).toBe(value);
  });

  it.each([
    "",
    " javascript:alert(document.domain)",
    "javascript:alert(document.domain)",
    "data:text/html,<script>alert(1)</script>",
    "file:///etc/passwd",
    "//loom.example/settings/tea",
    "https://user:secret@loom.example/settings/tea",
    "loom:///tea",
  ])("rejects an unsafe or ambiguous link: %s", (value) => {
    expect(safeLoomPanelUrl(value)).toBeNull();
  });

  it("rejects non-string values", () => {
    expect(safeLoomPanelUrl(null)).toBeNull();
    expect(safeLoomPanelUrl({ href: "https://loom.example/settings/tea" })).toBeNull();
  });
});
