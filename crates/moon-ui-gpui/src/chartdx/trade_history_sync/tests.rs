//! Regressions for trade-history drawing inputs and filtering.

use std::collections::HashMap;

use super::{trade_kind_visible, trade_mark_with};
use moon_core::config::ChartGraphicsCfg;
use moon_core::db::{ChartTradeRecord, OffsetSegment, ReportAxis};

/// Moving this filter back into the durable QUERY, or inverting either checkbox, must fail here.
///
/// The pair is the graphics popup's two trade-kind boxes, and the rule is one box per kind with no
/// interaction between them: unticking "real" must not touch emulator marks and vice versa, and
/// unticking both must leave nothing drawn rather than everything. The LOCATION matters as much as
/// the truth table - a display toggle must never decide which rows the history was read with,
/// because the 1000-row cap is applied after any SQL predicate and hiding one kind would then
/// surface older trades of the other that had been truncated away.
#[test]
fn each_trade_kind_checkbox_hides_only_its_own_kind() {
    let cfg = |real: bool, emulator: bool| ChartGraphicsCfg {
        show_real_trades: real,
        show_emulator_trades: emulator,
        ..ChartGraphicsCfg::default()
    };

    // Shipped default: everything visible.
    assert!(trade_kind_visible(&ChartGraphicsCfg::default(), false));
    assert!(trade_kind_visible(&ChartGraphicsCfg::default(), true));

    // One box each, no crosstalk.
    assert!(trade_kind_visible(&cfg(true, false), false));
    assert!(!trade_kind_visible(&cfg(true, false), true));
    assert!(!trade_kind_visible(&cfg(false, true), false));
    assert!(trade_kind_visible(&cfg(false, true), true));

    // Both off draws NOTHING - never everything, which is what a single tri-state predicate that
    // cannot express "neither" would have produced.
    assert!(!trade_kind_visible(&cfg(false, false), false));
    assert!(!trade_kind_visible(&cfg(false, false), true));
}

/// `chartdx/trade_history_sync.rs:trade_mark_with` must correct seconds before scaling milliseconds.
///
/// Replacing the two `axis.to_utc` calls with raw record dates would put entry and exit arrows on
/// the wrong candle whenever a core reports a non-zero clock offset.
#[test]
fn trade_mark_places_a_clock_skewed_trade_on_its_true_utc_candles() {
    let record = ChartTradeRecord {
        record_id: 9,
        core_uid: 42,
        coin: "BTCUSDT".into(),
        buy_date: 1_700_000_000,
        close_date: 1_700_000_900,
        buy_ms: None,
        close_ms: None,
        sell_set_date: 0,
        sell_set_ms: None,
        buy_set_ms: None,
        corridor: None,
        buy_price: 63_000.0,
        sell_price: 63_100.0,
        quantity: 0.25,
        is_short: false,
        emulator: false,
        profit: Some(25.0),
        quote: None,
        profit_pct: Some(0.16),
        report_uid: None,
    };
    let axis = ReportAxis::from_measured(
        HashMap::from([(
            record.core_uid,
            vec![OffsetSegment {
                from_utc: 0,
                offset_secs: 3_600,
            }],
        )]),
        chrono_tz::UTC,
    );

    let mark = trade_mark_with(&record, &axis, true, true);

    assert_eq!(mark.buy_ms, 1_699_996_400_000);
    assert_eq!(mark.close_ms, 1_699_997_300_000);
}

/// `chartdx/trade_history_sync.rs:trade_mark_with` -- reintroducing a tape-based nudge or converting
/// a raw millisecond through the seconds path would move an arrow away from the report instant
/// and make the chart disagree with the trade MoonBot actually recorded.
#[test]
fn trade_mark_uses_the_report_stamps_without_any_tape_input() {
    let record = ChartTradeRecord {
        record_id: 10,
        core_uid: 42,
        coin: "BTCUSDT".into(),
        buy_date: 1_700_000_000,
        close_date: 1_700_000_900,
        buy_ms: Some(1_700_000_000_766),
        close_ms: None,
        sell_set_date: 0,
        sell_set_ms: None,
        buy_set_ms: None,
        corridor: None,
        buy_price: 63_000.0,
        sell_price: 63_100.0,
        quantity: 0.25,
        is_short: false,
        emulator: false,
        profit: Some(25.0),
        quote: None,
        profit_pct: Some(0.16),
        report_uid: None,
    };

    let mark = trade_mark_with(&record, &ReportAxis::identity_core_local(), true, true);

    assert_eq!(mark.buy_ms, 1_700_000_000_766);
    assert_eq!(mark.close_ms, 1_700_000_900_000);
}

use super::{TradeGeometry, TradeHoverChange, built_span, take_trade_geometry_builds};
use crate::chartdx::{ChartEngine, PaneRender};
use moon_chart::view::ChartView;
use moon_core::config::ChartTheme;

/// A few round trips, far enough apart to draw as separate arrows.
fn hover_marks() -> Vec<moon_chart::trade_marks::TradeMark> {
    (0..6)
        .map(|i| moon_chart::trade_marks::TradeMark {
            buy_ms: 1_000_000 + i * 600_000,
            close_ms: 1_300_000 + i * 600_000,
            buy_price: 100.0 + i as f64,
            sell_price: 101.0 + i as f64,
            qty: 1.0,
            is_short: i % 2 == 1,
            show_entry: true,
            show_exit: true,
        })
        .collect()
}

fn hover_ctx(hovered: Option<(usize, bool)>) -> moon_chart::trade_marks::TradeGeometryCtx {
    moon_chart::trade_marks::TradeGeometryCtx {
        epoch_ms: 900_000.0,
        long_rgb: [10, 200, 30],
        short_rgb: [220, 20, 40],
        scale: 1.25,
        px_per_ms: 0.001,
        px_per_price: 20.0,
        arrow_scale: 1.0,
        connector_thickness: moon_chart::trade_marks::CONNECTOR_THICKNESS,
        hovered,
    }
}

/// `trade_history_sync.rs:built_span` padding its span by the window `W` instead of `Q`, or
/// `trade_history_sig` dropping the span-cell term, leaves arrows missing near a cell edge after
/// a zoom-out or a pan that does not rebuild. Every view that keys to the same `(cell, log2 Q)` —
/// any pan inside the cell, any zoom inside the power-of-two bucket — must lie inside the span the
/// geometry was built over, and the signature must move exactly when the cell does.
#[test]
fn built_span_covers_every_view_with_its_key_and_the_signature_follows_the_cell() {
    let width = 1024u32;
    let mut state = 0x7777_1111_u64;
    let mut next = move |n: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % n
    };
    let mut checked = 0;
    for _ in 0..2_000 {
        let ppm_built = 0.0005 + next(10_000) as f32 * 1e-6;
        let a_built = 1.7e12 + next(10_000_000_000) as f64;
        let (cell, log2_q, span) = built_span(a_built, width, ppm_built).unwrap();
        // Another view in the same cell and bucket: panned and zoomed.
        let ppm = ppm_built * (0.5 + next(1_000) as f32 / 1_000.0);
        let q = 2f64.powi(log2_q);
        let a = cell as f64 * q + next(q as u64) as f64;
        let Some((cell2, log2_q2, _)) = built_span(a, width, ppm) else {
            continue;
        };
        if (cell2, log2_q2) != (cell, log2_q) {
            continue;
        }
        let mut view = ChartView::new(1.7e12);
        view.px_per_ms = ppm;
        view.right_time_ms = a;
        let (left_rel, window) = view.visible_x(width as f32);
        let left = view.epoch_ms + f64::from(left_rel);
        let right = left + f64::from(window);
        // f32 relative time: allow its rounding at this epoch distance.
        let eps = 2048.0;
        assert!(
            left >= span.0 - eps && right <= span.1 + eps,
            "view [{left}, {right}] escapes span {span:?} built at a={a_built} ppm={ppm_built}"
        );
        checked += 1;
    }
    assert!(checked > 100, "{checked}");

    let engine = ChartEngine::new(0.0, ChartTheme::default());
    let data = engine.data.borrow();
    let mut view = ChartView::new(1.7e12);
    view.px_per_ms = 0.001;
    let (cell, log2_q, _) = built_span(1.7e12 + 5e6, data.w, view.px_per_ms).unwrap();
    let q = 2f64.powi(log2_q);
    let base = cell as f64 * q;
    view.right_time_ms = base + 1.0;
    let sig = data.trade_history_sig(&view);
    for step in [0.1, 0.4, 0.9, 0.999] {
        view.right_time_ms = base + q * step;
        assert_eq!(
            data.trade_history_sig(&view),
            sig,
            "pan inside the cell rebuilt"
        );
    }
    view.right_time_ms = base + q * 1.001;
    assert_ne!(
        data.trade_history_sig(&view),
        sig,
        "crossing the cell did not rebuild"
    );
    view.right_time_ms = base - q * 0.001;
    assert_ne!(
        data.trade_history_sig(&view),
        sig,
        "crossing the cell did not rebuild"
    );
}

/// `trade_history_sync.rs:set_trade_hover` falling back to `dirty_all_trade_panes` on a backend
/// that can patch, or `patch_trade_hover` missing the previous arrow, makes every hover rebuild
/// every pane (or leaves the old arrow drawn hot). A hover change must patch exactly the old and
/// the new arrow, with no geometry build.
#[test]
fn hover_change_patches_two_arrows_without_a_rebuild() {
    let engine = ChartEngine::new(0.0, ChartTheme::default());
    let marks = hover_marks();
    let ctx = hover_ctx(None);
    let (mut markers, mut segs) = (Vec::new(), Vec::new());
    let clusters =
        moon_chart::trade_marks::build_trade_geometry(&marks, &ctx, &mut markers, &mut segs);
    assert_eq!(clusters.len(), 12);
    {
        let mut state = engine.state.borrow_mut();
        let mut pane = PaneRender::new();
        // A built pane's signature; a full rebuild request resets it to the sentinel.
        pane.last_trade_history_sig = 42;
        pane.layers.set_userdata(&[], &[], &segs, &markers);
        pane.trade_geometry = TradeGeometry {
            order_by_t: moon_chart::trade_marks::order_by_time(&clusters),
            clusters,
            ctx: Some(ctx),
            ..Default::default()
        };
        state.panes.push(pane);
    }
    take_trade_geometry_builds();
    crate::chartdx::userdata::take_marker_patches();
    let mut data = engine.data.borrow_mut();
    assert_eq!(
        data.set_trade_hover(Some((0, 2, true))),
        TradeHoverChange::Patched
    );
    assert_eq!(crate::chartdx::userdata::take_marker_patches(), 1);
    assert_eq!(
        data.set_trade_hover(Some((0, 2, true))),
        TradeHoverChange::Unchanged
    );
    assert_eq!(
        data.set_trade_hover(Some((0, 3, false))),
        TradeHoverChange::Patched
    );
    let patches = crate::chartdx::userdata::take_marker_patches();
    let builds = take_trade_geometry_builds();
    println!("[hover] before=1 build after={builds} builds, {patches} patches");
    assert_eq!(patches, 2, "the old and the new arrow");
    assert_eq!(builds, 0);
    let render = data.render.borrow();
    assert_eq!(
        render.panes[0].last_trade_history_sig, 42,
        "a hover marked the pane dirty"
    );
}
