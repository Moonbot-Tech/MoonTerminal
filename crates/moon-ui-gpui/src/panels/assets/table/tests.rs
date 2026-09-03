//! Regression tests for Assets table columns and scope-sensitive Market Sell actions.

// Explicit imports, NOT `use super::*`: the parent re-exports `gpui::*`, which carries its
// own `test` and shadows the built-in attribute — `#[test]` then expands recursively.
use super::{AssetsScope, assets_columns, market_sell_core_is_authorized};
use crate::panels::assets::columns::AssetCol;

/// Pins `.no_grow()` on the actions column of
/// `panels/assets/table.rs::assets_columns`. The plausible edit: someone adds or reorders a
/// column and rewrites the builder without carrying the call over. The title-less
/// button column would rejoin the auto-width pool, claim a share of every viewport wider than
/// the column sum, and visibly push coin/qty/value apart again — the spread this pins shut.
#[test]
fn the_title_less_actions_column_never_stretches() {
    let columns = assets_columns(&AssetCol::ALL);
    let actions = columns
        .iter()
        .find(|c| c.key == "actions")
        .expect("the Assets table must offer an actions column");

    assert!(
        actions.no_grow,
        "the actions column holds two fixed-width buttons and no title, so it must stay out \
         of the auto-width pool"
    );
    assert!(
        actions.title.is_empty(),
        "the button column carries no header caption; its name exists only in the field selector"
    );
}

/// The table renders exactly the chosen fields, in canonical order, with nothing appended.
///
/// The plausible edit: restoring the old unconditional `push` of the action column after it became
/// a selectable field. The row builder emits one cell per VISIBLE field, so an extra column would
/// leave the table with more columns than cells.
#[test]
fn the_columns_follow_the_selected_fields_exactly() {
    let columns = assets_columns(&[AssetCol::Coin, AssetCol::Pnl]);
    let keys: Vec<&str> = columns.iter().map(|c| c.key.as_ref()).collect();
    assert_eq!(keys, vec!["coin", "pnl"]);
}

/// `table.rs:market_sell_core_is_authorized` must reject a group dialog after navigation removes
/// its captured core, while the explicitly global Assets window keeps its prior authority.
///
/// Mutation: treat every confirmation as global or test only whether the current scope is nonempty.
/// The stale dialog could then sell on core 7 while Auto shows core 9.
#[test]
fn stale_market_sell_dialog_cannot_target_the_previous_auto_core() {
    let group = AssetsScope::Group("desk".to_string());
    let global = AssetsScope::All;

    assert!(market_sell_core_is_authorized(&group, Some(&[7]), 7));
    assert!(!market_sell_core_is_authorized(&group, Some(&[9]), 7));
    assert!(!market_sell_core_is_authorized(&group, None, 7));
    assert!(market_sell_core_is_authorized(&global, None, 7));
}

/// The Market Sell Yes callback must re-read and validate effective scope before either command.
///
/// Mutation: bypass the helper in the dialog callback. The pure decision test remains green, but
/// this wiring assertion reddens before a stale confirmation can submit the sell.
#[test]
fn market_sell_yes_revalidates_scope_before_dispatch() {
    let source = include_str!("../table.rs");
    let callback = source
        .split_once("MoonButton::new(\"assets-msell-yes\")")
        .expect("Market Sell Yes callback must exist")
        .1;
    let scope_read = callback
        .find("let effective_scope = this.effective_scope(b);")
        .expect("Yes must re-read current effective scope");
    let authority = callback
        .find("market_sell_core_is_authorized(")
        .expect("Yes must validate the captured core");
    // Matched WITHOUT the `b.session` prefix: rustfmt breaks a deeply indented call across lines,
    // and this test pins dispatch ORDER, not the receiver's formatting.
    let position = callback
        .find(".market_sell_position(")
        .expect("position sell command must remain reachable");
    let token = callback
        .find(".market_sell_token(")
        .expect("token sell command must remain reachable");

    assert!(scope_read < authority && authority < position && authority < token);
}

/// A spot sale must send a live price it read at dispatch, never a zero and never the rendered one.
///
/// Mutation: go back to `price=0`, or carry `e.row.price` into the confirmation. The zero is what
/// made the core invent `quantity=499.99999` for a 0.005 BTC holding, and a wallet-derived row
/// prices in USDT rather than in the market's quote (2026-09-03).
#[test]
fn a_spot_market_sell_reads_a_live_price_before_dispatch() {
    let source = include_str!("../table.rs");
    let callback = source
        .split_once("MoonButton::new(\"assets-msell-yes\")")
        .expect("Market Sell Yes callback must exist")
        .1;
    let live = callback
        .find(".latest_price(core, &market_c)")
        .expect("Yes must re-read the live quote price");
    let token = callback
        .find(".market_sell_token(")
        .expect("token sell command must remain reachable");

    assert!(live < token, "the live price must be read before dispatch");
    let call = &callback[token..];
    assert!(
        call.contains("qty, price)") || call.contains("qty,\n") && call.contains("price,"),
        "the token sale must carry the held quantity and the freshly read price"
    );
}
