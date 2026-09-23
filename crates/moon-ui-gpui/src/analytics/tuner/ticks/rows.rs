//! The deal table's row order — a permutation over the loaded rows, filtered by the "fit only"
//! switch and cached against the data generation, the sort and the switch, so a repaint
//! that changed none of them reuses it instead of sorting hundreds of deals per frame.

use super::columns::*;
use super::state::{DealRow, TapeStatus, TicksState};

/// The sorted order and what it was built for.
pub(in crate::analytics::tuner) struct OrderCache {
    pub(in crate::analytics::tuner) rows_rev: u64,
    pub(in crate::analytics::tuner) sort: Option<(String, bool)>,
    pub(in crate::analytics::tuner) only_fit: bool,
    /// Indices into `TicksData::rows` — the shown rows only, when the switch hides the rest.
    pub(in crate::analytics::tuner) order: Vec<usize>,
}

/// The current order, rebuilt only when the rows, the sort or the "fit only" switch changed.
pub(in crate::analytics::tuner) fn order_for(state: &mut TicksState) -> &[usize] {
    let fresh = state.order.as_ref().is_some_and(|c| {
        c.rows_rev == state.rows_rev && c.sort == state.sort && c.only_fit == state.only_fit
    });
    if !fresh {
        let rows: &[DealRow] = state.data.data().map(|d| d.rows.as_slice()).unwrap_or(&[]);
        let mut order: Vec<usize> = (0..rows.len())
            .filter(|&i| !state.only_fit || rows[i].fit())
            .collect();
        if let Some((key, desc)) = &state.sort {
            sort_indices(rows, &state.plan[0], &mut order, key, *desc);
        }
        state.order = Some(OrderCache {
            rows_rev: state.rows_rev,
            sort: state.sort.clone(),
            only_fit: state.only_fit,
            order,
        });
    }
    state
        .order
        .as_ref()
        .map(|c| c.order.as_slice())
        .unwrap_or(&[])
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

fn sort_indices(
    rows: &[DealRow],
    plan: &std::collections::HashMap<i64, (f64, f64)>,
    order: &mut [usize],
    key: &str,
    desc: bool,
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
        // By the trail — the half the exit horizon is taken from; nothing held is the shortest.
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
        // `COL_TIME` and anything unknown: by the entry time.
        _ => by_f64(&|r| r.deal.buy_ms as f64, order),
    }
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
