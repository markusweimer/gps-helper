"""Valhalla map-matching backend (trace_attributes).

Returns per-point OSM way_id via edge lookups in the response.
"""
from __future__ import annotations

from typing import List, Sequence

from ..model import TracePoint
from .base import Backend, register


@register
class ValhallaBackend(Backend):
    name = "valhalla"
    default_url = "https://valhalla1.openstreetmap.de"

    def match(self, points: Sequence[TracePoint]) -> List[TracePoint]:
        if not points:
            return []
        shape = [{"lat": p.lat, "lon": p.lon} for p in points]
        body = {
            "shape": shape,
            "costing": "auto",
            "shape_match": "map_snap",
            "filters": {
                "attributes": [
                    "edge.way_id",
                    "edge.names",
                    "matched.point",
                    "matched.edge_index",
                    "matched.type",
                ],
                "action": "include",
            },
        }
        resp = self.http.post(f"{self.url}/trace_attributes", json=body)
        return parse_trace_attributes(resp.json(), points)


def parse_trace_attributes(
    data: dict, points: Sequence[TracePoint]
) -> List[TracePoint]:
    """Parse a Valhalla trace_attributes response into TracePoints.

    Output length matches `points`. If the response has fewer matched
    points than input (shouldn't happen with map_snap but defensive),
    missing tail points are passed through unmatched.
    """
    edges = data.get("edges", []) or []
    matched = data.get("matched_points", []) or []
    out: List[TracePoint] = []
    for i, src in enumerate(points):
        m = matched[i] if i < len(matched) else None
        lat = src.lat
        lon = src.lon
        way_id = None
        road_name = None
        if m:
            lat = m.get("lat", lat)
            lon = m.get("lon", lon)
            if m.get("type") == "unmatched":
                way_id = None
            else:
                ei = m.get("edge_index")
                if ei is not None and 0 <= ei < len(edges):
                    way_id = edges[ei].get("way_id")
                    names = edges[ei].get("names") or []
                    if names:
                        road_name = names[0]
        out.append(
            TracePoint(
                lat=lat, lon=lon,
                elevation=src.elevation, time=src.time,
                way_id=way_id, road_name=road_name, source=src.source,
            )
        )
    return out
