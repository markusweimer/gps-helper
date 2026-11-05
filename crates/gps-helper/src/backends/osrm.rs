//! OSRM map-matching backend with Overpass way ID lookup.
//!
//! OSRM's match service snaps points to roads but does not return OSM way IDs.
//! We ask for `annotations=nodes` so each matched leg carries the OSM node IDs
//! it traversed, then resolve those nodes to ways via a batched Overpass query.

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use serde::Deserialize;

use super::{Backend, invalid_response};
use crate::error::Result;
use crate::http::HttpClient;
use crate::model::TracePoint;

const OVERPASS_BATCH_SIZE: usize = 500;

/// Matches points through an OSRM server, using Overpass to find way IDs.
///
/// Resolved nodes are cached for the lifetime of the backend.
#[derive(Debug)]
pub struct OsrmBackend {
    url: String,
    overpass_url: String,
    http: HttpClient,
    node_way_cache: Mutex<HashMap<u64, Option<u64>>>,
}

impl OsrmBackend {
    /// The public demo server. Please self-host for more than occasional use.
    pub const DEFAULT_URL: &str = "https://router.project-osrm.org";
    /// The public Overpass instance.
    pub const DEFAULT_OVERPASS_URL: &str = "https://overpass-api.de/api/interpreter";

    /// Creates a backend. `url` overrides [`DEFAULT_URL`](Self::DEFAULT_URL).
    pub fn new(url: Option<&str>, http: HttpClient) -> Self {
        Self {
            url: url
                .unwrap_or(Self::DEFAULT_URL)
                .trim_end_matches('/')
                .to_owned(),
            overpass_url: Self::DEFAULT_OVERPASS_URL.to_owned(),
            http,
            node_way_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Uses a different Overpass endpoint.
    pub fn with_overpass_url(mut self, url: &str) -> Self {
        self.overpass_url = url.to_owned();
        self
    }

    fn resolve_nodes_to_ways(&self, nodes: &BTreeSet<u64>) -> Result<HashMap<u64, Option<u64>>> {
        let missing: Vec<u64> = {
            let cache = self
                .node_way_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            nodes
                .iter()
                .copied()
                .filter(|n| !cache.contains_key(n))
                .collect()
        };
        for batch in missing.chunks(OVERPASS_BATCH_SIZE) {
            let resolved = self.overpass_node_to_way(batch)?;
            let mut cache = self
                .node_way_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            cache.extend(resolved);
        }
        let cache = self
            .node_way_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        Ok(nodes
            .iter()
            .map(|&n| (n, cache.get(&n).copied().flatten()))
            .collect())
    }

    fn overpass_node_to_way(&self, nodes: &[u64]) -> Result<HashMap<u64, Option<u64>>> {
        let ids = nodes
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let query = format!(
            "[out:json][timeout:60];node(id:{ids})->.targets;way(bn.targets)[\"highway\"];out ids nodes;"
        );
        let body = self
            .http
            .post_form(&self.overpass_url, &[("data", &query)])?;
        parse_overpass(&body, nodes)
    }
}

impl Backend for OsrmBackend {
    fn match_points(&self, points: &[TracePoint]) -> Result<Vec<TracePoint>> {
        if points.is_empty() {
            return Ok(Vec::new());
        }
        let coords = points
            .iter()
            .map(|p| format!("{:.6},{:.6}", p.lon, p.lat))
            .collect::<Vec<_>>()
            .join(";");
        let url = format!("{}/match/v1/driving/{coords}", self.url);
        let body = self.http.get(
            &url,
            &[
                ("annotations", "nodes"),
                ("overview", "full"),
                ("geometries", "geojson"),
                ("tidy", "true"),
            ],
        )?;
        let response: MatchResponse =
            serde_json::from_str(&body).map_err(|e| invalid_response("osrm", e))?;

        let node_to_way = self.resolve_nodes_to_ways(&response.traversed_nodes())?;
        Ok(apply_match(&response, points, &node_to_way))
    }
}

#[derive(Deserialize)]
struct MatchResponse {
    tracepoints: Option<Vec<Option<Tracepoint>>>,
    matchings: Option<Vec<Matching>>,
}

#[derive(Deserialize)]
struct Tracepoint {
    location: Option<[f64; 2]>,
    matchings_index: Option<usize>,
    waypoint_index: Option<usize>,
}

#[derive(Deserialize)]
struct Matching {
    legs: Option<Vec<Leg>>,
}

#[derive(Deserialize)]
struct Leg {
    annotation: Option<Annotation>,
}

#[derive(Deserialize)]
struct Annotation {
    nodes: Option<Vec<u64>>,
}

impl Leg {
    fn nodes(&self) -> &[u64] {
        self.annotation
            .as_ref()
            .and_then(|a| a.nodes.as_deref())
            .unwrap_or_default()
    }
}

impl MatchResponse {
    fn matchings(&self) -> &[Matching] {
        self.matchings.as_deref().unwrap_or_default()
    }

    fn traversed_nodes(&self) -> BTreeSet<u64> {
        self.matchings()
            .iter()
            .flat_map(|m| m.legs.as_deref().unwrap_or_default())
            .flat_map(|leg| leg.nodes().iter().copied())
            .collect()
    }
}

fn apply_match(
    response: &MatchResponse,
    points: &[TracePoint],
    node_to_way: &HashMap<u64, Option<u64>>,
) -> Vec<TracePoint> {
    let tracepoints = response.tracepoints.as_deref().unwrap_or_default();
    points
        .iter()
        .enumerate()
        .map(|(i, source)| {
            let mut point = source.clone();
            point.way_id = None;
            point.road_name = None;
            if let Some(Some(tracepoint)) = tracepoints.get(i) {
                if let Some([lon, lat]) = tracepoint.location {
                    point.lat = lat;
                    point.lon = lon;
                }
                point.way_id = way_for_tracepoint(tracepoint, response.matchings(), node_to_way);
            }
            point
        })
        .collect()
}

/// Takes the first resolvable way among the nodes of the leg ending at this
/// waypoint, then the leg starting at it. OSRM gives no per-node coordinates,
/// so this can't pick the geometrically nearest node.
fn way_for_tracepoint(
    tracepoint: &Tracepoint,
    matchings: &[Matching],
    node_to_way: &HashMap<u64, Option<u64>>,
) -> Option<u64> {
    let matching = matchings.get(tracepoint.matchings_index?)?;
    let waypoint = tracepoint.waypoint_index?;
    let legs = matching.legs.as_deref().unwrap_or_default();
    let leg_ending_here = waypoint.checked_sub(1).and_then(|i| legs.get(i));
    let leg_starting_here = legs.get(waypoint);
    [leg_ending_here, leg_starting_here]
        .into_iter()
        .flatten()
        .flat_map(Leg::nodes)
        .find_map(|node| node_to_way.get(node).copied().flatten())
}

#[derive(Deserialize)]
struct OverpassResponse {
    elements: Option<Vec<OverpassElement>>,
}

#[derive(Deserialize)]
struct OverpassElement {
    #[serde(rename = "type")]
    kind: Option<String>,
    id: Option<u64>,
    nodes: Option<Vec<u64>>,
}

/// Maps each requested node to the first way containing it. Every requested
/// node gets an entry, so nodes on no way are cached rather than asked again.
fn parse_overpass(body: &str, requested: &[u64]) -> Result<HashMap<u64, Option<u64>>> {
    let data: OverpassResponse =
        serde_json::from_str(body).map_err(|e| invalid_response("overpass", e))?;
    let mut result: HashMap<u64, Option<u64>> = requested.iter().map(|&n| (n, None)).collect();
    let ways = data
        .elements
        .unwrap_or_default()
        .into_iter()
        .filter(|el| el.kind.as_deref() == Some("way"));
    for way in ways {
        for node in way.nodes.unwrap_or_default() {
            if let Some(slot @ None) = result.get_mut(&node) {
                *slot = way.id;
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracepoint(json: &str) -> Tracepoint {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn overpass_first_way_wins_and_unknown_nodes_map_to_none() {
        let body = r#"{"elements": [
            {"type": "node", "id": 999, "nodes": [1]},
            {"type": "way", "id": 10, "nodes": [1, 2, 77]},
            {"type": "way", "id": 20, "nodes": [2, 3]}
        ]}"#;
        let map = parse_overpass(body, &[1, 2, 3, 4]).unwrap();
        assert_eq!(map[&1], Some(10));
        assert_eq!(map[&2], Some(10));
        assert_eq!(map[&3], Some(20));
        assert_eq!(map[&4], None);
        assert!(!map.contains_key(&77));
    }

    #[test]
    fn way_lookup_prefers_the_leg_ending_at_the_waypoint() {
        let matchings: Vec<Matching> = serde_json::from_str(
            r#"[{"legs": [
                {"annotation": {"nodes": [1, 2]}},
                {"annotation": {"nodes": [3]}}
            ]}]"#,
        )
        .unwrap();
        let ways = HashMap::from([(1, None), (2, Some(20)), (3, Some(30))]);
        let at = |waypoint: usize| {
            let json = format!(r#"{{"matchings_index": 0, "waypoint_index": {waypoint}}}"#);
            way_for_tracepoint(&tracepoint(&json), &matchings, &ways)
        };
        assert_eq!(at(0), Some(20)); // only the leg starting here
        assert_eq!(at(1), Some(20)); // leg ending here wins
        assert_eq!(at(2), Some(30)); // only the leg ending here
        assert_eq!(at(3), None); // no adjacent legs
    }

    #[test]
    fn way_lookup_tolerates_missing_indexes() {
        let ways = HashMap::new();
        assert_eq!(way_for_tracepoint(&tracepoint("{}"), &[], &ways), None);
        let out_of_range = tracepoint(r#"{"matchings_index": 5, "waypoint_index": 0}"#);
        assert_eq!(way_for_tracepoint(&out_of_range, &[], &ways), None);
    }

    #[test]
    fn unmatched_points_pass_through_with_extras() {
        let response: MatchResponse = serde_json::from_str(
            r#"{"tracepoints": [null, {"location": [-105.5, 40.5]}], "matchings": []}"#,
        )
        .unwrap();
        let mut first = TracePoint::new(1.0, 2.0);
        first.extras.name = Some("keep me".to_owned());
        first.way_id = Some(9);
        let points = [first, TracePoint::new(3.0, 4.0), TracePoint::new(5.0, 6.0)];
        let out = apply_match(&response, &points, &HashMap::new());

        assert_eq!((out[0].lat, out[0].lon, out[0].way_id), (1.0, 2.0, None));
        assert_eq!(out[0].extras.name.as_deref(), Some("keep me"));
        assert_eq!((out[1].lat, out[1].lon), (40.5, -105.5));
        assert_eq!((out[2].lat, out[2].lon), (5.0, 6.0));
    }
}
