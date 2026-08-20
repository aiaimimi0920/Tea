import { describe, expect, it } from "vitest";

import {
  autoRefreshDelayMs,
  nextAutoRefreshFailureCount,
} from "./refreshBackoff";

describe("auto-refresh backoff", () => {
  it("backs off exponentially and caps the delay", () => {
    expect([0, 1, 2, 3, 4, 20].map((failures) => autoRefreshDelayMs(failures))).toEqual([
      8_000,
      16_000,
      32_000,
      64_000,
      120_000,
      120_000,
    ]);
  });

  it("resets after success and bounds the failure count", () => {
    expect(nextAutoRefreshFailureCount(3, false)).toBe(0);
    expect(nextAutoRefreshFailureCount(3, true)).toBe(4);
    expect(nextAutoRefreshFailureCount(Number.POSITIVE_INFINITY, true)).toBe(1);
    expect(nextAutoRefreshFailureCount(30, true)).toBe(30);
  });

  it("normalizes invalid delay inputs", () => {
    expect(autoRefreshDelayMs(Number.NaN, 0, 0)).toBe(1);
    expect(autoRefreshDelayMs(-5, 2_000, 1_000)).toBe(2_000);
  });
});
