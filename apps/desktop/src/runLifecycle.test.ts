import { describe, expect, it } from "vitest";

import { canRetryRun, canStopRun } from "./runLifecycle";

const run = (status?: string) => ({ id: "run-1", status });

describe("run lifecycle actions", () => {
  it("allows stop only for active run states", () => {
    expect(["queued", "running", "retrying"].map((status) => canStopRun(run(status)))).toEqual([
      true,
      true,
      true,
    ]);
    expect(["succeeded", "failed", "stopped"].map((status) => canStopRun(run(status)))).toEqual([
      false,
      false,
      false,
    ]);
    expect(canStopRun()).toBe(false);
  });

  it("allows retry only for failed or stopped runs", () => {
    expect(["failed", "stopped"].map((status) => canRetryRun(run(status)))).toEqual([true, true]);
    expect(["queued", "running", "succeeded", "retrying"].map((status) => canRetryRun(run(status)))).toEqual([
      false,
      false,
      false,
      false,
    ]);
    expect(canRetryRun()).toBe(false);
  });
});
