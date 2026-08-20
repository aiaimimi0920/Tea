export const autoRefreshBaseDelayMs = 8_000;
export const autoRefreshMaxDelayMs = 120_000;

const maxFailureExponent = 30;

export const nextAutoRefreshFailureCount = (
  consecutiveFailures: number,
  failed: boolean,
): number => {
  if (!failed) return 0;
  const normalized = Number.isFinite(consecutiveFailures)
    ? Math.max(0, Math.floor(consecutiveFailures))
    : 0;
  return Math.min(maxFailureExponent, normalized + 1);
};

export const autoRefreshDelayMs = (
  consecutiveFailures: number,
  baseDelayMs = autoRefreshBaseDelayMs,
  maxDelayMs = autoRefreshMaxDelayMs,
): number => {
  const base = Math.max(1, Math.floor(baseDelayMs));
  const maximum = Math.max(base, Math.floor(maxDelayMs));
  const failures = Number.isFinite(consecutiveFailures)
    ? Math.min(maxFailureExponent, Math.max(0, Math.floor(consecutiveFailures)))
    : 0;
  return Math.min(maximum, base * 2 ** failures);
};
