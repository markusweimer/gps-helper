//! Shared data types.

use chrono::{DateTime, FixedOffset};

/// A single GPS point, as it moves through alignment and simplification.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TracePoint {
    /// Latitude in degrees.
    pub lat: f64,
    /// Longitude in degrees.
    pub lon: f64,
    /// Elevation in meters.
    pub elevation: Option<f64>,
    /// When the point was recorded.
    pub time: Option<DateTime<FixedOffset>>,
    /// The OSM way this point is on, once known.
    pub way_id: Option<u64>,
    /// The name of the road this point is on, if the backend provided one.
    /// This is not stored in track output, so it does not survive a round trip
    /// through a file.
    pub road_name: Option<String>,
    /// Other fields from the source file, carried along so they can be written
    /// back out unchanged.
    pub extras: PointExtras,
}

impl TracePoint {
    /// Creates a point with only coordinates set.
    pub fn new(lat: f64, lon: f64) -> Self {
        Self {
            lat,
            lon,
            elevation: None,
            time: None,
            way_id: None,
            road_name: None,
            extras: PointExtras::default(),
        }
    }
}

/// Optional GPX trackpoint fields with no role in the pipeline.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct PointExtras {
    /// `<magvar>`: magnetic variation in degrees.
    pub magnetic_variation: Option<f64>,
    /// `<name>`: the point's name.
    pub name: Option<String>,
    /// `<cmt>`: a comment.
    pub comment: Option<String>,
    /// `<sym>`: a symbol name.
    pub symbol: Option<String>,
    /// `<type>`: a classification.
    pub kind: Option<String>,
    /// `<hdop>`: horizontal dilution of precision.
    pub horizontal_dilution: Option<f64>,
    /// `<vdop>`: vertical dilution of precision.
    pub vertical_dilution: Option<f64>,
    /// `<pdop>`: position dilution of precision.
    pub position_dilution: Option<f64>,
    /// Children of `<extensions>`, such as heart rate or cadence. The way ID
    /// extension is not included; it lives in [`TracePoint::way_id`].
    pub extensions: Vec<XmlElement>,
}

/// An XML element from a GPX `<extensions>` block.
#[derive(Debug, Clone, PartialEq)]
pub struct XmlElement {
    /// The prefix used in the source document, if any.
    pub prefix: Option<String>,
    /// The namespace URI the element belongs to, if any.
    pub namespace: Option<String>,
    /// The name without its prefix.
    pub local_name: String,
    /// The element's attributes, not including namespace declarations.
    pub attributes: Vec<XmlAttribute>,
    /// Child elements and text, in document order.
    pub children: Vec<XmlNode>,
}

/// An attribute on an [`XmlElement`].
#[derive(Debug, Clone, PartialEq)]
pub struct XmlAttribute {
    /// The prefix used in the source document, if any.
    pub prefix: Option<String>,
    /// The namespace URI the attribute belongs to, if any.
    pub namespace: Option<String>,
    /// The name without its prefix.
    pub local_name: String,
    /// The unescaped value.
    pub value: String,
}

/// Content inside an [`XmlElement`].
#[derive(Debug, Clone, PartialEq)]
pub enum XmlNode {
    /// A nested element.
    Element(XmlElement),
    /// Unescaped text.
    Text(String),
}
