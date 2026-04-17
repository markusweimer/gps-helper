# gps-helper

A small CLI that makes dense car-driving GPS traces friendly to downstream
route planners. It:

1. **Aligns** every trackpoint to the nearest OSM road using a map-matching
   service, and
2. **Simplifies** the aligned trace by dropping any point whose road
   (OSM `way_id`) is the same as its predecessor, keeping a couple of
   points before and after each road change so a router can reconstruct
   the geometry.

## Install

```bash
pip install -e .
```

This exposes a `gps-helper` console command.

## Usage

```bash
# Align only (snap to roads, annotate each point with its OSM way_id):
gps-helper align input.gpx -o aligned.gpx

# Simplify an already-aligned GPX:
gps-helper simplify aligned.gpx -o simplified.gpx

# Do both in one pass:
gps-helper process input.gpx -o simplified.gpx
```

### Common options

| Flag | Default | Notes |
| --- | --- | --- |
| `--backend {valhalla,osrm}` | `valhalla` | Map-matching backend. |
| `--backend-url URL` | backend-specific | Self-hosted endpoint override. |
| `--chunk-size N` | `100` | Points per request to the backend. |
| `--overlap N` | `5` | Overlap between successive chunks. |
| `--rate-limit R` | `1.0` | Max requests/second (polite default). |
| `--timeout S` | `30` | HTTP timeout in seconds. |
| `--context N` | `2` | Points to keep before/after each road change (`simplify`, `process`). |

## Backends

- **`valhalla`** (default) uses the `trace_attributes` endpoint. It returns
  OSM `way_id` per matched point natively, so road-change detection is
  accurate. Public default: `https://valhalla1.openstreetmap.de`.
- **`osrm`** uses the `match` service. OSRM does not expose `way_id`
  directly, so `gps-helper` resolves them via a batched Overpass query
  on the nearest matched nodes (cached per run). This is noticeably
  slower and more rate-limited than Valhalla. Public defaults:
  `https://router.project-osrm.org` and
  `https://overpass-api.de/api/interpreter`.

For anything more than occasional use please **self-host** Valhalla or
OSRM (and Overpass) and pass `--backend-url`. The public endpoints are
community services with strict rate limits.

## How simplification works

After alignment each trackpoint carries an OSM `way_id` (stored in a
`<gh:way_id>` GPX extension). The simplifier:

1. Always keeps the first and last point.
2. Scans consecutive points; whenever the `way_id` changes (or is
   unknown for either point), it keeps the preceding `N` points and the
   following `N` points (default `N=2`, set with `--context`).

All other points are dropped. The result retains enough shape around
junctions for a router to reconnect the dots while shedding the bulk of
redundant straight-line points.

## Development

```bash
pip install -e '.[dev]'
pytest
```
