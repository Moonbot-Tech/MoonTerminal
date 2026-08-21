//! Unit checks for caption resolution and the open-order figures behind it.
//!
//! Explicit imports throughout: the chartdx parent re-exports `gpui::*`, whose own `test` shadows
//! the built-in attribute and makes `#[test]` expand recursively.

use std::rc::Rc;

use moon_core::config::{
    ArbViewCfg, ChartLabelField, ChartLabelRow, ChartLabelsCfg, LabelAlign, LabelZone, PnlBasis,
};
use moon_core::feed::OrderRow;
use moon_core::util::fmt::DeltaSign;

use super::{LabelInputs, LabelState, basis_index, collect_open_stats, preview_row};

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

/// A configuration holding exactly the given captions, all on ONE row over the plot.
fn cfg_of(fields: &[ChartLabelField]) -> ChartLabelsCfg {
    let mut cfg = ChartLabelsCfg::empty();
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    for field in fields {
        row.push_part(*field);
    }
    cfg.rows[0] = row;
    cfg
}

fn texts_of(cfg: &ChartLabelsCfg, inputs: LabelInputs) -> Vec<String> {
    let mut state = LabelState::default();
    state.update(&Rc::new(cfg.clone()), inputs);
    state.texts.iter().map(|t| t.text.clone()).collect()
}

fn one_field(field: ChartLabelField, inputs: LabelInputs) -> Option<String> {
    texts_of(&cfg_of(&[field]), inputs).into_iter().next()
}

/// Two strategy captions can sit on one chart — the one holding an order and the one that last
/// fired a detect — so the detect one NAMES itself. A bare name would be unreadable beside the
/// other.
#[test]
fn the_detect_strategy_names_itself() {
    let inputs = LabelInputs {
        detect_strategy: "BTC Sniper".into(),
        ..Default::default()
    };
    let text = one_field(ChartLabelField::DetectStrategy, inputs).expect("prints");
    assert!(text.ends_with("BTC Sniper"), "{text:?}");
    assert!(text.contains(": "), "the caption prefix is there: {text:?}");

    // The order's strategy stays bare: it is the chart's original caption and the reference
    // terminal prints it without a prefix.
    let plain = one_field(
        ChartLabelField::OrderStrategy,
        LabelInputs {
            strategy: "BTC Sniper".into(),
            ..Default::default()
        },
    )
    .expect("prints");
    assert_eq!(plain, "BTC Sniper");
}

/// A core-supplied line can be a paragraph; the caption carries a readable head of it and says so.
#[test]
fn a_long_detect_line_is_cut() {
    let long = "ц".repeat(300);
    let text = one_field(
        ChartLabelField::DetectMsg,
        LabelInputs {
            detect_msg: long,
            ..Default::default()
        },
    )
    .expect("prints");
    assert!(text.chars().count() < 120, "{}", text.chars().count());
    assert!(text.ends_with('…'));
}

// --- the arbitrage column -------------------------------------------------------------------------

/// A quote from `venue` at `price`, against a market trading at 100.
fn arb_quote(code: u8, price: f64) -> moon_core::market::ArbQuote {
    moon_core::market::ArbQuote {
        venue: moon_core::market::ArbVenue::from_code(code),
        price,
        my_price: 100.0,
        spread_pct: price - 100.0,
        deposit_blocked: false,
        withdraw_blocked: false,
    }
}

fn arb_inputs(quotes: Vec<moon_core::market::ArbQuote>, view: ArbViewCfg) -> LabelInputs {
    LabelInputs {
        arb: quotes,
        arb_view: Some(Rc::new(view)),
        ..Default::default()
    }
}

/// ONE configured caption prints a LINE PER VENUE — that is the whole reason the column exists as a
/// field rather than as eight captions the user has to place by hand.
#[test]
fn the_column_prints_one_line_per_venue() {
    let cfg = cfg_of(&[ChartLabelField::ArbColumn]);
    let mut view = ArbViewCfg::default();
    view.venues = vec![
        moon_core::config::ArbVenueCfg::new(moon_core::market::ArbVenue::from_code(4)),
        moon_core::config::ArbVenueCfg::new(moon_core::market::ArbVenue::from_code(9)),
    ];

    let texts = texts_of(&cfg, arb_inputs(vec![arb_quote(4, 101.0), arb_quote(9, 99.0)], view));

    assert_eq!(texts.len(), 2, "one line per venue, from one caption");
    assert!(texts[0].starts_with("BinanceF"), "{:?}", texts[0]);
    assert!(texts[0].contains("+1.00%"), "{:?}", texts[0]);
    assert!(texts[1].contains("-1.00%"), "{:?}", texts[1]);
}

/// Each line takes a run index of its OWN, past every caption index — a venue that stops reporting
/// must not hand its retained run to the venue below it, which would reshape both.
#[test]
fn each_line_addresses_its_own_run() {
    let cfg = cfg_of(&[ChartLabelField::ArbColumn]);
    let mut state = LabelState::default();
    state.update(
        &Rc::new(cfg),
        arb_inputs(
            vec![arb_quote(4, 101.0), arb_quote(9, 99.0)],
            ArbViewCfg::default(),
        ),
    );

    let parts: Vec<usize> = state.texts.iter().map(|t| t.part).collect();
    assert_eq!(
        parts,
        vec![
            moon_core::config::ARB_PART_BASE,
            moon_core::config::ARB_PART_BASE + 1
        ]
    );
    assert!(
        parts
            .iter()
            .all(|p| *p >= moon_core::config::CHART_LABEL_PARTS),
        "an arbitrage line never occupies a caption index"
    );
}

/// What each line prints is the roster's choice, and it applies to the whole column.
#[test]
fn the_roster_decides_what_each_line_prints() {
    let cfg = cfg_of(&[ChartLabelField::ArbColumn]);
    let mut view = ArbViewCfg::default();
    view.venues = vec![moon_core::config::ArbVenueCfg::new(
        moon_core::market::ArbVenue::from_code(4),
    )];

    view.show = moon_core::config::ArbShow::Price;
    let price_only = texts_of(&cfg, arb_inputs(vec![arb_quote(4, 101.0)], view.clone()));
    assert!(!price_only[0].contains('%'), "{:?}", price_only[0]);

    view.show = moon_core::config::ArbShow::Spread;
    let spread_only = texts_of(&cfg, arb_inputs(vec![arb_quote(4, 101.0)], view));
    assert!(spread_only[0].contains("+1.00%"), "{:?}", spread_only[0]);
    assert!(!spread_only[0].contains("101"), "{:?}", spread_only[0]);
}

/// A venue that cannot be deposited to or withdrawn from is MARKED, not hidden: the spread is real
/// and the settlement is not, and a reader must not take one for the other.
#[test]
fn a_blocked_venue_is_marked() {
    let cfg = cfg_of(&[ChartLabelField::ArbColumn]);
    let mut quote = arb_quote(4, 101.0);
    quote.withdraw_blocked = true;

    let marked = texts_of(&cfg, arb_inputs(vec![quote], ArbViewCfg::default()));
    assert!(marked[0].contains('⛔'), "{:?}", marked[0]);

    let mut off = ArbViewCfg::default();
    off.mark_blocked = false;
    let unmarked = texts_of(&cfg, arb_inputs(vec![quote], off));
    assert!(!unmarked[0].contains('⛔'), "{:?}", unmarked[0]);
}

/// With no quotes the column prints nothing at all — not an empty line, and not a heading with no
/// rows under it.
#[test]
fn a_market_with_no_arbitrage_prints_nothing() {
    let cfg = cfg_of(&[ChartLabelField::ArbColumn]);
    let texts = texts_of(&cfg, arb_inputs(Vec::new(), ArbViewCfg::default()));
    assert!(texts.is_empty());
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

/// The caption's address travels with the text: it addresses the retained GPU run, and a hidden
/// neighbour must not shift it.
#[test]
fn the_caption_address_survives_a_skipped_neighbour() {
    // The shipped roster, with only some of its figures answering: the venue has no name here, the
    // Y-scale badge is hidden, and the whole position block has nothing open to report.
    let cfg = ChartLabelsCfg::default();
    let inputs = LabelInputs {
        ticker: "BTCUSDT".into(),
        core_name: "Core-1".into(),
        delta_1h: Some(1.5),
        ..Default::default()
    };
    let mut state = LabelState::default();
    let cfg_rc = Rc::new(cfg.clone());
    state.update(&cfg_rc, inputs);
    let addresses: Vec<(usize, usize)> = state.texts.iter().map(|t| (t.row, t.part)).collect();
    assert_eq!(
        addresses,
        vec![(0, 0), (0, 1), (2, 0)],
        "every caption keeps the address its CONFIGURATION gives it, whatever its neighbours          resolved to: the skipped venue does not renumber the deltas, and the skipped badge module          does not renumber the module after it"
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
    let cfg_rc = Rc::new(cfg.clone());
    assert!(
        state.update(&cfg_rc, inputs.clone()),
        "the first pass formats"
    );
    assert!(
        !state.update(&cfg_rc, inputs),
        "identical inputs must not reshape a single run"
    );
}

/// A price that ticks inside the printed rounding changes the INPUTS but not the drawn text, and
/// must not repaint the pane.
#[test]
fn a_tick_below_the_printed_precision_does_not_change_the_caption() {
    let cfg = cfg_of(&[ChartLabelField::Delta24h]);
    let mut state = LabelState::default();
    let mut inputs = LabelInputs {
        delta_24h: Some(1.234),
        ..Default::default()
    };
    let cfg_rc = Rc::new(cfg.clone());
    assert!(state.update(&cfg_rc, inputs.clone()));
    inputs.delta_24h = Some(1.2341);
    assert!(
        !state.update(&cfg_rc, inputs),
        "the same rounded text must report no change"
    );
}

#[test]
fn a_signed_figure_carries_its_sign_for_coloring() {
    let cfg = cfg_of(&[ChartLabelField::Delta1h]);
    let mut state = LabelState::default();
    let cfg_rc = Rc::new(cfg.clone());
    state.update(
        &cfg_rc,
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
    let cfg = cfg_of(&[ChartLabelField::Coin]);
    let mut state = LabelState::default();
    let cfg_rc = Rc::new(cfg.clone());
    state.update(
        &cfg_rc,
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

/// The basis is per CAPTION, so two captions on one chart can report different sets of orders.
#[test]
fn two_captions_can_read_different_bases() {
    let live = order(100.0, 110.0);
    let mut emu = order(100.0, 130.0);
    emu.emulator = true;
    emu.uid = 2;
    let inputs = inputs_with(&[live, emu]);
    let mut cfg = cfg_of(&[ChartLabelField::OpenPnlPct, ChartLabelField::OpenPnlPct]);
    cfg.rows[0].parts[0].pnl_basis = PnlBasis::Real;
    cfg.rows[0].parts[1].pnl_basis = PnlBasis::Emulator;
    let texts = texts_of(&cfg, inputs);
    assert_eq!(
        texts,
        vec!["PnL: +10.00%".to_string(), "PnL: +30.00%".to_string()]
    );
}

/// The caption flag is what turns a bare number into a labelled one.
#[test]
fn the_caption_flag_prefixes_the_field_name() {
    let mut cfg = cfg_of(&[ChartLabelField::Delta1h]);
    cfg.rows[0].parts[0].style.caption = Some(false);
    let bare = texts_of(
        &cfg,
        LabelInputs {
            delta_1h: Some(1.0),
            ..Default::default()
        },
    );
    cfg.rows[0].parts[0].style.caption = Some(true);
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
    let cfg = cfg_of(&[ChartLabelField::Quote]);
    let mut state = LabelState::default();
    let cfg_rc = Rc::new(cfg.clone());
    state.update(&cfg_rc, inputs);
    assert_eq!(state.texts[0].text, "USDT");
    assert_eq!(state.texts[0].sign, None);
}

/// A COIN-M contract carries no quote currency. Printing nothing is the honest answer; a guessed
/// "USD" would label money that is settled in the coin itself.
#[test]
fn a_market_without_a_quote_prints_nothing() {
    assert!(one_field(ChartLabelField::Quote, LabelInputs::default()).is_none());
}

/// The editor's sample line is built by the CHART's formatter, so a caption that prints nothing on
/// a real market still shows the reader what it would look like.
#[test]
fn the_preview_answers_for_every_field() {
    for field in ChartLabelField::ALL {
        let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        row.push_part(field);
        let captions = preview_row(&row);
        // One line for an ordinary caption; a column caption previews as the whole column, which is
        // the sample roster's two venues.
        let expected = if field.is_column() { 2 } else { 1 };
        assert_eq!(
            captions.len(),
            expected,
            "{field:?} printed the wrong number of lines in the preview"
        );
        assert!(
            captions.iter().all(|c| !c.text.trim().is_empty()),
            "{field:?} is blank"
        );
    }
}

/// A hidden caption is absent from the sample too: the preview answers "what will the chart print",
/// not "what is configured".
#[test]
fn the_preview_skips_a_hidden_caption_and_prints_the_name() {
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.push_part(ChartLabelField::Delta1h);
    row.push_part(ChartLabelField::Delta24h);
    row.parts[1].visible = false;
    row.name = "Дельты".to_string();
    row.show_name = true;
    let captions = preview_row(&row);
    assert_eq!(captions.len(), 2, "the name and the one visible caption");
    assert_eq!(captions[0].text, "Дельты");
    assert_eq!(
        captions[1].sign,
        Some(DeltaSign::Positive),
        "and a signed figure carries its sign, which is what colours it"
    );
}

/// The popup's eye switches a WHOLE module off, and the chart is where that has to be true: the
/// gates in the model only decide what the sync paths collect, not what the text pass prints.
#[test]
fn a_hidden_module_prints_nothing_at_all() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.push_part(ChartLabelField::Coin);
    row.name = "Инструмент".to_string();
    row.show_name = true;
    row.visible = false;
    cfg.rows[0] = row;
    let cfg_rc = Rc::new(cfg);
    let mut state = LabelState::default();
    state.update(
        &cfg_rc,
        LabelInputs {
            ticker: "BTCUSDT".into(),
            ..Default::default()
        },
    );
    assert!(
        state.texts.is_empty(),
        "a hidden module prints neither its captions nor its name"
    );
    assert!(
        preview_row(&cfg_rc.rows[0]).is_empty(),
        "and the editor's sample says the same"
    );
}
