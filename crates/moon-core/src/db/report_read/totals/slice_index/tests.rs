//! Frozen-loop parity for overlapping inclusive totals ranges.

use super::super::{ReportAxis, TotalsSlice, compare};
use super::SliceIndex;
use rusqlite::types::Value;
use std::collections::HashSet;

/// Losing a period total, a shared boundary, or a core-specific overlap changes displayed money.
#[test]
fn indexed_admission_matches_the_former_loop() {
    let mut slices = vec![TotalsSlice {
        core_uids: None,
        date_from: None,
        date_to: None,
    }];
    for day in 0..31 {
        slices.push(TotalsSlice {
            core_uids: None,
            date_from: Some(day * 86400),
            date_to: Some((day + 1) * 86400 - 1),
        });
    }
    slices.extend([
        TotalsSlice {
            core_uids: Some(vec![1, 2]),
            date_from: Some(86400),
            date_to: Some(3 * 86400),
        },
        TotalsSlice {
            core_uids: Some(vec![]),
            date_from: Some(86400),
            date_to: Some(86400),
        },
        TotalsSlice {
            core_uids: Some(vec![2]),
            date_from: None,
            date_to: Some(0),
        },
        TotalsSlice {
            core_uids: Some(vec![3]),
            date_from: Some(i64::MIN),
            date_to: Some(i64::MAX),
        },
        TotalsSlice {
            core_uids: None,
            date_from: Some(100),
            date_to: Some(99),
        },
    ]);
    let former = slices.iter().map(FormerSlicePlan::new).collect::<Vec<_>>();
    for core in [-1, 1, 2, 3] {
        for offset in [-7200, 0, 3600] {
            let index = SliceIndex::new(&slices, core, offset);
            let dates = [
                Value::Integer(-1),
                Value::Integer(0),
                Value::Integer(86400),
                Value::Integer(3 * 86400),
                Value::Real(86400.5),
                Value::Null,
                Value::Text("86400".into()),
                Value::Real(f64::NAN),
            ];
            for close in std::iter::once(None).chain(dates.iter().map(Some)) {
                let expected = former
                    .iter()
                    .enumerate()
                    .map(|(i, plan)| plan.admits(core, close, offset).map(|yes| yes.then_some(i)))
                    .collect::<Option<Vec<_>>>()
                    .map(|items| items.into_iter().flatten().collect::<Vec<_>>());
                let mut actual = Vec::new();
                let placed = index.matching(close, &mut actual);
                actual.sort_unstable();
                assert_eq!(
                    placed.map(|()| actual),
                    expected,
                    "core={core} offset={offset} close={close:?}"
                );
            }
        }
    }
}

/// One slice's admission test over the base pass's rows.
struct FormerSlicePlan {
    /// Cores admitted, `None` for every core the base admits.
    cores: Option<HashSet<i64>>,
    date_from: Option<i64>,
    date_to: Option<i64>,
}

impl FormerSlicePlan {
    fn new(slice: &TotalsSlice) -> Self {
        Self {
            // Inlined the way `build_where` inlines them, so the comparison is the SQL's own.
            cores: slice
                .core_uids
                .as_ref()
                .filter(|cores| !cores.is_empty())
                .map(|cores| cores.iter().map(|&uid| uid as i64).collect()),
            date_from: slice.date_from,
            date_to: slice.date_to,
        }
    }

    /// Whether this slice's own statement would admit a row the base pass returned.
    ///
    /// The base pass already applied everything the slice shares with it; what is left is the
    /// slice's core list and its window, shifted onto the core's clock exactly as
    /// `append_row_scope` shifts it.
    ///
    /// Args:
    ///     core: The row's `core_uid`.
    ///     close: The row's `closedate`, `None` on a source without the column (its statement
    ///         applies no window at all).
    ///     offset: The core's clock offset at the read's instant.
    ///
    /// Returns:
    ///     `None` when the close date is not a number this test can compare exactly.
    fn admits(&self, core: i64, close: Option<&Value>, offset: i32) -> Option<bool> {
        if self
            .cores
            .as_ref()
            .is_some_and(|cores| !cores.contains(&core))
        {
            return Some(false);
        }
        let Some(close) = close else {
            return Some(true);
        };
        let at_least =
            |bound: i64| compare(close, ReportAxis::shift_bound(bound, offset)).map(|o| o.is_ge());
        let at_most =
            |bound: i64| compare(close, ReportAxis::shift_bound(bound, offset)).map(|o| o.is_le());
        let from_ok = match self.date_from {
            Some(bound) => at_least(bound)?,
            None => true,
        };
        let to_ok = match self.date_to {
            Some(bound) => at_most(bound)?,
            None => true,
        };
        Some(from_ok && to_ok)
    }
}
