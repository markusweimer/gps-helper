from gps_helper.model import TracePoint
from gps_helper.simplify import simplify


def tp(i, way_id):
    return TracePoint(lat=float(i), lon=float(i), way_id=way_id)


def ids(points):
    return [int(p.lat) for p in points]


def test_empty():
    assert simplify([]) == []


def test_single():
    assert ids(simplify([tp(0, 1)])) == [0]


def test_all_same_way_keeps_endpoints_only():
    pts = [tp(i, 1) for i in range(5)]
    assert ids(simplify(pts)) == [0, 4]


def test_transition_keeps_context_default_2():
    # Road changes between index 3 and 4
    pts = [tp(i, 1) for i in range(4)] + [tp(i, 2) for i in range(4, 8)]
    # context=2 -> keep [2,3] before and [4,5] after, plus endpoints
    assert ids(simplify(pts, context=2)) == [0, 2, 3, 4, 5, 7]


def test_transition_context_zero_still_keeps_transition_boundaries():
    pts = [tp(i, 1) for i in range(3)] + [tp(i, 2) for i in range(3, 6)]
    # context=0 keeps nothing around the transition besides endpoints
    assert ids(simplify(pts, context=0)) == [0, 5]


def test_unknown_way_id_forces_transition():
    pts = [tp(0, 1), tp(1, 1), tp(2, None), tp(3, 1), tp(4, 1)]
    # transitions at i=2 (1->None) and i=3 (None->1) with context=1
    out = ids(simplify(pts, context=1))
    assert out == [0, 1, 2, 3, 4]


def test_multiple_transitions():
    pts = (
        [tp(i, 1) for i in range(3)]
        + [tp(i, 2) for i in range(3, 6)]
        + [tp(i, 3) for i in range(6, 9)]
    )
    # transitions at 3 and 6, context=1
    assert ids(simplify(pts, context=1)) == [0, 2, 3, 5, 6, 8]


def test_order_preserved_and_deduped():
    pts = [tp(i, 1 if i < 2 else 2) for i in range(4)]
    out = simplify(pts, context=3)
    # context covers whole range; every point kept exactly once
    assert ids(out) == [0, 1, 2, 3]
