// Explicit imports, NOT `use super::*`: the parent re-exports `gpui::*`, which carries its own
// `test` and shadows the built-in attribute — `#[test]` then expands recursively.
use crate::panels::assets::collect::AssetEntry;
use crate::panels::assets::columns::{AssetCol, order_rows, pnl_display, restore_sort};
use moon_core::config::TableSortPreference;
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

/// The PnL column must not rank rows by a number it refuses to print, under EITHER sort arrow.
///
/// `AssetRow::pnl_usdt` doubles as the server's accumulated profit whenever the feed could not
/// derive a live figure (`pnl_live == false`), and the cell shows a dash for exactly those rows.
/// The plausible edit this guards against is applying direction by reversing the WHOLE comparison
/// (i.e. dropping `order_rows`'s pin and going back to `compare(...).reverse()`): the dash row's
/// "sorts last" rule would then reverse right along with everything else and land it FIRST under
/// the descending arrow — dash rows floating to the top of a PnL-descending table.
#[test]
fn the_pnl_sort_pushes_rows_without_a_live_figure_last() {
    // The dash row carries the most NEGATIVE number, so a comparator reading the raw field would
    // lead with it ascending and trail with it descending — never keep it pinned last both ways
    // like the assertions below require.
    let rows = [
        entry("AAA", -5.0, true),
        entry("BBB", -900.0, false),
        entry("CCC", 3.0, true),
    ];
    assert_eq!(pnl_display(&rows[1]), None);

    let mut ascending = rows.clone();
    ascending.sort_by(|a, b| order_rows(AssetCol::Pnl, true, a, b));
    let order: Vec<&str> = ascending.iter().map(|e| e.row.coin.as_str()).collect();
    assert_eq!(
        order,
        vec!["AAA", "CCC", "BBB"],
        "ascending: dash row must sort last"
    );

    let mut descending = rows.clone();
    descending.sort_by(|a, b| order_rows(AssetCol::Pnl, false, a, b));
    let order: Vec<&str> = descending.iter().map(|e| e.row.coin.as_str()).collect();
    assert_eq!(
        order,
        vec!["CCC", "AAA", "BBB"],
        "descending: dash row must STILL sort last"
    );
}

/// The PnL pin is deliberately scoped to PnL alone: a non-finite `display_value` in the Value
/// column must NOT be pinned last by [`order_rows`] the way a PnL dash row is — it keeps the
/// direction-DEPENDENT ordering `compare` + reverse always gave it.
///
/// Mutation: broadening `AssetCol::missing_value` to also treat a non-finite Value as "nothing to
/// show" (an easy over-generalization once PnL's pin exists) would pin the broken-price row last
/// under BOTH arrows instead of flipping which end it lands on.
#[test]
fn value_non_finite_rows_keep_direction_dependent_ordering_not_pnls_pin() {
    let mut priced = entry("AAA", 0.0, true);
    priced.display_value = 10.0;
    let mut broken = entry("BBB", 0.0, true);
    broken.display_value = f64::NAN;
    let rows = [priced, broken];

    let mut ascending = rows.clone();
    ascending.sort_by(|a, b| order_rows(AssetCol::Value, true, a, b));
    let order: Vec<&str> = ascending.iter().map(|e| e.row.coin.as_str()).collect();
    assert_eq!(
        order,
        vec!["AAA", "BBB"],
        "ascending: broken price sorts last"
    );

    let mut descending = rows.clone();
    descending.sort_by(|a, b| order_rows(AssetCol::Value, false, a, b));
    let order: Vec<&str> = descending.iter().map(|e| e.row.coin.as_str()).collect();
    assert_eq!(
        order,
        vec!["BBB", "AAA"],
        "descending: broken price sorts FIRST, unlike a pinned PnL dash row"
    );
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

/// `assets/columns.rs:restore_sort` must retain a valid visible column and its direction.
///
/// Mutation: force `ascending = true` while restoring. A Value-descending choice would reopen with
/// the arrow and rows reversed, and the exact tuple assertion reddens.
#[test]
fn valid_asset_sort_restores_its_direction() {
    assert_eq!(
        restore_sort(
            Some(TableSortPreference {
                column: "value".to_string(),
                ascending: false,
            }),
            &[AssetCol::Coin, AssetCol::Value],
        ),
        Some((AssetCol::Value, false))
    );
}

/// `assets/columns.rs:restore_sort` must reject hidden, unknown, and action-only columns.
///
/// Mutation: omit the visibility/sortability filters. Assets could reopen ordered by an invisible
/// key with no arrow the user can click, and one of these assertions reddens.
#[test]
fn unusable_asset_sort_keeps_the_historical_default() {
    for column in ["value", "actions", "retired"] {
        assert_eq!(
            restore_sort(
                Some(TableSortPreference {
                    column: column.to_string(),
                    ascending: true,
                }),
                &[AssetCol::Coin],
            ),
            None,
            "{column} must not become an invisible active sort"
        );
    }
}
