//! Unit checks for caption resolution and the open-order figures behind it.
//!
//! Explicit imports throughout: the chartdx parent re-exports `gpui::*`, whose own `test` shadows
//! the built-in attribute and makes `#[test]` expand recursively.

use moon_core::config::{ChartLabelField, ChartLabelSlot, ChartLabelsCfg, LabelZone, PnlBasis};
use moon_core::feed::OrderRow;
use moon_core::util::fmt::DeltaSign;

use super::{LabelInputs, LabelState, basis_index, collect_open_stats};

/// One open BTC row with a filled one-unit long position.
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

fn inputs_with(rows: &[OrderRow]) -> LabelInputs {
    let (basis, strategy) = collect_open_stats(rows, "BTCUSDT");
    LabelInputs {
        ticker: "BTCUSDT".into(),
        core_name: "Core-1".into(),
        strategy,
        basis,
        ..Default::default()
    }
}

fn slot(field: ChartLabelField) -> ChartLabelSlot {
    ChartLabelSlot::new(field, LabelZone::ChartTop)
}

fn texts_of(cfg: &ChartLabelsCfg, inputs: LabelInputs) -> Vec<String> {
    let mut state = LabelState::default();
    state.update(cfg, inputs);
    state.texts.iter().map(|t| t.text.clone()).collect()
}

fn one_field(field: ChartLabelField, inputs: LabelInputs) -> Option<String> {
    let mut cfg = ChartLabelsCfg {
        slots: [ChartLabelSlot::default(); 16],
    };
    cfg.slots[0] = slot(field);
    texts_of(&cfg, inputs).into_iter().next()
}

// --- open-order figures -------------------------------------------------------------------------

#[test]
fn a_long_in_profit_reports_the_gain_across_every_basis() {
    let (basis, _) = collect_open_stats(&[order(100.0, 110.0)], "BTCUSDT");
    let all = basis[basis_index(PnlBasis::All)];
    assert_eq!(all.open_orders, 1);
    assert!(
        (all.pnl_quote - 10.0).abs() < 1e-9,
        "10 quote on a 1-unit move"
    );
    assert!((all.spent - 100.0).abs() < 1e-9);
    assert!(all.has_position);
    assert_eq!(
        basis[basis_index(PnlBasis::Emulator)].open_orders,
        0,
        "a live order is not an emulated one"
    );
}

/// A short earns when the price falls, and its entry leg is `buy_price` like a long's.
#[test]
fn a_short_earns_when_the_price_falls() {
    let mut row = order(100.0, 90.0);
    row.is_short = true;
    let (basis, _) = collect_open_stats(&[row], "BTCUSDT");
    let all = basis[basis_index(PnlBasis::All)];
    assert!(all.pnl_quote > 0.0, "a short profits from a decline");
    assert!(all.pos_size < 0.0, "a short position is signed negative");
}

#[test]
fn the_basis_separates_live_orders_from_emulated_ones() {
    let live = order(100.0, 110.0);
    let mut emu = order(100.0, 120.0);
    emu.emulator = true;
    emu.uid = 2;
    let (basis, _) = collect_open_stats(&[live, emu], "BTCUSDT");
    assert_eq!(basis[basis_index(PnlBasis::All)].open_orders, 2);
    assert_eq!(basis[basis_index(PnlBasis::Real)].open_orders, 1);
    assert_eq!(basis[basis_index(PnlBasis::Emulator)].open_orders, 1);
    assert!(
        (basis[basis_index(PnlBasis::Real)].pnl_quote - 10.0).abs() < 1e-9,
        "the real basis must not carry the emulated order's result"
    );
    assert!((basis[basis_index(PnlBasis::Emulator)].pnl_quote - 20.0).abs() < 1e-9);
}

/// A working entry is an open ORDER but holds no position: counting it as one would print a
/// result on money that was never spent.
#[test]
fn a_working_entry_counts_as_an_order_but_not_as_a_position() {
    let mut row = order(100.0, 110.0);
    row.filled = false;
    row.fill_pct = 0.0;
    let (basis, _) = collect_open_stats(&[row], "BTCUSDT");
    let all = basis[basis_index(PnlBasis::All)];
    assert_eq!(all.open_orders, 1);
    assert!(!all.has_position);
    assert_eq!(all.spent, 0.0);
}

/// `job_is_done` is the authoritative closure flag; a terminal order is not on the chart's books.
#[test]
fn a_finished_order_is_excluded_entirely() {
    let mut row = order(100.0, 110.0);
    row.job_is_done = true;
    let (basis, strategy) = collect_open_stats(&[row], "BTCUSDT");
    assert_eq!(basis[basis_index(PnlBasis::All)].open_orders, 0);
    assert!(strategy.is_empty());
}

#[test]
fn another_market_never_contributes() {
    let mut row = order(100.0, 110.0);
    row.market = "ETHUSDT".into();
    let (basis, _) = collect_open_stats(&[row], "BTCUSDT");
    assert_eq!(basis[basis_index(PnlBasis::All)].open_orders, 0);
}

#[test]
fn the_strategy_name_comes_from_the_newest_open_order() {
    let mut old = order(100.0, 110.0);
    old.uid = 1;
    old.strat_name = "Старая".into();
    let mut new = order(100.0, 110.0);
    new.uid = 7;
    new.strat_name = "Свежая".into();
    let (_, strategy) = collect_open_stats(&[old, new], "BTCUSDT");
    assert_eq!(strategy, "Свежая");
}

// --- caption resolution -------------------------------------------------------------------------

/// A field with nothing to report prints nothing at all, so the default configuration's optional
/// captions cost no rows on an ordinary chart.
#[test]
fn an_unresolved_field_occupies_no_row() {
    let cfg = ChartLabelsCfg::default();
    let inputs = LabelInputs {
        ticker: "BTCUSDT".into(),
        core_name: "Core-1".into(),
        ..Default::default()
    };
    let texts = texts_of(&cfg, inputs);
    assert_eq!(
        texts,
        vec!["BTCUSDT".to_string(), "Core-1".to_string()],
        "no scale badge and no comparison delta means two captions, not four"
    );
}

/// The slot index travels with the text: it addresses the retained GPU run, and a hidden
/// neighbour must not shift it.
#[test]
fn the_slot_index_survives_a_skipped_neighbour() {
    let cfg = ChartLabelsCfg::default();
    let inputs = LabelInputs {
        ticker: "BTCUSDT".into(),
        core_name: "Core-1".into(),
        ..Default::default()
    };
    let mut state = LabelState::default();
    state.update(&cfg, inputs);
    let slots: Vec<usize> = state.texts.iter().map(|t| t.slot).collect();
    assert_eq!(
        slots,
        vec![0, 2],
        "the coin keeps slot 0 and the core name slot 2 across the skipped badge"
    );
}

#[test]
fn re_running_with_identical_inputs_reports_no_change() {
    let cfg = ChartLabelsCfg::default();
    let inputs = LabelInputs {
        ticker: "BTCUSDT".into(),
        core_name: "Core-1".into(),
        ..Default::default()
    };
    let mut state = LabelState::default();
    assert!(state.update(&cfg, inputs.clone()), "the first pass formats");
    assert!(
        !state.update(&cfg, inputs),
        "identical inputs must not reshape a single run"
    );
}

/// A price that ticks inside the printed rounding changes the INPUTS but not the drawn text, and
/// must not repaint the pane.
#[test]
fn a_tick_below_the_printed_precision_does_not_change_the_caption() {
    let mut cfg = ChartLabelsCfg {
        slots: [ChartLabelSlot::default(); 16],
    };
    cfg.slots[0] = slot(ChartLabelField::Delta24h);
    let mut state = LabelState::default();
    let mut inputs = LabelInputs {
        delta_24h: Some(1.234),
        ..Default::default()
    };
    assert!(state.update(&cfg, inputs.clone()));
    inputs.delta_24h = Some(1.2341);
    assert!(
        !state.update(&cfg, inputs),
        "the same rounded text must report no change"
    );
}

#[test]
fn a_signed_figure_carries_its_sign_for_coloring() {
    let mut cfg = ChartLabelsCfg {
        slots: [ChartLabelSlot::default(); 16],
    };
    cfg.slots[0] = slot(ChartLabelField::Delta1h);
    let mut state = LabelState::default();
    state.update(
        &cfg,
        LabelInputs {
            delta_1h: Some(-2.5),
            ..Default::default()
        },
    );
    assert_eq!(state.texts[0].sign, Some(DeltaSign::Negative));
    assert!(state.texts[0].text.contains("-2.50%"));
}

/// The coin is a name, not a quantity: coloring it by sign would be meaningless, so it reports none.
#[test]
fn a_plain_name_reports_no_sign() {
    let mut cfg = ChartLabelsCfg {
        slots: [ChartLabelSlot::default(); 16],
    };
    cfg.slots[0] = slot(ChartLabelField::Coin);
    let mut state = LabelState::default();
    state.update(
        &cfg,
        LabelInputs {
            ticker: "BTCUSDT".into(),
            ..Default::default()
        },
    );
    assert_eq!(state.texts[0].sign, None);
}

#[test]
fn the_scale_badge_states_a_sub_percent_range_rather_than_zero() {
    let text = one_field(
        ChartLabelField::ScaleBadge,
        LabelInputs {
            scale_badge: Some(0),
            ..Default::default()
        },
    );
    assert_eq!(text.as_deref(), Some("<1%"));
}

/// An empty position prints no percentage: a confident `0.00%` would claim a flat position where
/// there is none at all.
#[test]
fn no_position_prints_no_pnl() {
    assert!(one_field(ChartLabelField::OpenPnlPct, inputs_with(&[])).is_none());
    assert!(one_field(ChartLabelField::OpenPnlMoney, inputs_with(&[])).is_none());
    assert!(one_field(ChartLabelField::OpenOrders, inputs_with(&[])).is_none());
}

#[test]
fn the_pnl_percentage_is_weighted_by_what_each_order_spent() {
    // 1 unit at 100 gaining 10, plus 1 unit at 300 gaining 30: 40 on 400 spent is exactly 10%.
    let mut second = order(300.0, 330.0);
    second.uid = 2;
    let text = one_field(
        ChartLabelField::OpenPnlPct,
        inputs_with(&[order(100.0, 110.0), second]),
    );
    assert_eq!(
        text.as_deref(),
        Some("PnL: +10.00%"),
        "the PnL default carries its caption"
    );
}

/// The basis is per SLOT, so two captions on one chart can report different sets of orders.
#[test]
fn two_slots_can_read_different_bases() {
    let live = order(100.0, 110.0);
    let mut emu = order(100.0, 130.0);
    emu.emulator = true;
    emu.uid = 2;
    let inputs = inputs_with(&[live, emu]);
    let mut cfg = ChartLabelsCfg {
        slots: [ChartLabelSlot::default(); 16],
    };
    cfg.slots[0] = slot(ChartLabelField::OpenPnlPct);
    cfg.slots[0].pnl_basis = PnlBasis::Real;
    cfg.slots[1] = slot(ChartLabelField::OpenPnlPct);
    cfg.slots[1].pnl_basis = PnlBasis::Emulator;
    let texts = texts_of(&cfg, inputs);
    assert_eq!(
        texts,
        vec!["PnL: +10.00%".to_string(), "PnL: +30.00%".to_string()]
    );
}

/// The caption flag is what turns a bare number into a labelled one.
#[test]
fn the_caption_flag_prefixes_the_field_name() {
    let mut cfg = ChartLabelsCfg {
        slots: [ChartLabelSlot::default(); 16],
    };
    cfg.slots[0] = slot(ChartLabelField::Delta1h);
    cfg.slots[0].style.caption = Some(false);
    let bare = texts_of(
        &cfg,
        LabelInputs {
            delta_1h: Some(1.0),
            ..Default::default()
        },
    );
    cfg.slots[0].style.caption = Some(true);
    let named = texts_of(
        &cfg,
        LabelInputs {
            delta_1h: Some(1.0),
            ..Default::default()
        },
    );
    assert_eq!(bare, vec!["+1.00%".to_string()]);
    // The prefix itself comes from the dictionary, so the assertion is about its SHAPE rather than
    // its wording: the test locale is not the one the operator runs.
    assert!(
        named[0].ends_with("+1.00%") && named[0].len() > bare[0].len(),
        "the caption prefixes the value rather than replacing it: {:?}",
        named[0]
    );
}

// --- market background and funding ---------------------------------------------------------------

/// Funding time far enough out that a countdown can be measured against it.
const FUNDING_AT_MS: i64 = 10_000_000;

fn ctx() -> moon_core::market::MarketContextReadout {
    moon_core::market::MarketContextReadout {
        exchange_1h_pct: -0.8,
        exchange_24h_pct: 1.5,
        btc_1h_pct: 0.25,
        btc_24h_pct: -2.0,
        btc_72h_pct: 4.75,
        funding_pct: Some(0.01),
        funding_at_ms: Some(FUNDING_AT_MS),
    }
}

/// Build the caption a field is expected to print, THROUGH the dictionary.
///
/// The tests run under whatever locale is active, so a literal Russian or English expectation
/// would pin the assertion to a language rather than to the behaviour it is about.
fn expect(key: &str, value: &str) -> String {
    format!("{}: {value}", rust_i18n::t!(key))
}

#[test]
fn the_background_deltas_print_with_their_sign() {
    let inputs = LabelInputs {
        context: Some(ctx()),
        ..Default::default()
    };
    assert_eq!(
        one_field(ChartLabelField::ExchangeDelta1h, inputs.clone()).as_deref(),
        Some(expect("chart_labels.short.exchange_1h", "-0.80%").as_str())
    );
    assert_eq!(
        one_field(ChartLabelField::BtcDelta72h, inputs).as_deref(),
        Some(expect("chart_labels.short.btc_72h", "+4.75%").as_str())
    );
}

/// A market with no funding — spot — prints nothing. A zero there would read as "funding is free
/// here", which is a different claim from "this venue does not charge it".
#[test]
fn a_market_without_funding_prints_nothing() {
    let mut c = ctx();
    c.funding_pct = None;
    c.funding_at_ms = None;
    let inputs = LabelInputs {
        context: Some(c),
        ..Default::default()
    };
    assert!(one_field(ChartLabelField::Funding, inputs.clone()).is_none());
    assert!(one_field(ChartLabelField::FundingIn, inputs).is_none());
}

#[test]
fn the_funding_countdown_states_hours_and_minutes() {
    let at = |remaining_ms: i64| LabelInputs {
        context: Some(ctx()),
        now_ms: FUNDING_AT_MS - remaining_ms,
        ..Default::default()
    };
    let hour_and_five = one_field(ChartLabelField::FundingIn, at(65 * 60_000));
    let expected = format!(
        "1{} 05{}",
        rust_i18n::t!("chart_labels.unit_hour"),
        rust_i18n::t!("chart_labels.unit_minute")
    );
    assert_eq!(
        hour_and_five.as_deref(),
        Some(expect("chart_labels.short.funding_in", &expected).as_str())
    );
    let forty_seven = one_field(ChartLabelField::FundingIn, at(47 * 60_000));
    let expected = format!("47{}", rust_i18n::t!("chart_labels.unit_minute"));
    assert_eq!(
        forty_seven.as_deref(),
        Some(expect("chart_labels.short.funding_in", &expected).as_str())
    );
}

/// A funding time already past is not printed: the core republishes the next one within seconds,
/// and a negative countdown reads as a stuck chart rather than as a stale field.
#[test]
fn an_elapsed_funding_time_prints_nothing() {
    let inputs = LabelInputs {
        context: Some(ctx()),
        now_ms: FUNDING_AT_MS + 1,
        ..Default::default()
    };
    assert!(one_field(ChartLabelField::FundingIn, inputs).is_none());
}

/// Without the snapshot read — the gate is off because nothing asks for it — the background fields
/// resolve to nothing rather than to zero.
#[test]
fn no_context_means_no_background_captions() {
    let inputs = LabelInputs::default();
    for field in [
        ChartLabelField::ExchangeDelta1h,
        ChartLabelField::BtcDelta24h,
        ChartLabelField::Funding,
        ChartLabelField::FundingIn,
    ] {
        assert!(
            one_field(field, inputs.clone()).is_none(),
            "{field:?} must stay silent without a snapshot"
        );
    }
}

/// The quote currency is the unit behind every money figure on the chart, and it is a NAME: it
/// prints as it comes and colours by nothing.
#[test]
fn the_quote_currency_prints_as_a_plain_name() {
    let inputs = LabelInputs {
        quote: "USDT".into(),
        ..Default::default()
    };
    let mut cfg = ChartLabelsCfg {
        slots: [ChartLabelSlot::default(); 16],
    };
    cfg.slots[0] = slot(ChartLabelField::Quote);
    let mut state = LabelState::default();
    state.update(&cfg, inputs);
    assert_eq!(state.texts[0].text, "USDT");
    assert_eq!(state.texts[0].sign, None);
}

/// A COIN-M contract carries no quote currency. Printing nothing is the honest answer; a guessed
/// "USD" would label money that is settled in the coin itself.
#[test]
fn a_market_without_a_quote_prints_nothing() {
    assert!(one_field(ChartLabelField::Quote, LabelInputs::default()).is_none());
}
