//! Valhalla map-matching backend (`trace_attributes`).
//!
//! Returns a per-point OSM way ID via edge lookups in the response.

use serde::Deserialize;
use serde_json::{Number, json};

use super::{Backend, invalid_response};
use crate::error::Result;
use crate::http::HttpClient;
use crate::model::TracePoint;

/// Matches points through a Valhalla server.
#[derive(Debug, Clone)]
pub struct ValhallaBackend {
    url: String,
    http: HttpClient,
}

impl ValhallaBackend {
    /// The public community server. Please self-host for more than occasional use.
    pub const DEFAULT_URL: &str = "https://valhalla1.openstreetmap.de";

    /// Creates a backend. `url` overrides [`DEFAULT_URL`](Self::DEFAULT_URL).
    pub fn new(url: Option<&str>, http: HttpClient) -> Self {
        Self {
            url: url
                .unwrap_or(Self::DEFAULT_URL)
                .trim_end_matches('/')
                .to_owned(),
            http,
        }
    }
}

impl Backend for ValhallaBackend {
    fn match_points(&self, points: &[TracePoint]) -> Result<Vec<TracePoint>> {
        if points.is_empty() {
            return Ok(Vec::new());
        }
        let shape: Vec<_> = points
            .iter()
            .map(|p| json!({"lat": p.lat, "lon": p.lon}))
            .collect();
        let body = json!({
            "shape": shape,
            "costing": "auto",
            "shape_match": "map_snap",
            "filters": {
                "attributes": [
                    "edge.way_id",
                    "edge.names",
                    "matched.point",
                    "matched.edge_index",
                    "matched.type",
                ],
                "action": "include",
            },
        });
        let url = format!("{}/trace_attributes", self.url);
        let response = self.http.post_json(&url, &body.to_string())?;
        parse_trace_attributes(&response, points)
    }
}

#[derive(Deserialize)]
struct TraceAttributes {
    edges: Option<Vec<Edge>>,
    matched_points: Option<Vec<Option<MatchedPoint>>>,
}

// The numeric fields are `Number` because servers use out-of-range values such
// as a maximal `edge_index` to mean "none"; those must not fail the whole parse.
#[derive(Deserialize)]
struct Edge {
    way_id: Option<Number>,
    names: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct MatchedPoint {
    lat: Option<f64>,
    lon: Option<f64>,
    #[serde(rename = "type")]
    kind: Option<String>,
    edge_index: Option<Number>,
}

/// Parses a Valhalla `trace_attributes` response body into points.
///
/// The output has one point for each of `points`, which supply the fields the
/// response doesn't. Points the response has no entry for pass through unmatched.
pub fn parse_trace_attributes(body: &str, points: &[TracePoint]) -> Result<Vec<TracePoint>> {
    let data: TraceAttributes =
        serde_json::from_str(body).map_err(|e| invalid_response("valhalla", e))?;
    let edges = data.edges.unwrap_or_default();
    let matched = data.matched_points.unwrap_or_default();

    let parsed = points.iter().enumerate().map(|(i, source)| {
        let mut point = source.clone();
        point.way_id = None;
        point.road_name = None;
        let Some(Some(m)) = matched.get(i) else {
            return point;
        };
        point.lat = m.lat.unwrap_or(point.lat);
        point.lon = m.lon.unwrap_or(point.lon);
        if m.kind.as_deref() == Some("unmatched") {
            return point;
        }
        let edge = m
            .edge_index
            .as_ref()
            .and_then(Number::as_u64)
            .and_then(|index| edges.get(usize::try_from(index).ok()?));
        if let Some(edge) = edge {
            point.way_id = edge.way_id.as_ref().and_then(Number::as_u64);
            point.road_name = edge.names.as_ref().and_then(|names| names.first()).cloned();
        }
        point
    });
    Ok(parsed.collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    const FIXTURE: &str = include_str!("../../tests/fixtures/valhalla_trace_attributes.json");

    #[test]
    fn parses_way_ids_names_and_coordinates() {
        let points = vec![TracePoint::new(0.0, 0.0); 5];
        let out = parse_trace_attributes(FIXTURE, &points).unwrap();

        let way_ids: Vec<_> = out.iter().map(|p| p.way_id).collect();
        assert_eq!(
            way_ids,
            [Some(1001), Some(1001), Some(1002), None, Some(1003)]
        );
        let names: Vec<_> = out.iter().map(|p| p.road_name.as_deref()).collect();
        assert_eq!(
            names,
            [
                Some("Main Street"),
                Some("Main Street"),
                Some("Oak Avenue"),
                None,
                Some("Oak Avenue")
            ]
        );
        // Coordinates are replaced with the matched ones.
        assert!((out[0].lat - 40.0).abs() < 1e-9);
        assert!((out[4].lon - -105.004).abs() < 1e-9);
    }

    #[test]
    fn passes_points_through_on_short_response() {
        let points = [TracePoint::new(1.0, 2.0), TracePoint::new(3.0, 4.0)];
        let body = r#"{"edges": [], "matched_points": []}"#;
        let out = parse_trace_attributes(body, &points).unwrap();
        assert_eq!(out, points);
    }

    #[test]
    fn out_of_range_edge_index_means_no_way() {
        let points = [TracePoint::new(1.0, 2.0)];
        for index in ["18446744073709551615", "-1", "5"] {
            let body = format!(
                r#"{{"edges": [{{"way_id": 7}}],
                     "matched_points": [{{"lat": 1.5, "type": "matched", "edge_index": {index}}}]}}"#
            );
            let out = parse_trace_attributes(&body, &points).unwrap();
            assert_eq!(out[0].way_id, None, "{index}");
            assert_eq!(out[0].lat, 1.5);
        }
    }

    #[test]
    fn clears_stale_way_data_and_keeps_other_fields() {
        let mut point = TracePoint::new(1.0, 2.0);
        point.elevation = Some(100.0);
        point.way_id = Some(9);
        point.road_name = Some("Old Road".to_owned());
        point.extras.name = Some("keep me".to_owned());
        let out = parse_trace_attributes("{}", &[point]).unwrap();
        assert_eq!(out[0].way_id, None);
        assert_eq!(out[0].road_name, None);
        assert_eq!(out[0].elevation, Some(100.0));
        assert_eq!(out[0].extras.name.as_deref(), Some("keep me"));
    }

    #[test]
    fn coordinates_parse_to_the_nearest_float() {
        let points = [TracePoint::new(1.0, 2.0)];
        let body = r#"{"matched_points": [{"lon": 105.79399959999999}]}"#;
        let out = parse_trace_attributes(body, &points).unwrap();
        assert_eq!(out[0].lon, 105.79399959999999);
    }

    #[test]
    fn invalid_json_is_an_error() {
        let err = parse_trace_attributes("<html>", &[]).unwrap_err();
        assert!(matches!(
            err,
            Error::InvalidResponse {
                service: "valhalla",
                ..
            }
        ));
    }
}
