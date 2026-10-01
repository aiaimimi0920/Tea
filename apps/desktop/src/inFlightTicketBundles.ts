import type { TeaTicketBundle } from "./teaClient";

// Only pending reads are retained, scoped to a connection and ticket. Settled
// responses are never cached; mutations invalidate their connection's readers.
const pendingBundles = new Map<string, {
  connection: string;
  request: Promise<TeaTicketBundle>;
}>();

export function invalidateTicketBundles(connection: string): void {
  for (const [key, pending] of pendingBundles) {
    if (pending.connection === connection) pendingBundles.delete(key);
  }
}

export async function shareTicketBundleRequest(
  connection: string,
  id: string,
  read: () => Promise<TeaTicketBundle>,
): Promise<TeaTicketBundle> {
  const key = JSON.stringify([connection, id]);
  const existing = pendingBundles.get(key);
  if (existing) return existing.request;

  const request = read();
  pendingBundles.set(key, { connection, request });
  try {
    return await request;
  } finally {
    // Invalidating an older read must not let its cleanup remove a newer read.
    if (pendingBundles.get(key)?.request === request) pendingBundles.delete(key);
  }
}
