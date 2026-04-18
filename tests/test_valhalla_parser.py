import json
import pathlib

from gps_helper.backends.valhalla import parse_trace_attributes
from gps_helper.model import TracePoint

FIXTURE = pathlib.Path(__file__).parent / "fixtures" / "valhalla_trace_attributes.json"


def test_parse_trace_attributes():
    data = json.loads(FIXTURE.read_text())
    pts = [TracePoint(lat=0.0, lon=0.0) for _ in range(5)]
    out = parse_trace_attributes(data, pts)
    assert [p.way_id for p in out] == [1001, 1001, 1002, None, 1003]
    assert [p.road_name for p in out] == [
        "Main Street", "Main Street", "Oak Avenue", None, "Oak Avenue",
    ]
    # Coordinates replaced with matched coordinates
    assert abs(out[0].lat - 40.0) < 1e-9
    assert abs(out[4].lon - -105.004) < 1e-9


def test_parse_trace_attributes_passthrough_on_short_response():
    data = {"edges": [], "matched_points": []}
    pts = [TracePoint(lat=1.0, lon=2.0), TracePoint(lat=3.0, lon=4.0)]
    out = parse_trace_attributes(data, pts)
    assert [(p.lat, p.lon, p.way_id) for p in out] == [
        (1.0, 2.0, None), (3.0, 4.0, None),
    ]
