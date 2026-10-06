//! Synthetic refresh measurements and input synchronization regressions.

use crate::chartdx::{ChartEngine, PaneRender};
use moon_core::config::{ChartLabelsCfg, ChartTheme};
use moon_core::market::{LiqSpanReadout, VolumeAt, VolumeSpan, VolumeSpanReadout};
use std::{hint::black_box, rc::Rc, time::Instant};

impl crate::chartdx::text::LabelState {
    /// Retain the owned-input adapter for formatter regressions without a production clone path.
    pub(in crate::chartdx) fn update(
        &mut self,
        cfg: &Rc<ChartLabelsCfg>,
        arb_view: &Rc<moon_core::config::ArbViewCfg>,
        inputs: crate::chartdx::text::LabelInputs,
    ) -> bool {
        self.update_with(cfg, arb_view, |held| {
            if *held == inputs {
                return false;
            }
            *held = inputs;
            true
        })
    }
}

/// Build one pane with the large input collections an unchanged refresh used to clone.
fn refresh_fixture() -> ChartEngine {
    let engine = ChartEngine::new(1_700_000_000_000.0, ChartTheme::default());
    {
        let data = engine.data.borrow();
        let mut state = data.render.borrow_mut();
        state.panes.push(PaneRender::new());
        let mut cfg = ChartLabelsCfg::empty();
        cfg.rows[0] = moon_core::config::strategy_filters_row();
        state.chart_labels = Rc::new(cfg);
        let pane = &mut state.panes[0];
        pane.filter_lines = (0..300)
            .map(|i| format!("synthetic strategy {i}: waiting for trigger"))
            .collect();
        pane.label_volumes = (1..=200)
            .map(|i| {
                (
                    (VolumeSpan::Trades(i), VolumeAt::Now),
                    VolumeSpanReadout {
                        trades: i,
                        ..Default::default()
                    },
                )
            })
            .collect();
        pane.label_liquidations = (1..=50)
            .map(|i| {
                (
                    (VolumeSpan::Trades(i), VolumeAt::Now),
                    LiqSpanReadout {
                        count: i,
                        ..Default::default()
                    },
                )
            })
            .collect();
        pane.label_arb = (0..40)
            .map(|i| moon_core::market::ArbQuote {
                venue: moon_core::market::ArbVenue::from_code(i),
                dex_name: format!("synthetic{i}"),
                price: 100.0,
                my_price: 99.0,
                spread_pct: 1.0,
                deposit_blocked: false,
                withdraw_blocked: false,
            })
            .collect();
        state.refresh_pane_labels(0);
    }
    engine
}

/// Measures the production refresh path for 10000 unchanged synthetic snapshots.
#[test]
#[ignore]
fn label_inputs_refresh_bench() {
    let _locale = crate::test_locale::force("en");
    let engine = refresh_fixture();
    let data = engine.data.borrow();
    let mut state = data.render.borrow_mut();
    let start = Instant::now();
    let mut changes = 0;
    for _ in 0..10_000 {
        changes += usize::from(black_box(state.refresh_pane_labels(0)));
    }
    assert_eq!(changes, 0);
    println!(
        "label_inputs_refresh_bench test profile: {:?}; refreshes=10000 changed={changes}",
        start.elapsed()
    );
}

/// An unchanged refresh must retain formatted storage and skip the rebuild generation.
#[test]
fn unchanged_refresh_reports_no_change_and_keeps_texts() {
    let _locale = crate::test_locale::force("en");
    let engine = refresh_fixture();
    let data = engine.data.borrow();
    let mut state = data.render.borrow_mut();
    let generation = state.panes[0].labels.generation;
    let pointer = state.panes[0].labels.texts.as_ptr();
    let texts = state.panes[0].labels.texts.clone();
    assert!(!state.refresh_pane_labels(0));
    assert_eq!(state.panes[0].labels.generation, generation);
    assert_eq!(state.panes[0].labels.texts.as_ptr(), pointer);
    assert_eq!(state.panes[0].labels.texts, texts);
}

/// Mutate each source independently so an omitted sync field leaves the generation unchanged.
fn change_input(state: &mut crate::chartdx::RenderState, field: usize) {
    let pane = &mut state.panes[0];
    match field {
        0 => pane.ticker = "SYNTH".into(),
        1 => pane.core_name = "synthetic core".into(),
        2 => pane.venue = "synthetic venue".into(),
        3 => pane.quote = "USDT".into(),
        4 => pane.label_strategy = "synthetic strategy".into(),
        5 => pane.label_detect_strategy = "synthetic detect".into(),
        6 => pane.label_detect_msg = "synthetic message".into(),
        7 => pane.filter_lines.push("another line".into()),
        8 => state.trade_labels = Some(Rc::new(crate::chartdx::TradeLabels::default())),
        9 => pane.cached_last_price = Some(100.0),
        10 => pane.scale_badge = Some(20),
        11 => pane.time_scale_s = Some(60),
        12 => {
            pane.orderbook_only = true;
            pane.cached_last_price = Some(100.0);
            state.compare_ref_price = Some(99.0);
        }
        13 => pane.delta_1h = Some(1.0),
        14 => pane.delta_24h = Some(2.0),
        15 => pane.label_context = Some(Default::default()),
        16 => pane.label_figures = Some(Default::default()),
        17 => pane.label_windows = Some(Default::default()),
        18 => pane.label_volumes[0].1.buy_quote = 42.0,
        19 => pane.label_liquidations[0].1.quote = 42.0,
        20 => pane.label_cursor_ms = Some(42),
        21 => pane.label_arb[0].dex_name = "changed synthetic dex".into(),
        22 => pane.label_arb_reachable.push((1, "synthetic dex".into())),
        23 => pane.label_now_ms += 1,
        24 => state.chart_tf_ms += 1,
        25 => pane.label_basis[0].open_orders = 1,
        26 => pane.label_actions.live = !pane.label_actions.live,
        27 => pane.label_scroll.push((0, 1)),
        _ => panic!("unknown input"),
    }
}

/// Input changes advance the row-plan revision only when their formatted captions differ.
#[test]
fn each_input_field_change_reports_change() {
    let _locale = crate::test_locale::force("en");
    for field in 0..28 {
        let engine = refresh_fixture();
        let data = engine.data.borrow();
        let mut state = data.render.borrow_mut();
        // Isolate comparison from its price prerequisite: field 12 changes only the anchor.
        if field == 12 {
            state.panes[0].orderbook_only = true;
            state.panes[0].cached_last_price = Some(100.0);
            state.refresh_pane_labels(0);
        }
        let before = state.panes[0].labels.generation;
        let texts = state.panes[0].labels.texts.clone();
        change_input(&mut state, field);
        state.refresh_pane_labels(0);
        let expected = before + u64::from(state.panes[0].labels.texts != texts);
        assert_eq!(state.panes[0].labels.generation, expected, "field {field}");
        assert!(!state.refresh_pane_labels(0), "repeat field {field}");
        assert_eq!(
            state.panes[0].labels.generation, expected,
            "repeat field {field}"
        );
    }
}

/// Rebuilding an unprinted input must preserve the revision and reuse the caption row plan.
#[test]
fn generation_holds_when_rebuild_prints_the_same_texts() {
    let _locale = crate::test_locale::force("en");
    let engine = refresh_fixture();
    let data = engine.data.borrow();
    let mut state = data.render.borrow_mut();
    let generation = state.panes[0].labels.generation;
    let texts = state.panes[0].labels.texts.clone();
    let cfg = state.chart_labels.clone();
    let font = state.label_font_px();
    let mut cache = None;
    super::super::RowPlanCache::working_rows(&mut cache, generation, &cfg, font, |zone, align| {
        state.collect_rows(&cfg, &texts, zone, align)
    });

    state.panes[0].label_now_ms += 1;
    assert!(!state.refresh_pane_labels(0));
    assert_eq!(state.panes[0].labels.texts, texts);
    assert_eq!(state.panes[0].labels.generation, generation);
    let view = state.arb_view.clone();
    let now_ms = state.panes[0].label_now_ms;
    state.panes[0].labels.update_with(&cfg, &view, |held| {
        assert_eq!(held.now_ms, now_ms);
        false
    });

    let mut builder_calls = 0;
    super::super::RowPlanCache::working_rows(
        &mut cache,
        state.panes[0].labels.generation,
        &cfg,
        font,
        |zone, align| {
            builder_calls += 1;
            state.collect_rows(&cfg, &texts, zone, align)
        },
    );
    assert_eq!(builder_calls, 0);
}

/// Independently assemble the previous owned-input path for formatter equivalence.
fn owned_inputs(state: &crate::chartdx::RenderState) -> crate::chartdx::text::LabelInputs {
    let pr = &state.panes[0];
    crate::chartdx::text::LabelInputs {
        ticker: pr.ticker.clone(),
        core_name: pr.core_name.clone(),
        venue: pr.venue.clone(),
        quote: pr.quote.clone(),
        strategy: pr.label_strategy.clone(),
        detect_strategy: pr.label_detect_strategy.clone(),
        detect_msg: pr.label_detect_msg.clone(),
        filter_lines: pr.filter_lines.clone(),
        trade: state.trade_labels.clone(),
        last_price: pr.cached_last_price,
        scale_badge: pr.scale_badge,
        time_scale_s: pr.time_scale_s,
        compare_pct: state
            .compare_ref_price
            .filter(|_| pr.orderbook_only)
            .zip(pr.cached_last_price)
            .filter(|(r, l)| *r > 0.0 && *l > 0.0)
            .map(|(r, l)| (l - r) / r * 100.0),
        delta_1h: pr.delta_1h,
        delta_24h: pr.delta_24h,
        context: pr.label_context,
        figures: pr.label_figures.clone(),
        windows: pr.label_windows,
        volumes: pr.label_volumes.clone(),
        liquidations: pr.label_liquidations.clone(),
        cursor_ms: pr.label_cursor_ms,
        arb: pr.label_arb.clone(),
        arb_reachable: pr.label_arb_reachable.clone(),
        now_ms: pr.label_now_ms,
        chart_tf_ms: state.chart_tf_ms,
        basis: pr.label_basis,
        actions: pr.label_actions,
        column_scroll: pr.label_scroll.clone(),
    }
}

/// Field mapping drift must not change captions compared with the previous owned-input assembly.
#[test]
fn texts_identical_to_owned_inputs_path() {
    let _locale = crate::test_locale::force("en");
    let engine = refresh_fixture();
    let data = engine.data.borrow();
    let mut state = data.render.borrow_mut();
    let mut owned = crate::chartdx::text::LabelState::default();
    for field in 0..28 {
        change_input(&mut state, field);
        let expected = owned_inputs(&state);
        owned.update(&state.chart_labels, &state.arb_view, expected.clone());
        state.refresh_pane_labels(0);
        assert_eq!(state.panes[0].labels.texts, owned.texts, "field {field}");
        // Verify fields that the selected label configuration does not print as well.
        let cfg = state.chart_labels.clone();
        let view = state.arb_view.clone();
        state.panes[0].labels.update_with(&cfg, &view, |held| {
            assert_eq!(*held, expected, "held field {field}");
            false
        });
    }
}
