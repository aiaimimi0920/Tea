export type LocalNotes = Record<string, string[]>;
export type WatchStates = Record<string, true>;

const parseRecord = (raw: string | null): Record<string, unknown> | null => {
  if (!raw) return null;
  try {
    const parsed: unknown = JSON.parse(raw);
    return typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
};

export const parseWatchStates = (raw: string | null): WatchStates => {
  const parsed = parseRecord(raw);
  if (!parsed) return {};
  return Object.fromEntries(
    Object.entries(parsed).flatMap(([ticketId, watched]) =>
      ticketId.trim().length > 0 && watched === true ? [[ticketId, true] as const] : [],
    ),
  );
};

export const parseLocalNotes = (raw: string | null): LocalNotes => {
  const parsed = parseRecord(raw);
  if (!parsed) return {};
  return Object.fromEntries(
    Object.entries(parsed).flatMap(([ticketId, value]) => {
      if (!ticketId.trim() || !Array.isArray(value)) return [];
      const notes = Array.from(
        new Set(
          value
            .filter((item): item is string => typeof item === "string")
            .map((item) => item.trim())
            .filter(Boolean),
        ),
      );
      return notes.length > 0 ? [[ticketId, notes] as const] : [];
    }),
  );
};

export const toggleWatchedTicket = (current: WatchStates, ticketId: string): WatchStates => {
  const next = { ...current };
  if (current[ticketId]) {
    delete next[ticketId];
  } else {
    next[ticketId] = true;
  }
  return next;
};

export const removeTicketLocalNote = (
  current: LocalNotes,
  ticketId: string,
  note: string,
): LocalNotes => {
  const next = { ...current };
  const remaining = (current[ticketId] ?? []).filter((currentNote) => currentNote !== note);
  if (remaining.length > 0) {
    next[ticketId] = remaining;
  } else {
    delete next[ticketId];
  }
  return next;
};
