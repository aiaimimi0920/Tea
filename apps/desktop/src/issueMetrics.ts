export type MetricCounts = {
  comments: number;
  runs: number;
};

type MetricTouch = {
  actor: string;
  createdAt?: string;
  group: string;
  label: string;
};

type MetricSnapshot = MetricCounts & {
  latestTouch?: MetricTouch;
};

export function mergeIssueMetricCounts<T extends MetricCounts>(
  current: Record<string, T>,
  id: string,
  comments: number,
  runs: number,
): Record<string, T> {
  // Detail polling must not invalidate issue-list memos when only identities changed.
  if (current[id]?.comments === comments && current[id]?.runs === runs) return current;
  return {
    ...current,
    [id]: {
      ...current[id],
      comments,
      runs,
    } as T,
  };
}

/**
 * Keep the current map identity when a refresh produced the same user-facing
 * metric values. The API creates fresh latest-touch objects on every response,
 * so a plain object identity check would still rerender the entire issue list.
 */
export function reuseUnchangedIssueMetrics<T extends MetricSnapshot>(
  current: Record<string, T>,
  next: Record<string, T>,
): Record<string, T> {
  const currentIds = Object.keys(current);
  const nextIds = Object.keys(next);
  if (currentIds.length !== nextIds.length) return next;

  for (const id of nextIds) {
    const previous = current[id];
    const updated = next[id];
    if (!previous || previous.comments !== updated.comments || previous.runs !== updated.runs) {
      return next;
    }
    const previousTouch = previous.latestTouch;
    const updatedTouch = updated.latestTouch;
    if (
      Boolean(previousTouch) !== Boolean(updatedTouch) ||
      (previousTouch && updatedTouch &&
        (previousTouch.actor !== updatedTouch.actor ||
          previousTouch.createdAt !== updatedTouch.createdAt ||
          previousTouch.group !== updatedTouch.group ||
          previousTouch.label !== updatedTouch.label))
    ) {
      return next;
    }
  }

  return current;
}
