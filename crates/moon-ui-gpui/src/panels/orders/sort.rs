//! Row execution predicates, the sort comparator, and the table change-signature.

use super::*;
use std::cmp::Ordering;

/// Return whether the entry has executed, using the authoritative worker status rather than a
/// leg's `fill_pct`.
///
/// The phase machine is shared by both directions: `None` and `BuySet` mean the entry is pending;
/// `BuyDone` and every represented `Sell*` phase mean the position has been entered. Using the
/// entry leg's `fill_pct` previously read an empty `sell_order` for shorts and left them displayed
/// as `Short-S` indefinitely.
pub(crate) fn executed(r: &OrderRow) -> bool {
    matches!(
        r.status.as_str(),
        "BuyDone" | "SellSet" | "SellDone" | "SellFail" | "SellCancel" | "SellAlmostDone"
    )
}
/// Return whether either a long or short entry has executed, for `SellFirst` sorting.
pub(super) fn is_sell(r: &OrderRow) -> bool {
    executed(r)
}
/// Return whether this is a pending long entry, displayed as `BUY`.
pub(super) fn is_buy(r: &OrderRow) -> bool {
    !r.is_short && !executed(r)
}

/// Return the displayed-side grouping key, accounting for execution; zero sorts first.
fn primary_key(p: PrimarySort, r: &OrderRow) -> u8 {
    match p {
        // `ProfitFirst` is handled separately in `sort_entries`, so its group is neutral here.
        PrimarySort::Creation | PrimarySort::ProfitFirst => 0,
        PrimarySort::SellFirst => u8::from(!is_sell(r)),
        PrimarySort::BuyFirst => u8::from(!is_buy(r)),
    }
}

/// Apply the base primary sort plus the newest/oldest UID tie-breaker.
///
/// [`OrdersPanel::rebuild_cache`] subsequently applies the stable Main lift over this order.
pub(super) fn sort_entries(entries: &mut [OrderEntry], view: &OrdersViewState) {
    if let Some(col) = view.table_sort {
        sort_by_table_column(entries, col, view.table_sort_desc, view.newest_first);
        return;
    }

    // Profit-first sorting uses descending locally calculated PnL. Rows without a position sort
    // last, with UID as the newest/oldest tie-breaker just like the other modes.
    if view.primary == PrimarySort::ProfitFirst {
        entries.sort_by(|a, b| {
            let pa = table::order_pnl(&a.row);
            let pb = table::order_pnl(&b.row);
            // Treat `None` as negative infinity so it sorts at the bottom.
            let key = |v: Option<f64>| v.unwrap_or(f64::NEG_INFINITY);
            key(pb)
                .partial_cmp(&key(pa))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let c = a.row.uid.cmp(&b.row.uid);
                    if view.newest_first { c.reverse() } else { c }
                })
        });
        return;
    }
    entries.sort_by(|a, b| {
        let ka = primary_key(view.primary, &a.row);
        let kb = primary_key(view.primary, &b.row);
        ka.cmp(&kb).then_with(|| {
            let c = a.row.uid.cmp(&b.row.uid);
            if view.newest_first { c.reverse() } else { c }
        })
    });
}

fn sort_by_table_column(
    entries: &mut [OrderEntry],
    col: OrdCol,
    desc: bool,
    newest_first: bool,
) {
    entries.sort_by(|a, b| {
        let ord = compare_col(col, a, b)
            .then_with(|| a.core_name.cmp(&b.core_name))
            .then_with(|| a.row.coin.cmp(&b.row.coin))
            .then_with(|| {
                let c = a.row.uid.cmp(&b.row.uid);
                if newest_first { c.reverse() } else { c }
            });
        if desc { ord.reverse() } else { ord }
    });
}

fn compare_col(col: OrdCol, a: &OrderEntry, b: &OrderEntry) -> Ordering {
    match col {
        OrdCol::Core => a.core_name.cmp(&b.core_name),
        OrdCol::Side => side_label(&a.row).cmp(side_label(&b.row)),
        OrdCol::Token => a
            .row
            .coin
            .cmp(&b.row.coin)
            .then_with(|| a.row.market_display.cmp(&b.row.market_display)),
        OrdCol::Size => cmp_f64(a.row.size, b.row.size),
        OrdCol::Buy => cmp_f64(a.row.buy_price, b.row.buy_price),
        OrdCol::CurP => cmp_f64(a.row.price as f64, b.row.price as f64),
        OrdCol::TpPrice => cmp_f64(a.row.sell_price, b.row.sell_price),
        OrdCol::Fill => cmp_f64(a.row.fill_pct as f64, b.row.fill_pct as f64),
        OrdCol::Pnl => cmp_opt(table::order_pnl(&a.row), table::order_pnl(&b.row)),
        OrdCol::PnlPct => cmp_opt(table::order_pnl_pct(&a.row), table::order_pnl_pct(&b.row)),
        OrdCol::PnlTp => cmp_opt(table::order_pnl_at_tp(&a.row), table::order_pnl_at_tp(&b.row)),
        OrdCol::Strat => a.row.strat.cmp(&b.row.strat),
        OrdCol::StratName => a.row.strat_name.cmp(&b.row.strat_name),
        OrdCol::Sl | OrdCol::Ts | OrdCol::Vstop => Ordering::Equal,
    }
}

fn cmp_f64(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

fn cmp_opt(a: Option<f64>, b: Option<f64>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => cmp_f64(a, b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn side_label(row: &OrderRow) -> &'static str {
    if row.is_short {
        if executed(row) { "Short-B" } else { "Short-S" }
    } else if executed(row) {
        "SELL"
    } else {
        "BUY"
    }
}

/// Return the effective scope's order-table signature from each core's table revision.
///
/// This deliberately uses the table revision rather than chart-line revisions so numeric fields and
/// statuses refresh independently of chart userdata. Core IDs keep membership changes observable
/// even when two cores currently share the same revision.
///
/// Args:
///     b: Backend snapshot containing the per-core order stores.
///     scope: Effective query scope whose changes may affect this panel.
///
/// Returns:
///     Deterministic signature independent of out-of-scope core activity.
pub(super) fn orders_sig(b: &Backend, scope: &EffectiveCoreScope) -> u64 {
    let store = b.session.store();
    scope.ids().iter().fold(0u64, |signature, core| {
        signature
            .wrapping_mul(31)
            .wrapping_add(*core)
            .wrapping_mul(31)
            .wrapping_add(store.core(*core).map_or(0, |data| data.orders_table_rev))
    })
}
