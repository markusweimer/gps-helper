//! Trace simplification based on OSM way ID transitions.

use crate::model::TracePoint;

/// The usual number of points to keep on each side of a road change.
pub const DEFAULT_CONTEXT: usize = 2;

/// Drops points that don't straddle a road (way ID) change.
///
/// - Always keeps the first and last points.
/// - For each index `i` where the way ID differs from the previous point's,
///   keeps `i - context .. i + context`. An unknown way ID always counts as a
///   difference, since a failed match is worth preserving context around.
/// - The result preserves order and has no duplicates.
pub fn simplify(points: &[TracePoint], context: usize) -> Vec<TracePoint> {
    let n = points.len();
    if n <= 1 {
        return points.to_vec();
    }

    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;

    for i in 1..n {
        if is_transition(&points[i - 1], &points[i]) {
            let start = i.saturating_sub(context);
            let end = i.saturating_add(context).min(n);
            keep[start..end].fill(true);
        }
    }

    points
        .iter()
        .zip(keep)
        .filter(|&(_, keep)| keep)
        .map(|(point, _)| point.clone())
        .collect()
}

/// Picks one representative point per way ID run, plus the first and last.
///
/// For each consecutive run of the same way ID, the midpoint of the run is
/// chosen. Points with no way ID each form their own run. The result suits
/// GPX route output and is much sparser than [`simplify`].
pub fn simplify_to_route(points: &[TracePoint]) -> Vec<TracePoint> {
    let n = points.len();
    if n <= 1 {
        return points.to_vec();
    }

    let mut keep = vec![0];
    let mut run_start = 0;
    for run_end in (1..=n).filter(|&i| i == n || is_transition(&points[i - 1], &points[i])) {
        let mid = (run_start + run_end) / 2;
        if mid != 0 && mid != n - 1 {
            keep.push(mid);
        }
        run_start = run_end;
    }
    keep.push(n - 1);

    keep.into_iter().map(|i| points[i].clone()).collect()
}

fn is_transition(prev: &TracePoint, cur: &TracePoint) -> bool {
    match (prev.way_id, cur.way_id) {
        (Some(prev), Some(cur)) => prev != cur,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tp(i: u32, way_id: Option<u64>) -> TracePoint {
        let mut point = TracePoint::new(f64::from(i), f64::from(i));
        point.way_id = way_id;
        point
    }

    fn run(range: std::ops::Range<u32>, way_id: u64) -> Vec<TracePoint> {
        range.map(|i| tp(i, Some(way_id))).collect()
    }

    fn ids(points: &[TracePoint]) -> Vec<u32> {
        points.iter().map(|p| p.lat as u32).collect()
    }

    #[test]
    fn empty() {
        assert!(simplify(&[], 2).is_empty());
        assert!(simplify_to_route(&[]).is_empty());
    }

    #[test]
    fn single() {
        let points = [tp(0, Some(1))];
        assert_eq!(ids(&simplify(&points, 2)), [0]);
        assert_eq!(ids(&simplify_to_route(&points)), [0]);
    }

    #[test]
    fn all_same_way_keeps_endpoints_only() {
        assert_eq!(ids(&simplify(&run(0..5, 1), 2)), [0, 4]);
    }

    #[test]
    fn transition_keeps_context() {
        let points = [run(0..4, 1), run(4..8, 2)].concat();
        assert_eq!(ids(&simplify(&points, 2)), [0, 2, 3, 4, 5, 7]);
    }

    #[test]
    fn context_zero_keeps_only_endpoints() {
        let points = [run(0..3, 1), run(3..6, 2)].concat();
        assert_eq!(ids(&simplify(&points, 0)), [0, 5]);
    }

    #[test]
    fn huge_context_does_not_overflow() {
        let points = [run(0..2, 1), run(2..4, 2)].concat();
        assert_eq!(ids(&simplify(&points, usize::MAX)), [0, 1, 2, 3]);
    }

    #[test]
    fn unknown_way_id_forces_transition() {
        let points = [
            tp(0, Some(1)),
            tp(1, Some(1)),
            tp(2, None),
            tp(3, Some(1)),
            tp(4, Some(1)),
        ];
        assert_eq!(ids(&simplify(&points, 1)), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn multiple_transitions() {
        let points = [run(0..3, 1), run(3..6, 2), run(6..9, 3)].concat();
        assert_eq!(ids(&simplify(&points, 1)), [0, 2, 3, 5, 6, 8]);
    }

    #[test]
    fn order_preserved_and_deduped() {
        let points = [run(0..2, 1), run(2..4, 2)].concat();
        assert_eq!(ids(&simplify(&points, 3)), [0, 1, 2, 3]);
    }

    #[test]
    fn route_all_same_way_keeps_first_midpoint_last() {
        assert_eq!(ids(&simplify_to_route(&run(0..10, 1))), [0, 5, 9]);
    }

    #[test]
    fn route_two_roads() {
        let points = [run(0..4, 1), run(4..8, 2)].concat();
        assert_eq!(ids(&simplify_to_route(&points)), [0, 2, 6, 7]);
    }

    #[test]
    fn route_three_roads() {
        let points = [run(0..4, 1), run(4..8, 2), run(8..12, 3)].concat();
        assert_eq!(ids(&simplify_to_route(&points)), [0, 2, 6, 10, 11]);
    }

    #[test]
    fn route_unknown_way_ids_each_form_a_run() {
        let points = [tp(0, Some(1)), tp(1, None), tp(2, None), tp(3, Some(1))];
        // Mids of the runs [0,1) [1,2) [2,3) [3,4) are 0, 1, 2, 3.
        assert_eq!(ids(&simplify_to_route(&points)), [0, 1, 2, 3]);
    }

    #[test]
    fn route_preserves_road_names() {
        let named = |i, way, name: &str| {
            let mut point = tp(i, Some(way));
            point.road_name = Some(name.to_owned());
            point
        };
        let points = [
            named(0, 1, "Main St"),
            named(1, 1, "Main St"),
            named(2, 2, "Oak Ave"),
            named(3, 2, "Oak Ave"),
        ];
        let route = simplify_to_route(&points);
        let names: Vec<_> = route
            .iter()
            .filter_map(|p| p.road_name.as_deref())
            .collect();
        assert!(names.contains(&"Main St"));
        assert!(names.contains(&"Oak Ave"));
    }
}
