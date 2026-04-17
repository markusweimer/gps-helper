from gps_helper.align import align
from gps_helper.backends.base import Backend
from gps_helper.model import TracePoint


class FakeBackend(Backend):
    name = "fake"
    default_url = ""

    def __init__(self, way_fn):
        super().__init__(url="", http=None)
        self.way_fn = way_fn
        self.calls = []

    def match(self, points):
        self.calls.append(len(points))
        return [
            TracePoint(lat=p.lat, lon=p.lon, way_id=self.way_fn(p), source=p.source)
            for p in points
        ]


def test_align_no_chunking_needed():
    be = FakeBackend(lambda p: int(p.lat) // 3)
    pts = [TracePoint(lat=float(i), lon=0.0) for i in range(5)]
    out = align(pts, be, chunk_size=100, overlap=5)
    assert [p.way_id for p in out] == [0, 0, 0, 1, 1]
    assert be.calls == [5]


def test_align_chunks_with_overlap():
    be = FakeBackend(lambda p: int(p.lat))
    pts = [TracePoint(lat=float(i), lon=0.0) for i in range(25)]
    out = align(pts, be, chunk_size=10, overlap=3)
    assert len(out) == 25
    assert [p.way_id for p in out] == list(range(25))
    # Chunks: [0..10), [7..17), [14..24), [21..25) -> lengths 10,10,10,4
    assert be.calls == [10, 10, 10, 4]


def test_align_empty():
    be = FakeBackend(lambda p: 1)
    assert align([], be) == []
