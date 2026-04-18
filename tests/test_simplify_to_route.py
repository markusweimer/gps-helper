from gps_helper.model import TracePoint
from gps_helper.simplify import simplify_to_route


def tp(i, way_id, name=None):
    return TracePoint(lat=float(i), lon=float(i), way_id=way_id, road_name=name)


def ids(points):
    return [int(p.lat) for p in points]


def test_empty():
    assert simplify_to_route([]) == []


def test_single():
    assert ids(simplify_to_route([tp(0, 1)])) == [0]


def test_all_same_way():
    # 10 points on the same road -> first, midpoint, last
    pts = [tp(i, 1) for i in range(10)]
    out = simplify_to_route(pts)
    assert ids(out) == [0, 5, 9]


def test_two_roads():
    # 4 pts on road 1, 4 pts on road 2
    pts = [tp(i, 1) for i in range(4)] + [tp(i, 2) for i in range(4, 8)]
    out = simplify_to_route(pts)
    # first=0, mid of run1=2, mid of run2=6, last=7
    assert ids(out) == [0, 2, 6, 7]


def test_unknown_way_ids_each_own_run():
    pts = [tp(0, 1), tp(1, None), tp(2, None), tp(3, 1)]
    out = simplify_to_route(pts)
    # None points each form their own run -> lots of representatives
    # first=0, mid(0,1)=0 (dup), mid(1,2)=1, mid(2,3)=2, mid(3,4)=3 (last dup)
    assert 0 in ids(out) and 3 in ids(out)
    assert len(out) >= 3


def test_preserves_road_names():
    pts = [tp(0, 1, "Main St"), tp(1, 1, "Main St"),
           tp(2, 2, "Oak Ave"), tp(3, 2, "Oak Ave")]
    out = simplify_to_route(pts)
    names = [p.road_name for p in out]
    assert "Main St" in names
    assert "Oak Ave" in names


def test_three_roads():
    pts = (
        [tp(i, 1) for i in range(4)]
        + [tp(i, 2) for i in range(4, 8)]
        + [tp(i, 3) for i in range(8, 12)]
    )
    out = simplify_to_route(pts)
    # first, mid of each run, last
    assert ids(out) == [0, 2, 6, 10, 11]
