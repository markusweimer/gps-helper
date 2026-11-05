//! Align orchestration: chunking, backend invocation, stitching.

use crate::backends::Backend;
use crate::error::{Error, Result};
use crate::model::TracePoint;

/// How [`align`] splits a trace into backend requests.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AlignOptions {
    /// Maximum points per backend request.
    pub chunk_size: usize,
    /// Points shared by successive chunks. Must be less than `chunk_size`.
    pub overlap: usize,
}

impl Default for AlignOptions {
    fn default() -> Self {
        Self {
            chunk_size: 100,
            overlap: 5,
        }
    }
}

/// A snapshot of alignment progress, reported after each chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Progress {
    /// Points finished so far.
    pub done_points: usize,
    /// Points in the whole trace.
    pub total_points: usize,
    /// The chunk that just finished, counting from 1.
    pub chunk: usize,
    /// Chunks in the whole trace.
    pub total_chunks: usize,
}

/// Map-matches `points` via `backend`, chunked for public-endpoint limits.
///
/// Chunks overlap by [`AlignOptions::overlap`] points; for the overlap region
/// the results from the earlier chunk win.
pub fn align(
    points: &[TracePoint],
    backend: &dyn Backend,
    options: &AlignOptions,
) -> Result<Vec<TracePoint>> {
    align_with_progress(points, backend, options, |_| {})
}

/// Like [`align`], calling `on_progress` after each chunk.
pub fn align_with_progress(
    points: &[TracePoint],
    backend: &dyn Backend,
    options: &AlignOptions,
    mut on_progress: impl FnMut(Progress),
) -> Result<Vec<TracePoint>> {
    let AlignOptions {
        chunk_size,
        overlap,
    } = *options;
    if chunk_size == 0 {
        return Err(Error::InvalidOption("chunk_size must be positive".into()));
    }
    if overlap >= chunk_size {
        return Err(Error::InvalidOption(
            "overlap must be less than chunk_size".into(),
        ));
    }
    let n = points.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    let step = chunk_size - overlap;
    let total_chunks = if n > chunk_size {
        (n - overlap).div_ceil(step)
    } else {
        1
    };

    let mut out = Vec::with_capacity(n);
    let mut chunk_number = 0;
    let mut start = 0_usize;
    loop {
        let end = start.saturating_add(chunk_size).min(n);
        let chunk = &points[start..end];
        let matched = backend.match_points(chunk)?;
        if matched.len() != chunk.len() {
            return Err(Error::BackendLengthMismatch {
                returned: matched.len(),
                expected: chunk.len(),
            });
        }
        // After the first chunk, the leading points duplicate the previous tail.
        let skip = if start == 0 { 0 } else { overlap };
        out.extend(matched.into_iter().skip(skip));
        chunk_number += 1;
        on_progress(Progress {
            done_points: out.len(),
            total_points: n,
            chunk: chunk_number,
            total_chunks,
        });
        if end == n {
            return Ok(out);
        }
        start += step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeBackend<F> {
        way_fn: F,
        calls: Mutex<Vec<usize>>,
    }

    impl<F> FakeBackend<F> {
        fn new(way_fn: F) -> Self {
            Self {
                way_fn,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<usize> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl<F: Fn(&TracePoint) -> u64 + Send + Sync> Backend for FakeBackend<F> {
        fn match_points(&self, points: &[TracePoint]) -> Result<Vec<TracePoint>> {
            self.calls.lock().unwrap().push(points.len());
            Ok(points
                .iter()
                .map(|p| {
                    let mut p = p.clone();
                    p.way_id = Some((self.way_fn)(&p));
                    p
                })
                .collect())
        }
    }

    fn trace(n: u32) -> Vec<TracePoint> {
        (0..n).map(|i| TracePoint::new(f64::from(i), 0.0)).collect()
    }

    fn options(chunk_size: usize, overlap: usize) -> AlignOptions {
        AlignOptions {
            chunk_size,
            overlap,
        }
    }

    fn way_ids(points: &[TracePoint]) -> Vec<u64> {
        points.iter().map(|p| p.way_id.unwrap()).collect()
    }

    #[test]
    fn no_chunking_needed() {
        let backend = FakeBackend::new(|p: &TracePoint| p.lat as u64 / 3);
        let out = align(&trace(5), &backend, &options(100, 5)).unwrap();
        assert_eq!(way_ids(&out), [0, 0, 0, 1, 1]);
        assert_eq!(backend.calls(), [5]);
    }

    #[test]
    fn chunks_with_overlap() {
        let backend = FakeBackend::new(|p: &TracePoint| p.lat as u64);
        let out = align(&trace(25), &backend, &options(10, 3)).unwrap();
        assert_eq!(way_ids(&out), (0..25).collect::<Vec<_>>());
        // Chunks: [0..10), [7..17), [14..24), [21..25)
        assert_eq!(backend.calls(), [10, 10, 10, 4]);
    }

    #[test]
    fn empty() {
        let backend = FakeBackend::new(|_: &TracePoint| 1);
        assert!(
            align(&[], &backend, &AlignOptions::default())
                .unwrap()
                .is_empty()
        );
        assert!(backend.calls().is_empty());
    }

    #[test]
    fn invalid_options_are_rejected() {
        let backend = FakeBackend::new(|_: &TracePoint| 1);
        for bad in [options(0, 0), options(5, 5), options(5, 6)] {
            let err = align(&trace(1), &backend, &bad).unwrap_err();
            assert!(matches!(err, Error::InvalidOption(_)), "{err}");
        }
    }

    #[test]
    fn progress_counts_reach_the_total() {
        let backend = FakeBackend::new(|_: &TracePoint| 1);
        let mut seen = Vec::new();
        align_with_progress(&trace(25), &backend, &options(10, 3), |p| seen.push(p)).unwrap();
        let summary: Vec<_> = seen
            .iter()
            .map(|p| (p.done_points, p.chunk, p.total_chunks))
            .collect();
        assert_eq!(summary, [(10, 1, 4), (17, 2, 4), (24, 3, 4), (25, 4, 4)]);
    }

    #[test]
    fn length_mismatch_is_an_error() {
        struct Dropper;
        impl Backend for Dropper {
            fn match_points(&self, _: &[TracePoint]) -> Result<Vec<TracePoint>> {
                Ok(Vec::new())
            }
        }
        let err = align(&trace(3), &Dropper, &AlignOptions::default()).unwrap_err();
        assert!(matches!(
            err,
            Error::BackendLengthMismatch {
                returned: 0,
                expected: 3
            }
        ));
    }
}
