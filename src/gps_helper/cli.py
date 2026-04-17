"""Command-line interface."""
from __future__ import annotations

import argparse
import sys
from typing import List, Optional, Sequence

from . import __version__
from . import backends as backends_mod
from .align import align as align_points
from .gpx_io import has_any_way_id, load_points, write_points
from .http import HttpClient
from .model import TracePoint
from .simplify import simplify as simplify_points


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="gps-helper",
        description=(
            "Snap GPS traces to OSM roads and/or simplify them by keeping "
            "only points near road changes."
        ),
    )
    p.add_argument("--version", action="version", version=f"%(prog)s {__version__}")
    sub = p.add_subparsers(dest="command", required=True)

    def add_backend_flags(sp: argparse.ArgumentParser) -> None:
        sp.add_argument(
            "--backend",
            choices=backends_mod.available(),
            default="valhalla",
            help="Map-matching backend (default: valhalla).",
        )
        sp.add_argument("--backend-url", default=None,
                        help="Override the backend base URL.")
        sp.add_argument("--chunk-size", type=int, default=100,
                        help="Max points per backend request (default: 100).")
        sp.add_argument("--overlap", type=int, default=5,
                        help="Overlapping points between chunks (default: 5).")
        sp.add_argument("--rate-limit", type=float, default=1.0,
                        help="Max requests per second (default: 1.0).")
        sp.add_argument("--timeout", type=float, default=30.0,
                        help="HTTP timeout in seconds (default: 30).")

    def add_io_flags(sp: argparse.ArgumentParser) -> None:
        sp.add_argument("input", help="Input GPX file.")
        sp.add_argument("-o", "--output", required=True, help="Output GPX file.")

    sp_align = sub.add_parser("align", help="Snap a GPX trace to OSM roads.")
    add_io_flags(sp_align)
    add_backend_flags(sp_align)

    sp_simp = sub.add_parser(
        "simplify",
        help="Simplify an aligned GPX trace (keep points near road changes).",
    )
    add_io_flags(sp_simp)
    sp_simp.add_argument("--context", type=int, default=2,
                         help="Points to keep before & after each road change "
                              "(default: 2).")

    sp_proc = sub.add_parser(
        "process", help="Align then simplify in a single run.",
    )
    add_io_flags(sp_proc)
    add_backend_flags(sp_proc)
    sp_proc.add_argument("--context", type=int, default=2,
                         help="Points to keep before & after each road change "
                              "(default: 2).")

    return p


def _make_backend(args) -> tuple:
    http = HttpClient(timeout=args.timeout, rate_limit=args.rate_limit)
    cls = backends_mod.get(args.backend)
    return cls(url=args.backend_url, http=http), http


def _do_align(points: Sequence[TracePoint], args) -> List[TracePoint]:
    backend, http = _make_backend(args)
    try:
        return align_points(
            points, backend,
            chunk_size=args.chunk_size, overlap=args.overlap,
        )
    finally:
        http.close()


def main(argv: Optional[Sequence[str]] = None) -> int:
    args = _build_parser().parse_args(argv)

    if args.command == "align":
        points = load_points(args.input)
        aligned = _do_align(points, args)
        write_points(aligned, args.output)
        print(f"Aligned {len(points)} -> {len(aligned)} points "
              f"(wrote {args.output}).", file=sys.stderr)
        return 0

    if args.command == "simplify":
        points = load_points(args.input)
        if not has_any_way_id(points):
            print(
                "error: input has no <gh:way_id> extensions; run "
                "`gps-helper align` first or use `gps-helper process`.",
                file=sys.stderr,
            )
            return 2
        result = simplify_points(points, context=args.context)
        write_points(result, args.output)
        print(f"Simplified {len(points)} -> {len(result)} points "
              f"(wrote {args.output}).", file=sys.stderr)
        return 0

    if args.command == "process":
        points = load_points(args.input)
        aligned = _do_align(points, args)
        result = simplify_points(aligned, context=args.context)
        write_points(result, args.output)
        print(
            f"Processed {len(points)} -> aligned {len(aligned)} -> "
            f"simplified {len(result)} points (wrote {args.output}).",
            file=sys.stderr,
        )
        return 0

    return 1  # pragma: no cover


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
