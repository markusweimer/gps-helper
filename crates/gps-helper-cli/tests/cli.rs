//! Runs the real binary end to end against a local mock Valhalla server.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};
use std::thread;

const INPUT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="test" xmlns="http://www.topografix.com/GPX/1/1"
     xmlns:tpx="urn:garmin:tpx">
  <trk><trkseg>
    <trkpt lat="40.0" lon="-105.0"><ele>1600</ele><time>2024-03-01T10:00:00Z</time>
      <extensions><tpx:hr>140</tpx:hr></extensions></trkpt>
    <trkpt lat="40.1" lon="-105.1"/>
    <trkpt lat="40.2" lon="-105.2"/>
    <trkpt lat="40.3" lon="-105.3"/>
    <trkpt lat="40.4" lon="-105.4"/>
    <trkpt lat="40.5" lon="-105.5"/>
  </trkseg></trk>
</gpx>
"#;

const VALHALLA_RESPONSE: &str = r#"{
  "edges": [{"way_id": 1, "names": ["Main St"]}, {"way_id": 2, "names": []}],
  "matched_points": [
    {"lat": 41.0, "lon": -106.0, "type": "matched", "edge_index": 0},
    {"lat": 41.1, "lon": -106.1, "type": "matched", "edge_index": 0},
    {"lat": 41.2, "lon": -106.2, "type": "matched", "edge_index": 0},
    {"lat": 41.3, "lon": -106.3, "type": "matched", "edge_index": 1},
    {"lat": 41.4, "lon": -106.4, "type": "matched", "edge_index": 1},
    {"lat": 41.5, "lon": -106.5, "type": "matched", "edge_index": 1}
  ]
}"#;

fn gps_helper(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gps-helper"))
        .args(args)
        .output()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Answers `requests` POSTs with `status` and `body`, then stops.
fn mock_server(requests: usize, status: u16, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    thread::spawn(move || {
        for _ in 0..requests {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let lower = line.trim().to_ascii_lowercase();
                if let Some(value) = lower.strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                if lower.is_empty() {
                    break;
                }
            }
            let mut request_body = vec![0; length];
            reader.read_exact(&mut request_body).unwrap();
            let response = format!(
                "HTTP/1.1 {status} Mock\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    url
}

fn write_input(dir: &Path) -> String {
    let path = dir.join("ride.gpx");
    fs::write(&path, INPUT).unwrap();
    path.to_str().unwrap().to_owned()
}

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

#[test]
fn process_writes_all_three_outputs_next_to_the_input() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path());
    let url = mock_server(1, 200, VALHALLA_RESPONSE);

    let output = gps_helper(&[
        "process",
        &input,
        "--backend-url",
        &url,
        "--rate-limit",
        "0",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let log = stderr(&output);
    assert!(log.contains("6 points loaded"), "{log}");
    assert!(log.contains("chunk 1/1 (100% of points)"), "{log}");
    assert!(
        log.contains("Done: 6 points -> 6 aligned, 6 simplified, 4 route points."),
        "{log}"
    );

    let aligned = fs::read_to_string(dir.path().join("ride.aligned.gpx")).unwrap();
    assert_eq!(count(&aligned, "<trkpt "), 6);
    assert_eq!(count(&aligned, "<gh:way_id>1</gh:way_id>"), 3);
    assert_eq!(count(&aligned, "<gh:way_id>2</gh:way_id>"), 3);
    assert!(aligned.contains(r#"lat="41.0" lon="-106.0""#), "{aligned}");
    assert!(aligned.contains("<ele>1600.0</ele>"), "{aligned}");
    assert!(
        aligned.contains("<time>2024-03-01T10:00:00Z</time>"),
        "{aligned}"
    );
    assert!(aligned.contains("<tpx:hr>140</tpx:hr>"), "{aligned}");

    let simplified = fs::read_to_string(dir.path().join("ride.simplified.gpx")).unwrap();
    assert_eq!(count(&simplified, "<trkpt "), 6);

    let route = fs::read_to_string(dir.path().join("ride.route.gpx")).unwrap();
    assert_eq!(count(&route, "<rtept "), 4);
    assert_eq!(count(&route, "<name>Main St</name>"), 2);
    assert_eq!(count(&route, "<name>way 2</name>"), 2);
}

#[test]
fn process_uses_output_as_base_name() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path());
    let url = mock_server(1, 200, VALHALLA_RESPONSE);
    let base = dir.path().join("out.gpx");

    let output = gps_helper(&[
        "process",
        &input,
        "-o",
        path_str(&base),
        "--backend-url",
        &url,
        "--rate-limit",
        "0",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    for name in ["out.aligned.gpx", "out.simplified.gpx", "out.route.gpx"] {
        assert!(dir.path().join(name).exists(), "{name}");
    }
}

#[test]
fn align_then_simplify_in_both_formats() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path());
    let url = mock_server(1, 200, VALHALLA_RESPONSE);
    let aligned = dir.path().join("aligned.gpx");

    let output = gps_helper(&[
        "align",
        &input,
        "-o",
        path_str(&aligned),
        "--backend-url",
        &url,
        "--rate-limit",
        "0",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).contains("Done: 6 -> 6 points."));

    let track = dir.path().join("track.gpx");
    let output = gps_helper(&[
        "simplify",
        path_str(&aligned),
        "-o",
        path_str(&track),
        "--context",
        "1",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("Done: 6 -> 4 points (2 removed)."),
        "{}",
        stderr(&output)
    );
    assert_eq!(count(&fs::read_to_string(&track).unwrap(), "<trkpt "), 4);

    let route = dir.path().join("route.gpx");
    let output = gps_helper(&[
        "simplify",
        path_str(&aligned),
        "-o",
        path_str(&route),
        "--format",
        "route",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let route = fs::read_to_string(&route).unwrap();
    assert_eq!(count(&route, "<rtept "), 4);
    // Road names are not stored in track files, so way IDs name the points.
    assert_eq!(count(&route, "<name>way 1</name>"), 2);
}

#[test]
fn simplify_refuses_unaligned_input_with_exit_code_2() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path());
    let out = dir.path().join("out.gpx");
    let output = gps_helper(&["simplify", &input, "-o", path_str(&out)]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("no <gh:way_id> extensions"));
    assert!(!out.exists());
}

#[test]
fn backend_errors_exit_1_without_a_panic_and_leave_no_output() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path());
    let url = mock_server(1, 400, r#"{"error": "No suitable edges"}"#);
    let output = gps_helper(&[
        "process",
        &input,
        "--backend-url",
        &url,
        "--rate-limit",
        "0",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let log = stderr(&output);
    assert!(log.contains("error: HTTP 400"), "{log}");
    assert!(log.contains("No suitable edges"), "{log}");
    assert!(!log.contains("panicked"), "{log}");
    assert!(!dir.path().join("ride.aligned.gpx").exists());
}

#[test]
fn bad_input_files_exit_1() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.gpx");
    let missing = dir.path().join("missing.gpx");
    let output = gps_helper(&["simplify", path_str(&missing), "-o", path_str(&out)]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("missing.gpx"));

    let broken = dir.path().join("broken.gpx");
    fs::write(&broken, "<gpx><trk>").unwrap();
    let output = gps_helper(&["simplify", path_str(&broken), "-o", path_str(&out)]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("invalid GPX"));
}

#[test]
fn invalid_options_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path());
    let out = dir.path().join("out.gpx");
    let output_path = path_str(&out);

    // Chunking rules come from the library, so these exit 1.
    let output = gps_helper(&["align", &input, "-o", output_path, "--chunk-size", "0"]);
    assert_eq!(output.status.code(), Some(1));
    let output = gps_helper(&[
        "align",
        &input,
        "-o",
        output_path,
        "--chunk-size",
        "5",
        "--overlap",
        "5",
    ]);
    assert_eq!(output.status.code(), Some(1));

    // These never parse, so clap exits 2.
    let bad_usage: [&[&str]; 5] = [
        &["align", &input, "-o", output_path, "--backend", "nope"],
        &["align", &input, "-o", output_path, "--timeout", "0"],
        &["align", &input],
        &["simplify", &input, "-o", output_path, "--context", "-1"],
        &["simplify", &input, "-o", output_path, "--format", "nope"],
    ];
    for args in bad_usage {
        assert_eq!(gps_helper(args).status.code(), Some(2), "{args:?}");
    }
    assert_eq!(gps_helper(&[]).status.code(), Some(2));
}

#[test]
fn version_flag_prints_the_version() {
    let output = gps_helper(&["--version"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.trim(),
        concat!("gps-helper ", env!("CARGO_PKG_VERSION"))
    );
}

const ROUTE_INPUT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="test" xmlns="http://www.topografix.com/GPX/1/1"
     xmlns:gpxx="http://www.garmin.com/xmlschemas/GpxExtensions/v3">
  <wpt lat="1.5" lon="2.5"><name>Stop</name></wpt>
  <rte>
    <name>Morning</name>
    <rtept lat="10" lon="20">
      <extensions><gpxx:RoutePointExtension>
        <gpxx:rpt lat="10" lon="20"/>
        <gpxx:rpt lat="10.5" lon="20.5"/>
      </gpxx:RoutePointExtension></extensions>
    </rtept>
    <rtept lat="11" lon="21"/>
  </rte>
  <rte><rtept lat="30" lon="40"/></rte>
</gpx>
"#;

#[test]
fn route_to_track_writes_next_to_the_input_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("trip.gpx");
    fs::write(&input, ROUTE_INPUT).unwrap();

    let output = gps_helper(&["route-to-track", path_str(&input)]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).contains("Morning: 2 route points -> 3 track points."));
    assert!(stderr(&output).contains("(unnamed): 1 route points -> 1 track points."));

    let xml = fs::read_to_string(dir.path().join("trip.track.gpx")).unwrap();
    assert_eq!(count(&xml, "<trk>"), 2);
    assert_eq!(count(&xml, "<trkpt"), 4);
    assert_eq!(count(&xml, "<wpt"), 1);
    assert_eq!(count(&xml, "<rte"), 0);
}

#[test]
fn route_to_track_honors_output_and_feeds_the_other_commands() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("trip.gpx");
    let track = dir.path().join("chosen.gpx");
    fs::write(&input, ROUTE_INPUT).unwrap();

    let output = gps_helper(&["route-to-track", path_str(&input), "-o", path_str(&track)]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(track.exists());
    assert!(!dir.path().join("trip.track.gpx").exists());

    // The result is an ordinary track file, so the pipeline can read it.
    let simplified = dir.path().join("simplified.gpx");
    let simplify = gps_helper(&["simplify", path_str(&track), "-o", path_str(&simplified)]);
    assert_eq!(simplify.status.code(), Some(2), "{}", stderr(&simplify));
    assert!(stderr(&simplify).contains("no <gh:way_id>"));
}

#[test]
fn route_to_track_refuses_files_without_routes_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path());

    let output = gps_helper(&["route-to-track", &input]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("no <rte> routes"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}
