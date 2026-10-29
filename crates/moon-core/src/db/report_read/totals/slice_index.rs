//! Per-core time-range lookup for all overlapping totals slices.

use rusqlite::types::Value;

use super::{ReportAxis, TotalsSlice, compare};

/// One inclusive range, shifted to the same core clock as the SQL predicate.
struct Interval {
    from: Option<i64>,
    to: Option<i64>,
    index: usize,
}

/// Nonoverlapping lanes let one binary search find each range containing a row.
///
/// A period total and its days occupy two lanes; arbitrary overlaps remain supported.
/// Core membership and clock shifts are resolved once per core, rather than per row.
pub(super) struct SliceIndex {
    lanes: Vec<Vec<Interval>>,
    all: Vec<usize>,
    real_exact: bool,
    bounded: bool,
}

impl SliceIndex {
    /// Compile admitted slices into disjoint lanes without changing their output indices.
    pub(super) fn new(slices: &[TotalsSlice], core: i64, offset: i32) -> Self {
        let mut intervals = slices
            .iter()
            .enumerate()
            .filter_map(|(index, slice)| {
                if slice.core_uids.as_ref().is_some_and(|cores| {
                    !cores.is_empty() && !cores.iter().any(|uid| *uid as i64 == core)
                }) {
                    return None;
                }
                Some(Interval {
                    from: slice
                        .date_from
                        .map(|bound| ReportAxis::shift_bound(bound, offset)),
                    to: slice
                        .date_to
                        .map(|bound| ReportAxis::shift_bound(bound, offset)),
                    index,
                })
            })
            .collect::<Vec<_>>();
        let all = intervals.iter().map(|interval| interval.index).collect();
        let real_exact = intervals.iter().all(|interval| {
            [interval.from, interval.to]
                .into_iter()
                .flatten()
                .all(|bound| (bound as f64) as i64 == bound && bound.unsigned_abs() < 1 << 53)
        });
        let bounded = intervals
            .iter()
            .any(|interval| interval.from.is_some() || interval.to.is_some());
        intervals.sort_by_key(|interval| interval.from);
        let mut lanes: Vec<Vec<Interval>> = Vec::new();
        for interval in intervals {
            let lane = lanes.iter().position(|lane| {
                lane.last().is_some_and(|last| {
                    last.to
                        .zip(interval.from)
                        .is_some_and(|(to, from)| to < from)
                })
            });
            if let Some(lane) = lane {
                lanes[lane].push(interval);
            } else {
                lanes.push(vec![interval]);
            }
        }
        Self {
            lanes,
            all,
            real_exact,
            bounded,
        }
    }

    /// Return exactly the slices the former per-row admission loop accepted.
    ///
    /// A missing close column ignores time bounds, just as each slice's SQL does.
    /// Unrepresentable SQLite comparisons keep the existing per-slice read semantics.
    pub(super) fn matching(&self, close: Option<&Value>, output: &mut Vec<usize>) -> Option<()> {
        output.clear();
        let Some(close) = close else {
            output.extend_from_slice(&self.all);
            return Some(());
        };
        if self.all.is_empty() {
            return Some(());
        }
        if self.bounded {
            match close {
                Value::Integer(_) => {}
                Value::Real(value) if self.real_exact && !value.is_nan() => {}
                _ => return None,
            }
        }
        for lane in &self.lanes {
            let end = lane.partition_point(|interval| {
                interval
                    .from
                    .is_none_or(|from| compare(close, from).is_some_and(|order| order.is_ge()))
            });
            if let Some(interval) =
                end.checked_sub(1)
                    .map(|index| &lane[index])
                    .filter(|interval| {
                        interval
                            .to
                            .is_none_or(|to| compare(close, to).is_some_and(|order| order.is_le()))
                    })
            {
                output.push(interval.index);
            }
        }
        Some(())
    }
}

#[cfg(test)]
mod tests;
