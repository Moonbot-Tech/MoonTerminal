//! Pins the exact predicate text and bound values `append_row_scope` emits for every row scope.

use std::collections::{HashMap, HashSet};

use super::*;
use crate::db::{OffsetSegment, ReportAxis};

/// A core whose clock runs four hours behind true UTC.
const BEHIND: u64 = 11;
/// A core whose clock runs three hours ahead of true UTC.
const AHEAD: u64 = 22;
/// Period start, well in the past.
const FROM: i64 = 400_000_000;
/// Period end that has already passed on every clock.
const PAST_TO: i64 = 500_000_000;
/// Period end that still lies ahead on every clock.
const FUTURE_TO: i64 = 4_000_000_000;

/// Columns of a replica that records close dates.
fn with_closedate() -> HashSet<String> {
    HashSet::from(["closedate".to_string()])
}

/// Two cores on two constant offsets, so a read over both builds two offset groups.
fn two_clocks() -> ReportAxis {
    ReportAxis::from_measured(
        HashMap::from([
            (
                BEHIND,
                vec![OffsetSegment {
                    from_utc: 0,
                    offset_secs: -14_400,
                }],
            ),
            (
                AHEAD,
                vec![OffsetSegment {
                    from_utc: 0,
                    offset_secs: 10_800,
                }],
            ),
        ]),
        chrono_tz::UTC,
    )
}

/// One core whose offset changed inside the period, so its history has two segments.
fn changed_clock() -> ReportAxis {
    ReportAxis::from_measured(
        HashMap::from([(
            BEHIND,
            vec![
                OffsetSegment {
                    from_utc: 0,
                    offset_secs: 0,
                },
                OffsetSegment {
                    from_utc: 450_000_000,
                    offset_secs: 3_600,
                },
            ],
        )]),
        chrono_tz::UTC,
    )
}

/// Build a filter from the fields these cases vary.
fn filter(
    rows: RowScope,
    cores: &[u64],
    axis: ReportAxis,
    date_from: Option<i64>,
    date_to: Option<i64>,
) -> ReportFilter {
    ReportFilter {
        core_uids: cores.to_vec(),
        date_from,
        date_to,
        rows,
        axis,
        ..ReportFilter::default()
    }
}

/// Run `append_row_scope` on empty buffers and render the predicate and its bound values.
fn shape(f: &ReportFilter, cols: &HashSet<String>) -> String {
    let mut sql = String::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    append_row_scope(&mut sql, &mut params, f, cols);
    let bound = params
        .iter()
        .map(|p| format!("{:?}", p.to_sql().expect("bind a scope value")))
        .collect::<Vec<_>>();
    format!("{sql} | [{}]", bound.join(", "))
}

/// Every branch of the builder, each case named for the path it takes.
fn cases() -> Vec<(&'static str, ReportFilter, HashSet<String>)> {
    let none = HashSet::new();
    let open_basis = ReportFilter {
        period_basis: PeriodBasis::OpenDate,
        ..filter(
            RowScope::ClosedAndOpen,
            &[BEHIND],
            ReportAxis::default(),
            Some(FROM),
            Some(PAST_TO),
        )
    };
    vec![
        ("open-basis", open_basis, with_closedate()),
        (
            "open",
            filter(RowScope::Open, &[], ReportAxis::default(), None, None),
            with_closedate(),
        ),
        (
            "open-no-column",
            filter(RowScope::Open, &[], ReportAxis::default(), None, None),
            none.clone(),
        ),
        (
            "closed-unbounded",
            filter(RowScope::Closed, &[], ReportAxis::default(), None, None),
            with_closedate(),
        ),
        (
            "closed-no-column",
            filter(
                RowScope::Closed,
                &[BEHIND],
                two_clocks(),
                Some(FROM),
                Some(PAST_TO),
            ),
            none.clone(),
        ),
        (
            "closed-and-open-no-column",
            filter(
                RowScope::ClosedAndOpen,
                &[BEHIND, AHEAD],
                two_clocks(),
                Some(FROM),
                Some(FUTURE_TO),
            ),
            none.clone(),
        ),
        (
            "closed-and-open-two-groups-bounded",
            filter(
                RowScope::ClosedAndOpen,
                &[BEHIND, AHEAD],
                two_clocks(),
                Some(FROM),
                Some(FUTURE_TO),
            ),
            with_closedate(),
        ),
        (
            "closed-two-groups-bounded",
            filter(
                RowScope::Closed,
                &[BEHIND, AHEAD],
                two_clocks(),
                Some(FROM),
                Some(PAST_TO),
            ),
            with_closedate(),
        ),
        (
            "closed-two-groups-from-only",
            filter(
                RowScope::Closed,
                &[BEHIND, AHEAD],
                two_clocks(),
                Some(FROM),
                None,
            ),
            with_closedate(),
        ),
        (
            "closed-one-group-bounded",
            filter(
                RowScope::Closed,
                &[BEHIND],
                two_clocks(),
                Some(FROM),
                Some(PAST_TO),
            ),
            with_closedate(),
        ),
        (
            "open-if-current-two-groups",
            filter(
                RowScope::OpenIfCurrent,
                &[BEHIND, AHEAD],
                two_clocks(),
                Some(FROM),
                Some(FUTURE_TO),
            ),
            with_closedate(),
        ),
        (
            "open-if-current-ended",
            filter(
                RowScope::OpenIfCurrent,
                &[BEHIND, AHEAD],
                two_clocks(),
                Some(FROM),
                Some(PAST_TO),
            ),
            with_closedate(),
        ),
        (
            "open-if-current-one-group",
            filter(
                RowScope::OpenIfCurrent,
                &[BEHIND],
                two_clocks(),
                None,
                Some(FUTURE_TO),
            ),
            with_closedate(),
        ),
        (
            "open-if-current-no-column",
            filter(
                RowScope::OpenIfCurrent,
                &[BEHIND],
                two_clocks(),
                None,
                Some(FUTURE_TO),
            ),
            none.clone(),
        ),
        (
            "closed-and-open-if-current-ended",
            filter(
                RowScope::ClosedAndOpenIfCurrent,
                &[BEHIND],
                two_clocks(),
                Some(FROM),
                Some(PAST_TO),
            ),
            with_closedate(),
        ),
        (
            "closed-and-open-if-current-unscoped",
            filter(
                RowScope::ClosedAndOpenIfCurrent,
                &[],
                two_clocks(),
                Some(FROM),
                Some(FUTURE_TO),
            ),
            with_closedate(),
        ),
        (
            "closed-and-open-changed-clock",
            filter(
                RowScope::ClosedAndOpen,
                &[BEHIND],
                changed_clock(),
                Some(FROM),
                Some(FUTURE_TO),
            ),
            with_closedate(),
        ),
    ]
}

/// Exact text and bound values `append_row_scope` emitted for each case before it was split
/// into its steps. Any change to a branch's shape, its bound order or a value reds its case.
const PINNED: [(&str, &str); 17] = [
    ("open-basis", r#" AND 1=0 | []"#),
    (
        "open",
        r#" AND (NOT (typeof(r."closedate") IN ('integer','real') AND r."closedate" > 0)) | []"#,
    ),
    ("open-no-column", r#" AND 1=0 | []"#),
    (
        "closed-unbounded",
        r#" AND (typeof(r."closedate") IN ('integer','real') AND r."closedate" > 0) | []"#,
    ),
    (
        "closed-no-column",
        r#" AND r.core_uid IN (11) AND 1=0 | []"#,
    ),
    ("closed-and-open-no-column", r#" | []"#),
    (
        "closed-and-open-two-groups-bounded",
        r#" AND ((r."closedate" >= ? AND r."closedate" <= ? AND ((r.core_uid IN (11) AND (typeof(+r."closedate") IN ('integer','real') AND +r."closedate" > 0) AND +r."closedate" >= ? AND +r."closedate" <= ?) OR (r.core_uid IN (22) AND (typeof(+r."closedate") IN ('integer','real') AND +r."closedate" > 0) AND +r."closedate" >= ? AND +r."closedate" <= ?))) OR ((r.core_uid IN (11) AND (NOT (typeof(r."closedate") IN ('integer','real') AND r."closedate" > 0))) OR (r.core_uid IN (22) AND (NOT (typeof(r."closedate") IN ('integer','real') AND r."closedate" > 0))))) | [Owned(Integer(399985600)), Owned(Integer(4000010800)), Owned(Integer(399985600)), Owned(Integer(3999985600)), Owned(Integer(400010800)), Owned(Integer(4000010800))]"#,
    ),
    (
        "closed-two-groups-bounded",
        r#" AND (r."closedate" >= ? AND r."closedate" <= ? AND ((r.core_uid IN (11) AND (typeof(+r."closedate") IN ('integer','real') AND +r."closedate" > 0) AND +r."closedate" >= ? AND +r."closedate" <= ?) OR (r.core_uid IN (22) AND (typeof(+r."closedate") IN ('integer','real') AND +r."closedate" > 0) AND +r."closedate" >= ? AND +r."closedate" <= ?))) | [Owned(Integer(399985600)), Owned(Integer(500010800)), Owned(Integer(399985600)), Owned(Integer(499985600)), Owned(Integer(400010800)), Owned(Integer(500010800))]"#,
    ),
    (
        "closed-two-groups-from-only",
        r#" AND ((r.core_uid IN (11) AND (typeof(r."closedate") IN ('integer','real') AND +r."closedate" > 0) AND r."closedate" >= ?) OR (r.core_uid IN (22) AND (typeof(r."closedate") IN ('integer','real') AND +r."closedate" > 0) AND r."closedate" >= ?)) | [Owned(Integer(399985600)), Owned(Integer(400010800))]"#,
    ),
    (
        "closed-one-group-bounded",
        r#" AND r.core_uid IN (11) AND (typeof(r."closedate") IN ('integer','real') AND r."closedate" > 0) AND r."closedate" >= ? AND r."closedate" <= ? | [Owned(Integer(399985600)), Owned(Integer(499985600))]"#,
    ),
    (
        "open-if-current-two-groups",
        r#" AND (NOT (typeof(r."closedate") IN ('integer','real') AND r."closedate" > 0)) AND ((r.core_uid IN (11) AND 1=1) OR (r.core_uid IN (22) AND 1=1)) | []"#,
    ),
    ("open-if-current-ended", r#" AND 1=0 | []"#),
    (
        "open-if-current-one-group",
        r#" AND (NOT (typeof(r."closedate") IN ('integer','real') AND r."closedate" > 0)) AND (r.core_uid IN (11) AND 1=1) | []"#,
    ),
    (
        "open-if-current-no-column",
        r#" AND 1=0 AND (r.core_uid IN (11) AND 1=1) | []"#,
    ),
    (
        "closed-and-open-if-current-ended",
        r#" AND r.core_uid IN (11) AND (typeof(r."closedate") IN ('integer','real') AND r."closedate" > 0) AND r."closedate" >= ? AND r."closedate" <= ? | [Owned(Integer(399985600)), Owned(Integer(499985600))]"#,
    ),
    (
        "closed-and-open-if-current-unscoped",
        r#" AND ((r."closedate" >= ? AND r."closedate" <= ? AND ((r.core_uid IN (11) AND (typeof(+r."closedate") IN ('integer','real') AND +r."closedate" > 0) AND +r."closedate" >= ? AND +r."closedate" <= ?) OR (r.core_uid IN (22) AND (typeof(+r."closedate") IN ('integer','real') AND +r."closedate" > 0) AND +r."closedate" >= ? AND +r."closedate" <= ?) OR (r.core_uid NOT IN (11,22) AND (typeof(+r."closedate") IN ('integer','real') AND +r."closedate" > 0) AND +r."closedate" >= ? AND +r."closedate" <= ?))) OR ((r.core_uid IN (11) AND (NOT (typeof(r."closedate") IN ('integer','real') AND r."closedate" > 0))) OR (r.core_uid IN (22) AND (NOT (typeof(r."closedate") IN ('integer','real') AND r."closedate" > 0))) OR (r.core_uid NOT IN (11,22) AND (NOT (typeof(r."closedate") IN ('integer','real') AND r."closedate" > 0))))) | [Owned(Integer(399985600)), Owned(Integer(4000010800)), Owned(Integer(399985600)), Owned(Integer(3999985600)), Owned(Integer(400010800)), Owned(Integer(4000010800)), Owned(Integer(400000000)), Owned(Integer(4000000000))]"#,
    ),
    (
        "closed-and-open-changed-clock",
        r#" AND ((r."closedate" >= ? AND r."closedate" <= ? AND ((r.core_uid IN (11) AND (typeof(+r."closedate") IN ('integer','real') AND +r."closedate" > 0) AND +r."closedate" >= ? AND +r."closedate" <= ? AND +r."closedate" < 450000000) OR (r.core_uid IN (11) AND (typeof(+r."closedate") IN ('integer','real') AND +r."closedate" > 0) AND +r."closedate" >= ? AND +r."closedate" <= ? AND +r."closedate" >= 450000000))) OR ((r.core_uid IN (11) AND (NOT (typeof(r."closedate") IN ('integer','real') AND r."closedate" > 0))))) | [Owned(Integer(400000000)), Owned(Integer(4000003600)), Owned(Integer(400000000)), Owned(Integer(4000000000)), Owned(Integer(400003600)), Owned(Integer(4000003600))]"#,
    ),
];

/// Every row scope, column set and offset layout keeps the predicate it emitted before.
#[test]
fn row_scope_shapes_stay_pinned() {
    let cases = cases();
    assert_eq!(cases.len(), PINNED.len(), "one pinned shape per case");
    for ((name, f, cols), (pinned_name, pinned)) in cases.into_iter().zip(PINNED) {
        assert_eq!(
            name, pinned_name,
            "cases and pins are listed in the same order"
        );
        assert_eq!(shape(&f, &cols), pinned, "row scope shape of case {name}");
    }
}
