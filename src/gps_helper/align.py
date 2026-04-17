"""Align orchestration: chunking, backend invocation, stitching."""
from __future__ import annotations

from typing import List, Sequence

from .backends.base import Backend
from .model import TracePoint


def align(
    points: Sequence[TracePoint],
    backend: Backend,
    *,
    chunk_size: int = 100,
    overlap: int = 5,
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
    out: List[TracePoint] = []
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
        if end == n:
            break
        i += step
    return out
