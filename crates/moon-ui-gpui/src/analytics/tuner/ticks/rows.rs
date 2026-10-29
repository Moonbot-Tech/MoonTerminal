//! The deal table's row order — a permutation over the loaded rows, filtered by the "fit only"
//! switch and chip, cached against the data generation, sort, filters, tail floor and source reach so an
//! unrelated repaint reuses it instead of sorting hundreds of deals per frame.

use super::columns::*;
use super::state::{DealRow, TapeStatus, TicksState};
use super::why::{DealChip, DealWhy};

/// Classification reused while rows, the floor and source availability remain unchanged.
pub(in crate::analytics::tuner) struct Classified {
    rows_rev: u64,
    min_tail_s: u32,
    refusal_revision: u64,
    at_ms: i64,
    valid_until_ms: i64,
    chips: Vec<DealChip>,
}

/// The next retention edge that can change a covered short tail's sample bucket.
pub(in crate::analytics::tuner) fn next_source_change(
    row: &DealRow,
    min_tail_s: u32,
    now_ms: i64,
) -> Option<i64> {
    let address = row.address.as_ref()?;
    let (_, trail) = row.held?;
    if row.tape != TapeStatus::Covered || trail >= i64::from(min_tail_s) * 1_000 {
        return None;
    }
    retention_edge(
        address,
        [row.deal.close_ms.saturating_add(trail.max(0))],
        now_ms,
    )
}

/// The first instant past `now_ms` at which the clock alone can move one row's verdict: a
/// covered short tail's source edge ([`next_source_change`]), or for a missing row any retention
/// edge its window, close or held trail crosses — where "cannot be fetched" starts to be said
/// or a short tail stops being fillable and is admitted. The card's reload validity and the
/// list's readiness deadlines both read this one rule.
pub(in crate::analytics::tuner) fn next_verdict_flip(
    row: &DealRow,
    min_tail_s: u32,
    now_ms: i64,
) -> Option<i64> {
    if row.tape != TapeStatus::Missing {
        return next_source_change(row, min_tail_s, now_ms);
    }
    window_retention_edge(row, now_ms)
}

/// Missing tape and pending archives share the same window's source-retention boundaries.
fn window_retention_edge(row: &DealRow, now_ms: i64) -> Option<i64> {
    let address = row.address.as_ref()?;
    let deal = &row.deal;
    let window = moon_core::db::tuner::ticks::model_window(
        deal,
        moon_core::market::trade_replay::margin_ms(),
        moon_core::market::trade_replay::long_position_ms(),
    );
    let hull = window.as_ref().and_then(|w| w.focus_spans().hull());
    let close = window.as_ref().map_or(deal.close_ms, |w| w.close_ms);
    let trail = row
        .held
        .map(|(_, trail)| close.saturating_add(trail.max(0)));
    retention_edge(
        address,
        hull.into_iter()
            .flat_map(|(from, to)| [from, to])
            .chain([deal.close_ms, close])
            .chain(trail),
        now_ms,
    )
}

/// Whole-scope figures used by the KPI, search controls and summary on unrelated repaints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::analytics::tuner) struct Population {
    pub fit: usize,
    pub covered: usize,
    pub replayable: usize,
    pub readiness: super::readiness::Readiness,
}

/// Population validity includes time and station availability, independently of row sorting.
pub(in crate::analytics::tuner) struct PopulationCache {
    rows_rev: u64,
    data: Option<usize>,
    min_tail_s: u32,
    refusal_revision: u64,
    station: bool,
    margin_ms: i64,
    long_position_ms: i64,
    at_ms: i64,
    valid_until_ms: i64,
    value: Population,
}

/// Read cached figures until a row, policy, refusal or source-retention edge changes them.
pub(in crate::analytics::tuner) fn population_at(
    state: &TicksState,
    min_tail_s: u32,
    now_ms: i64,
) -> Population {
    let admission = super::tail::Admission::at(min_tail_s, now_ms);
    let data = state.data.data();
    let identity = data.map(|data| std::sync::Arc::as_ptr(data) as usize);
    let station = crate::backend::station::known_cached();
    let margin_ms = moon_core::market::trade_replay::margin_ms();
    let long_position_ms = moon_core::market::trade_replay::long_position_ms();
    let mut cache = state.population.borrow_mut();
    if let Some(cache) = cache.as_ref().filter(|c| {
        c.rows_rev == state.rows_rev
            && c.data == identity
            && c.min_tail_s == admission.min_s
            && c.refusal_revision == admission.refusal_revision
            && c.station == station
            && c.margin_ms == margin_ms
            && c.long_position_ms == long_position_ms
            && now_ms >= c.at_ms
            && now_ms < c.valid_until_ms
    }) {
        return cache.value;
    }
    let mut value = Population::default();
    let mut valid_until_ms = i64::MAX;
    if let Some(data) = data {
        for row in &data.rows {
            let fit = row.fit_with(&admission);
            value.fit += usize::from(fit);
            value.replayable += usize::from(fit && row.ticks.is_some());
            let edge = if matches!(
                row.tape,
                TapeStatus::Refused(
                    moon_core::market::trade_replay::TickStatus::ArchivePending { .. }
                )
            ) {
                window_retention_edge(row, now_ms)
            } else {
                next_verdict_flip(row, admission.min_s, now_ms)
            };
            if let Some(edge) = edge {
                valid_until_ms = valid_until_ms.min(edge);
            }
        }
        value.covered = data.covered();
        value.readiness = data.readiness(now_ms, admission.min_s);
    }
    *cache = Some(PopulationCache {
        rows_rev: state.rows_rev,
        data: identity,
        min_tail_s: admission.min_s,
        refusal_revision: admission.refusal_revision,
        station,
        margin_ms,
        long_position_ms,
        at_ms: now_ms,
        valid_until_ms,
        value,
    });
    value
}

/// The earliest instant past `now_ms` at which any of `anchors` falls out of one of the
/// address's sources: the core archive, the venue's REST route (less its safety margin) or its
/// daily archive.
fn retention_edge(
    address: &super::state::RowAddress,
    anchors: impl IntoIterator<Item = i64>,
    now_ms: i64,
) -> Option<i64> {
    use moon_core::market::trade_replay::venue_caps::{
        RETENTION_SAFETY_MS, archive_route, trade_route,
    };
    let retentions: Vec<i64> = [
        Some(moon_core::market::CORE_ARCHIVE_RETENTION_MS),
        address
            .venue
            .and_then(trade_route)
            .and_then(|route| route.retention_ms())
            .map(|retention| retention.saturating_sub(RETENTION_SAFETY_MS)),
        address
            .venue
            .and_then(|venue| archive_route(venue, &address.market))
            .map(|_| i64::MAX),
    ]
    .into_iter()
    .flatten()
    .collect();
    anchors
        .into_iter()
        .flat_map(|anchor| {
            retentions
                .iter()
                .map(move |&retention| anchor.saturating_add(retention).saturating_add(1))
        })
        .filter(|&edge| edge > now_ms)
        .min()
}

/// Reuse classification until a refusal, retention edge, row or floor changes.
fn classify(state: &mut TicksState, min_tail_s: u32, now_ms: i64) {
    let admission = super::tail::Admission::at(min_tail_s, now_ms);
    let refusal_revision = admission.refusal_revision;
    if state.classified.as_ref().is_some_and(|c| {
        c.rows_rev == state.rows_rev
            && c.min_tail_s == min_tail_s
            && c.refusal_revision == refusal_revision
            && now_ms >= c.at_ms
            && now_ms < c.valid_until_ms
    }) {
        return;
    }
    let chips = state
        .data
        .data()
        .map(|d| {
            d.rows
                .iter()
                .map(|r| DealWhy::of_with(r, &admission).chip())
                .collect()
        })
        .unwrap_or_default();
    let valid_until_ms = state
        .data
        .data()
        .and_then(|data| {
            data.rows
                .iter()
                .filter_map(|row| next_source_change(row, min_tail_s, now_ms))
                .min()
        })
        .unwrap_or(i64::MAX);
    state.order = None;
    state.chip_counts = None;
    state.classified = Some(Classified {
        rows_rev: state.rows_rev,
        min_tail_s,
        refusal_revision: admission.refusal_revision,
        at_ms: now_ms,
        valid_until_ms,
        chips,
    });
}

/// The sorted order and what it was built for.
pub(in crate::analytics::tuner) struct OrderCache {
    pub(in crate::analytics::tuner) rows_rev: u64,
    pub(in crate::analytics::tuner) sort: Option<(String, bool)>,
    pub(in crate::analytics::tuner) only_fit: bool,
    pub(in crate::analytics::tuner) chip: Option<DealChip>,
    pub(in crate::analytics::tuner) min_tail_s: u32,
    /// Indices into `TicksData::rows` — the shown rows only, when the switch hides the rest.
    pub(in crate::analytics::tuner) order: Vec<usize>,
}

/// Rebuild after classification changes, sorting or either filter changes.
pub(in crate::analytics::tuner) fn order_for(
    state: &mut TicksState,
    min_tail_s: u32,
    now_ms: i64,
) -> &[usize] {
    classify(state, min_tail_s, now_ms);
    let fresh = state.order.as_ref().is_some_and(|c| {
        c.rows_rev == state.rows_rev
            && c.sort == state.sort
            && c.only_fit == state.only_fit
            && c.chip == state.chip
            && c.min_tail_s == min_tail_s
    });
    if !fresh {
        let empty = super::snapshots::SnapshotRows::default();
        let rows = state.data.data().map(|d| &d.rows).unwrap_or(&empty);
        let mut order: Vec<usize> = (0..rows.len())
            .filter(|&i| {
                let reason = state.classified.as_ref().expect("classified rows").chips[i];
                (!state.only_fit || matches!(reason, DealChip::Fit | DealChip::AcceptedShortTail))
                    && state.chip.is_none_or(|chip| {
                        if chip == DealChip::WithTape {
                            DealChip::tape(&rows[i]).is_some()
                        } else {
                            reason == chip
                        }
                    })
            })
            .collect();
        if let Some((key, desc)) = &state.sort {
            sort_indices(
                rows,
                &state.plan,
                &mut order,
                key,
                *desc,
                &state.classified.as_ref().expect("classified rows").chips,
            );
        }
        state.order = Some(OrderCache {
            rows_rev: state.rows_rev,
            sort: state.sort.clone(),
            only_fit: state.only_fit,
            chip: state.chip,
            min_tail_s,
            order,
        });
    }
    state
        .order
        .as_ref()
        .map(|c| c.order.as_slice())
        .unwrap_or(&[])
}

/// A fixed-decimal signed figure whose sign is the rounded value's.
///
/// Rounding happens before the sign, so a value that rounds to zero is unsigned and
/// [`moon_core::util::fmt::DeltaSign::Zero`] — `{:+.2}` would print `-0.00` and a colour picked
/// from the raw value would disagree with the text.
///
/// Args:
///     value: Raw signed figure.
///     decimals: Places to round and show.
///
/// Returns:
///     The text and the sign that text represents. Non-finite input is an em dash at zero.
pub(in crate::analytics::tuner) fn paint_fixed(
    value: f64,
    decimals: usize,
) -> (String, moon_core::util::fmt::DeltaSign) {
    moon_core::util::fmt::signed_fixed(value, decimals)
        .unwrap_or_else(|| ("—".to_string(), moon_core::util::fmt::DeltaSign::Zero))
}

/// Result of a deal in per cent of what was spent, as the report has it.
pub(in crate::analytics::tuner) fn result_pct(row: &DealRow) -> f64 {
    let d = &row.deal;
    if d.buy_price <= 0.0 {
        return 0.0;
    }
    let raw = (d.sell_price - d.buy_price) / d.buy_price * 100.0;
    if d.is_short { -raw } else { raw }
}

/// Rank of a tape status for sorting: covered first, then fetchable, then the rest.
fn tape_rank(tape: TapeStatus) -> u8 {
    match tape {
        TapeStatus::Covered => 0,
        TapeStatus::Fetching => 1,
        TapeStatus::Missing => 2,
        TapeStatus::Refused(_) => 3,
        TapeStatus::NoAddress => 4,
    }
}

/// Rank of a verdict for sorting: both hits first, unanswered last.
fn model_rank(row: &DealRow) -> u8 {
    match row.verdict {
        None => 6,
        Some(v) => {
            let score = |x: Option<bool>| match x {
                Some(true) => 0,
                Some(false) => 2,
                None => 1,
            };
            score(v.entry) + score(v.exit)
        }
    }
}

/// Stable sorts preserve the loaded chronological order within equal values or reason buckets.
fn sort_indices(
    rows: &super::snapshots::SnapshotRows<DealRow>,
    plan: &std::collections::HashMap<i64, (f64, f64)>,
    order: &mut [usize],
    key: &str,
    desc: bool,
    chips: &[DealChip],
) {
    let by_f64 = |f: &dyn Fn(&DealRow) -> f64, order: &mut [usize]| {
        order.sort_by(|&a, &b| {
            let (x, y) = (f(&rows[a]), f(&rows[b]));
            let c = x.total_cmp(&y);
            if desc { c.reverse() } else { c }
        });
    };
    match key {
        COL_COIN => order.sort_by(|&a, &b| {
            let c = rows[a].deal.coin.cmp(&rows[b].deal.coin);
            if desc { c.reverse() } else { c }
        }),
        COL_KIND => order.sort_by(|&a, &b| {
            let c = rows[a].deal.kind.cmp(&rows[b].deal.kind);
            if desc { c.reverse() } else { c }
        }),
        COL_CORE => order.sort_by(|&a, &b| {
            let c = rows[a].deal.core_name.cmp(&rows[b].deal.core_name);
            if desc { c.reverse() } else { c }
        }),
        COL_RESULT => by_f64(&result_pct, order),
        // Unpriced sorts as the smallest.
        COL_PROFIT => by_f64(&|r| r.deal.profit.unwrap_or(f64::MIN), order),
        // By the number the cell shows — per cent in percent mode, money otherwise; a deal В1
        // makes no trade of sorts as the smallest, like an unpriced one.
        COL_PLAN => {
            let pct = crate::analytics::pnl_is_pct();
            by_f64(
                &|r| {
                    plan.get(&r.deal.report_uid)
                        .map_or(
                            f64::MIN,
                            |(money, percent)| if pct { *percent } else { *money },
                        )
                },
                order,
            )
        }
        COL_DURATION => by_f64(&|r| (r.deal.close_ms - r.deal.buy_ms) as f64, order),
        // By each deal's held exit trail; nothing held is the shortest.
        COL_HELD => by_f64(
            &|r| r.held.map(|(_, trail)| trail as f64).unwrap_or(-1.0),
            order,
        ),
        COL_REASON => order.sort_by(|&a, &b| {
            let c = rows[a].deal.sell_reason.cmp(&rows[b].deal.sell_reason);
            if desc { c.reverse() } else { c }
        }),
        COL_TAPE => by_f64(&|r| f64::from(tape_rank(r.tape)), order),
        COL_MODEL => by_f64(&|r| f64::from(model_rank(r)), order),
        COL_WHY => order.sort_by(|&a, &b| {
            let cmp = chips[a].rank().cmp(&chips[b].rank());
            if desc { cmp.reverse() } else { cmp }
        }),
        // `COL_TIME` and anything unknown: by the entry time.
        _ => by_f64(&|r| r.deal.buy_ms as f64, order),
    }
}

/// Cached counts for the entire population; the tape bucket deliberately overlaps the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics::tuner) struct ChipCounts {
    pub(in crate::analytics::tuner) rows_rev: u64,
    pub(in crate::analytics::tuner) min_tail_s: u32,
    pub(in crate::analytics::tuner) counts: [usize; DealChip::ALL.len()],
}

impl ChipCounts {
    /// Read one bucket in the same order the footer uses.
    pub(in crate::analytics::tuner) fn get(self, chip: DealChip) -> usize {
        self.counts[chip as usize]
    }
}

/// Recount after classification changes, never on an unrelated repaint.
pub(in crate::analytics::tuner) fn counts_at(
    state: &mut TicksState,
    min_tail_s: u32,
    now_ms: i64,
) -> ChipCounts {
    classify(state, min_tail_s, now_ms);
    if let Some(counts) = state
        .chip_counts
        .filter(|c| c.rows_rev == state.rows_rev && c.min_tail_s == min_tail_s)
    {
        return counts;
    }
    let mut counts = ChipCounts {
        rows_rev: state.rows_rev,
        min_tail_s,
        counts: [0; DealChip::ALL.len()],
    };
    if let Some(data) = state.data.data() {
        for (row, chip) in data
            .rows
            .iter()
            .zip(&state.classified.as_ref().expect("classified rows").chips)
        {
            counts.counts[*chip as usize] += 1;
            if DealChip::tape(row).is_some() {
                counts.counts[DealChip::WithTape as usize] += 1;
            }
        }
    }
    state.chip_counts = Some(counts);
    counts
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
