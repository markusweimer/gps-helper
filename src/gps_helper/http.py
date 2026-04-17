"""HTTP session with retries, rate limiting, and a polite User-Agent."""
from __future__ import annotations

import threading
import time
from typing import Optional

import requests
from requests.adapters import HTTPAdapter

try:  # urllib3 v2
    from urllib3.util.retry import Retry
except ImportError:  # pragma: no cover
    from requests.packages.urllib3.util.retry import Retry  # type: ignore

from . import __version__

DEFAULT_USER_AGENT = (
    f"gps-helper/{__version__} "
    "(+https://github.com/gps-helper; OSM map-matching client)"
)


class RateLimiter:
    """Simple sleep-based rate limiter (requests per second)."""

    def __init__(self, rate: float) -> None:
        self.min_interval = 1.0 / rate if rate and rate > 0 else 0.0
        self._last = 0.0
        self._lock = threading.Lock()

    def wait(self) -> None:
        if self.min_interval <= 0:
            return
        with self._lock:
            now = time.monotonic()
            delay = self._last + self.min_interval - now
            if delay > 0:
                time.sleep(delay)
                now = time.monotonic()
            self._last = now


class HttpClient:
    """requests.Session wrapper: retries on 429/5xx, optional rate limiting."""

    def __init__(
        self,
        *,
        user_agent: str = DEFAULT_USER_AGENT,
        timeout: float = 30.0,
        rate_limit: float = 0.0,
        total_retries: int = 5,
        backoff_factor: float = 1.0,
    ) -> None:
        self.timeout = timeout
        self.rate_limiter = RateLimiter(rate_limit)
        self.session = requests.Session()
        self.session.headers["User-Agent"] = user_agent

        retry = Retry(
            total=total_retries,
            status_forcelist=(429, 500, 502, 503, 504),
            allowed_methods=frozenset(["GET", "POST"]),
            backoff_factor=backoff_factor,
            respect_retry_after_header=True,
            raise_on_status=False,
        )
        adapter = HTTPAdapter(max_retries=retry)
        self.session.mount("http://", adapter)
        self.session.mount("https://", adapter)

    def get(self, url: str, **kwargs) -> requests.Response:
        self.rate_limiter.wait()
        kwargs.setdefault("timeout", self.timeout)
        resp = self.session.get(url, **kwargs)
        resp.raise_for_status()
        return resp

    def post(self, url: str, **kwargs) -> requests.Response:
        self.rate_limiter.wait()
        kwargs.setdefault("timeout", self.timeout)
        resp = self.session.post(url, **kwargs)
        resp.raise_for_status()
        return resp

    def close(self) -> None:
        self.session.close()

    def __enter__(self) -> "HttpClient":
        return self

    def __exit__(self, *exc) -> None:
        self.close()
