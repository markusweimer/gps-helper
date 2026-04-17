"""Shared data types."""
from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional


@dataclass
class TracePoint:
    """A single GPS trackpoint passed to a matcher."""

    lat: float
    lon: float
    elevation: Optional[float] = None
    time: Optional[datetime] = None
    # Opaque per-point metadata preserved across align/simplify
    # (e.g. way_id once matched).
    way_id: Optional[int] = None
    # Original gpxpy trackpoint reference, kept so we can re-emit the point
    # with its original extensions/extras untouched.
    source: object = field(default=None, repr=False, compare=False)
