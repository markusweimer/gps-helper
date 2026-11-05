//! GPX reading and writing, with support for the way ID extension.
//!
//! This is built directly on an XML parser because points must round-trip
//! with their `<extensions>` intact, including ones this crate knows nothing
//! about. Output is always GPX 1.1.
//!
//! [`read_points`] reads trackpoints, and writes them back as one track with one
//! segment or one route. [`read_routes`] reads routes and waypoints instead, for
//! [`write_routes_as_tracks`] to turn into one track per route. Metadata is
//! always ignored.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset, NaiveDateTime, SecondsFormat};
use quick_xml::escape::{escape, partial_escape, resolve_xml_entity};
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::name::{QName, ResolveResult};
use quick_xml::{NsReader, XmlVersion};

use crate::error::{Error, Result};
use crate::model::{TracePoint, XmlAttribute, XmlElement, XmlNode};

/// The XML namespace of the `<way_id>` extension.
pub const WAY_ID_NAMESPACE: &str = "https://github.com/gps-helper/gps-helper";

const WAY_ID_PREFIX: &str = "gh";
const WAY_ID_LOCAL_NAME: &str = "way_id";
const GARMIN_NAMESPACE: &str = "http://www.garmin.com/xmlschemas/GpxExtensions/v3";
const GPX_NAMESPACE: &str = "http://www.topografix.com/GPX/1/1";
const XSI_NAMESPACE: &str = "http://www.w3.org/2001/XMLSchema-instance";
const SCHEMA_LOCATION: &str =
    "http://www.topografix.com/GPX/1/1 http://www.topografix.com/GPX/1/1/gpx.xsd";
const CREATOR: &str = concat!("gps-helper/", env!("CARGO_PKG_VERSION"));

/// Deepest nesting allowed inside `<extensions>`, so that a malicious file
/// cannot overflow the stack.
const MAX_EXTENSION_DEPTH: usize = 64;

/// Whether any point has a way ID, meaning the trace has been aligned.
pub fn has_any_way_id(points: &[TracePoint]) -> bool {
    points.iter().any(|p| p.way_id.is_some())
}

/// The routes and waypoints of a GPX file.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct RouteFile {
    /// The file's waypoints.
    pub waypoints: Vec<TracePoint>,
    /// The file's routes.
    pub routes: Vec<Route>,
}

/// A route, flattened into the path it follows.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct Route {
    /// The route's name, if it has one.
    pub name: Option<String>,
    /// How many `<rtept>` via-points the route has.
    pub via_point_count: usize,
    /// The via-points and Garmin shape points in order, with only coordinates
    /// set. Consecutive points at the same position are merged.
    pub points: Vec<TracePoint>,
}

// --- Reading -----------------------------------------------------------------

/// Reads all trackpoints in a GPX file, across all tracks and segments.
pub fn load_points(path: impl AsRef<Path>) -> Result<Vec<TracePoint>> {
    load_with(path.as_ref(), read_points)
}

/// Reads all routes and waypoints in a GPX file.
pub fn load_routes(path: impl AsRef<Path>) -> Result<RouteFile> {
    load_with(path.as_ref(), read_routes)
}

fn load_with<T>(path: &Path, read: impl FnOnce(BufReader<File>) -> Result<T>) -> Result<T> {
    let file = File::open(path).map_err(|source| io_error(path, source))?;
    read(BufReader::new(file)).map_err(|error| match error {
        Error::Io { source, .. } => io_error(path, source),
        other => other,
    })
}

/// Reads all trackpoints from a GPX document, across all tracks and segments.
///
/// Fields that can't be parsed cause an error, except `<time>` (devices write
/// many odd formats, so an unreadable time just becomes `None`) and the way ID
/// (an unreadable one means the point is not aligned). Times with no UTC offset
/// are taken as UTC, as the GPX specification requires.
pub fn read_points(reader: impl BufRead) -> Result<Vec<TracePoint>> {
    Parser::new(reader).document(Parser::root_children)
}

/// Reads all routes and waypoints from a GPX document.
///
/// Devices that calculate a route store the road geometry between via-points as
/// Garmin `<gpxx:rpt>` shape points, which become part of [`Route::points`]. A
/// route without them has only its via-points, so the path it describes is
/// straight lines between those.
pub fn read_routes(reader: impl BufRead) -> Result<RouteFile> {
    Parser::new(reader).document(Parser::route_root_children)
}

struct Parser<R> {
    reader: NsReader<R>,
    buf: Vec<u8>,
}

impl<R: BufRead> Parser<R> {
    fn new(reader: R) -> Self {
        Self {
            reader: NsReader::from_reader(reader),
            buf: Vec::new(),
        }
    }

    fn next_event(&mut self) -> Result<Event<'static>> {
        self.buf.clear();
        match self.reader.read_event_into(&mut self.buf) {
            Ok(event) => Ok(event.into_owned()),
            Err(error) => Err(self.xml_error(error)),
        }
    }

    fn xml_error(&self, error: quick_xml::Error) -> Error {
        match error {
            quick_xml::Error::Io(source) => Error::Io {
                path: None,
                source: io::Error::new(source.kind(), source.to_string()),
            },
            error => {
                let position = self.reader.error_position();
                Error::InvalidGpx(format!("{error} (near byte {position})"))
            }
        }
    }

    fn unexpected_eof(&self) -> Error {
        Error::InvalidGpx("file ends before all elements are closed".into())
    }

    fn skip_element(&mut self, name: QName) -> Result<()> {
        self.buf.clear();
        match self.reader.read_to_end_into(name, &mut self.buf) {
            Ok(_) => Ok(()),
            Err(error) => Err(self.xml_error(error)),
        }
    }

    fn document<T: Default>(
        &mut self,
        mut root_children: impl FnMut(&mut Self, &mut T) -> Result<()>,
    ) -> Result<T> {
        let mut content = T::default();
        let mut found_root = false;
        loop {
            match self.next_event()? {
                Event::Start(_) if found_root => {
                    return Err(Error::InvalidGpx("more than one root element".into()));
                }
                Event::Start(_) => {
                    found_root = true;
                    root_children(self, &mut content)?;
                }
                Event::Empty(_) => found_root = true,
                Event::Eof if found_root => return Ok(content),
                Event::Eof => return Err(Error::InvalidGpx("no root element".into())),
                _ => {}
            }
        }
    }

    fn root_children(&mut self, points: &mut Vec<TracePoint>) -> Result<()> {
        loop {
            match self.next_event()? {
                Event::Start(e) if e.local_name().as_ref() == "trk" => self.track(points)?,
                Event::Start(e) => self.skip_element(e.name())?,
                Event::End(_) => return Ok(()),
                Event::Eof => return Err(self.unexpected_eof()),
                _ => {}
            }
        }
    }

    fn track(&mut self, points: &mut Vec<TracePoint>) -> Result<()> {
        loop {
            match self.next_event()? {
                Event::Start(e) if e.local_name().as_ref() == "trkseg" => self.segment(points)?,
                Event::Start(e) => self.skip_element(e.name())?,
                Event::End(_) => return Ok(()),
                Event::Eof => return Err(self.unexpected_eof()),
                _ => {}
            }
        }
    }

    fn segment(&mut self, points: &mut Vec<TracePoint>) -> Result<()> {
        loop {
            match self.next_event()? {
                Event::Start(e) if e.local_name().as_ref() == "trkpt" => {
                    let mut point = point_from_attributes(&e)?;
                    self.point_children(&mut point)?;
                    points.push(point);
                }
                Event::Empty(e) if e.local_name().as_ref() == "trkpt" => {
                    points.push(point_from_attributes(&e)?);
                }
                Event::Start(e) => self.skip_element(e.name())?,
                Event::End(_) => return Ok(()),
                Event::Eof => return Err(self.unexpected_eof()),
                _ => {}
            }
        }
    }

    fn point_children(&mut self, point: &mut TracePoint) -> Result<()> {
        loop {
            let start = match self.next_event()? {
                Event::Start(e) => e,
                Event::End(_) => return Ok(()),
                Event::Eof => return Err(self.unexpected_eof()),
                _ => continue,
            };
            let extras = &mut point.extras;
            match start.local_name().as_ref() {
                "ele" => point.elevation = parse_float_field("ele", &self.text()?)?,
                "time" => point.time = parse_time(&self.text()?),
                "magvar" => extras.magnetic_variation = parse_float_field("magvar", &self.text()?)?,
                "name" => extras.name = non_empty(self.text()?),
                "cmt" => extras.comment = non_empty(self.text()?),
                "sym" => extras.symbol = non_empty(self.text()?),
                "type" => extras.kind = non_empty(self.text()?),
                "hdop" => extras.horizontal_dilution = parse_float_field("hdop", &self.text()?)?,
                "vdop" => extras.vertical_dilution = parse_float_field("vdop", &self.text()?)?,
                "pdop" => extras.position_dilution = parse_float_field("pdop", &self.text()?)?,
                "extensions" => self.extensions(point)?,
                _ => self.skip_element(start.name())?,
            }
        }
    }

    fn route_root_children(&mut self, file: &mut RouteFile) -> Result<()> {
        loop {
            match self.next_event()? {
                Event::Start(e) if e.local_name().as_ref() == "wpt" => {
                    let mut point = point_from_attributes(&e)?;
                    self.point_children(&mut point)?;
                    file.waypoints.push(point);
                }
                Event::Empty(e) if e.local_name().as_ref() == "wpt" => {
                    file.waypoints.push(point_from_attributes(&e)?);
                }
                Event::Start(e) if e.local_name().as_ref() == "rte" => {
                    file.routes.push(self.route()?);
                }
                Event::Empty(e) if e.local_name().as_ref() == "rte" => {
                    file.routes.push(Route::default());
                }
                Event::Start(e) => self.skip_element(e.name())?,
                Event::End(_) => return Ok(()),
                Event::Eof => return Err(self.unexpected_eof()),
                _ => {}
            }
        }
    }

    fn route(&mut self) -> Result<Route> {
        let mut route = Route::default();
        loop {
            match self.next_event()? {
                Event::Start(e) if e.local_name().as_ref() == "name" => {
                    route.name = non_empty(self.text()?);
                }
                Event::Start(e) if e.local_name().as_ref() == "rtept" => {
                    route.via_point_count += 1;
                    push_distinct(&mut route.points, point_from_attributes(&e)?);
                    self.route_point_children(&mut route)?;
                }
                Event::Empty(e) if e.local_name().as_ref() == "rtept" => {
                    route.via_point_count += 1;
                    push_distinct(&mut route.points, point_from_attributes(&e)?);
                }
                Event::Start(e) => self.skip_element(e.name())?,
                Event::End(_) => return Ok(route),
                Event::Eof => return Err(self.unexpected_eof()),
                _ => {}
            }
        }
    }

    fn route_point_children(&mut self, route: &mut Route) -> Result<()> {
        loop {
            match self.next_event()? {
                Event::Start(e) if e.local_name().as_ref() == "extensions" => {
                    self.shape_points(route)?;
                }
                Event::Start(e) => self.skip_element(e.name())?,
                Event::End(_) => return Ok(()),
                Event::Eof => return Err(self.unexpected_eof()),
                _ => {}
            }
        }
    }

    fn shape_points(&mut self, route: &mut Route) -> Result<()> {
        loop {
            let element = match self.next_event()? {
                Event::Start(e) => self.element(&e, 1)?,
                Event::Empty(e) => element_from_start(&self.reader, &e)?,
                Event::End(_) => return Ok(()),
                Event::Eof => return Err(self.unexpected_eof()),
                _ => continue,
            };
            collect_shape_points(&element, &mut route.points)?;
        }
    }

    /// Reads an element's text up to its end tag, ignoring any child elements.
    fn text(&mut self) -> Result<String> {
        let mut text = String::new();
        loop {
            match self.next_event()? {
                Event::Text(e) => text.push_str(&e.xml10_content()),
                Event::CData(e) => text.push_str(&e.xml10_content()),
                Event::GeneralRef(e) => push_reference(&mut text, &e)?,
                Event::Start(e) => self.skip_element(e.name())?,
                Event::End(_) => return Ok(text),
                Event::Eof => return Err(self.unexpected_eof()),
                _ => {}
            }
        }
    }

    fn extensions(&mut self, point: &mut TracePoint) -> Result<()> {
        loop {
            let element = match self.next_event()? {
                Event::Start(e) => self.element(&e, 1)?,
                Event::Empty(e) => element_from_start(&self.reader, &e)?,
                Event::End(_) => return Ok(()),
                Event::Eof => return Err(self.unexpected_eof()),
                _ => continue,
            };
            if !is_way_id(&element) {
                point.extras.extensions.push(element);
            } else if point.way_id.is_none() {
                point.way_id = way_id_value(&element);
            }
        }
    }

    fn element(&mut self, start: &BytesStart, depth: usize) -> Result<XmlElement> {
        if depth > MAX_EXTENSION_DEPTH {
            return Err(Error::InvalidGpx("extensions are nested too deeply".into()));
        }
        let mut element = element_from_start(&self.reader, start)?;
        let mut text = String::new();
        loop {
            let child = match self.next_event()? {
                Event::Text(e) => {
                    text.push_str(&e.xml10_content());
                    continue;
                }
                Event::CData(e) => {
                    text.push_str(&e.xml10_content());
                    continue;
                }
                Event::GeneralRef(e) => {
                    push_reference(&mut text, &e)?;
                    continue;
                }
                Event::Start(e) => self.element(&e, depth + 1)?,
                Event::Empty(e) => element_from_start(&self.reader, &e)?,
                Event::End(_) => {
                    flush_text(&mut text, &mut element.children);
                    return Ok(element);
                }
                Event::Eof => return Err(self.unexpected_eof()),
                _ => continue,
            };
            flush_text(&mut text, &mut element.children);
            element.children.push(XmlNode::Element(child));
        }
    }
}

fn push_reference(text: &mut String, reference: &BytesRef) -> Result<()> {
    let invalid = || format!("unknown entity &{};", reference.xml10_content());
    if let Some(ch) = reference
        .resolve_char_ref()
        .map_err(|_| Error::InvalidGpx(invalid()))?
    {
        text.push(ch);
        return Ok(());
    }
    let resolved = resolve_xml_entity(&reference.xml10_content())
        .ok_or_else(|| Error::InvalidGpx(invalid()))?;
    text.push_str(resolved);
    Ok(())
}

/// Moves pending text into `children`. Whitespace-only text is just formatting.
fn flush_text(text: &mut String, children: &mut Vec<XmlNode>) {
    if !text.trim().is_empty() {
        children.push(XmlNode::Text(text.clone()));
    }
    text.clear();
}

fn point_from_attributes(start: &BytesStart) -> Result<TracePoint> {
    let (mut lat, mut lon) = (None, None);
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|e| Error::InvalidGpx(e.to_string()))?;
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|e| Error::InvalidGpx(e.to_string()))?;
        match attribute.key.as_ref() {
            "lat" => lat = parse_float_field("lat", &value)?,
            "lon" => lon = parse_float_field("lon", &value)?,
            _ => {}
        }
    }
    let tag = start.local_name();
    let tag = tag.as_ref();
    let lat = lat.ok_or_else(|| Error::InvalidGpx(format!("{tag} has no lat")))?;
    let lon = lon.ok_or_else(|| Error::InvalidGpx(format!("{tag} has no lon")))?;
    Ok(TracePoint::new(lat, lon))
}

/// Adds any shape points in `element` or its descendants to `points`.
fn collect_shape_points(element: &XmlElement, points: &mut Vec<TracePoint>) -> Result<()> {
    if element.namespace.as_deref() == Some(GARMIN_NAMESPACE) && element.local_name == "rpt" {
        let coordinate = |field: &str| {
            let value = element
                .attributes
                .iter()
                .find(|a| a.namespace.is_none() && a.local_name == field)
                .map_or("", |a| a.value.as_str());
            parse_float_field(field, value)?
                .ok_or_else(|| Error::InvalidGpx(format!("rpt has no {field}")))
        };
        push_distinct(
            points,
            TracePoint::new(coordinate("lat")?, coordinate("lon")?),
        );
    }
    for child in &element.children {
        if let XmlNode::Element(child) = child {
            collect_shape_points(child, points)?;
        }
    }
    Ok(())
}

/// Adds a point unless the previous one is at the same position.
fn push_distinct(points: &mut Vec<TracePoint>, point: TracePoint) {
    let repeated = points
        .last()
        .is_some_and(|last| last.lat == point.lat && last.lon == point.lon);
    if !repeated {
        points.push(point);
    }
}

fn element_from_start<R>(reader: &NsReader<R>, start: &BytesStart) -> Result<XmlElement> {
    let (resolved, local_name) = reader.resolver().resolve_element(start.name());
    let mut element = XmlElement {
        prefix: prefix_of(start.name()),
        namespace: namespace_of(resolved)?,
        local_name: local_name.as_ref().to_owned(),
        attributes: Vec::new(),
        children: Vec::new(),
    };
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|e| Error::InvalidGpx(e.to_string()))?;
        // Namespace declarations are regenerated when writing.
        if attribute.key.as_namespace_binding().is_some() {
            continue;
        }
        let (resolved, local_name) = reader.resolver().resolve_attribute(attribute.key);
        element.attributes.push(XmlAttribute {
            prefix: prefix_of(attribute.key),
            namespace: namespace_of(resolved)?,
            local_name: local_name.as_ref().to_owned(),
            value: attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|e| Error::InvalidGpx(e.to_string()))?
                .into_owned(),
        });
    }
    Ok(element)
}

fn prefix_of(name: QName) -> Option<String> {
    let prefix = name.prefix()?;
    Some(prefix.as_ref().to_owned())
}

fn namespace_of(resolved: ResolveResult) -> Result<Option<String>> {
    match resolved {
        ResolveResult::Bound(namespace) => Ok(Some(namespace.as_ref().to_owned())),
        ResolveResult::Unbound => Ok(None),
        ResolveResult::Unknown(prefix) => Err(Error::InvalidGpx(format!(
            "namespace prefix {prefix:?} is not declared"
        ))),
    }
}

fn is_way_id(element: &XmlElement) -> bool {
    element.namespace.as_deref() == Some(WAY_ID_NAMESPACE)
        && element.local_name == WAY_ID_LOCAL_NAME
}

fn way_id_value(element: &XmlElement) -> Option<u64> {
    element.children.iter().find_map(|child| match child {
        XmlNode::Text(text) => text.trim().parse().ok(),
        XmlNode::Element(_) => None,
    })
}

fn non_empty(text: String) -> Option<String> {
    (!text.is_empty()).then_some(text)
}

fn parse_float_field(field: &str, text: &str) -> Result<Option<f64>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<f64>() {
        Ok(value) if value.is_finite() => Ok(Some(value)),
        _ => Err(Error::InvalidGpx(format!(
            "invalid value for {field}: {text:?}"
        ))),
    }
}

fn parse_time(text: &str) -> Option<DateTime<FixedOffset>> {
    let text = text.trim();
    let parsed = |separator: &str| {
        let with_offset = format!("%Y-%m-%d{separator}%H:%M:%S%.f%#z");
        let naive = format!("%Y-%m-%d{separator}%H:%M:%S%.f");
        DateTime::parse_from_str(text, &with_offset)
            .ok()
            .or_else(|| {
                let time = NaiveDateTime::parse_from_str(text, &naive).ok()?;
                Some(time.and_utc().fixed_offset())
            })
    };
    parsed("T").or_else(|| parsed(" "))
}

// --- Writing -----------------------------------------------------------------

/// Writes points to a file as a single track with a single segment.
pub fn save_points(path: impl AsRef<Path>, points: &[TracePoint]) -> Result<()> {
    let path = path.as_ref();
    fs::write(path, render_track(points)).map_err(|source| io_error(path, source))
}

/// Writes points as a single track with a single segment.
///
/// [`TracePoint::extras`] are written back out, and the way ID (if any) as a
/// `<gh:way_id>` extension. [`TracePoint::road_name`] is not stored.
pub fn write_points(mut writer: impl Write, points: &[TracePoint]) -> Result<()> {
    writer
        .write_all(render_track(points).as_bytes())
        .map_err(stream_error)
}

/// Writes points to a file as a GPX route.
pub fn save_route(path: impl AsRef<Path>, points: &[TracePoint]) -> Result<()> {
    let path = path.as_ref();
    fs::write(path, render_route(points)).map_err(|source| io_error(path, source))
}

/// Writes points as a GPX route (`<rte>`/`<rtept>`), which route planners treat
/// as via-points to fill in road geometry between.
///
/// Each route point is named after its road, or else its way ID, for
/// readability in GPS apps.
pub fn write_route(mut writer: impl Write, points: &[TracePoint]) -> Result<()> {
    writer
        .write_all(render_route(points).as_bytes())
        .map_err(stream_error)
}

/// Writes routes to a file as tracks, with waypoints kept alongside them.
pub fn save_routes_as_tracks(path: impl AsRef<Path>, file: &RouteFile) -> Result<()> {
    let path = path.as_ref();
    fs::write(path, render_routes_as_tracks(file)).map_err(|source| io_error(path, source))
}

/// Writes routes as tracks, one per route, with waypoints kept alongside them.
///
/// Apps draw a track exactly as recorded, where they may recalculate a route.
pub fn write_routes_as_tracks(mut writer: impl Write, file: &RouteFile) -> Result<()> {
    writer
        .write_all(render_routes_as_tracks(file).as_bytes())
        .map_err(stream_error)
}

fn io_error(path: &Path, source: io::Error) -> Error {
    Error::Io {
        path: Some(PathBuf::from(path)),
        source,
    }
}

fn stream_error(source: io::Error) -> Error {
    Error::Io { path: None, source }
}

/// The namespace prefixes in effect at a point in the output, innermost last.
/// `None` is the default namespace.
type Scope = Vec<(Option<String>, String)>;

fn root_scope(points: &[TracePoint]) -> Scope {
    let mut scope: Scope = vec![
        (None, GPX_NAMESPACE.to_owned()),
        (Some(WAY_ID_PREFIX.to_owned()), WAY_ID_NAMESPACE.to_owned()),
        (Some("xsi".to_owned()), XSI_NAMESPACE.to_owned()),
    ];
    for point in points {
        for extension in &point.extras.extensions {
            hoist_prefixes(extension, &mut scope);
        }
    }
    scope
}

/// Declares prefixes once on the root element rather than on every point, which
/// keeps files small. If two namespaces want one prefix, the first gets the root
/// declaration and the writer declares the other on the elements that use it.
fn hoist_prefixes(element: &XmlElement, scope: &mut Scope) {
    let mut hoist = |prefix: &Option<String>, namespace: &Option<String>| {
        if let (Some(prefix), Some(namespace)) = (prefix, namespace) {
            let taken = prefix == "xml" || scope.iter().any(|(p, _)| p.as_ref() == Some(prefix));
            if !taken {
                scope.push((Some(prefix.clone()), namespace.clone()));
            }
        }
    };
    hoist(&element.prefix, &element.namespace);
    for attribute in &element.attributes {
        hoist(&attribute.prefix, &attribute.namespace);
    }
    for child in &element.children {
        if let XmlNode::Element(child) = child {
            hoist_prefixes(child, scope);
        }
    }
}

fn open_document(out: &mut String, scope: &Scope) {
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gpx");
    for (prefix, namespace) in scope {
        push_namespace_declaration(out, prefix.as_deref(), namespace);
    }
    let _ = writeln!(
        out,
        " xsi:schemaLocation=\"{SCHEMA_LOCATION}\" version=\"1.1\" creator=\"{CREATOR}\">"
    );
}

fn push_namespace_declaration(out: &mut String, prefix: Option<&str>, namespace: &str) {
    let namespace = escape(namespace);
    match prefix {
        Some(prefix) => {
            let _ = write!(out, " xmlns:{prefix}=\"{namespace}\"");
        }
        None => {
            let _ = write!(out, " xmlns=\"{namespace}\"");
        }
    }
}

fn render_track(points: &[TracePoint]) -> String {
    let mut scope = root_scope(points);
    let mut out = String::new();
    open_document(&mut out, &scope);
    out.push_str("  <trk>\n    <trkseg>\n");
    for point in points {
        render_point(&mut out, "trkpt", point, 6, &mut scope);
    }
    out.push_str("    </trkseg>\n  </trk>\n</gpx>\n");
    out
}

fn render_point(out: &mut String, tag: &str, point: &TracePoint, indent: usize, scope: &mut Scope) {
    let extras = &point.extras;
    push_point_open(out, tag, point, indent);
    if let Some(elevation) = point.elevation {
        push_field(out, indent + 2, "ele", &format_float(elevation));
    }
    if let Some(time) = point.time {
        let time = time.to_rfc3339_opts(SecondsFormat::AutoSi, true);
        push_field(out, indent + 2, "time", &time);
    }
    if let Some(value) = extras.magnetic_variation {
        push_field(out, indent + 2, "magvar", &format_float(value));
    }
    let text_fields = [
        ("name", &extras.name),
        ("cmt", &extras.comment),
        ("sym", &extras.symbol),
        ("type", &extras.kind),
    ];
    for (tag, value) in text_fields {
        if let Some(value) = value {
            push_field(out, indent + 2, tag, value);
        }
    }
    let dilution_fields = [
        ("hdop", extras.horizontal_dilution),
        ("vdop", extras.vertical_dilution),
        ("pdop", extras.position_dilution),
    ];
    for (tag, value) in dilution_fields {
        if let Some(value) = value {
            push_field(out, indent + 2, tag, &format_float(value));
        }
    }
    if !extras.extensions.is_empty() || point.way_id.is_some() {
        let _ = writeln!(out, "{:indent$}  <extensions>", "");
        for extension in &extras.extensions {
            push_element(out, extension, scope, Some(indent + 4));
        }
        if let Some(way_id) = point.way_id {
            let tag = format!("{WAY_ID_PREFIX}:{WAY_ID_LOCAL_NAME}");
            push_field(out, indent + 4, &tag, &way_id.to_string());
        }
        let _ = writeln!(out, "{:indent$}  </extensions>", "");
    }
    let _ = writeln!(out, "{:indent$}</{tag}>", "");
}

fn render_routes_as_tracks(file: &RouteFile) -> String {
    let mut scope = root_scope(&file.waypoints);
    let mut out = String::new();
    open_document(&mut out, &scope);
    for waypoint in &file.waypoints {
        render_point(&mut out, "wpt", waypoint, 2, &mut scope);
    }
    for route in &file.routes {
        out.push_str("  <trk>\n");
        if let Some(name) = &route.name {
            push_field(&mut out, 4, "name", name);
        }
        out.push_str("    <trkseg>\n");
        for point in &route.points {
            render_point(&mut out, "trkpt", point, 6, &mut scope);
        }
        out.push_str("    </trkseg>\n  </trk>\n");
    }
    out.push_str("</gpx>\n");
    out
}

fn render_route(points: &[TracePoint]) -> String {
    const INDENT: usize = 4;
    let scope = vec![
        (None, GPX_NAMESPACE.to_owned()),
        (Some("xsi".to_owned()), XSI_NAMESPACE.to_owned()),
    ];
    let mut out = String::new();
    open_document(&mut out, &scope);
    out.push_str("  <rte>\n");
    for point in points {
        push_point_open(&mut out, "rtept", point, INDENT);
        if let Some(elevation) = point.elevation {
            push_field(&mut out, INDENT + 2, "ele", &format_float(elevation));
        }
        if let Some(name) = route_point_name(point) {
            push_field(&mut out, INDENT + 2, "name", &name);
        }
        let _ = writeln!(out, "{:INDENT$}</rtept>", "");
    }
    out.push_str("  </rte>\n</gpx>\n");
    out
}

fn route_point_name(point: &TracePoint) -> Option<String> {
    match (point.road_name.as_deref(), point.way_id) {
        (Some(name), _) if !name.is_empty() => Some(name.to_owned()),
        (_, Some(way_id)) if way_id != 0 => Some(format!("way {way_id}")),
        _ => None,
    }
}

fn push_point_open(out: &mut String, tag: &str, point: &TracePoint, indent: usize) {
    let lat = format_float(point.lat);
    let lon = format_float(point.lon);
    let _ = writeln!(out, "{:indent$}<{tag} lat=\"{lat}\" lon=\"{lon}\">", "");
}

fn push_field(out: &mut String, indent: usize, tag: &str, value: &str) {
    let value = partial_escape(value);
    let _ = writeln!(out, "{:indent$}<{tag}>{value}</{tag}>", "");
}

/// Writes an element, declaring whatever namespaces `scope` doesn't already
/// provide. With `indent` set, it goes on lines of its own.
fn push_element(out: &mut String, element: &XmlElement, scope: &mut Scope, indent: Option<usize>) {
    let scope_len = scope.len();
    let name = qualified_name(
        element.prefix.as_deref(),
        element.namespace.as_deref(),
        &element.local_name,
    );

    if let Some(indent) = indent {
        let _ = write!(out, "{:indent$}", "");
    }
    let _ = write!(out, "<{name}");

    let element_prefix = element
        .prefix
        .as_deref()
        .filter(|_| element.namespace.is_some());
    declare_if_needed(
        out,
        scope,
        element_prefix,
        element.namespace.as_deref().unwrap_or(""),
    );
    for attribute in &element.attributes {
        if let (Some(prefix), Some(namespace)) = (&attribute.prefix, &attribute.namespace) {
            declare_if_needed(out, scope, Some(prefix), namespace);
        }
    }
    for attribute in &element.attributes {
        let name = qualified_name(
            attribute.prefix.as_deref(),
            attribute.namespace.as_deref(),
            &attribute.local_name,
        );
        let _ = write!(out, " {name}=\"{}\"", escape(&attribute.value));
    }

    let only_elements = element
        .children
        .iter()
        .all(|c| matches!(c, XmlNode::Element(_)));
    if element.children.is_empty() {
        out.push_str("/>");
    } else {
        out.push('>');
        let child_indent = indent.filter(|_| only_elements).map(|indent| indent + 2);
        if child_indent.is_some() {
            out.push('\n');
        }
        for child in &element.children {
            match child {
                XmlNode::Element(child) => push_element(out, child, scope, child_indent),
                XmlNode::Text(text) => out.push_str(&partial_escape(text)),
            }
        }
        if let Some(indent) = indent.filter(|_| only_elements) {
            let _ = write!(out, "{:indent$}", "");
        }
        let _ = write!(out, "</{name}>");
    }
    if indent.is_some() {
        out.push('\n');
    }
    scope.truncate(scope_len);
}

fn qualified_name(prefix: Option<&str>, namespace: Option<&str>, local_name: &str) -> String {
    match (prefix, namespace) {
        (Some(prefix), Some(_)) => format!("{prefix}:{local_name}"),
        _ => local_name.to_owned(),
    }
}

fn declare_if_needed(out: &mut String, scope: &mut Scope, prefix: Option<&str>, namespace: &str) {
    if prefix == Some("xml") {
        return;
    }
    let current = scope
        .iter()
        .rev()
        .find(|(p, _)| p.as_deref() == prefix)
        .map_or("", |(_, namespace)| namespace.as_str());
    if current != namespace {
        push_namespace_declaration(out, prefix, namespace);
        scope.push((prefix.map(str::to_owned), namespace.to_owned()));
    }
}

/// Formats a float for XML: never in exponent form, which `xsd:decimal` forbids.
fn format_float(value: f64) -> String {
    let text = value.to_string();
    if text.contains('.') {
        text
    } else {
        text + ".0"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WRAP_OPEN: &str = "<?xml version='1.0' encoding='UTF-8'?>
<gpx version='1.1' creator='test' xmlns='http://www.topografix.com/GPX/1/1'
     xmlns:gh='https://github.com/gps-helper/gps-helper'
     xmlns:tpx='urn:garmin:tpx'>
  <trk><trkseg>";
    const WRAP_CLOSE: &str = "</trkseg></trk></gpx>";

    fn read(xml: &str) -> Result<Vec<TracePoint>> {
        read_points(xml.as_bytes())
    }

    fn read_segment(body: &str) -> Vec<TracePoint> {
        read(&format!("{WRAP_OPEN}{body}{WRAP_CLOSE}")).unwrap()
    }

    fn render(points: &[TracePoint]) -> String {
        let mut out = Vec::new();
        write_points(&mut out, points).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn render_rte(points: &[TracePoint]) -> String {
        let mut out = Vec::new();
        write_route(&mut out, points).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn round_trip(points: &[TracePoint]) -> Vec<TracePoint> {
        read(&render(points)).unwrap()
    }

    #[test]
    fn way_id_round_trip_keeps_coordinates_and_elevation() {
        let mut points = read_segment(
            "<trkpt lat='40.0' lon='-105.0'><ele>1600</ele></trkpt>
             <trkpt lat='40.001' lon='-105.001'></trkpt>",
        );
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].way_id, None);

        points[0].way_id = Some(42);
        points[1].way_id = Some(99);
        let reread = round_trip(&points);
        assert_eq!(reread[0].way_id, Some(42));
        assert_eq!(reread[1].way_id, Some(99));
        assert!((reread[0].lat - 40.0).abs() < 1e-9);
        assert!((reread[1].lon - -105.001).abs() < 1e-9);
        assert_eq!(reread[0].elevation, Some(1600.0));
    }

    #[test]
    fn write_uses_snapped_coordinates() {
        let mut points = read_segment("<trkpt lat='40.0' lon='-105.0'></trkpt>");
        points[0].lat = 40.5;
        points[0].lon = -105.5;
        points[0].way_id = Some(7);
        let reread = round_trip(&points);
        assert!((reread[0].lat - 40.5).abs() < 1e-9);
        assert!((reread[0].lon - -105.5).abs() < 1e-9);
        assert_eq!(reread[0].way_id, Some(7));
    }

    #[test]
    fn reads_all_tracks_and_segments_but_not_other_points() {
        let points = read(
            "<gpx>
               <wpt lat='1' lon='1'/>
               <trk><trkseg><trkpt lat='2' lon='2'/></trkseg>
                    <trkseg><trkpt lat='3' lon='3'/></trkseg></trk>
               <trk><trkseg><trkpt lat='4' lon='4'/></trkseg></trk>
               <rte><rtept lat='5' lon='5'/></rte>
               <extensions><trk><trkseg><trkpt lat='6' lon='6'/></trkseg></trk></extensions>
             </gpx>",
        )
        .unwrap();
        let lats: Vec<_> = points.iter().map(|p| p.lat).collect();
        assert_eq!(lats, [2.0, 3.0, 4.0]);
    }

    #[test]
    fn extras_round_trip_with_escaping() {
        let points = read_segment(
            "<trkpt lat='1' lon='2'>
               <magvar>3.5</magvar><name>A &lt;one&gt; &amp; &#x22;two&#34;</name>
               <cmt><![CDATA[a<b]]></cmt><sym>Flag</sym><type>t</type>
               <hdop>1.25</hdop><vdop>2</vdop><pdop>3.0</pdop>
             </trkpt>",
        );
        for point in [&points[0], &round_trip(&points)[0]] {
            let extras = &point.extras;
            assert_eq!(extras.name.as_deref(), Some("A <one> & \"two\""));
            assert_eq!(extras.comment.as_deref(), Some("a<b"));
            assert_eq!(extras.symbol.as_deref(), Some("Flag"));
            assert_eq!(extras.kind.as_deref(), Some("t"));
            assert_eq!(extras.magnetic_variation, Some(3.5));
            assert_eq!(extras.horizontal_dilution, Some(1.25));
            assert_eq!(extras.vertical_dilution, Some(2.0));
            assert_eq!(extras.position_dilution, Some(3.0));
        }
    }

    #[test]
    fn foreign_extensions_round_trip_as_valid_xml() {
        let points = read_segment(
            "<trkpt lat='1' lon='2'><extensions>
               <tpx:TrackPointExtension>
                 <tpx:hr>140</tpx:hr><tpx:cad tpx:unit='r&amp;m' plain='1'>80</tpx:cad>
               </tpx:TrackPointExtension>
               <gh:way_id>42</gh:way_id>
               <foo xmlns='urn:foo' a='1'>bar &amp; baz<inner/></foo>
               <unspecified>in gpx namespace</unspecified>
             </extensions></trkpt>",
        );
        assert_eq!(points[0].way_id, Some(42));
        assert_eq!(points[0].extras.extensions.len(), 3);

        let xml = render(&points);
        assert!(xml.contains(r#"xmlns:tpx="urn:garmin:tpx""#), "{xml}");
        assert_eq!(
            xml.matches("urn:garmin:tpx").count(),
            1,
            "declared once: {xml}"
        );
        assert!(xml.contains("<tpx:hr>140</tpx:hr>"), "{xml}");

        let reread = read(&xml).unwrap();
        assert_eq!(reread, points);
    }

    #[test]
    fn prefix_collisions_stay_namespace_correct() {
        let points = read_segment(
            "<trkpt lat='1' lon='1'><extensions><x:a xmlns:x='urn:one'>1</x:a></extensions></trkpt>
             <trkpt lat='2' lon='2'><extensions><x:a xmlns:x='urn:two'>2</x:a></extensions></trkpt>
             <trkpt lat='3' lon='3'><extensions><gh:a xmlns:gh='urn:three'>3</gh:a></extensions></trkpt>",
        );
        let reread = round_trip(&points);
        let namespaces: Vec<_> = reread
            .iter()
            .map(|p| p.extras.extensions[0].namespace.as_deref().unwrap())
            .collect();
        assert_eq!(namespaces, ["urn:one", "urn:two", "urn:three"]);
    }

    #[test]
    fn extension_in_no_namespace_stays_there() {
        let points = read(
            "<gpx><trk><trkseg><trkpt lat='1' lon='1'><extensions><plain>x</plain>\
                           </extensions></trkpt></trkseg></trk></gpx>",
        )
        .unwrap();
        let reread = round_trip(&points);
        assert_eq!(reread[0].extras.extensions[0].namespace, None);
    }

    #[test]
    fn way_id_is_found_by_namespace_not_prefix() {
        let points = read_segment(
            "<trkpt lat='1' lon='1'><extensions>
               <other:way_id xmlns:other='https://github.com/gps-helper/gps-helper'>5</other:way_id>
             </extensions></trkpt>
             <trkpt lat='2' lon='2'><extensions>
               <tpx:way_id>6</tpx:way_id>
             </extensions></trkpt>",
        );
        assert_eq!(points[0].way_id, Some(5));
        assert!(points[0].extras.extensions.is_empty());
        assert_eq!(points[1].way_id, None);
        assert_eq!(points[1].extras.extensions.len(), 1);
    }

    #[test]
    fn unreadable_way_id_means_unaligned() {
        for (text, expected) in [(" 43 ", Some(43)), ("abc", None), ("", None), ("-1", None)] {
            let points = read_segment(&format!(
                "<trkpt lat='1' lon='1'><extensions><gh:way_id>{text}</gh:way_id></extensions></trkpt>"
            ));
            assert_eq!(points[0].way_id, expected, "{text:?}");
            assert!(points[0].extras.extensions.is_empty());
        }
    }

    #[test]
    fn has_any_way_id_detects_alignment() {
        let mut points = vec![TracePoint::new(0.0, 0.0); 2];
        assert!(!has_any_way_id(&points));
        points[1].way_id = Some(1);
        assert!(has_any_way_id(&points));
    }

    #[test]
    fn time_formats() {
        let parse = |text| parse_time(text).map(|t| t.to_rfc3339_opts(SecondsFormat::AutoSi, true));
        let cases = [
            ("2024-03-01T10:00:00Z", "2024-03-01T10:00:00Z"),
            ("2024-03-01T10:00:00+00:00", "2024-03-01T10:00:00Z"),
            (
                "2024-03-01T10:00:01.5+02:00",
                "2024-03-01T10:00:01.500+02:00",
            ),
            ("2024-03-01T10:00:01-0330", "2024-03-01T10:00:01-03:30"),
            ("2024-03-01T10:00:01+02", "2024-03-01T10:00:01+02:00"),
            ("2024-03-01 10:00:01Z", "2024-03-01T10:00:01Z"),
            ("2024-3-1T1:02:03Z", "2024-03-01T01:02:03Z"),
            (" 2024-03-01T10:00:02 ", "2024-03-01T10:00:02Z"),
        ];
        for (input, expected) in cases {
            assert_eq!(parse(input).as_deref(), Some(expected), "{input}");
        }
        for garbage in ["garbage", "", "2024-13-01T00:00:00Z"] {
            assert_eq!(parse(garbage), None, "{garbage}");
        }
    }

    #[test]
    fn unreadable_time_becomes_none() {
        let points = read_segment("<trkpt lat='1' lon='1'><time>garbage</time></trkpt>");
        assert_eq!(points[0].time, None);
    }

    #[test]
    fn empty_fields_are_absent() {
        let points =
            read_segment("<trkpt lat='1' lon='1'><ele></ele><name/><hdop> </hdop></trkpt>");
        assert_eq!(points[0].elevation, None);
        assert_eq!(points[0].extras.name, None);
        assert_eq!(points[0].extras.horizontal_dilution, None);
    }

    #[test]
    fn floats_are_never_written_in_exponent_form() {
        assert_eq!(format_float(40.0), "40.0");
        assert_eq!(format_float(-105.123456789012), "-105.123456789012");
        assert_eq!(format_float(0.000001), "0.000001");
        assert_eq!(format_float(1e21), "1000000000000000000000.0");
    }

    #[test]
    fn malformed_input_is_an_error() {
        let bad_documents = [
            "",
            "not xml",
            "<gpx><trk>",
            "<gpx><trk><trkseg><trkpt lat='1' lon='1'>",
            "<gpx><trk><trkseg><trkpt lat='1' lon='1'><extensions><a>",
            "<gpx><trk></gpx>",
            "<gpx/><gpx></gpx>",
            "<gpx><trk><trkseg><trkpt lat='x' lon='1'/></trkseg></trk></gpx>",
            "<gpx><trk><trkseg><trkpt lat='NaN' lon='1'/></trkseg></trk></gpx>",
            "<gpx><trk><trkseg><trkpt lat='inf' lon='1'/></trkseg></trk></gpx>",
            "<gpx><trk><trkseg><trkpt lon='1'/></trkseg></trk></gpx>",
            "<gpx><trk><trkseg><trkpt lat='1'/></trkseg></trk></gpx>",
            "<gpx><trk><trkseg><trkpt lat='1' lon='1'><ele>high</ele></trkpt></trkseg></trk></gpx>",
            "<gpx><trk><trkseg><trkpt lat='1' lon='1'><name>&nope;</name></trkpt></trkseg></trk></gpx>",
            "<gpx><trk><trkseg><trkpt lat='1' lon='1'><extensions><u:a/></extensions></trkpt></trkseg></trk></gpx>",
        ];
        for xml in bad_documents {
            let err = read(xml).expect_err(xml);
            assert!(matches!(err, Error::InvalidGpx(_)), "{xml}: {err}");
        }
    }

    #[test]
    fn invalid_utf8_is_an_error_not_a_panic() {
        let bytes = b"<gpx><trk><trkseg><trkpt lat='1' lon='1'><name>\xff\xfe</name></trkpt></trkseg></trk></gpx>";
        assert!(read_points(&bytes[..]).is_err());
    }

    #[test]
    fn bom_is_accepted() {
        let points =
            read("\u{feff}<gpx><trk><trkseg><trkpt lat='1' lon='1'/></trkseg></trk></gpx>");
        assert_eq!(points.unwrap().len(), 1);
    }

    #[test]
    fn root_without_tracks_has_no_points() {
        assert!(read("<nope/>").unwrap().is_empty());
        assert!(read("<gpx></gpx>").unwrap().is_empty());
    }

    #[test]
    fn deep_extension_nesting_is_rejected_not_a_stack_overflow() {
        let depth = 200_000;
        let xml = format!(
            "{WRAP_OPEN}<trkpt lat='1' lon='1'><extensions>{}{}</extensions></trkpt>{WRAP_CLOSE}",
            "<a>".repeat(depth),
            "</a>".repeat(depth),
        );
        assert!(matches!(read(&xml), Err(Error::InvalidGpx(_))));
    }

    #[test]
    fn route_structure_and_names() {
        let named = |lat, way, name: Option<&str>| {
            let mut point = TracePoint::new(lat, -105.0);
            point.way_id = way;
            point.road_name = name.map(str::to_owned);
            point
        };
        let xml = render_rte(&[
            named(40.0, Some(1), Some("Main & St")),
            named(40.01, Some(2), Some("")),
            named(40.02, Some(3), None),
            named(40.03, None, None),
            named(40.04, Some(0), None),
        ]);
        assert!(xml.contains("<rte>"), "{xml}");
        assert!(!xml.contains("<trk>"), "{xml}");
        assert!(!xml.contains("way_id"), "{xml}");
        assert_eq!(xml.matches("<rtept ").count(), 5, "{xml}");
        let names: Vec<_> = xml
            .lines()
            .filter(|l| l.contains("<name>"))
            .map(str::trim)
            .collect();
        assert_eq!(
            names,
            [
                "<name>Main &amp; St</name>",
                "<name>way 2</name>",
                "<name>way 3</name>"
            ]
        );
    }

    #[test]
    fn empty_outputs_are_well_formed() {
        assert!(read(&render(&[])).unwrap().is_empty());
        assert!(read(&render_rte(&[])).unwrap().is_empty());
    }

    #[test]
    fn missing_file_error_names_the_path() {
        let err = load_points("/no/such/file.gpx").unwrap_err();
        assert!(matches!(err, Error::Io { .. }));
        assert!(err.to_string().contains("/no/such/file.gpx"), "{err}");
    }

    #[test]
    fn save_and_load_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.gpx");
        let mut point = TracePoint::new(1.0, 2.0);
        point.way_id = Some(3);
        save_points(&path, std::slice::from_ref(&point)).unwrap();
        assert_eq!(load_points(&path).unwrap(), [point.clone()]);
        save_route(&path, &[point]).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("<rtept"));
    }

    const GARMIN_ROUTE: &str = "<?xml version='1.0' encoding='UTF-8'?>
<gpx version='1.1' creator='test' xmlns='http://www.topografix.com/GPX/1/1'
     xmlns:gpxx='http://www.garmin.com/xmlschemas/GpxExtensions/v3'
     xmlns:other='urn:other'>
  <wpt lat='1.5' lon='2.5'><name>Caf&#233; &amp; Bar</name><sym>Flag</sym>
    <extensions><other:note>hi</other:note></extensions></wpt>
  <wpt lat='3.5' lon='4.5'/>
  <trk><trkseg><trkpt lat='50' lon='50'/></trkseg></trk>
  <rte>
    <name>Coast &lt;&amp;&gt; Hills</name>
    <rtept lat='10' lon='20'>
      <ele>100</ele><name>Start</name>
      <extensions><gpxx:RoutePointExtension>
        <gpxx:Subclass>000000</gpxx:Subclass>
        <gpxx:rpt lat='10' lon='20'/>
        <gpxx:rpt lat='10.1' lon='20.1'><gpxx:Subclass>01</gpxx:Subclass></gpxx:rpt>
        <other:rpt lat='99' lon='99'/>
        <gpxx:rpt lat='11' lon='21'/>
      </gpxx:RoutePointExtension></extensions>
    </rtept>
    <rtept lat='11' lon='21'/>
    <rtept lat='10' lon='20'/>
  </rte>
  <rte><rtept lat='30' lon='40'/><rtept lat='30' lon='40'/></rte>
  <rte/>
</gpx>";

    fn coordinates(points: &[TracePoint]) -> Vec<(f64, f64)> {
        points.iter().map(|p| (p.lat, p.lon)).collect()
    }

    #[test]
    fn routes_flatten_shape_points_in_order_and_merge_repeats() {
        let file = read_routes(GARMIN_ROUTE.as_bytes()).unwrap();
        let route = &file.routes[0];
        assert_eq!(route.name.as_deref(), Some("Coast <&> Hills"));
        assert_eq!(route.via_point_count, 3);
        // Only repeats in a row merge, so returning to the start is kept.
        assert_eq!(
            coordinates(&route.points),
            [(10.0, 20.0), (10.1, 20.1), (11.0, 21.0), (10.0, 20.0)]
        );
        assert_eq!(route.points[0], TracePoint::new(10.0, 20.0));
    }

    #[test]
    fn routes_without_shape_points_or_names_still_count() {
        let file = read_routes(GARMIN_ROUTE.as_bytes()).unwrap();
        assert_eq!(file.routes.len(), 3);
        assert_eq!(file.routes[1].name, None);
        assert_eq!(file.routes[1].via_point_count, 2);
        assert_eq!(coordinates(&file.routes[1].points), [(30.0, 40.0)]);
        assert_eq!(file.routes[2], Route::default());
    }

    #[test]
    fn reading_routes_keeps_waypoints_and_ignores_tracks() {
        let file = read_routes(GARMIN_ROUTE.as_bytes()).unwrap();
        assert_eq!(coordinates(&file.waypoints), [(1.5, 2.5), (3.5, 4.5)]);
        assert_eq!(file.waypoints[0].extras.name.as_deref(), Some("Café & Bar"));
        assert_eq!(file.waypoints[0].extras.symbol.as_deref(), Some("Flag"));
        assert_eq!(read(GARMIN_ROUTE).unwrap().len(), 1);
    }

    #[test]
    fn routes_are_written_as_one_track_each_after_the_waypoints() {
        let file = read_routes(GARMIN_ROUTE.as_bytes()).unwrap();
        let mut out = Vec::new();
        write_routes_as_tracks(&mut out, &file).unwrap();
        let xml = String::from_utf8(out).unwrap();

        assert_eq!(xml.matches("<trk>").count(), 3);
        assert!(!xml.contains("<rte"));
        assert!(!xml.contains("gpxx"));
        assert!(xml.contains("<name>Coast &lt;&amp;&gt; Hills</name>"));
        assert!(xml.find("<wpt").unwrap() < xml.find("<trk>").unwrap());

        let points = read(&xml).unwrap();
        assert_eq!(points.len(), 5);
        assert_eq!(points[1], TracePoint::new(10.1, 20.1));

        let reread = read_routes(xml.as_bytes()).unwrap();
        assert_eq!(reread.waypoints, file.waypoints);
    }

    #[test]
    fn unnamed_routes_give_unnamed_tracks() {
        let route = Route {
            points: vec![TracePoint::new(1.0, 2.0)],
            ..Route::default()
        };
        let file = RouteFile {
            routes: vec![route],
            ..RouteFile::default()
        };
        let xml = render_routes_as_tracks(&file);
        assert!(!xml.contains("<name>"));
    }

    #[test]
    fn invalid_route_coordinates_are_errors() {
        let wrap = |body: &str| {
            format!(
                "<gpx xmlns:gpxx='http://www.garmin.com/xmlschemas/GpxExtensions/v3'>{body}</gpx>"
            )
        };
        let shape = |rpt: &str| {
            wrap(&format!(
                "<rte><rtept lat='1' lon='1'><extensions>{rpt}</extensions></rtept></rte>"
            ))
        };
        let cases = [
            (wrap("<wpt lon='1'/>"), "wpt has no lat"),
            (wrap("<rte><rtept lat='1'/></rte>"), "rtept has no lon"),
            (shape("<gpxx:rpt lon='1'/>"), "rpt has no lat"),
            (shape("<gpxx:rpt lat='1' lon=''/>"), "rpt has no lon"),
            (
                shape("<gpxx:rpt lat='x' lon='1'/>"),
                "invalid value for lat",
            ),
            (
                shape("<gpxx:rpt lat='NaN' lon='1'/>"),
                "invalid value for lat",
            ),
            (wrap("<rte><rtept lat='1' lon='1'>"), "invalid GPX"),
        ];
        for (xml, expected) in cases {
            let error = read_routes(xml.as_bytes()).unwrap_err().to_string();
            assert!(error.contains(expected), "{xml}: {error}");
        }
    }
}
