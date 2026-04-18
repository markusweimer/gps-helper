"""GPX read/write with gps-helper way_id extension support."""
from __future__ import annotations

from typing import Iterable, List, Optional, Sequence

import gpxpy
import gpxpy.gpx
from lxml import etree

from .model import TracePoint

# Namespace for our per-point extensions.
GH_NS = "https://github.com/gps-helper/gps-helper"
GH_PREFIX = "gh"
_WAY_ID_TAG = f"{{{GH_NS}}}way_id"


def load_points(path: str) -> List[TracePoint]:
    """Read all trackpoints from a GPX file into TracePoint objects."""
    with open(path, "r", encoding="utf-8") as f:
        gpx = gpxpy.parse(f)
    points: List[TracePoint] = []
    for track in gpx.tracks:
        for segment in track.segments:
            for pt in segment.points:
                points.append(
                    TracePoint(
                        lat=pt.latitude,
                        lon=pt.longitude,
                        elevation=pt.elevation,
                        time=pt.time,
                        way_id=_read_way_id(pt),
                        source=pt,
                    )
                )
    return points


def _read_way_id(pt: gpxpy.gpx.GPXTrackPoint) -> Optional[int]:
    for ext in getattr(pt, "extensions", None) or []:
        if getattr(ext, "tag", None) == _WAY_ID_TAG:
            text = (ext.text or "").strip()
            if text:
                try:
                    return int(text)
                except ValueError:
                    return None
    return None


def _set_way_id(pt: gpxpy.gpx.GPXTrackPoint, way_id: Optional[int]) -> None:
    # Remove any existing way_id extension first.
    exts = list(getattr(pt, "extensions", None) or [])
    exts = [e for e in exts if getattr(e, "tag", None) != _WAY_ID_TAG]
    if way_id is not None:
        el = etree.Element(_WAY_ID_TAG, nsmap={GH_PREFIX: GH_NS})
        el.text = str(way_id)
        exts.append(el)
    pt.extensions = exts


def write_points(points: Sequence[TracePoint], path: str) -> None:
    """Write points to a GPX file as a single track/segment.

    Preserves per-point extras from `source` when available, and replaces
    lat/lon with the (possibly snapped) values from the TracePoint. The
    way_id (if any) is written as a <gh:way_id> extension.
    """
    gpx = gpxpy.gpx.GPX()
    gpx.nsmap[GH_PREFIX] = GH_NS
    track = gpxpy.gpx.GPXTrack()
    gpx.tracks.append(track)
    segment = gpxpy.gpx.GPXTrackSegment()
    track.segments.append(segment)

    for tp in points:
        src: Optional[gpxpy.gpx.GPXTrackPoint] = tp.source if isinstance(
            tp.source, gpxpy.gpx.GPXTrackPoint
        ) else None
        new_pt = gpxpy.gpx.GPXTrackPoint(
            latitude=tp.lat,
            longitude=tp.lon,
            elevation=tp.elevation if tp.elevation is not None
            else (src.elevation if src else None),
            time=tp.time if tp.time is not None else (src.time if src else None),
        )
        if src is not None:
            # Carry over optional fields.
            for attr in ("name", "comment", "symbol", "type",
                         "horizontal_dilution", "vertical_dilution",
                         "position_dilution", "speed", "magnetic_variation"):
                val = getattr(src, attr, None)
                if val is not None:
                    setattr(new_pt, attr, val)
            # Copy non-way_id extensions verbatim.
            for ext in getattr(src, "extensions", None) or []:
                if getattr(ext, "tag", None) != _WAY_ID_TAG:
                    new_pt.extensions.append(ext)
        _set_way_id(new_pt, tp.way_id)
        segment.points.append(new_pt)

    with open(path, "w", encoding="utf-8") as f:
        f.write(gpx.to_xml())


def write_route(points: Sequence[TracePoint], path: str) -> None:
    """Write points as a GPX route (<rte>/<rtept>).

    Each routepoint gets a <name> from road_name or way_id for readability
    in GPS apps.
    """
    gpx = gpxpy.gpx.GPX()
    route = gpxpy.gpx.GPXRoute()
    gpx.routes.append(route)

    for tp in points:
        name = tp.road_name or (f"way {tp.way_id}" if tp.way_id else None)
        rpt = gpxpy.gpx.GPXRoutePoint(
            latitude=tp.lat,
            longitude=tp.lon,
            elevation=tp.elevation,
            name=name,
        )
        route.points.append(rpt)

    with open(path, "w", encoding="utf-8") as f:
        f.write(gpx.to_xml())


def has_any_way_id(points: Iterable[TracePoint]) -> bool:
    return any(p.way_id is not None for p in points)
