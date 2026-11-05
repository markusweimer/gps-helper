//! Runs the HTTP backends against a local mock server.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use gps_helper::backends::{Backend, OsrmBackend, ValhallaBackend};
use gps_helper::http::{HttpClient, HttpConfig};
use gps_helper::{AlignOptions, Error, TracePoint, align};

#[derive(Debug, Clone)]
struct Request {
    method: String,
    target: String,
    user_agent: String,
    content_type: String,
    body: String,
}

/// Serves canned responses in order, recording each request.
struct MockServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl MockServer {
    /// Each response is `(status, extra header lines, body)`.
    fn start(responses: Vec<(u16, &'static str, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let mut queue = VecDeque::from(responses);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Some((status, headers, body)) = queue.pop_front() else {
                    return;
                };
                let mut stream = stream.unwrap();
                let request = read_request(&mut BufReader::new(stream.try_clone().unwrap()));
                recorded.lock().unwrap().push(request);
                let response = format!(
                    "HTTP/1.1 {status} Mock\r\ncontent-length: {}\r\nconnection: close\r\n{headers}\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        Self { addr, requests }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

fn read_request(reader: &mut impl BufRead) -> Request {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap().to_owned();
    let target = parts.next().unwrap().to_owned();
    let (mut length, mut user_agent, mut content_type) = (0, String::new(), String::new());
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        let header = header.trim();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').unwrap();
        match name.to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().unwrap(),
            "user-agent" => user_agent = value.trim().to_owned(),
            "content-type" => content_type = value.trim().to_owned(),
            _ => {}
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    Request {
        method,
        target,
        user_agent,
        content_type,
        body: String::from_utf8(body).unwrap(),
    }
}

fn http_config() -> HttpConfig {
    let mut config = HttpConfig::default();
    config.rate_limit = 0.0;
    config.timeout = Duration::from_secs(5);
    config.backoff_factor = Duration::from_millis(1);
    config
}

fn client() -> HttpClient {
    HttpClient::new(http_config()).unwrap()
}

fn points() -> Vec<TracePoint> {
    vec![TracePoint::new(40.0, -105.0), TracePoint::new(40.1, -105.1)]
}

fn valhalla_ok() -> String {
    r#"{"edges": [{"way_id": 11, "names": ["Main St"]}],
        "matched_points": [{"lat": 40.5, "lon": -105.5, "type": "matched", "edge_index": 0},
                           {"lat": 40.6, "lon": -105.6, "type": "matched", "edge_index": 0}]}"#
        .to_owned()
}

#[test]
fn valhalla_sends_the_expected_request() {
    let server = MockServer::start(vec![(200, "", valhalla_ok())]);
    // The trailing slash must be tolerated.
    let backend = ValhallaBackend::new(Some(&format!("{}/", server.url())), client());
    let out = backend.match_points(&points()).unwrap();

    assert_eq!(out[0].way_id, Some(11));
    assert_eq!(out[1].road_name.as_deref(), Some("Main St"));
    assert_eq!((out[1].lat, out[1].lon), (40.6, -105.6));

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/trace_attributes");
    assert_eq!(request.content_type, "application/json");
    assert!(request.user_agent.starts_with("gps-helper/"), "{request:?}");
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(body["costing"], "auto");
    assert_eq!(body["shape_match"], "map_snap");
    assert_eq!(body["shape"][1]["lat"], 40.1);
    assert_eq!(body["shape"][1]["lon"], -105.1);
    assert_eq!(body["filters"]["action"], "include");
    assert_eq!(body["filters"]["attributes"].as_array().unwrap().len(), 5);
}

#[test]
fn empty_input_makes_no_request() {
    let server = MockServer::start(vec![]);
    let backend = ValhallaBackend::new(Some(&server.url()), client());
    assert!(backend.match_points(&[]).unwrap().is_empty());
    assert!(server.requests().is_empty());
}

#[test]
fn custom_user_agent_is_sent() {
    let server = MockServer::start(vec![(200, "", valhalla_ok())]);
    let mut config = http_config();
    config.user_agent = "my-tool/2.0".to_owned();
    let http = HttpClient::new(config).unwrap();
    ValhallaBackend::new(Some(&server.url()), http)
        .match_points(&points())
        .unwrap();
    assert_eq!(server.requests()[0].user_agent, "my-tool/2.0");
}

#[test]
fn retries_retryable_statuses_until_success() {
    let server = MockServer::start(vec![
        (503, "", "busy".to_owned()),
        (429, "retry-after: 0\r\n", "slow down".to_owned()),
        (502, "", "bad gateway".to_owned()),
        (200, "", valhalla_ok()),
    ]);
    let backend = ValhallaBackend::new(Some(&server.url()), client());
    assert_eq!(backend.match_points(&points()).unwrap()[0].way_id, Some(11));
    assert_eq!(server.requests().len(), 4);
}

#[test]
fn gives_up_after_max_retries_and_reports_the_last_status() {
    let responses = (0..10)
        .map(|_| (503, "", "still busy".to_owned()))
        .collect();
    let server = MockServer::start(responses);
    let mut config = http_config();
    config.max_retries = 2;
    let backend = ValhallaBackend::new(Some(&server.url()), HttpClient::new(config).unwrap());

    let err = backend.match_points(&points()).unwrap_err();
    match err {
        Error::HttpStatus { status, body, .. } => {
            assert_eq!(status, 503);
            assert_eq!(body, "still busy");
        }
        other => panic!("unexpected error: {other}"),
    }
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn does_not_retry_client_errors_and_surfaces_the_explanation() {
    let body = r#"{"error": "No suitable edges near location"}"#.to_owned();
    let server = MockServer::start(vec![(400, "", body), (200, "", valhalla_ok())]);
    let backend = ValhallaBackend::new(Some(&server.url()), client());

    let err = backend.match_points(&points()).unwrap_err();
    assert!(matches!(err, Error::HttpStatus { status: 400, .. }));
    assert!(err.to_string().contains("No suitable edges"), "{err}");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn connection_failures_are_retried_then_reported() {
    // Bind then drop to get a port that refuses connections.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut config = http_config();
    config.max_retries = 1;
    let url = format!("http://127.0.0.1:{port}");
    let backend = ValhallaBackend::new(Some(&url), HttpClient::new(config).unwrap());
    let err = backend.match_points(&points()).unwrap_err();
    assert!(matches!(err, Error::Transport { .. }), "{err}");
}

#[test]
fn success_with_a_non_json_body_is_an_error() {
    let server = MockServer::start(vec![(200, "", "<html>".to_owned())]);
    let backend = ValhallaBackend::new(Some(&server.url()), client());
    let err = backend.match_points(&points()).unwrap_err();
    assert!(matches!(
        err,
        Error::InvalidResponse {
            service: "valhalla",
            ..
        }
    ));
}

#[test]
fn rate_limit_spaces_requests_including_retries() {
    let server = MockServer::start(vec![
        (503, "retry-after: 0\r\n", String::new()),
        (200, "", valhalla_ok()),
        (200, "", valhalla_ok()),
    ]);
    let mut config = http_config();
    config.rate_limit = 20.0; // 50 ms apart
    let backend = ValhallaBackend::new(Some(&server.url()), HttpClient::new(config).unwrap());
    let started = Instant::now();
    backend.match_points(&points()).unwrap();
    backend.match_points(&points()).unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(100),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn align_stitches_backend_responses() {
    let server = MockServer::start(vec![(200, "", valhalla_ok()), (200, "", valhalla_ok())]);
    let backend = ValhallaBackend::new(Some(&server.url()), client());
    let mut options = AlignOptions::default();
    options.chunk_size = 2;
    options.overlap = 1;
    let trace = vec![TracePoint::new(1.0, 1.0); 3];
    assert_eq!(align(&trace, &backend, &options).unwrap().len(), 3);
    assert_eq!(server.requests().len(), 2);
}

fn osrm_ok() -> String {
    r#"{"code": "Ok",
        "tracepoints": [
            {"location": [-105.5, 40.5], "matchings_index": 0, "waypoint_index": 0},
            null,
            {"location": [-105.7, 40.7], "matchings_index": 0, "waypoint_index": 1}
        ],
        "matchings": [{"geometry": {"coordinates": []},
                       "legs": [{"annotation": {"nodes": [100, 200]}}]}]}"#
        .to_owned()
}

fn overpass_ok() -> String {
    r#"{"elements": [{"type": "way", "id": 55, "nodes": [200, 300]}]}"#.to_owned()
}

#[test]
fn osrm_resolves_way_ids_through_overpass() {
    let osrm = MockServer::start(vec![(200, "", osrm_ok())]);
    let overpass = MockServer::start(vec![(200, "", overpass_ok())]);
    let backend = OsrmBackend::new(Some(&osrm.url()), client())
        .with_overpass_url(&format!("{}/api/interpreter", overpass.url()));

    let trace = [
        TracePoint::new(40.0, -105.0),
        TracePoint::new(40.123456789, -105.1),
        TracePoint::new(40.2, -105.2),
    ];
    let out = backend.match_points(&trace).unwrap();

    assert_eq!(
        (out[0].lat, out[0].lon, out[0].way_id),
        (40.5, -105.5, Some(55))
    );
    assert_eq!((out[1].lat, out[1].way_id), (40.123456789, None));
    assert_eq!(
        (out[2].lat, out[2].lon, out[2].way_id),
        (40.7, -105.7, Some(55))
    );

    let request = &osrm.requests()[0];
    assert_eq!(request.method, "GET");
    assert_eq!(
        request.target,
        "/match/v1/driving/-105.000000,40.000000;-105.100000,40.123457;-105.200000,40.200000\
         ?annotations=nodes&overview=full&geometries=geojson&tidy=true"
    );

    let request = &overpass.requests()[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/api/interpreter");
    assert_eq!(request.content_type, "application/x-www-form-urlencoded");
    assert!(
        request.body.starts_with("data=%5Bout%3Ajson%5D"),
        "{}",
        request.body
    );
    assert!(request.body.contains("100%2C200"), "{}", request.body);
}

#[test]
fn osrm_caches_node_lookups_between_chunks() {
    let osrm = MockServer::start(vec![(200, "", osrm_ok()), (200, "", osrm_ok())]);
    let overpass = MockServer::start(vec![(200, "", overpass_ok())]);
    let backend = OsrmBackend::new(Some(&osrm.url()), client()).with_overpass_url(&overpass.url());

    let trace = vec![TracePoint::new(1.0, 1.0); 3];
    let first = backend.match_points(&trace).unwrap();
    let second = backend.match_points(&trace).unwrap();
    assert_eq!(first, second);
    assert_eq!(osrm.requests().len(), 2);
    assert_eq!(overpass.requests().len(), 1);
}

#[test]
fn osrm_batches_overpass_queries() {
    let nodes: Vec<String> = (1..=501).map(|n| n.to_string()).collect();
    let response = format!(
        r#"{{"tracepoints": [], "matchings": [{{"legs": [{{"annotation": {{"nodes": [{}]}}}}]}}]}}"#,
        nodes.join(",")
    );
    let osrm = MockServer::start(vec![(200, "", response)]);
    let empty = r#"{"elements": []}"#.to_owned();
    let overpass = MockServer::start(vec![(200, "", empty.clone()), (200, "", empty)]);
    let backend = OsrmBackend::new(Some(&osrm.url()), client()).with_overpass_url(&overpass.url());

    backend.match_points(&points()).unwrap();
    assert_eq!(overpass.requests().len(), 2);
}
