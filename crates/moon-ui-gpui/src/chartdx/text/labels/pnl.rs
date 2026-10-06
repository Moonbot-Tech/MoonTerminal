use super::*;

/// Collect the open-position figures for every basis in ONE pass over a market's orders.
///
/// The arithmetic is [`moon_core::feed::order_math`]'s, not this module's: the Orders table, the Assets panel
/// and the chart's own overlay all state this number, and a second formula here would be a fourth
/// answer to the same question. In particular the entry price is the one the feed RESOLVED — the
/// raw `buy_price` is a break-even including round-trip commission — and every price flows through
/// a NaN-rejecting gate.
///
/// Args:
///     rows: The core's order rows, unfiltered.
///     market: Market key the pane is showing.
///
/// Returns:
///     Figures indexed by [`basis_index`], and the strategy name of the newest open order.
pub(in crate::chartdx) fn collect_open_stats(
    rows: &[moon_core::feed::OrderRow],
    market: &str,
) -> ([BasisStats; 3], String) {
    let mut out = [BasisStats::default(); 3];
    // Newest wins: uid increases with creation, so the caption names the strategy that acted last.
    let mut newest: Option<(u64, &str)> = None;
    for row in rows.iter().filter(|r| r.market == market && !r.job_is_done) {
        if !row.strat_name.is_empty() && newest.is_none_or(|(uid, _)| row.uid > uid) {
            newest = Some((row.uid, row.strat_name.as_str()));
        }
        // Every live row counts as an OPEN ORDER, whether or not it holds a position yet: that
        // figure is about orders, and a working entry is one.
        for basis in PnlBasis::ALL {
            if basis.accepts(row.emulator) {
                out[basis_index(basis)].open_orders += 1;
            }
        }
        // Exposure asks less than the PnL does: a size and a mark, no entry price. Each figure
        // degrades on its own inputs rather than dragging the others down with it — the rule the
        // chart overlay these captions replaced was careful about.
        let mark = f64::from(row.price);
        if let Some(qty) = position_qty(row).filter(|_| mark.is_finite() && mark > 0.0) {
            for basis in PnlBasis::ALL {
                if basis.accepts(row.emulator) {
                    let s = &mut out[basis_index(basis)];
                    s.exposure += qty * mark;
                    s.has_exposure = true;
                }
            }
        }
        let (Some(qty), Some(pnl)) = (position_qty(row), order_pnl(row)) else {
            continue;
        };
        let spent = row.buy_price * qty;
        for basis in PnlBasis::ALL {
            if !basis.accepts(row.emulator) {
                continue;
            }
            let s = &mut out[basis_index(basis)];
            s.pos_size += if row.is_short { -qty } else { qty };
            s.spent += spent;
            s.pnl_quote += pnl;
            s.has_position = true;
        }
    }
    let strategy = newest.map(|(_, name)| name.to_string()).unwrap_or_default();
    (out, strategy)
}
