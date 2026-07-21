use super::*;

#[test]
fn evicted_cross_ranges_reports_overwritten_ring_slots() {
    assert_eq!(cross_append_ranges(3, 4, 5), [(3, 2), (0, 2)]);
    assert_eq!(evicted_cross_ranges(0, 3, 5, 3), [(0, 1), (0, 0)]);
    assert_eq!(evicted_cross_ranges(2, 5, 5, 2), [(2, 2), (0, 0)]);
    assert!(ranges_have_entries(&evicted_cross_ranges(2, 5, 5, 2)));
    assert!(!ranges_have_entries(&evicted_cross_ranges(0, 2, 5, 2)));
}

#[test]
fn evicted_cross_ranges_handles_wrapped_full_ring() {
    assert_eq!(evicted_cross_ranges(4, 5, 5, 3), [(4, 1), (0, 2)]);
}
