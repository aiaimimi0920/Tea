const ALLOWED_LOOM_PANEL_PROTOCOLS = new Set(["http:", "https:", "loom:"]);

export function safeLoomPanelUrl(value: unknown): string | null {
  if (typeof value !== "string" || value.length === 0 || value.trim() !== value) return null;

  try {
    const parsed = new URL(value);
    if (
      !ALLOWED_LOOM_PANEL_PROTOCOLS.has(parsed.protocol) ||
      !parsed.hostname ||
      parsed.username.length > 0 ||
      parsed.password.length > 0
    ) {
      return null;
    }
    return value;
  } catch {
    return null;
  }
}
