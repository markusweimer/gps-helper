//! Map-matching backends.

mod osrm;
mod valhalla;

pub use osrm::OsrmBackend;
pub use valhalla::{ValhallaBackend, parse_trace_attributes};

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::model::TracePoint;

/// A map-matching service.
pub trait Backend: Send + Sync {
    /// Snaps `points` to roads.
    ///
    /// Must return exactly one point per input point, in order, with
    /// [`way_id`](TracePoint::way_id) set where a road could be identified.
    /// Implementations should start from a clone of each input point, so that
    /// fields such as [`extras`](TracePoint::extras) are preserved.
    fn match_points(&self, points: &[TracePoint]) -> Result<Vec<TracePoint>>;
}

/// The names [`by_name`] accepts, in sorted order.
pub fn available() -> &'static [&'static str] {
    &["osrm", "valhalla"]
}

/// Creates a built-in backend by name, for tools that let their users choose.
///
/// `url` overrides the backend's default endpoint.
pub fn by_name(name: &str, url: Option<&str>, http: HttpClient) -> Result<Box<dyn Backend>> {
    match name {
        "osrm" => Ok(Box::new(OsrmBackend::new(url, http))),
        "valhalla" => Ok(Box::new(ValhallaBackend::new(url, http))),
        _ => Err(Error::UnknownBackend {
            name: name.to_owned(),
            available: available().iter().map(|&name| name.to_owned()).collect(),
        }),
    }
}

fn invalid_response(service: &'static str, source: serde_json::Error) -> Error {
    Error::InvalidResponse {
        service,
        source: Box::new(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpConfig;

    #[test]
    fn every_available_name_builds() {
        let http = HttpClient::new(HttpConfig::default()).unwrap();
        for name in available() {
            assert!(by_name(name, None, http.clone()).is_ok(), "{name}");
        }
    }

    #[test]
    fn unknown_name_lists_the_alternatives() {
        let http = HttpClient::new(HttpConfig::default()).unwrap();
        let err = by_name("nope", None, http).err().unwrap();
        assert_eq!(
            err.to_string(),
            r#"unknown backend "nope"; available: osrm, valhalla"#
        );
    }
}
