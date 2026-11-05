//! Snap GPS traces to OSM roads and simplify them for route planners.
//!
//! The pipeline has three steps, each usable on its own:
//!
//! 1. [`gpx`] reads and writes GPX files as [`TracePoint`]s.
//! 2. [`align()`] map-matches points through a [`Backend`], attaching an OSM
//!    way ID to each one. [`backends`] has the Valhalla and OSRM clients, and
//!    you can implement the trait for your own matcher.
//! 3. [`simplify()`] and [`simplify_to_route`] drop redundant points.
//!
//! ```
//! use gps_helper::{TracePoint, gpx, simplify_to_route};
//!
//! let points: Vec<TracePoint> = [(40.0, 1), (40.1, 1), (40.2, 1), (40.3, 2)]
//!     .into_iter()
//!     .map(|(lat, way)| {
//!         let mut point = TracePoint::new(lat, -105.0);
//!         point.way_id = Some(way);
//!         point
//!     })
//!     .collect();
//!
//! let route = simplify_to_route(&points);
//!
//! let mut xml = Vec::new();
//! gpx::write_route(&mut xml, &route)?;
//! assert!(String::from_utf8(xml)?.contains("<name>way 1</name>"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! This crate does blocking I/O and never prints. Long-running calls report
//! through callbacks such as [`align_with_progress`].
#![warn(missing_docs)]

mod align;
pub mod backends;
mod error;
pub mod gpx;
pub mod http;
mod model;
mod simplify;

pub use align::{AlignOptions, Progress, align, align_with_progress};
pub use backends::Backend;
pub use chrono;
pub use error::{Error, Result};
pub use model::{PointExtras, TracePoint, XmlAttribute, XmlElement, XmlNode};
pub use simplify::{DEFAULT_CONTEXT, simplify, simplify_to_route};
