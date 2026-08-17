//! Row execution predicates, the sort comparator, and the table change-signature.

use super::*;

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

/// Compare optional displayed values while keeping unavailable cells last under either direction.
fn optional_order<T: Ord>(
    left: Option<T>,
    right: Option<T>,
    ascending: bool,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (left, right) {
        (Some(left), Some(right)) => {
            let order = left.cmp(&right);
            if ascending { order } else { order.reverse() }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Compare optional floating-point display values with the same missing-value rule.
fn optional_f64_order(
    left: Option<f64>,
    right: Option<f64>,
    ascending: bool,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (
        left.filter(|value| value.is_finite()),
        right.filter(|value| value.is_finite()),
    ) {
        (Some(left), Some(right)) => {
            let order = left.total_cmp(&right);
            if ascending { order } else { order.reverse() }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Apply the selected header comparator or the legacy primary/newest menu order.
///
/// [`OrdersPanel::rebuild_cache`] subsequently applies the stable Main lift over this order.
pub(super) fn sort_entries(
    entries: &mut [OrderEntry],
    view: &OrdersViewState,
    stop_overlay: &std::collections::HashMap<(CoreId, u64, u8), (bool, std::time::Instant)>,
) {
    if let Some((column, ascending)) = view.header_sort {
        entries.sort_by(|a, b| {
            let primary = match column {
                OrdCol::Core => optional_order(
                    Some(a.core_name.as_str()),
                    Some(b.core_name.as_str()),
                    ascending,
                ),
                OrdCol::Side => optional_order(
                    Some(side_sort_key(&a.row)),
                    Some(side_sort_key(&b.row)),
                    ascending,
                ),
                OrdCol::Token => optional_order(
                    Some(a.row.coin.as_str()),
                    Some(b.row.coin.as_str()),
                    ascending,
                ),
                OrdCol::Size => numeric_order(a.row.size, b.row.size, ascending),
                OrdCol::Buy => numeric_order(a.row.buy_price, b.row.buy_price, ascending),
                OrdCol::CurP => numeric_order(a.row.price as f64, b.row.price as f64, ascending),
                OrdCol::TpPrice => optional_f64_order(
                    (a.row.sell_price > 0.0).then_some(a.row.sell_price),
                    (b.row.sell_price > 0.0).then_some(b.row.sell_price),
                    ascending,
                ),
                OrdCol::Fill => numeric_order(
                    f64::from(a.row.fill_pct),
                    f64::from(b.row.fill_pct),
                    ascending,
                ),
                OrdCol::Pnl => optional_f64_order(
                    crate::order_math::order_pnl(&a.row),
                    crate::order_math::order_pnl(&b.row),
                    ascending,
                ),
                OrdCol::PnlPct => optional_f64_order(
                    crate::order_math::order_pnl_pct(&a.row),
                    crate::order_math::order_pnl_pct(&b.row),
                    ascending,
                ),
                OrdCol::PnlTp => optional_f64_order(
                    crate::order_math::order_pnl_at_tp(&a.row),
                    crate::order_math::order_pnl_at_tp(&b.row),
                    ascending,
                ),
                OrdCol::Sl | OrdCol::Ts | OrdCol::Vstop => {
                    let kind = match column {
                        OrdCol::Sl => moon_core::feed::OrderStopKind::StopLoss,
                        OrdCol::Ts => moon_core::feed::OrderStopKind::Trailing,
                        OrdCol::Vstop => moon_core::feed::OrderStopKind::VStop,
                        _ => unreachable!(),
                    };
                    let value = |entry: &OrderEntry| {
                        table::effective_stop_value(
                            entry,
                            kind,
                            stop_overlay
                                .get(&(entry.core, entry.row.uid, table::stop_tag(kind)))
                                .map(|(value, _)| *value),
                        )
                    };
                    optional_order(Some(value(a)), Some(value(b)), ascending)
                }
                OrdCol::Strat => optional_order(
                    Some(a.row.strat.as_str()),
                    Some(b.row.strat.as_str()),
                    ascending,
                ),
                OrdCol::StratName => optional_order(
                    (a.row.strat_id != 0 && !a.row.strat_name.is_empty())
                        .then_some(a.row.strat_name.as_str()),
                    (b.row.strat_id != 0 && !b.row.strat_name.is_empty())
                        .then_some(b.row.strat_name.as_str()),
                    ascending,
                ),
            };
            primary.then_with(|| a.core.cmp(&b.core).then_with(|| a.row.uid.cmp(&b.row.uid)))
        });
        return;
    }

    // Profit-first sorting uses descending locally calculated PnL. Rows without a position sort
    // last, with UID as the newest/oldest tie-breaker just like the other modes.
    if view.primary == PrimarySort::ProfitFirst {
        entries.sort_by(|a, b| {
            let pa = crate::order_math::order_pnl(&a.row);
            let pb = crate::order_math::order_pnl(&b.row);
            // Treat `None` as negative infinity so it sorts at the bottom.
            let key = |v: Option<f64>| v.unwrap_or(f64::NEG_INFINITY);
            key(pb)
                .partial_cmp(&key(pa))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let c = a.row.uid.cmp(&b.row.uid);
                    if view.newest_first { c.reverse() } else { c }
                })
                .then_with(|| a.core.cmp(&b.core))
        });
        return;
    }
    entries.sort_by(|a, b| {
        let ka = primary_key(view.primary, &a.row);
        let kb = primary_key(view.primary, &b.row);
        ka.cmp(&kb)
            .then_with(|| {
                let c = a.row.uid.cmp(&b.row.uid);
                if view.newest_first { c.reverse() } else { c }
            })
            .then_with(|| a.core.cmp(&b.core))
    });
}

/// Return the allocation-free lexicographic rank of the exact displayed side label.
fn side_sort_key(row: &OrderRow) -> u8 {
    let base = match (row.is_short, executed(row)) {
        (false, false) => 0, // BUY
        (false, true) => 2,  // SELL
        (true, true) => 4,   // Short-B
        (true, false) => 6,  // Short-S
    };
    base + u8::from(row.emulator)
}

/// Compare two finite displayed numbers in the requested direction.
fn numeric_order(left: f64, right: f64, ascending: bool) -> std::cmp::Ordering {
    let order = left.total_cmp(&right);
    if ascending { order } else { order.reverse() }
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

#[cfg(test)]
mod tests;
