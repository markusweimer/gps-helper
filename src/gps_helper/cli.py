"""Command-line interface."""
from __future__ import annotations

import argparse
import os
import sys
from typing import List, Optional, Sequence

from . import __version__
from . import backends as backends_mod
from .align import align as align_points
from .gpx_io import has_any_way_id, load_points, write_points, write_route
from .http import HttpClient
from .model import TracePoint
from .simplify import simplify as simplify_points
from .simplify import simplify_to_route


def _progress(done: int, total: int, chunk: int, total_chunks: int) -> None:
    pct = done * 100 // total if total else 100
    print(
        f"\r  Aligning: chunk {chunk}/{total_chunks} "
        f"({pct}% of points)",
        end="", flush=True, file=sys.stderr,
    )


def _finish_progress() -> None:
    print(file=sys.stderr)  # newline after \r progress


def _suffixed_path(base: str, suffix: str) -> str:
    """Derive a suffixed output path: 'ride.gpx' + 'aligned' -> 'ride.aligned.gpx'."""
    root, ext = os.path.splitext(base)
    return f"{root}.{suffix}{ext}"


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
    sp_simp.add_argument("--format", "-f", choices=["track", "route"],
                         default="track",
                         help="Output format: track (<trk>) or route (<rte>). "
                              "Default: track.")

    sp_proc = sub.add_parser(
        "process", help="Align then simplify in a single run.",
    )
    sp_proc.add_argument("input", help="Input GPX file.")
    sp_proc.add_argument("-o", "--output", default=None,
                         help="Base name for outputs. Defaults to input name. "
                              "Produces <base>.aligned.gpx, <base>.simplified.gpx, "
                              "<base>.route.gpx.")
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
        result = align_points(
            points, backend,
            chunk_size=args.chunk_size, overlap=args.overlap,
            on_progress=_progress,
        )
        _finish_progress()
        return result
    finally:
        http.close()


def main(argv: Optional[Sequence[str]] = None) -> int:
    args = _build_parser().parse_args(argv)

    if args.command == "align":
        print(f"Loading {args.input} ...", file=sys.stderr)
        points = load_points(args.input)
        print(f"  {len(points)} points loaded.", file=sys.stderr)
        aligned = _do_align(points, args)
        print(f"Writing {args.output} ...", file=sys.stderr)
        write_points(aligned, args.output)
        print(f"Done: {len(points)} -> {len(aligned)} points.", file=sys.stderr)
        return 0

    if args.command == "simplify":
        print(f"Loading {args.input} ...", file=sys.stderr)
        points = load_points(args.input)
        print(f"  {len(points)} points loaded.", file=sys.stderr)
        if not has_any_way_id(points):
            print(
                "error: input has no <gh:way_id> extensions; run "
                "`gps-helper align` first or use `gps-helper process`.",
                file=sys.stderr,
            )
            return 2
        if args.format == "route":
            result = simplify_to_route(points)
            print(f"Writing route {args.output} ...", file=sys.stderr)
            write_route(result, args.output)
        else:
            result = simplify_points(points, context=args.context)
            print(f"Writing {args.output} ...", file=sys.stderr)
            write_points(result, args.output)
        print(f"Done: {len(points)} -> {len(result)} points "
              f"({len(points) - len(result)} removed).", file=sys.stderr)
        return 0

    if args.command == "process":
        base = args.output or args.input
        path_aligned = _suffixed_path(base, "aligned")
        path_simplified = _suffixed_path(base, "simplified")
        path_route = _suffixed_path(base, "route")

        print(f"Loading {args.input} ...", file=sys.stderr)
        points = load_points(args.input)
        print(f"  {len(points)} points loaded.", file=sys.stderr)
        aligned = _do_align(points, args)

        print(f"Writing {path_aligned} ...", file=sys.stderr)
        write_points(aligned, path_aligned)

        print("Simplifying ...", file=sys.stderr)
        simplified = simplify_points(aligned, context=args.context)
        print(f"Writing {path_simplified} ...", file=sys.stderr)
        write_points(simplified, path_simplified)

        route_pts = simplify_to_route(aligned)
        print(f"Writing {path_route} ...", file=sys.stderr)
        write_route(route_pts, path_route)

        print(
            f"Done: {len(points)} points -> "
            f"{len(aligned)} aligned, "
            f"{len(simplified)} simplified, "
            f"{len(route_pts)} route points.",
            file=sys.stderr,
        )
        print(f"  {path_aligned}", file=sys.stderr)
        print(f"  {path_simplified}", file=sys.stderr)
        print(f"  {path_route}", file=sys.stderr)
        return 0

    return 1  # pragma: no cover


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
