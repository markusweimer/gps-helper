from gps_helper.gpx_io import write_route
from gps_helper.model import TracePoint

import gpxpy


def test_write_route_produces_rte(tmp_path):
    pts = [
        TracePoint(lat=40.0, lon=-105.0, way_id=1, road_name="Main St"),
        TracePoint(lat=40.01, lon=-105.01, way_id=2, road_name="Oak Ave"),
        TracePoint(lat=40.02, lon=-105.02, way_id=3),
    ]
    out = tmp_path / "route.gpx"
    write_route(pts, str(out))

    with open(str(out)) as f:
        gpx = gpxpy.parse(f)

    assert len(gpx.routes) == 1
    assert len(gpx.tracks) == 0
    rte = gpx.routes[0]
    assert len(rte.points) == 3
    assert rte.points[0].name == "Main St"
    assert rte.points[1].name == "Oak Ave"
    assert rte.points[2].name == "way 3"
    assert abs(rte.points[0].latitude - 40.0) < 1e-9


def test_write_route_no_way_id():
    """Route point with no way_id or road_name gets no name."""
    import tempfile, os
    pts = [TracePoint(lat=40.0, lon=-105.0)]
    with tempfile.NamedTemporaryFile(suffix=".gpx", delete=False) as f:
        path = f.name
    try:
        write_route(pts, path)
        with open(path) as f:
            gpx = gpxpy.parse(f)
        assert gpx.routes[0].points[0].name is None
    finally:
        os.unlink(path)
