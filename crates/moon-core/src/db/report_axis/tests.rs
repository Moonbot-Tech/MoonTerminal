use std::collections::HashMap;

use super::*;

/// `ReportAxis::identity_core_local` -- adding a default offset or display-zone conversion would
/// shift a replicated timestamp away from the wall clock MoonBot itself reports.
#[test]
fn identity_axis_keeps_core_local_seconds_and_uses_utc() {
    let axis = ReportAxis::identity_core_local();
    let stored = 1_700_000_123;

    assert_eq!(axis.to_utc(stored, 41), stored);
    assert_eq!(axis.from_utc(stored, 41), stored);
    assert_eq!(axis.zone(), chrono_tz::UTC);
}

/// `ReportAxis::offset_secs` -- coalescing an unmeasured core with a measured UTC core would make
/// the diagnosis surface claim that an unknown server is known to run on UTC.
#[test]
fn offset_lookup_distinguishes_unmeasured_from_measured_utc() {
    let axis = ReportAxis::from_measured(
        HashMap::from([(
            7,
            vec![OffsetSegment {
                from_utc: 0,
                offset_secs: 0,
            }],
        )]),
        chrono_tz::UTC,
    );

    assert_eq!(axis.offset_secs(6, 10), None);
    assert_eq!(axis.offset_secs(7, 10), Some(0));
}

/// `ReportAxis::is_utc_identity` -- treating one retained non-zero segment as identity lets the
/// Summary stream sort that core on raw local time, while treating an empty, measured-zero, or
/// invalid-only axis as shifted pays for a scalar sort whose key is identical to `closedate`.
#[test]
fn utc_identity_requires_every_retained_segment_to_be_zero() {
    let empty = ReportAxis::identity_core_local();
    let measured_zero = ReportAxis::from_measured(
        HashMap::from([(
            1,
            vec![OffsetSegment {
                from_utc: 10,
                offset_secs: 0,
            }],
        )]),
        chrono_tz::Europe::Warsaw,
    );
    let historical_mixed = ReportAxis::from_measured(
        HashMap::from([(
            1,
            vec![
                OffsetSegment {
                    from_utc: 10,
                    offset_secs: 0,
                },
                OffsetSegment {
                    from_utc: 20,
                    offset_secs: 3_600,
                },
            ],
        )]),
        chrono_tz::UTC,
    );
    let invalid_only = ReportAxis::from_measured(
        HashMap::from([(
            3,
            vec![OffsetSegment {
                from_utc: 30,
                offset_secs: MAX_OFFSET_SECS + 1,
            }],
        )]),
        chrono_tz::UTC,
    );

    assert!(empty.is_utc_identity());
    assert!(measured_zero.is_utc_identity());
    assert!(!historical_mixed.is_utc_identity());
    assert!(invalid_only.is_utc_identity());
}

/// `ReportAxis::offset_secs` -- letting a pre-observation timestamp fall through instead of using
/// the earliest segment loses the correction for older rows and shifts their report times.
#[test]
fn segment_selection_uses_earliest_exact_boundary_and_latest_segments() {
    let axis = ReportAxis::from_measured(
        HashMap::from([(
            9,
            vec![
                OffsetSegment {
                    from_utc: 100,
                    offset_secs: 3_600,
                },
                OffsetSegment {
                    from_utc: 200,
                    offset_secs: 7_200,
                },
            ],
        )]),
        chrono_tz::UTC,
    );

    assert_eq!(axis.offset_secs(9, 99), Some(3_600));
    assert_eq!(axis.offset_secs(9, 100), Some(3_600));
    assert_eq!(axis.offset_secs(9, 200), Some(7_200));
    assert_eq!(axis.offset_secs(9, 201), Some(7_200));
}

/// `ReportAxis::from_measured` -- retaining an impossible clock offset lets a broken measurement
/// move report history by more than any real time zone instead of safely ignoring it.
#[test]
fn measured_segments_are_sorted_and_invalid_offsets_are_dropped() {
    let axis = ReportAxis::from_measured(
        HashMap::from([(
            3,
            vec![
                OffsetSegment {
                    from_utc: 300,
                    offset_secs: 3_600,
                },
                OffsetSegment {
                    from_utc: 400,
                    offset_secs: MAX_OFFSET_SECS + 1,
                },
                OffsetSegment {
                    from_utc: 100,
                    offset_secs: -3_600,
                },
            ],
        )]),
        chrono_tz::UTC,
    );

    assert_eq!(axis.offset_secs(3, 50), Some(-3_600));
    assert_eq!(axis.offset_secs(3, 300), Some(3_600));
    assert_eq!(axis.offset_secs(3, 400), Some(3_600));
}

/// `ReportAxis::to_utc` and `ReportAxis::from_utc` -- using opposite signs in either conversion
/// makes date windows miss the report rows that belong to the selected core-local period.
#[test]
fn measured_offset_conversions_round_trip() {
    let axis = ReportAxis::from_measured(
        HashMap::from([(
            12,
            vec![OffsetSegment {
                from_utc: 0,
                offset_secs: 19_800,
            }],
        )]),
        chrono_tz::UTC,
    );
    let true_utc = 1_700_010_000;

    let core_local = axis.from_utc(true_utc, 12);
    assert_eq!(core_local, true_utc + 19_800);
    assert_eq!(axis.to_utc(core_local, 12), true_utc);
}

/// `ReportAxis::groups` -- grouping by input position or treating unknown offsets as a separate
/// state makes per-core window predicates diverge and can exclude rows from a shared report.
#[test]
fn groups_share_offsets_use_zero_for_unknown_and_preserve_input_order() {
    let axis = ReportAxis::from_measured(
        HashMap::from([
            (
                1,
                vec![OffsetSegment {
                    from_utc: 0,
                    offset_secs: 3_600,
                }],
            ),
            (
                2,
                vec![OffsetSegment {
                    from_utc: 0,
                    offset_secs: 3_600,
                }],
            ),
            (
                3,
                vec![OffsetSegment {
                    from_utc: 0,
                    offset_secs: -18_000,
                }],
            ),
        ]),
        chrono_tz::UTC,
    );

    assert_eq!(
        axis.groups(&[3, 1, 4, 2], 1_700_000_000),
        vec![(-18_000, vec![3]), (3_600, vec![1, 2]), (0, vec![4])]
    );
}

/// `ReportAxis::measured_groups` -- returning groups in the `HashMap`'s own iteration order
/// instead of sorted by offset makes the SQL an unbounded read builds reshuffle branch order
/// between two runs holding the exact same measurements, which is indistinguishable from a real
/// data change to anything diffing that SQL, and unsorted per-group core lists would do the same
/// to each branch's own `core_uid IN (...)` list.
#[test]
fn measured_groups_orders_by_offset_then_by_core_within_each_group() {
    let axis = ReportAxis::from_measured(
        HashMap::from([
            (
                30,
                vec![OffsetSegment {
                    from_utc: 0,
                    offset_secs: -18_000,
                }],
            ),
            (
                10,
                vec![OffsetSegment {
                    from_utc: 0,
                    offset_secs: 3_600,
                }],
            ),
            (
                20,
                vec![OffsetSegment {
                    from_utc: 0,
                    offset_secs: 3_600,
                }],
            ),
        ]),
        chrono_tz::UTC,
    );

    assert_eq!(
        axis.measured_groups(1_700_000_000),
        vec![(-18_000, vec![30]), (3_600, vec![10, 20])],
        "groups must be ordered by offset ascending, and each group's cores ascending by uid, \
         regardless of the HashMap's own iteration order"
    );
}

/// `ReportAxis::measured_cores` -- an unbounded read's catch-all branch excludes exactly this
/// list, so an unsorted or incomplete result either lets a measured core fall through to the
/// identity catch-all (silently un-correcting its rows) or excludes a core that was never
/// measured at all.
#[test]
fn measured_cores_lists_every_measured_core_uid_ascending() {
    let axis = ReportAxis::from_measured(
        HashMap::from([
            (
                40,
                vec![OffsetSegment {
                    from_utc: 0,
                    offset_secs: 3_600,
                }],
            ),
            (
                5,
                vec![OffsetSegment {
                    from_utc: 0,
                    offset_secs: -3_600,
                }],
            ),
        ]),
        chrono_tz::UTC,
    );

    assert_eq!(axis.measured_cores(), vec![5, 40]);
}

/// `ReportAxis::shift_bound` -- adding the offset in the wrong direction (subtracting instead of
/// adding) makes a WINDOW BOUND move opposite to how a core's own local clock reads relative to
/// UTC, so a period boundary is compared against the wrong core-local instant: an ahead-of-UTC
/// core's rows near the edge would be wrongly excluded, and a behind-of-UTC core's wrongly
/// included, or vice versa depending on the boundary side.
///
/// Independent oracle: the expected core-local value is the plain arithmetic sum, spelled out
/// with a literal rather than by calling `shift_bound` a second time.
#[test]
fn shift_bound_adds_the_offset_to_move_a_true_utc_bound_into_core_local_terms() {
    let true_utc = 1_700_010_000i64;

    assert_eq!(ReportAxis::shift_bound(true_utc, 3_600), 1_700_013_600);
    assert_eq!(ReportAxis::shift_bound(true_utc, -3_600), 1_700_006_400);
}
