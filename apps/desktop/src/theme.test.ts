import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const sourceDirectory = dirname(fileURLToPath(import.meta.url));
const theme = readFileSync(join(sourceDirectory, "theme.css"), "utf8");
const styles = readFileSync(join(sourceDirectory, "styles.css"), "utf8");

const definitions = (css: string) =>
  new Set(Array.from(css.matchAll(/--([a-z0-9-]+)\s*:/g), (match) => match[1]));
const references = (css: string) =>
  new Set(Array.from(css.matchAll(/var\(--([a-z0-9-]+)/g), (match) => match[1]));

describe("Tea theme contract", () => {
  it("keeps the canonical Neuro base colors in the theme entrypoint", () => {
    expect(theme).toContain("--neuro-signal-yellow: #d9ff38;");
    expect(theme).toContain("--neuro-signal-green: #22c55e;");
    expect(theme).toContain("--neuro-info-blue: #06b6d4;");
    expect(theme).toContain("--neuro-danger-red: #f43f5e;");
    expect(theme).toContain("--neuro-bg: #06080d;");
    expect(theme).toContain("--neuro-text: #f7f8ef;");
  });

  it("keeps product styles free of direct colors and gradients", () => {
    expect(styles.match(/#[0-9a-f]{3,8}/gi) ?? []).toEqual([]);
    expect(styles.match(/rgba?\([^)]*\)/gi) ?? []).toEqual([]);
    expect(styles.match(/gradient\(/gi) ?? []).toEqual([]);
  });

  it("defines every semantic token consumed by product styles", () => {
    const themeDefinitions = definitions(theme);
    const missing = Array.from(references(styles)).filter((name) => !themeDefinitions.has(name));
    expect(missing).toEqual([]);
  });
});
