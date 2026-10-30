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
/// [`apply_main_lift`] then stably lifts Main-associated rows unless `view.header_sort` is set.
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
                OrdCol::Pnl => optional_f64_order(a.pnl, b.pnl, ascending),
                OrdCol::PnlPct => optional_f64_order(a.pnl_pct, b.pnl_pct, ascending),
                OrdCol::PnlTp => optional_f64_order(a.pnl_tp, b.pnl_tp, ascending),
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
            let pa = a.pnl;
            let pb = b.pnl;
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

/// Reuse the previous permutation when row membership and the active sort keys are unchanged.
/// Return the new rows' signature for the next revision without hashing the old rows again.
pub(super) fn sort_or_reuse_entries(
    entries: &mut Vec<OrderEntry>,
    previous: &[OrderEntry],
    previous_signature: Option<u64>,
    view: &OrdersViewState,
    stop_overlay: &std::collections::HashMap<(CoreId, u64, u8), (bool, std::time::Instant)>,
) -> u64 {
    let signature = sort_signature(entries, view, stop_overlay);
    if !previous.is_empty() && previous_signature == Some(signature) {
        let expected = entries.len();
        let mut by_id: HashMap<_, _> = entries
            .drain(..)
            .map(|entry| ((entry.core, entry.row.uid), entry))
            .collect();
        let mut reordered = Vec::with_capacity(expected);
        for old in previous {
            if let Some(entry) = by_id.remove(&(old.core, old.row.uid)) {
                reordered.push(entry);
            }
        }
        if reordered.len() == expected && by_id.is_empty() {
            *entries = reordered;
            return signature;
        }
        reordered.extend(by_id.into_values());
        *entries = reordered;
    }
    sort_entries(entries, view, stop_overlay);
    signature
}

/// Small FNV-style hash for scalar sort keys, avoiding two SipHash instances per row.
struct RowHasher(u64);

impl RowHasher {
    /// Start with the standard FNV-1a offset basis.
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl std::hash::Hasher for RowHasher {
    /// Fold bytes without allocating or retaining row data.
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    /// Fold one machine word in one step.
    fn write_u64(&mut self, value: u64) {
        self.0 ^= value;
        self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
    }

    /// Fold one smaller integer in one step.
    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }

    /// Fold one byte in one step.
    fn write_u8(&mut self, value: u8) {
        self.write_u64(u64::from(value));
    }

    /// Return the mixed scalar signature.
    fn finish(&self) -> u64 {
        self.0
    }
}

/// Hash row membership and only the values consulted by the active comparator.
///
/// Per-row hashes are summed so a harmless input permutation does not force another sort.
fn sort_signature(
    entries: &[OrderEntry],
    view: &OrdersViewState,
    stop_overlay: &std::collections::HashMap<(CoreId, u64, u8), (bool, std::time::Instant)>,
) -> u64 {
    use std::hash::{Hash, Hasher};

    fn optional_hash(value: Option<f64>, h: &mut impl Hasher) {
        value
            .filter(|value| value.is_finite())
            .map(f64::to_bits)
            .hash(h);
    }

    let mut view_hash = RowHasher::new();
    view.header_sort
        .map(|(column, ascending)| (column.key(), ascending))
        .hash(&mut view_hash);
    view.primary.to_u8().hash(&mut view_hash);
    view.newest_first.hash(&mut view_hash);
    let mut rows = 0u64;
    let mut squares = 0u64;
    let mut count = 0u64;
    for entry in entries {
        let mut h = RowHasher::new();
        entry.core.hash(&mut h);
        entry.row.uid.hash(&mut h);
        if let Some((column, _)) = view.header_sort {
            match column {
                OrdCol::Core => entry.core_name.hash(&mut h),
                OrdCol::Side => side_sort_key(&entry.row).hash(&mut h),
                OrdCol::Token => entry.row.coin.hash(&mut h),
                OrdCol::Size => entry.row.size.to_bits().hash(&mut h),
                OrdCol::Buy => entry.row.buy_price.to_bits().hash(&mut h),
                OrdCol::CurP => entry.row.price.to_bits().hash(&mut h),
                OrdCol::TpPrice => optional_hash(
                    (entry.row.sell_price > 0.0).then_some(entry.row.sell_price),
                    &mut h,
                ),
                OrdCol::Fill => entry.row.fill_pct.to_bits().hash(&mut h),
                OrdCol::Pnl => optional_hash(entry.pnl, &mut h),
                OrdCol::PnlPct => optional_hash(entry.pnl_pct, &mut h),
                OrdCol::PnlTp => optional_hash(entry.pnl_tp, &mut h),
                OrdCol::Sl | OrdCol::Ts | OrdCol::Vstop => {
                    let kind = match column {
                        OrdCol::Sl => moon_core::feed::OrderStopKind::StopLoss,
                        OrdCol::Ts => moon_core::feed::OrderStopKind::Trailing,
                        OrdCol::Vstop => moon_core::feed::OrderStopKind::VStop,
                        _ => unreachable!(),
                    };
                    table::effective_stop_value(
                        entry,
                        kind,
                        stop_overlay
                            .get(&(entry.core, entry.row.uid, table::stop_tag(kind)))
                            .map(|(value, _)| *value),
                    )
                    .hash(&mut h);
                }
                OrdCol::Strat => entry.row.strat.hash(&mut h),
                OrdCol::StratName => (entry.row.strat_id != 0 && !entry.row.strat_name.is_empty())
                    .then_some(&entry.row.strat_name)
                    .hash(&mut h),
            }
        } else if view.primary == PrimarySort::ProfitFirst {
            entry.pnl.map(f64::to_bits).hash(&mut h);
        } else {
            primary_key(view.primary, &entry.row).hash(&mut h);
        }
        let hash = h.finish();
        rows = rows.wrapping_add(hash);
        squares = squares.wrapping_add(hash.wrapping_mul(hash));
        count += 1;
    }
    view_hash.finish() ^ rows.rotate_left(17) ^ squares.rotate_left(41) ^ count.wrapping_mul(31)
}

/// Stably lift Main-associated rows over the comparator order unless an explicit header sort
/// is active.
///
/// `view.header_sort` is the persisted header-click override (`table_persist` per `ctx_id`).
/// When it is `Some`, the user asked for that column order and the passive Main-on-top default
/// does not run. Highlighting is computed separately and is unchanged by this function.
///
/// Args:
///     entries: Rows already ordered by [`sort_entries`].
///     view: Current table view, including `header_sort` and `main_on_top`.
///     highlight: `(core, uid)` of the first base-sorted row for each Main-open pair.
///     main_open: `(core, market)` pairs open in the group's Main stack.
pub(super) fn apply_main_lift(
    entries: &mut [OrderEntry],
    view: &OrdersViewState,
    highlight: &std::collections::HashSet<(CoreId, u64)>,
    main_open: &std::collections::HashSet<(CoreId, String)>,
) {
    if view.header_sort.is_some() {
        return;
    }
    match view.main_on_top {
        MainOnTop::Off => {}
        MainOnTop::Highlighted => {
            entries.sort_by_key(|e| u8::from(!highlight.contains(&(e.core, e.row.uid))));
        }
        MainOnTop::AllTicker => {
            let markets: std::collections::HashSet<&str> =
                main_open.iter().map(|(_, m)| m.as_str()).collect();
            entries.sort_by_key(|e| u8::from(!markets.contains(e.row.market.as_str())));
        }
    }
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

/// Return data and name signatures for the effective scope without owning session names.
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
///     The original data signature and a separate name signature, both scoped to the same IDs.
pub(super) fn orders_signatures(b: &Backend, scope: &EffectiveCoreScope) -> (u64, u64) {
    let store = b.session.store();
    orders_signatures_from(scope.ids().iter().map(|core| {
        let name = b
            .session
            .sessions()
            .iter()
            .find(|session| session.id == *core)
            .map(|session| session.name.as_str());
        (
            *core,
            store.core(*core).map_or(0, |data| data.orders_table_rev),
            name,
        )
    }))
}

/// Fold explicit scoped revisions and borrowed names in one pass, preserving the data formula.
pub(super) fn orders_signatures_from<'a>(
    cores: impl Iterator<Item = (CoreId, u64, Option<&'a str>)>,
) -> (u64, u64) {
    use std::hash::{Hash, Hasher};
    let mut names = std::collections::hash_map::DefaultHasher::new();
    let data = cores.fold(0u64, |signature, (core, revision, name)| {
        core.hash(&mut names);
        name.hash(&mut names);
        signature
            .wrapping_mul(31)
            .wrapping_add(core)
            .wrapping_mul(31)
            .wrapping_add(revision)
    });
    (data, names.finish())
}

#[cfg(test)]
mod tests;
