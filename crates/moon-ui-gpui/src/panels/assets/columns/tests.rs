// Explicit imports, NOT `use super::*`: the parent re-exports `gpui::*`, which carries its own
// `test` and shadows the built-in attribute — `#[test]` then expands recursively.
use crate::panels::assets::collect::AssetEntry;
use crate::panels::assets::columns::{AssetCol, pnl_display};
use moon_core::feed::AssetRow;

/// One asset row carrying only what the PnL column reads.
fn entry(coin: &str, pnl: f64, live: bool) -> AssetEntry {
    AssetEntry {
        core: 1,
        core_name: "core".to_string(),
        row: AssetRow {
            market: format!("{coin}USDT"),
            coin: coin.to_string(),
            quote: "USDT".to_string(),
            listed: 3,
            qty: 0.0,
            qty_full: 0.0,
            price: 1.0,
            value_usdt: 0.0,
            min_lot_usd: 0.0,
            is_quote_asset: false,
            mark_price: 1.0,
            pos_size: 1.0,
            pos_price: 1.0,
            liq_price: 0.0,
            leverage: 1,
            pnl_usdt: pnl,
            pnl_live: live,
        },
        value: 0.0,
        display_value: 0.0,
        market_exists: true,
    }
}

/// The PnL column must not rank rows by a number it refuses to print.
///
/// `AssetRow::pnl_usdt` doubles as the server's accumulated profit whenever the feed could not
/// derive a live figure (`pnl_live == false`), and the cell shows a dash for exactly those rows.
/// The plausible edit is a comparator that reads `row.pnl_usdt` directly again: it compiles, sorts,
/// and silently orders dash rows among the real ones by an invisible value.
#[test]
fn the_pnl_sort_pushes_rows_without_a_live_figure_last() {
    // The dash row carries the most NEGATIVE number, so a comparator reading the raw field would
    // lead with it ascending and trail with it descending — the opposite of both assertions below.
    let rows = [
        entry("AAA", -5.0, true),
        entry("BBB", -900.0, false),
        entry("CCC", 3.0, true),
    ];
    assert_eq!(pnl_display(&rows[1]), None);

    let mut ascending = rows.clone();
    ascending.sort_by(|a, b| AssetCol::Pnl.compare(a, b));
    let order: Vec<&str> = ascending.iter().map(|e| e.row.coin.as_str()).collect();
    assert_eq!(order, vec!["AAA", "CCC", "BBB"]);

    let mut descending = rows.clone();
    descending.sort_by(|a, b| AssetCol::Pnl.compare(a, b).reverse());
    let order: Vec<&str> = descending.iter().map(|e| e.row.coin.as_str()).collect();
    assert_eq!(order, vec!["BBB", "CCC", "AAA"]);
}

/// A non-finite PnL is not a displayable figure either, whatever the feed's flag says.
#[test]
fn a_non_finite_pnl_has_nothing_to_display() {
    assert_eq!(pnl_display(&entry("AAA", f64::INFINITY, true)), None);
    assert_eq!(pnl_display(&entry("AAA", f64::NAN, true)), None);
    assert_eq!(pnl_display(&entry("AAA", -2.5, true)), Some(-2.5));
}

/// Ticker order ignores the case exchanges happen to report.
///
/// Wallet rows carry raw exchange casing, so a byte comparison would file every mixed-case token
/// after the uppercase ones — `kPEPE` after `ZRO` instead of between `JUP` and `LINK`.
#[test]
fn coin_order_ignores_exchange_casing() {
    let mut rows = [entry("ZRO", 0.0, true), entry("kPEPE", 0.0, true)];
    rows.sort_by(|a, b| AssetCol::Coin.compare(a, b));
    let order: Vec<&str> = rows.iter().map(|e| e.row.coin.as_str()).collect();
    assert_eq!(order, vec!["kPEPE", "ZRO"]);
}
