"""Backend registry."""
from __future__ import annotations

from .base import Backend, available, get, register  # noqa: F401
# Import concrete backends for side-effect registration.
from . import valhalla  # noqa: F401
from . import osrm  # noqa: F401
