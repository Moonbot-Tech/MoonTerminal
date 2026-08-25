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
