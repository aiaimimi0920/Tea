export const decodeTimelineEntryHash = (hash: string) => {
  const raw = hash.replace(/^#/, "");
  try {
    return decodeURIComponent(raw);
  } catch {
    return raw;
  }
};

const buildTimelineEntryLink = (entryId: string) => {
  if (typeof window === "undefined") return `#${entryId}`;
  const url = new URL(window.location.href);
  url.hash = entryId;
  return url.toString();
};

export const copyTimelineEntryLink = async (entryId: string): Promise<boolean> => {
  if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(buildTimelineEntryLink(entryId));
      return true;
    } catch {
      // Fall through to in-page navigation when WebView clipboard access is denied.
    }
  }
  if (typeof window !== "undefined") {
    try {
      window.location.hash = entryId;
    } catch {
      // A failed fallback must still resolve so click handlers cannot leak a rejection.
    }
  }
  return false;
};
