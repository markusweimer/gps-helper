"""Trace simplification based on OSM way_id transitions."""
from __future__ import annotations

from typing import List, Sequence

from .model import TracePoint


def simplify(
    points: Sequence[TracePoint],
    *,
    context: int = 2,
) -> List[TracePoint]:
    """Drop points that don't straddle a road (way_id) change.

    - Always keep the first and last points.
    - For each index i where way_id[i] != way_id[i-1] (treating None as a
      distinct value that always differs), keep indexes [i-context .. i-1]
      and [i .. i+context-1].
    - Result preserves order and is deduplicated.
    """
    if context < 0:
        raise ValueError("context must be >= 0")
    n = len(points)
    if n == 0:
        return []
    if n == 1:
        return [points[0]]

    keep = [False] * n
    keep[0] = True
    keep[-1] = True

    for i in range(1, n):
        if _is_transition(points[i - 1].way_id, points[i].way_id):
            for j in range(max(0, i - context), i):
                keep[j] = True
            for j in range(i, min(n, i + context)):
                keep[j] = True

    return [points[i] for i in range(n) if keep[i]]


def simplify_to_route(
    points: Sequence[TracePoint],
) -> List[TracePoint]:
    """Pick one representative point per way_id run, plus first and last.

    For each consecutive run of the same way_id, the midpoint of the run
    is chosen. Points with way_id=None each form their own run.
    The result is suitable for GPX route output — much sparser than
    context-based simplification.
    """
    n = len(points)
    if n == 0:
        return []
    if n == 1:
        return [points[0]]

    # Build runs: list of (start, end_exclusive) for each way_id group.
    runs: List[tuple] = []
    run_start = 0
    for i in range(1, n):
        prev_wid = points[i - 1].way_id
        cur_wid = points[i].way_id
        if prev_wid is None or cur_wid is None or prev_wid != cur_wid:
            runs.append((run_start, i))
            run_start = i
    runs.append((run_start, n))

    keep: List[int] = []
    keep.append(0)
    for start, end in runs:
        mid = (start + end) // 2
        if mid != 0 and mid != n - 1:
            keep.append(mid)
    keep.append(n - 1)

    # Deduplicate while preserving order.
    seen: set = set()
    result: List[TracePoint] = []
    for idx in keep:
        if idx not in seen:
            seen.add(idx)
            result.append(points[idx])
    return result


def _is_transition(prev_way: object, cur_way: object) -> bool:
    # None is always a transition (match failure => preserve context).
    if prev_way is None or cur_way is None:
        return True
    return prev_way != cur_way
