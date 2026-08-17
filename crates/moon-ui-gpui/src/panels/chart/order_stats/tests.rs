//! Unit checks for the chart overlay's open-order facts and their drop order.
//!
//! Explicit imports throughout: the chart parent re-exports `gpui::*`, whose own `test` shadows the
//! built-in attribute and makes `#[test]` expand recursively.

use moon_core::feed::OrderRow;

use super::{OrderStats, StatTone, order_stats};

/// Build one open BTC market row with a filled one-unit position.
///
/// Returns:
///     A complete row whose entry and mark may be varied without a GPUI fixture.
fn order(entry: f64, mark: f32) -> OrderRow {
    OrderRow {
        market: "BTCUSDT".into(),
        market_display: "BTCUSDT".into(),
        coin: "BTC".into(),
        quote: "USDT".into(),
        is_short: false,
        size: 1.0,
        remaining_size: 1.0,
        sl_on: false,
        ts_on: false,
        vstop_on: false,
        sl_fixed: false,
        ts_fixed: false,
        vstop_fixed: false,
        vstop_level: 0.0,
        vstop_vol: 0.0,
        buy_price: entry,
        sell_price: 0.0,
        create_time_ms: 0.0,
        sell_create_time_ms: 0.0,
        entry_fill_time_ms: 0.0,
        price: mark,
        fill_pct: 100.0,
        strat: "test".into(),
        strat_name: String::new(),
        strat_id: 1,
        status: String::new(),
        uid: 1,
        emulator: false,
        job_is_done: false,
        pending: false,
        filled: true,
        stop_loss: None,
        trailing: None,
        take_profit: None,
        vstop: None,
        pending_cond: None,
        liq: None,
        panic_sell: false,
        is_moon_shot: false,
        corridor_price_down: 0.0,
        corridor_price_up: 0.0,
        buy_trace: None,
        sell_trace: None,
    }
}

/// Assemble order facts with a deterministic one-unit string measurement.
///
/// Args:
///     rows: Rows to filter for the active BTC market.
///     budget: Whole-overlay width in one-character units.
///
/// Returns:
///     The chart overlay facts, which exist for every non-empty active market.
fn stats(rows: &[OrderRow], budget: f32) -> OrderStats {
    order_stats(rows, "BTCUSDT", budget, &|_| 1.0).expect("the fixture has an open BTC order")
}

/// Keep `order_stats::sum_position` exposing rows whose entry price has not landed.
///
/// Breakage: gating mark notional behind `order_pnl(row).is_some()`. The chart would omit real
/// current exposure merely because the feed had not delivered an entry price yet.
#[test]
fn exposure_survives_an_order_without_an_entry_price() {
    let facts = stats(&[order(0.0, 50.0)], 20.0);

    assert_eq!(facts.essential.len(), 1);
    assert_eq!(facts.tail.len(), 1, "only the current notional is knowable");
    assert_eq!(facts.tail[0].tone, StatTone::Soft);
    assert!(
        facts.tail[0].text.contains("50"),
        "got {:?}",
        facts.tail[0].text
    );
}

/// Keep `order_stats::sum_position` dividing PnL by entry rather than displayed mark notional.
///
/// Breakage: using `mark_notional` as the percentage denominator. A doubled market price would
/// show a 50% gain instead of the position's independently calculated 100% return.
#[test]
fn percentage_uses_entry_notional_even_when_mark_notional_differs() {
    let facts = stats(&[order(50.0, 100.0)], 20.0);

    assert_eq!(facts.tail.len(), 3);
    assert!(
        facts.tail[0].text.contains("50"),
        "PnL: {:?}",
        facts.tail[0].text
    );
    assert!(
        facts.tail[1].text.contains("100"),
        "sum: {:?}",
        facts.tail[1].text
    );
    assert!(
        facts.tail[2].text.contains("100"),
        "percent: {:?}",
        facts.tail[2].text
    );
}

/// Keep `order_stats` silent when its active market has no open rows.
///
/// Breakage: returning a zero-filled `OrderStats` for an empty market. The chart would claim a
/// settled zero position when no order snapshot for that market exists at all.
#[test]
fn an_empty_active_market_has_no_overlay_facts() {
    assert!(order_stats(&[], "BTCUSDT", 20.0, &|_| 1.0).is_none());
}

/// Keep `order_stats::fit` from skipping a too-wide middle fact.
///
/// Breakage: continuing after the current-notional figure fails its width check. The percentage
/// would appear in the sum's visual slot and mislead a user about which money figure is absent.
#[test]
fn width_fit_stops_at_the_first_fact_that_does_not_fit() {
    let facts = order_stats(&[order(100.0, 200.0)], "BTCUSDT", 3.0, &|text| {
        if text.contains("200") { 10.0 } else { 1.0 }
    })
    .expect("the row is open");

    assert_eq!(
        facts.tail.len(),
        1,
        "the percent must not jump over the sum"
    );
    assert!(
        facts.tail[0].text.contains("100"),
        "got {:?}",
        facts.tail[0].text
    );
    assert_eq!(facts.tail[0].tone, StatTone::Positive);
}

/// Keep `order_stats` dropping percent, then sum, while retaining PnL and the count.
///
/// Breakage: changing the candidate priority or allowing the essential count to be dropped. A
/// narrow chart would discard its most useful live profit first or lose the scope of its figures.
#[test]
fn narrow_budgets_drop_tail_facts_in_the_declared_priority_order() {
    let row = order(100.0, 200.0);
    let all = stats(&[row.clone()], 4.0);
    let no_percent = stats(&[row.clone()], 3.0);
    let pnl_only = stats(&[row.clone()], 2.0);
    let count_only = stats(&[row.clone()], 1.0);
    let negative_budget = stats(&[row], -1.0);

    assert_eq!(all.tail.len(), 3);
    assert_eq!(no_percent.tail.len(), 2);
    assert_eq!(pnl_only.tail.len(), 1);
    assert!(count_only.tail.is_empty() && negative_budget.tail.is_empty());
    assert!(all.tail[2].text.ends_with('%'));
    assert!(no_percent.tail[1].text.contains("200"));
    assert!(pnl_only.tail[0].text.contains("100"));
    assert_eq!(negative_budget.essential.len(), 1);
}

/// Keep `order_stats::is_open_here` excluding terminal rows from every overlay figure.
///
/// Breakage: removing the `job_is_done` gate. A completed order awaiting deferred removal would
/// inflate the count and money shown as still open on the chart.
#[test]
fn completed_rows_do_not_contribute_to_count_or_aggregates() {
    let open = order(100.0, 200.0);
    let mut completed = order(100.0, 200.0);
    completed.job_is_done = true;

    let facts = stats(&[open, completed], 20.0);

    assert_eq!(facts.essential.len(), 1);
    assert!(facts.essential[0].text.contains('1'));
    assert_eq!(facts.tail.len(), 3);
    assert!(facts.tail[0].text.contains("100"));
    assert!(facts.tail[1].text.contains("200"));
}

/// Keep `order_stats::is_open_here` matching the feed market key rather than display text.
///
/// Breakage: filtering on `market_display`. Hyperliquid's `@206` data-key order would disappear
/// from its chart because its user-facing label is `UENAUSDT`.
#[test]
fn market_filter_uses_the_data_key_not_the_display_label() {
    let mut hyperliquid = order(100.0, 200.0);
    hyperliquid.market = "@206".into();
    hyperliquid.market_display = "UENAUSDT".into();

    assert!(order_stats(&[hyperliquid.clone()], "@206", 20.0, &|_| 1.0).is_some());
    assert!(order_stats(&[hyperliquid], "UENAUSDT", 20.0, &|_| 1.0).is_none());
}

/// Keep `order_stats::sum_position` adding long and short exposure while netting directional PnL.
///
/// Breakage: applying the short direction to current notional. Equal long and short positions
/// would falsely state zero exposure even though both remain at risk on the active chart.
#[test]
fn mixed_direction_positions_add_exposure_and_net_directional_profit() {
    let long = order(100.0, 110.0);
    let mut short = order(100.0, 90.0);
    short.is_short = true;

    let facts = stats(&[long, short], 20.0);

    assert_eq!(facts.tail.len(), 3);
    assert!(
        facts.tail[0].text.contains("20"),
        "PnL: {:?}",
        facts.tail[0].text
    );
    assert!(
        facts.tail[1].text.contains("200"),
        "sum: {:?}",
        facts.tail[1].text
    );
    assert!(
        facts.tail[2].text.contains("10"),
        "percent: {:?}",
        facts.tail[2].text
    );
}

/// Keep a profitable short's overlay PnL positive and explicitly plus-signed.
///
/// Breakage: losing the short direction before formatting. Falling prices would paint a short's
/// gain as a red loss in the chart overlay while the Orders table is expected to agree.
#[test]
fn a_profitable_short_uses_positive_tone_and_a_plus_signed_amount() {
    let mut short = order(100.0, 80.0);
    short.is_short = true;

    let facts = stats(&[short], 20.0);

    assert_eq!(facts.tail[0].tone, StatTone::Positive);
    assert!(
        facts.tail[0].text.contains("+20"),
        "got {:?}",
        facts.tail[0].text
    );
}

/// Keep a PnL amount that rounds to zero visually and tonally neutral.
///
/// Breakage: classifying the raw negative value before rounding. A displayed zero would carry a
/// minus sign or red tone, making the chart's text and colour disagree about the same amount.
#[test]
fn a_loss_that_rounds_away_is_soft_and_not_minus_signed() {
    let facts = stats(&[order(100.0, 99.996)], 20.0);

    assert_eq!(facts.tail[0].tone, StatTone::Soft);
    assert!(
        !facts.tail[0].text.contains('-'),
        "got {:?}",
        facts.tail[0].text
    );
}

/// Keep `order_stats` count-only when open rows have no held position.
///
/// Breakage: rendering zero money facts for a working but unfilled order. The chart would claim a
/// flat position rather than honestly state that an order exists without exposure yet.
#[test]
fn open_orders_without_positions_state_their_count_and_no_money() {
    let mut unfilled = order(100.0, 200.0);
    unfilled.filled = false;
    unfilled.fill_pct = 0.0;

    let facts = stats(&[unfilled], 20.0);

    assert_eq!(facts.essential.len(), 1);
    assert!(facts.essential[0].text.contains('1'));
    assert!(facts.tail.is_empty());
}

/// Keep `order_stats` charging the count badge against the whole width budget.
///
/// Breakage: passing the full budget directly to `fit`. A tail fact would be painted even though
/// the count badge has already consumed the only available overlay space.
#[test]
fn the_essential_count_spends_the_budget_before_tail_facts() {
    let facts = stats(&[order(100.0, 200.0)], 1.0);

    assert_eq!(facts.essential.len(), 1);
    assert!(facts.tail.is_empty());
}
