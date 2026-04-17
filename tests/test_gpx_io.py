from gps_helper.gpx_io import load_points, write_points
from gps_helper.model import TracePoint


def test_way_id_roundtrip(tmp_path):
    src = tmp_path / "in.gpx"
    src.write_text(
        """<?xml version='1.0' encoding='UTF-8'?>
<gpx version='1.1' creator='test' xmlns='http://www.topografix.com/GPX/1/1'>
  <trk><trkseg>
    <trkpt lat='40.0' lon='-105.0'><ele>1600</ele></trkpt>
    <trkpt lat='40.001' lon='-105.001'></trkpt>
  </trkseg></trk>
</gpx>
"""
    )
    pts = load_points(str(src))
    assert len(pts) == 2
    assert pts[0].way_id is None

    pts[0].way_id = 42
    pts[1].way_id = 99
    out = tmp_path / "out.gpx"
    write_points(pts, str(out))

    reread = load_points(str(out))
    assert [p.way_id for p in reread] == [42, 99]
    # Coordinates preserved
    assert abs(reread[0].lat - 40.0) < 1e-9
    assert abs(reread[1].lon - -105.001) < 1e-9
    # Elevation carried through
    assert reread[0].elevation == 1600.0


def test_write_replaces_snapped_coords(tmp_path):
    src = tmp_path / "in.gpx"
    src.write_text(
        """<?xml version='1.0' encoding='UTF-8'?>
<gpx version='1.1' creator='test' xmlns='http://www.topografix.com/GPX/1/1'>
  <trk><trkseg>
    <trkpt lat='40.0' lon='-105.0'></trkpt>
  </trkseg></trk>
</gpx>
"""
    )
    pts = load_points(str(src))
    # Simulate snapping: move the point and assign a way.
    snapped = [TracePoint(lat=40.5, lon=-105.5, way_id=7, source=pts[0].source)]
    out = tmp_path / "out.gpx"
    write_points(snapped, str(out))

    reread = load_points(str(out))
    assert abs(reread[0].lat - 40.5) < 1e-9
    assert abs(reread[0].lon - -105.5) < 1e-9
    assert reread[0].way_id == 7
