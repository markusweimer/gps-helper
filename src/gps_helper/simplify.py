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


def _is_transition(prev_way: object, cur_way: object) -> bool:
    # None is always a transition (match failure => preserve context).
    if prev_way is None or cur_way is None:
        return True
    return prev_way != cur_way
