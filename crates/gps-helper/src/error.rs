//! The crate-wide error type.

use std::path::PathBuf;

/// A boxed error from a lower-level library or a custom [`Backend`](crate::Backend).
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Shorthand for a [`Result`](std::result::Result) with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong in this crate.
///
/// Errors from the underlying HTTP, XML and JSON libraries are boxed so those
/// libraries stay out of the public API.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Reading or writing failed. `path` is set when a file was involved.
    #[error("{}{source}", path.as_ref().map(|p| format!("{}: ", p.display())).unwrap_or_default())]
    Io {
        /// The file being accessed, if any.
        path: Option<PathBuf>,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// The input is not well-formed XML, or is not valid GPX.
    #[error("invalid GPX: {0}")]
    InvalidGpx(String),

    /// An option was out of range.
    #[error("invalid option: {0}")]
    InvalidOption(String),

    /// No backend is registered under this name.
    #[error("unknown backend {name:?}; available: {}", available.join(", "))]
    UnknownBackend {
        /// The name that was asked for.
        name: String,
        /// The names that exist.
        available: Vec<String>,
    },

    /// A backend broke the contract of returning one point per input point.
    #[error("backend returned {returned} points for a chunk of {expected}")]
    BackendLengthMismatch {
        /// How many points came back.
        returned: usize,
        /// How many points were sent.
        expected: usize,
    },

    /// A request failed without an HTTP response, after any retries.
    #[error("request to {url} failed: {source}")]
    Transport {
        /// The request URL, possibly shortened.
        url: String,
        /// The underlying error.
        #[source]
        source: BoxError,
    },

    /// The server answered with an error status, after any retries.
    #[error("HTTP {status} from {url}: {body}")]
    HttpStatus {
        /// The request URL, possibly shortened.
        url: String,
        /// The response status code.
        status: u16,
        /// The start of the response body. Map-matching services explain
        /// failures such as "no suitable edges" here.
        body: String,
    },

    /// The server answered successfully, but the body could not be understood.
    #[error("invalid response from {service}: {source}")]
    InvalidResponse {
        /// Which service sent the response.
        service: &'static str,
        /// The underlying error.
        #[source]
        source: BoxError,
    },

    /// A failure inside a custom [`Backend`](crate::Backend) implementation.
    #[error(transparent)]
    Backend(BoxError),
}
