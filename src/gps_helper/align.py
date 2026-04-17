"""Align orchestration: chunking, backend invocation, stitching."""
from __future__ import annotations

import math
from typing import Callable, List, Optional, Sequence

from .backends.base import Backend
from .model import TracePoint

# progress(done_points, total_points, chunk_index, total_chunks)
ProgressCallback = Callable[[int, int, int, int], None]


def align(
    points: Sequence[TracePoint],
    backend: Backend,
    *,
    chunk_size: int = 100,
    overlap: int = 5,
    on_progress: Optional[ProgressCallback] = None,
) -> List[TracePoint]:
    """Map-match `points` via `backend`, chunked for public-endpoint limits.

    Chunks overlap by `overlap` points; for the overlap region we keep
    results from the earlier chunk (they had more context on the left).
    """
    if chunk_size <= 0:
        raise ValueError("chunk_size must be positive")
    if overlap < 0 or overlap >= chunk_size:
        raise ValueError("overlap must be >= 0 and < chunk_size")
    n = len(points)
    if n == 0:
        return []

    step = chunk_size - overlap
    total_chunks = max(1, math.ceil((n - overlap) / step)) if n > chunk_size else 1
    out: List[TracePoint] = []
    chunk_idx = 0
    i = 0
    while i < n:
        end = min(i + chunk_size, n)
        chunk = list(points[i:end])
        matched = backend.match(chunk)
        if len(matched) != len(chunk):
            raise RuntimeError(
                f"Backend returned {len(matched)} points for a chunk of "
                f"{len(chunk)}"
            )
        if i == 0:
            out.extend(matched)
        else:
            # Drop the first `overlap` matched points - they duplicate
            # the tail of the previous chunk.
            out.extend(matched[overlap:])
        chunk_idx += 1
        if on_progress:
            on_progress(len(out), n, chunk_idx, total_chunks)
        if end == n:
            break
        i += step
    return out
