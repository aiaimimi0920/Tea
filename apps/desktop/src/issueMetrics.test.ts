import { describe, expect, it } from "vitest";

import { mergeIssueMetricCounts, reuseUnchangedIssueMetrics } from "./issueMetrics";

type TestMetric = {
  comments: number;
  latestTouch?: { label: string };
  runs: number;
};

describe("mergeIssueMetricCounts", () => {
  it("preserves the map and metadata when detail polling repeats the same counts", () => {
    const current = {
      first: { comments: 1, runs: 2, latestTouch: { label: "Reviewed" } },
    };
    expect(mergeIssueMetricCounts(current, "first", 1, 2)).toBe(current);
    expect(mergeIssueMetricCounts(current, "first", 1, 3)).not.toBe(current);
    expect(mergeIssueMetricCounts(current, "first", 2, 2)).not.toBe(current);
  });

  it("updates counts without dropping existing metadata or sibling entries", () => {
    const current: Record<string, TestMetric> = {
      first: {
        comments: 1,
        latestTouch: { label: "reviewed" },
        runs: 2,
      },
      second: { comments: 3, runs: 4 },
    };

    const next = mergeIssueMetricCounts(current, "first", 5, 6);

    expect(next).not.toBe(current);
    expect(next.first).toEqual({
      comments: 5,
      latestTouch: { label: "reviewed" },
      runs: 6,
    });
    expect(next.second).toBe(current.second);
    expect(current.first).toEqual({
      comments: 1,
      latestTouch: { label: "reviewed" },
      runs: 2,
    });
  });

  it("creates counts for a previously unseen issue", () => {
    const next = mergeIssueMetricCounts<TestMetric>({}, "new", 2, 1);

    expect(next.new).toEqual({ comments: 2, runs: 1 });
  });
});

describe("reuseUnchangedIssueMetrics", () => {
  it("keeps the current map when refreshed values are equivalent", () => {
    const current = {
      first: {
        comments: 1,
        latestTouch: { actor: "human", createdAt: "2026-08-11T00:00:00Z", group: "human", label: "Review" },
        runs: 2,
      },
    };
    const next = {
      first: {
        comments: 1,
        latestTouch: { actor: "human", createdAt: "2026-08-11T00:00:00Z", group: "human", label: "Review" },
        runs: 2,
      },
    };

    expect(reuseUnchangedIssueMetrics(current, next)).toBe(current);
  });

  it("returns the refreshed map when a count or latest touch changes", () => {
    const current = { first: { comments: 1, runs: 2 } };
    const next = { first: { comments: 2, runs: 2 } };

    expect(reuseUnchangedIssueMetrics(current, next)).toBe(next);
  });
});
