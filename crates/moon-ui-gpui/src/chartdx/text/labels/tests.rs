//! Unit checks for caption resolution and the open-order figures behind it.
//!
//! Explicit imports throughout: the chartdx parent re-exports `gpui::*`, whose own `test` shadows
//! the built-in attribute and makes `#[test]` expand recursively.

use std::rc::Rc;

use moon_core::config::{
    ArbViewCfg, ChartLabelField, ChartLabelPart, ChartLabelRow, ChartLabelsCfg, LabelAlign,
    LabelZone, PnlBasis,
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
    state.update(
        &Rc::new(cfg.clone()),
        &Rc::new(ArbViewCfg::default()),
        inputs,
    );
    // Prefix and value are stored apart and drawn apart; a test that reads what the chart SHOWS has
    // to put them back together, which is also what the non-split drawing path does.
    state
        .texts
        .iter()
        .map(|t| format!("{}{}", t.prefix, t.text))
        .collect()
}

/// Like [`texts_of`], for a column whose roster is not the shipped one.
fn texts_with(cfg: &ChartLabelsCfg, pair: (LabelInputs, Rc<ArbViewCfg>)) -> Vec<String> {
    let (inputs, view) = pair;
    let mut state = LabelState::default();
    state.update(&Rc::new(cfg.clone()), &view, inputs);
    state
        .texts
        .iter()
        .map(|t| format!("{}{}", t.prefix, t.text))
        .collect()
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

/// The core ends every detect line with the strategy that fired it. That is the ONE thing this
/// caption does not need — the strategy has a caption of its own — and it is the widest part of the
/// line, so it goes.
#[test]
fn a_detect_line_drops_the_strategy_tail() {
    let text = one_field(
        ChartLabelField::DetectMsg,
        LabelInputs {
            detect_msg: "[SpreadDetection: TD: 43%  dP: 1.1%] (strategy <SP_L_D3h20>)".into(),
            ..Default::default()
        },
    )
    .expect("prints");
    assert!(!text.contains("strategy <"), "{text:?}");
    assert!(text.ends_with("dP: 1.1%]"), "{text:?}");

    // A line that was ONLY the tail keeps its kind and loses the colon that introduced nothing.
    let bare = one_field(
        ChartLabelField::DetectMsg,
        LabelInputs {
            detect_msg: "MoonStrike: (strategy <STRIKE_13_L>)".into(),
            ..Default::default()
        },
    )
    .expect("prints");
    assert!(bare.ends_with("MoonStrike"), "{bare:?}");

    // Mid-sentence, it is part of what the detect said and stays — including when the line happens
    // to end in the same two characters the tail does.
    for line in [
        "(strategy <A>) fired twice",
        "see (strategy <A>) below (x>)",
    ] {
        let inline = one_field(
            ChartLabelField::DetectMsg,
            LabelInputs {
                detect_msg: line.into(),
                ..Default::default()
            },
        )
        .expect("prints");
        assert!(inline.ends_with(line), "{inline:?}");
    }

    // A line that was NOTHING but the tail prints nothing at all. An empty caption is not an empty
    // string: it would still open its module's line and reserve its plate.
    assert_eq!(
        one_field(
            ChartLabelField::DetectMsg,
            LabelInputs {
                detect_msg: "(strategy <A>)".into(),
                ..Default::default()
            },
        ),
        None
    );
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
    assert!(text.chars().count() <= 201, "{}", text.chars().count());
    assert!(text.ends_with('…'));
}

/// The prefix and the value are kept APART, because only the value takes the colour. Gluing them
/// into one string is what made "Фандинг: +3.90%" a block of green.
#[test]
fn a_caption_keeps_its_prefix_beside_its_value() {
    let cfg = cfg_of(&[ChartLabelField::Funding]);
    let mut state = LabelState::default();
    state.update(&Rc::new(cfg), &Rc::new(ArbViewCfg::default()), LabelInputs {
            context: Some(moon_core::market::MarketContextReadout {
                funding_pct: Some(3.9),
                funding_at_ms: Some(1),
                ..Default::default()
            }),
            ..Default::default()
        },
    );

    let entry = state.texts.first().expect("funding prints");
    assert_eq!(entry.text, "+3.90%", "the value carries no prefix");
    assert!(entry.prefix.ends_with(": "), "{:?}", entry.prefix);
    assert!(entry.sign.is_some(), "and it is coloured by its sign");
}

/// Below the threshold a by-sign caption keeps its FIGURE and loses its colour — a column of
/// hundredths of a percent painted red and green is noise, and hiding the value would be worse.
#[test]
fn a_figure_below_the_colour_threshold_prints_without_a_sign() {
    let mut cfg = cfg_of(&[ChartLabelField::Delta1h]);
    cfg.rows[0].parts[0].style.color_min_pct = Some(1.0);
    let quiet = LabelInputs {
        delta_1h: Some(0.4),
        ..Default::default()
    };
    let loud = LabelInputs {
        delta_1h: Some(1.4),
        ..Default::default()
    };

    let mut state = LabelState::default();
    state.update(
        &Rc::new(cfg.clone()),
        &Rc::new(ArbViewCfg::default()),
        quiet,
    );
    let entry = state.texts.first().expect("prints");
    assert_eq!(entry.text, "+0.40%", "the value is still there");
    assert!(entry.sign.is_none(), "but it is not painted");

    let mut state = LabelState::default();
    state.update(&Rc::new(cfg), &Rc::new(ArbViewCfg::default()), loud);
    assert!(state.texts[0].sign.is_some(), "past the threshold it is");
}

// --- the arbitrage column -------------------------------------------------------------------------

/// A quote from `venue` at `price`, against a market trading at 100.
fn arb_quote(code: u8, price: f64) -> moon_core::market::ArbQuote {
    moon_core::market::ArbQuote {
        venue: moon_core::market::ArbVenue::from_code(code),
        dex_name: String::new(),
        price,
        my_price: 100.0,
        spread_pct: price - 100.0,
        deposit_blocked: false,
        withdraw_blocked: false,
    }
}

/// Inputs holding a column's quotes, paired with the roster they are arranged by.
///
/// The roster is a separate cache key on `LabelState`, not an input — it is compared by pointer —
/// so a test hands it alongside rather than inside.
fn arb_inputs(
    quotes: Vec<moon_core::market::ArbQuote>,
    view: ArbViewCfg,
) -> (LabelInputs, Rc<ArbViewCfg>) {
    (
        LabelInputs {
            arb: quotes,
            ..Default::default()
        },
        Rc::new(view),
    )
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

    let texts = texts_with(
        &cfg,
        arb_inputs(vec![arb_quote(4, 101.0), arb_quote(9, 99.0)], view),
    );

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
    let (inputs, view) = arb_inputs(
        vec![arb_quote(4, 101.0), arb_quote(9, 99.0)],
        ArbViewCfg::default(),
    );
    let mut state = LabelState::default();
    state.update(&Rc::new(cfg), &view, inputs);

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
    let price_only = texts_with(&cfg, arb_inputs(vec![arb_quote(4, 101.0)], view.clone()));
    assert!(!price_only[0].contains('%'), "{:?}", price_only[0]);

    view.show = moon_core::config::ArbShow::Spread;
    let spread_only = texts_with(&cfg, arb_inputs(vec![arb_quote(4, 101.0)], view));
    assert!(spread_only[0].contains("+1.00%"), "{:?}", spread_only[0]);
    assert!(!spread_only[0].contains("101"), "{:?}", spread_only[0]);
}

/// The venue's NAME is the line's prefix, so "colour the value only" paints the price and the
/// spread while the venue itself stays readable.
#[test]
fn the_venue_name_is_the_lines_prefix() {
    let cfg = cfg_of(&[ChartLabelField::ArbColumn]);
    let texts = {
        let (inputs, view) = arb_inputs(vec![arb_quote(4, 101.0)], ArbViewCfg::default());
        let mut state = LabelState::default();
        state.update(&Rc::new(cfg), &view, inputs);
        state.texts.clone()
    };

    let line = texts.first().expect("one venue prints");
    assert_eq!(line.prefix, "BinanceF ", "the reference terminal's spelling");
    assert!(line.text.contains("+1.00%"), "{:?}", line.text);
    assert!(!line.text.contains("BinanceF"), "{:?}", line.text);
}

/// The column is a TABLE: venue names left-aligned in one column, prices and percentages
/// right-aligned in theirs, so the decimal points stand under each other. The chart draws captions
/// in a monospaced face, so padding with spaces is exact — and it is what the reference terminal's
/// own column looks like.
#[test]
fn the_column_lines_its_cells_up() {
    let cfg = cfg_of(&[ChartLabelField::ArbColumn]);
    let mut view = ArbViewCfg::default();
    view.venues = vec![
        moon_core::config::ArbVenueCfg::new(moon_core::market::ArbVenue::from_code(4)),
        moon_core::config::ArbVenueCfg::new(moon_core::market::ArbVenue::from_code(101)),
    ];
    // "BinanceF" is eight characters and "UpBit" five; the prices differ in width too.
    let quotes = vec![arb_quote(4, 9.5), arb_quote(101, 101.25)];

    let (inputs, view) = arb_inputs(quotes, view);
    let mut state = LabelState::default();
    state.update(&Rc::new(cfg), &view, inputs);

    let lines: Vec<_> = state.texts.iter().collect();
    assert_eq!(lines.len(), 2);
    let name_widths: Vec<usize> = lines.iter().map(|l| l.prefix.chars().count()).collect();
    assert_eq!(
        name_widths[0], name_widths[1],
        "the name column is one width: {:?}",
        lines.iter().map(|l| l.prefix.clone()).collect::<Vec<_>>()
    );
    assert!(lines[0].prefix.starts_with("BinanceF"), "{:?}", lines[0].prefix);
    assert!(lines[1].prefix.starts_with("UpBit "), "{:?}", lines[1].prefix);

    let value_widths: Vec<usize> = lines.iter().map(|l| l.text.chars().count()).collect();
    assert_eq!(
        value_widths[0], value_widths[1],
        "price and percent columns are one width each: {:?}",
        lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>()
    );
}

/// The roster's floor shortens the COLUMN: a dozen venues quoting the same price are a dozen lines
/// of nothing, and the reader asked to see only what moved.
#[test]
fn the_roster_floor_drops_quiet_venues() {
    let cfg = cfg_of(&[ChartLabelField::ArbColumn]);
    let mut view = ArbViewCfg {
        min_abs_pct: 1.0,
        ..ArbViewCfg::default()
    };
    view.venues = vec![
        moon_core::config::ArbVenueCfg::new(moon_core::market::ArbVenue::from_code(4)),
        moon_core::config::ArbVenueCfg::new(moon_core::market::ArbVenue::from_code(9)),
    ];

    let texts = texts_with(
        &cfg,
        arb_inputs(vec![arb_quote(4, 100.5), arb_quote(9, 102.0)], view),
    );

    assert_eq!(texts.len(), 1, "only the venue past the floor prints");
    assert!(texts[0].starts_with("GateF"), "{:?}", texts[0]);
}

/// A venue that cannot be deposited to or withdrawn from is MARKED, not hidden: the spread is real
/// and the settlement is not, and a reader must not take one for the other.
#[test]
fn a_blocked_venue_is_marked() {
    let cfg = cfg_of(&[ChartLabelField::ArbColumn]);
    let mut quote = arb_quote(4, 101.0);
    quote.withdraw_blocked = true;

    let marked = texts_with(&cfg, arb_inputs(vec![quote.clone()], ArbViewCfg::default()));
    assert!(marked[0].contains('⛔'), "{:?}", marked[0]);

    let off = ArbViewCfg {
        mark_blocked: false,
        ..ArbViewCfg::default()
    };
    let unmarked = texts_with(&cfg, arb_inputs(vec![quote], off));
    assert!(!unmarked[0].contains('⛔'), "{:?}", unmarked[0]);
}

/// With no quotes the column prints nothing at all — not an empty line, and not a heading with no
/// rows under it.
#[test]
fn a_market_with_no_arbitrage_prints_nothing() {
    let cfg = cfg_of(&[ChartLabelField::ArbColumn]);
    let texts = texts_with(&cfg, arb_inputs(Vec::new(), ArbViewCfg::default()));
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
        vec![
            "BTCUSDT".to_string(),
            "Core-1".to_string(),
            // The two volume blocks head themselves whatever the market says: the period is a
            // SETTING, and a heading that vanished would take the right-click target with it.
            "1m".to_string(),
            "Cursor 10s".to_string(),
            // And the MEASURING block prints dashes rather than nothing, so it keeps its shape
            // while the pointer is off the plot. The live block beside it simply has no figures
            // yet and prints none.
            "Bv: —".to_string(),
            "Sv: —".to_string(),
            "L: —".to_string(),
        ],
        "no scale badge and no comparison delta still means those two cost no captions"
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
    let view_rc = Rc::new(ArbViewCfg::default());
    state.update(&cfg_rc, &view_rc, inputs);
    let addresses: Vec<(usize, usize)> = state.texts.iter().map(|t| (t.row, t.part)).collect();
    assert_eq!(
        addresses,
        vec![
            (0, 0),
            (0, 1),
            (2, 0),
            // The volume headings, and the measuring block's four lines.
            (3, 0),
            (4, 0),
            (4, 1),
            (4, 2),
            (4, 3)
        ],
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
    let view_rc = Rc::new(ArbViewCfg::default());
    assert!(
        state.update(&cfg_rc, &view_rc, inputs.clone()),
        "the first pass formats"
    );
    assert!(
        !state.update(&cfg_rc, &view_rc, inputs),
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
    let view_rc = Rc::new(ArbViewCfg::default());
    assert!(state.update(&cfg_rc, &view_rc, inputs.clone()));
    inputs.delta_24h = Some(1.2341);
    assert!(
        !state.update(&cfg_rc, &view_rc, inputs),
        "the same rounded text must report no change"
    );
}

#[test]
fn a_signed_figure_carries_its_sign_for_coloring() {
    let cfg = cfg_of(&[ChartLabelField::Delta1h]);
    let mut state = LabelState::default();
    let cfg_rc = Rc::new(cfg.clone());
    let view_rc = Rc::new(ArbViewCfg::default());
    state.update(
        &cfg_rc,
        &view_rc,
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
    let view_rc = Rc::new(ArbViewCfg::default());
    state.update(
        &cfg_rc,
        &view_rc,
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
        Some("Open: +10.00%"),
        "the open-order default carries its caption"
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
        vec!["Open: +10.00%".to_string(), "Open: +30.00%".to_string()]
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
///
/// Elapsed by more than the caption's own minute, because that is the finest state it can observe:
/// the clock it is handed is quantized to the minute it prints, so "one millisecond past" is not a
/// moment this caption can be in — and asserting on one would be asserting on a resolution the
/// caption deliberately does not have.
#[test]
fn an_elapsed_funding_time_prints_nothing() {
    let inputs = LabelInputs {
        context: Some(ctx()),
        now_ms: FUNDING_AT_MS + 60_001,
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
    let view_rc = Rc::new(ArbViewCfg::default());
    state.update(&cfg_rc, &view_rc, inputs);
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
        let captions = preview_row(&row, TF_5M);
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
    let captions = preview_row(&row, TF_5M);
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
    let view_rc = Rc::new(ArbViewCfg::default());
    let mut state = LabelState::default();
    state.update(
        &cfg_rc,
        &view_rc,
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
        preview_row(&cfg_rc.rows[0], TF_5M).is_empty(),
        "and the editor's sample says the same"
    );
}

/// The core's own per-coin counter is left at zero on part of its venues while MoonBot shows a
/// figure there, so a zero is "the core said nothing", not "traded to break even". Judged on the
/// ROUNDED value: a coin-margined core reports fractions of a BTC, which two decimals cannot show.
#[test]
fn a_zero_core_pnl_prints_nothing() {
    let figures_with = |core_pnl: Option<f64>| LabelInputs {
        figures: Some(moon_core::market::MarketFiguresReadout {
            core_pnl,
            ..Default::default()
        }),
        ..Default::default()
    };

    assert!(
        one_field(ChartLabelField::SessionPnl, figures_with(Some(0.0))).is_none(),
        "an exact zero prints nothing"
    );
    assert!(
        one_field(ChartLabelField::SessionPnl, figures_with(Some(0.0004))).is_none(),
        "and so does an amount that rounds away"
    );
    assert!(
        one_field(ChartLabelField::SessionPnl, figures_with(None)).is_none(),
        "an absent counter prints nothing either"
    );
    let text = one_field(ChartLabelField::SessionPnl, figures_with(Some(-12.4)))
        .expect("a real amount prints");
    assert!(text.ends_with("-12.4"), "{text:?}");
}

/// The core's Session counter is the opposite case: the core states it as a snapshot of its own,
/// so a zero MEANS "nothing since the reset" and only absence — a core too old to publish it at
/// all — leaves the caption unprinted. The two must not be collapsed into one rule.
#[test]
fn a_zero_session_still_prints_but_an_absent_one_does_not() {
    let figures_with = |session: Option<f64>| LabelInputs {
        figures: Some(moon_core::market::MarketFiguresReadout {
            session,
            ..Default::default()
        }),
        ..Default::default()
    };

    let zero = one_field(ChartLabelField::SessionProfit, figures_with(Some(0.0)))
        .expect("a stated zero prints");
    assert!(zero.ends_with('0') && !zero.contains('+'), "{zero:?}");
    let dust = one_field(ChartLabelField::SessionProfit, figures_with(Some(-0.004)))
        .expect("an amount that rounds away still prints its magnitude");
    assert!(
        !dust.contains('+') && !dust.contains('-'),
        "a loss rounding to zero must not wear a plus: {dust:?}"
    );
    assert!(
        one_field(ChartLabelField::SessionProfit, figures_with(None)).is_none(),
        "a core that publishes no session profit prints nothing"
    );
    let text = one_field(ChartLabelField::SessionProfit, figures_with(Some(48.15)))
        .expect("a real amount prints");
    assert!(text.ends_with("+48.15"), "{text:?}");
}

/// A period's readout, as the sync path hands one over.
fn volumes(readout: moon_core::market::VolumeSpanReadout) -> LabelInputs {
    LabelInputs {
        volumes: vec![(
            (
                moon_core::market::VolumeSpan::Millis(
                    moon_core::config::LabelWindow::default().millis(),
                ),
                moon_core::market::VolumeAt::Now,
            ),
            readout,
        )],
        ..Default::default()
    }
}

/// A whole reading of a lopsided minute: three quarters bought.
fn sample_readout() -> moon_core::market::VolumeSpanReadout {
    moon_core::market::VolumeSpanReadout {
        buy_quote: 12_000.0,
        sell_quote: 4_000.0,
        buy_base: 0.6,
        sell_base: 0.2,
        trades: 418,
        complete: true,
        base_exact: true,
        total_quote_candles: None,
    }
}

/// The block's three figures must come from ONE reading: `Bv + Sv` is what `Vol` prints.
///
/// Breakage: a total from a second source — which is where it used to come from — lets a chart
/// print `Bv 12k`, `Sv 4k` and a total that is neither their sum nor anybody's number.
#[test]
fn the_volume_total_is_the_sum_of_the_two_sides() {
    let inputs = volumes(sample_readout());
    let bought = one_field(ChartLabelField::WindowBuyVolume, inputs.clone()).expect("prints");
    let sold = one_field(ChartLabelField::WindowSellVolume, inputs.clone()).expect("prints");
    let total = one_field(ChartLabelField::WindowVolume, inputs.clone()).expect("prints");
    let share = one_field(ChartLabelField::WindowBuyShare, inputs).expect("prints");

    assert!(bought.contains("12"), "{bought:?}");
    assert!(sold.contains("4"), "{sold:?}");
    assert!(total.contains("16"), "12k bought plus 4k sold is 16k: {total:?}");
    assert!(share.contains("75"), "{share:?}");
}

/// A period the retained history does not cover is MARKED, never hidden and never presented whole.
///
/// Breakage: a chart that says `Bv 12.7k` over a minute it has ten seconds of is stating a figure
/// the terminal does not have — and the reader has no way to see that from the caption.
#[test]
fn an_uncovered_period_is_marked() {
    let short = moon_core::market::VolumeSpanReadout {
        complete: false,
        ..sample_readout()
    };
    let text = one_field(ChartLabelField::WindowBuyVolume, volumes(short)).expect("still prints");
    assert!(text.contains('~'), "{text:?}");

    let whole =
        one_field(ChartLabelField::WindowBuyVolume, volumes(sample_readout())).expect("prints");
    assert!(!whole.contains('~'), "{whole:?}");
}

/// A coin figure over a stretch served by mini-candles is marked too: those rows carry a value and
/// no quantity, so the coin amount is short by exactly that stretch.
#[test]
fn a_coin_figure_is_marked_when_the_aggregates_answered() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.push_part(ChartLabelField::WindowBuyVolume);
    row.parts[0].units = moon_core::config::VolumeUnits::Base;
    cfg.rows[0] = row;

    let inexact = moon_core::market::VolumeSpanReadout {
        base_exact: false,
        ..sample_readout()
    };
    let text = texts_of(&cfg, volumes(inexact)).remove(0);
    assert!(text.contains('~'), "{text:?}");

    // The same caption in MONEY is whole: the quote halves are exact over every span.
    let mut money = cfg.clone();
    money.rows[0].parts[0].units = moon_core::config::VolumeUnits::Quote;
    let text = texts_of(&money, volumes(inexact)).remove(0);
    assert!(!text.contains('~'), "{text:?}");
}

/// The heading prints the PERIOD, and prints it before any history has arrived.
///
/// Breakage: a heading that waited for data would take the right-click target with it — the menu
/// that sets the period would be unreachable exactly while the period is unset.
#[test]
fn the_heading_names_the_period_without_any_reading() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.push_part(ChartLabelField::WindowSpanName);
    cfg.rows[0] = row;

    let text = texts_of(&cfg, LabelInputs::default()).remove(0);
    assert!(!text.is_empty(), "the heading prints with no volumes at all");

    let mut trades = cfg.clone();
    trades.rows[0].parts[0].span = moon_core::config::LabelSpan::Trades(500);
    let text = texts_of(&trades, LabelInputs::default()).remove(0);
    assert!(text.contains("500"), "a trade span names its count: {text:?}");
}

/// The bar states the side's share of the whole, and a silent market draws none.
///
/// Breakage: a full bar over a market that did not trade reads as "all buying", and a share taken
/// from the COIN halves would disagree with the bar on the line above whenever the two sides traded
/// at different prices.
#[test]
fn the_bar_is_the_side_s_share_and_a_silent_market_has_none() {
    let cfg = cfg_of(&[ChartLabelField::WindowBuyVolume]);
    let cfg_rc = Rc::new(cfg);
    let mut state = LabelState::default();
    state.update(
        &cfg_rc,
        &Rc::new(ArbViewCfg::default()),
        volumes(sample_readout()),
    );
    let bar = state.texts[0].bar.expect("a buy caption draws a bar");
    assert!(!bar.sell);
    assert!((bar.fill - 0.75).abs() < 0.001, "{:?}", bar.fill);

    let silent = moon_core::market::VolumeSpanReadout {
        buy_quote: 0.0,
        sell_quote: 0.0,
        ..sample_readout()
    };
    let mut state = LabelState::default();
    state.update(&cfg_rc, &Rc::new(ArbViewCfg::default()), volumes(silent));
    assert!(
        state.texts.first().and_then(|t| t.bar).is_none(),
        "nothing traded, so there is nothing to compare"
    );
}

/// On a caption that reads a period, the prefix switch governs the PERIOD and nothing else.
///
/// Breakage: `Bv` is not decoration on that line — it says which side the figure is. Dropping it
/// with the period leaves two bare numbers under one heading, and the reader cannot tell the buying
/// half from the selling one. Switching the period off is what a reader asks for once the block's
/// heading already states it.
#[test]
fn the_prefix_switch_drops_the_period_and_keeps_the_name() {
    let mut part = ChartLabelPart::new(ChartLabelField::WindowBuyVolume);
    part.window = moon_core::config::LabelWindow::M5;

    let with = super::caption_prefix(&part, true, TF_5M);
    let without = super::caption_prefix(&part, false, TF_5M);

    assert!(
        with.starts_with("Bv"),
        "the side is named either way: {with:?}"
    );
    assert!(
        with.contains('5'),
        "the period is spelled when it is on: {with:?}"
    );
    assert!(
        without.starts_with("Bv"),
        "the side survives the switch: {without:?}"
    );
    assert!(
        !without.contains('5'),
        "the period is what the switch removes: {without:?}"
    );
}

/// A caption that reads NO period keeps the old meaning: the switch prints its caption or nothing.
#[test]
fn the_prefix_switch_still_removes_a_plain_caption() {
    let part = ChartLabelPart::new(ChartLabelField::OpenOrders);
    assert!(!super::caption_prefix(&part, true, TF_5M).is_empty());
    assert!(super::caption_prefix(&part, false, TF_5M).is_empty());
}

/// The five-minute chart every countdown test measures against.
const TF_5M: i64 = 5 * 60_000;

/// Candle buckets are floored on the Unix epoch, so a countdown is pure clock arithmetic: on a
/// boundary the candle has just opened and the FULL period remains. Zero is never printed — it
/// would claim a candle that closes and never reopens.
#[test]
fn a_candle_countdown_on_a_boundary_states_the_full_period() {
    let inputs = LabelInputs {
        now_ms: 0,
        chart_tf_ms: TF_5M,
        ..Default::default()
    };
    let text = one_field(ChartLabelField::TfCloseIn, inputs).expect("prints");
    let expected = format!(
        "5{} 00{}",
        rust_i18n::t!("chart_labels.unit_minute"),
        rust_i18n::t!("chart_labels.unit_second")
    );
    assert!(text.ends_with(&expected), "{text:?}");
}

/// Three steps, because the figure a reader needs changes with the distance: seconds are noise an
/// hour out and minutes alone are useless in the last one.
#[test]
fn the_candle_countdown_steps_from_hours_down_to_seconds() {
    let at = |remaining_ms: i64, tf_ms: i64| {
        let inputs = LabelInputs {
            // A boundary minus the remainder: the bucket grid starts at the epoch.
            now_ms: tf_ms - remaining_ms,
            chart_tf_ms: tf_ms,
            ..Default::default()
        };
        one_field(ChartLabelField::TfCloseIn, inputs).expect("prints")
    };
    let (h, m, s) = (
        rust_i18n::t!("chart_labels.unit_hour"),
        rust_i18n::t!("chart_labels.unit_minute"),
        rust_i18n::t!("chart_labels.unit_second"),
    );
    let day = 24 * 3_600_000;
    assert!(
        at(2 * 3_600_000 + 5 * 60_000, day).ends_with(&format!("2{h} 05{m}")),
        "past an hour the seconds are dropped"
    );
    assert!(
        at(47 * 60_000 + 3_000, 3_600_000).ends_with(&format!("47{m} 03{s}")),
        "inside the hour both halves are printed"
    );
    assert!(
        at(42_000, TF_5M).ends_with(&format!("42{s}")),
        "the last minute is seconds alone"
    );
}

/// Rounded UP: the last second reads `1с` and then the candle rolls. Rounding down would print a
/// zero for a whole second, which reads as a stopped chart.
#[test]
fn the_candle_countdown_rounds_a_part_second_up() {
    let inputs = LabelInputs {
        now_ms: TF_5M - 1,
        chart_tf_ms: TF_5M,
        ..Default::default()
    };
    let text = one_field(ChartLabelField::TfCloseIn, inputs).expect("prints");
    let expected = format!("1{}", rust_i18n::t!("chart_labels.unit_second"));
    assert!(text.ends_with(&expected), "{text:?}");
}

/// The prefix names the TIMEFRAME, resolved: two countdowns on one chart are unreadable otherwise,
/// and printing the word `Авто` would name the setting rather than the period it currently means.
#[test]
fn the_candle_countdown_prefix_names_the_resolved_timeframe() {
    let auto = ChartLabelPart::new(ChartLabelField::TfCloseIn);
    let hour = ChartLabelPart {
        tf: moon_core::config::LabelTf::H1,
        ..auto
    };
    let auto_prefix = super::caption_prefix(&auto, false, TF_5M);
    assert!(
        auto_prefix.contains(&*rust_i18n::t!("chart_labels.tf.m5")),
        "auto follows the chart: {auto_prefix:?}"
    );
    assert!(
        !auto_prefix.contains(&*rust_i18n::t!("chart_labels.tf.auto")),
        "the setting is not the period: {auto_prefix:?}"
    );
    let hour_prefix = super::caption_prefix(&hour, false, TF_5M);
    assert!(
        hour_prefix.contains(&*rust_i18n::t!("chart_labels.tf.h1")),
        "a fixed timeframe ignores the chart: {hour_prefix:?}"
    );

    // The switch drops the WORD and keeps the period: the period is what tells two countdowns
    // apart, which is the mirror of the window rule and the reason for it.
    let named = super::caption_prefix(&hour, true, TF_5M);
    assert!(
        named.contains(&*rust_i18n::t!("chart_labels.tf.h1")),
        "{named:?}"
    );
    assert!(
        named.len() > hour_prefix.len(),
        "{named:?} vs {hour_prefix:?}"
    );
}

/// The SAME instant must print the same figure whichever step the clock is on. The quantum is a
/// cost control; it is not allowed to change the number, and a caption that reads differently
/// depending on which OTHER caption shares its chart is a caption nobody can trust.
#[test]
fn a_candle_countdown_reads_the_same_on_either_clock_step() {
    let day: i64 = 24 * 3_600_000;
    // Ten hours and thirty seconds before the daily candle closes, on a boundary-anchored grid.
    let now: i64 = day - (10 * 3_600_000 + 30_000);
    let at = |quantum: i64| {
        let inputs = LabelInputs {
            now_ms: now.div_euclid(quantum) * quantum,
            chart_tf_ms: day,
            ..Default::default()
        };
        one_field(ChartLabelField::TfCloseIn, inputs).expect("prints")
    };
    assert_eq!(
        at(1_000),
        at(60_000),
        "the second step and the minute step must agree"
    );
}

/// The funding countdown must not move because a CANDLE countdown was added beside it. Funding's
/// target is not on the bucket grid, so a finer shared clock shifts the minute it prints rather
/// than sharpening it — the caption reads its own minute regardless of which step is in force.
#[test]
fn the_funding_countdown_ignores_the_finer_shared_clock() {
    let at = |now_ms: i64| {
        one_field(
            ChartLabelField::FundingIn,
            LabelInputs {
                context: Some(ctx()),
                now_ms,
                chart_tf_ms: TF_5M,
                ..Default::default()
            },
        )
    };
    // The same instant, quantized to the minute and to the second: one figure either way.
    let both_steps_agree = |true_now: i64| {
        assert_eq!(
            at(true_now.div_euclid(60_000) * 60_000),
            at(true_now.div_euclid(1_000) * 1_000),
            "the two clock steps disagree at {true_now}"
        );
    };
    both_steps_agree(FUNDING_AT_MS - (47 * 60_000 + 30_000));
    // And at the boundary, where the caption stops printing: the step must not decide the instant
    // the caption disappears either, which is the half of this the figure test alone would miss.
    both_steps_agree(FUNDING_AT_MS - 30_000);
    both_steps_agree(FUNDING_AT_MS + 30_000);
}

/// The trade captions answer from the HANDED trade and from nothing else.
///
/// A live chart is never handed one, so the three fields print nothing there rather than falling
/// back to what the market is doing now — which is the whole point of them being their own fields
/// instead of a mode of the detect captions beside them.
#[test]
fn the_trade_captions_print_nothing_without_a_trade() {
    for field in [
        ChartLabelField::TradeStrategy,
        ChartLabelField::TradeDetect,
        ChartLabelField::TradeSellReason,
    ] {
        assert!(
            one_field(
                field,
                LabelInputs {
                    // Everything a LIVE chart would have, and no trade: the captions must not
                    // borrow any of it.
                    detect_strategy: "BTC Sniper".into(),
                    detect_msg: "Delta 5m 3.4%".into(),
                    strategy: "Alpha".into(),
                    ..Default::default()
                }
            )
            .is_none(),
            "{field:?} printed without a trade"
        );
    }
}

/// Handed a trade, each caption states its own half of it, and the strategy names itself: a chart
/// can print the strategy that fired, the one holding an order and the one that owns this trade.
#[test]
fn a_handed_trade_states_its_strategy_line_and_exit() {
    let handed = |trade: crate::chartdx::TradeLabels| LabelInputs {
        trade: Some(Rc::new(trade)),
        ..Default::default()
    };
    let trade = crate::chartdx::TradeLabels {
        strategy: "Hook Short".into(),
        detect: "Hook Short Depth: 2.47% R: 120%".into(),
        sell_reason: "Auto Price Down".into(),
    };

    let strategy =
        one_field(ChartLabelField::TradeStrategy, handed(trade.clone())).expect("prints");
    assert!(strategy.ends_with("Hook Short"), "{strategy:?}");
    assert!(strategy.contains(": "), "it names itself: {strategy:?}");

    let detect = one_field(ChartLabelField::TradeDetect, handed(trade.clone())).expect("prints");
    assert_eq!(detect, "Hook Short Depth: 2.47% R: 120%");

    let exit = one_field(ChartLabelField::TradeSellReason, handed(trade)).expect("prints");
    assert!(exit.ends_with("Auto Price Down"), "{exit:?}");

    // A trade the replica had nothing to say about prints nothing, rather than an empty plate.
    assert!(
        one_field(
            ChartLabelField::TradeDetect,
            handed(crate::chartdx::TradeLabels::default())
        )
        .is_none()
    );
}

/// The trade's detect line is PROSE and wraps like the live one, instead of being cut at the first
/// module boundary: it is the widest thing this chart prints.
#[test]
fn the_trade_detect_line_wraps_like_the_live_one() {
    assert!(ChartLabelField::TradeDetect.wraps());
    assert!(!ChartLabelField::TradeStrategy.wraps());
    assert!(!ChartLabelField::TradeSellReason.wraps());
}
