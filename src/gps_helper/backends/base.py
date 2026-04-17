"""Map-matching backend protocol and registry."""
from __future__ import annotations

from abc import ABC, abstractmethod
from typing import Dict, List, Sequence, Type

from ..model import TracePoint


class Backend(ABC):
    """A map-matching backend.

    Implementations take a list of raw TracePoints and return the same
    number of TracePoints, snapped to roads, with .way_id populated where
    the backend could identify a road (or None on match failure).
    """

    name: str = ""
    default_url: str = ""

    def __init__(self, url: str | None = None, http=None) -> None:
        self.url = (url or self.default_url).rstrip("/")
        self.http = http

    @abstractmethod
    def match(self, points: Sequence[TracePoint]) -> List[TracePoint]:
        ...


_REGISTRY: Dict[str, Type[Backend]] = {}


def register(cls: Type[Backend]) -> Type[Backend]:
    if not cls.name:
        raise ValueError("Backend must set a non-empty `name`")
    _REGISTRY[cls.name] = cls
    return cls


def available() -> List[str]:
    return sorted(_REGISTRY)


def get(name: str) -> Type[Backend]:
    try:
        return _REGISTRY[name]
    except KeyError as e:
        raise ValueError(
            f"Unknown backend {name!r}. Available: {', '.join(available())}"
        ) from e
