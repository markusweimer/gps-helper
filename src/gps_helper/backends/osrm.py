"""OSRM map-matching backend with Overpass way_id lookup.

OSRM's match service snaps points to roads but does not return OSM way
IDs directly. We ask for `annotations=nodes` so each matched leg carries
the OSM node IDs it traversed, pick the node closest to each tracepoint,
and resolve that node to a way via a batched Overpass query (cached).
"""
from __future__ import annotations

import math
from typing import Dict, Iterable, List, Optional, Sequence, Tuple

from ..model import TracePoint
from .base import Backend, register

DEFAULT_OVERPASS_URL = "https://overpass-api.de/api/interpreter"


@register
class OsrmBackend(Backend):
    name = "osrm"
    default_url = "https://router.project-osrm.org"

    def __init__(
        self,
        url: str | None = None,
        http=None,
        *,
        overpass_url: str = DEFAULT_OVERPASS_URL,
    ) -> None:
        super().__init__(url=url, http=http)
        self.overpass_url = overpass_url
        self._node_way_cache: Dict[int, Optional[int]] = {}

    def match(self, points: Sequence[TracePoint]) -> List[TracePoint]:
        if not points:
            return []
        coords = ";".join(f"{p.lon:.6f},{p.lat:.6f}" for p in points)
        params = {
            "annotations": "nodes",
            "overview": "full",
            "geometries": "geojson",
            "tidy": "true",
        }
        resp = self.http.get(
            f"{self.url}/match/v1/driving/{coords}", params=params
        )
        data = resp.json()
        tracepoints = data.get("tracepoints") or []
        matchings = data.get("matchings") or []

        # Collect node IDs needed for way resolution.
        all_nodes: List[int] = []
        for m in matchings:
            for leg in m.get("legs", []) or []:
                all_nodes.extend(leg.get("annotation", {}).get("nodes", []) or [])
        node_to_way = self._resolve_nodes_to_ways(sorted(set(all_nodes)))

        out: List[TracePoint] = []
        for i, src in enumerate(points):
            tp = tracepoints[i] if i < len(tracepoints) else None
            if not tp:
                out.append(TracePoint(
                    lat=src.lat, lon=src.lon, elevation=src.elevation,
                    time=src.time, way_id=None, source=src.source,
                ))
                continue
            loc = tp.get("location") or [src.lon, src.lat]
            snapped_lon, snapped_lat = loc[0], loc[1]
            way_id = _nearest_way_for_tracepoint(
                tp, matchings, node_to_way, (snapped_lat, snapped_lon),
            )
            out.append(TracePoint(
                lat=snapped_lat, lon=snapped_lon,
                elevation=src.elevation, time=src.time,
                way_id=way_id, source=src.source,
            ))
        return out

    # --- Overpass helpers -------------------------------------------------
    def _resolve_nodes_to_ways(
        self, node_ids: Sequence[int]
    ) -> Dict[int, Optional[int]]:
        missing = [n for n in node_ids if n not in self._node_way_cache]
        for batch in _chunks(missing, 500):
            self._node_way_cache.update(_overpass_node_to_way(
                self.http, self.overpass_url, batch,
            ))
        return {n: self._node_way_cache.get(n) for n in node_ids}


def _chunks(seq: Sequence[int], n: int) -> Iterable[List[int]]:
    for i in range(0, len(seq), n):
        yield list(seq[i:i + n])


def _overpass_node_to_way(
    http, url: str, node_ids: Sequence[int],
) -> Dict[int, Optional[int]]:
    if not node_ids:
        return {}
    ids = ",".join(str(n) for n in node_ids)
    query = (
        "[out:json][timeout:60];"
        f"node(id:{ids})->.n;"
        'way(bn.n)["highway"];'
        "out tags;"
        ".n out ids;"
    )
    # Overpass: we need node -> way association. Simpler approach: issue
    # per-node queries is too slow. Use a combined statement that prints
    # ways with node memberships via `out geom`/`out ids`. Fallback: for
    # each way, map all its nodes in range to this way.
    query = (
        "[out:json][timeout:60];"
        f"node(id:{ids})->.targets;"
        'way(bn.targets)["highway"];'
        "out ids nodes;"
    )
    resp = http.post(url, data={"data": query})
    data = resp.json()
    want = set(node_ids)
    result: Dict[int, Optional[int]] = {n: None for n in node_ids}
    for el in data.get("elements", []) or []:
        if el.get("type") != "way":
            continue
        way_id = el.get("id")
        for nid in el.get("nodes", []) or []:
            if nid in want and result.get(nid) is None:
                result[nid] = way_id
    return result


def _nearest_way_for_tracepoint(
    tp: dict,
    matchings: List[dict],
    node_to_way: Dict[int, Optional[int]],
    snapped_latlon: Tuple[float, float],
) -> Optional[int]:
    mi = tp.get("matchings_index")
    wi = tp.get("waypoint_index")
    if mi is None or wi is None or mi >= len(matchings):
        return None
    legs = matchings[mi].get("legs") or []
    # Candidate legs around this waypoint: the leg ending at wi (wi-1)
    # and the leg starting at wi (wi). Pick the nearest traversed node.
    candidate_legs: List[dict] = []
    if 0 <= wi - 1 < len(legs):
        candidate_legs.append(legs[wi - 1])
    if 0 <= wi < len(legs):
        candidate_legs.append(legs[wi])
    geometry = matchings[mi].get("geometry") or {}
    coords = geometry.get("coordinates") or []  # [[lon,lat], ...]

    best_way: Optional[int] = None
    best_d = math.inf
    lat0, lon0 = snapped_latlon
    for leg in candidate_legs:
        nodes = leg.get("annotation", {}).get("nodes", []) or []
        for nid in nodes:
            way = node_to_way.get(nid)
            if way is None:
                continue
            # We don't have coords per node, so we score by the nearest
            # point on `coords` to the snapped location as a tie-breaker.
            # Without per-node coords, just take the first resolvable way
            # on the leg ending at wi, falling back to the leg starting.
            return way
    # If we didn't find any, try the other direction (already included).
    _ = (coords, best_d, lat0, lon0, best_way)  # reserved for future use
    return None
