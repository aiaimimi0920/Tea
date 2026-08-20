import type { TeaRun } from "./teaClient";

const stoppableRunStatuses = new Set(["queued", "running", "retrying"]);
const retryableRunStatuses = new Set(["failed", "stopped"]);

export const canStopRun = (run?: TeaRun): boolean => stoppableRunStatuses.has(run?.status ?? "");
export const canRetryRun = (run?: TeaRun): boolean => retryableRunStatuses.has(run?.status ?? "");
