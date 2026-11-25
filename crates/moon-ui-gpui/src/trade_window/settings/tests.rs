//! Persistence and headless layout coverage for the trade-window settings.

// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use gpui::AppContext;
use moon_core::market::candles::{
    CANDLE_MODE_FILLED, CANDLE_MODE_OFF, CANDLE_MODE_OUTLINE, CANDLE_MODE_OUTLINE_IN_ZONE,
    CandleViewCfg,
};

use super::{
    PINNED_TF_MIN, TRADE_WINDOW_MODES, candles_for_default, drawn_graphics, graphics_for_default,
    main_chart_marker_scale, pinned_candle_view, trade_window_mode_segment,
};

/// A layout whose Main, Compare and trade-window kinds each carry a cross size of their own.
fn layout_with_kind_sizes(
    main: f32,
    compare: f32,
    trade: f32,
) -> moon_core::config::layout::WindowLayout {
    use moon_core::config::ChartTabKind;
    let mut layout = moon_core::config::layout::WindowLayout::default();
    for (kind, size) in [
        (ChartTabKind::Main, main),
        (ChartTabKind::Compare, compare),
        (ChartTabKind::Trade, trade),
    ] {
        let mut cfg = layout.chart_graphics_for(kind);
        cfg.marker_scale = size;
        layout.store_chart_graphics(kind, cfg);
    }
    layout
}

/// One tab spec carrying, or not, a cross size of its own.
fn spec(
    group: &str,
    num: u32,
    marker_scale: Option<f32>,
) -> crate::persistence::chart_persist::ChartTabSpec {
    let mut spec = crate::persistence::chart_persist::ChartTabSpec::new(
        group.to_string(),
        num,
        moon_core::config::ChartBucket::Shared,
    );
    spec.chart_graphics = marker_scale.map(|size| moon_core::config::ChartGraphicsCfg {
        marker_scale: size,
        ..Default::default()
    });
    spec
}

/// The crosses follow the Main TAB of the trade's own window: its per-tab size, not the
/// trade-window kind's, not another window's Main, not a numbered tab beside it (#1261).
///
/// Breakage: reading the trade-window kind's set — the reported bug — answers 0.5; resolving the
/// window by anything but the core's group answers the other window's 3.0; matching on the group
/// alone picks up the AddTo tab's 1.5.
#[test]
fn the_cross_size_is_the_main_tab_of_the_cores_own_window() {
    let config = crate::test_support::fixtures::app_config(20);
    let (group_a, group_b) = (
        config.servers[0].group.clone(),
        config.servers[10].group.clone(),
    );
    assert_ne!(group_a, group_b, "the fixture spans two windows");
    let layout = layout_with_kind_sizes(0.7, 1.0, 0.5);
    let specs = vec![
        spec(&group_a, 1, Some(1.5)),
        spec(&group_b, 0, Some(3.0)),
        spec(&group_a, 0, Some(2.0)),
    ];
    let core_a = config.servers[0].id;
    let core_b = config.servers[10].id;
    assert_eq!(
        main_chart_marker_scale(&layout, &config.servers, &specs, core_a),
        2.0
    );
    assert_eq!(
        main_chart_marker_scale(&layout, &config.servers, &specs, core_b),
        3.0
    );
}

/// A Main tab with no size of its own draws the Main default, and so do the crosses; so does an
/// unknown core or a window whose Main tab stored nothing.
///
/// Breakage: falling back to the trade-window kind — the reported bug — answers 0.5 everywhere.
#[test]
fn a_main_tab_without_its_own_size_answers_the_main_default() {
    let config = crate::test_support::fixtures::app_config(20);
    let group_a = config.servers[0].group.clone();
    let layout = layout_with_kind_sizes(0.7, 1.0, 0.5);
    let core_a = config.servers[0].id;
    let core_b = config.servers[10].id;
    let plain = vec![spec(&group_a, 0, None)];
    assert_eq!(
        main_chart_marker_scale(&layout, &config.servers, &plain, core_a),
        0.7
    );
    assert_eq!(
        main_chart_marker_scale(&layout, &config.servers, &plain, core_b),
        0.7,
        "a window whose Main tab stored nothing"
    );
    assert_eq!(
        main_chart_marker_scale(&layout, &config.servers, &plain, 9_999),
        0.7,
        "a core the configuration no longer lists"
    );
}

/// Only the size is borrowed: drawing takes the main chart's, and storing puts the kind's own
/// back, so a row in the popup never writes the main chart's size into the trade-window set.
///
/// Breakage: copying the main chart's whole set would move the price-line flag below; storing what
/// is drawn would freeze the main chart's size into the kind and outlive the next change there.
#[test]
fn only_the_cross_size_is_borrowed_and_never_stored() {
    let stored = moon_core::config::ChartGraphicsCfg {
        marker_scale: 0.5,
        last_price_line: false,
        ..Default::default()
    };
    let drawn = drawn_graphics(stored, 2.0);
    assert_eq!(drawn.marker_scale, 2.0);
    assert!(!drawn.last_price_line, "the kind's own flag stays");
    let written = graphics_for_default(drawn, stored.marker_scale);
    assert_eq!(written, moon_chart::normalize_chart_graphics(stored));
}

/// A trade-window edit must survive restart without changing Main or resetting the other flag.
#[test]
fn price_line_toggles_round_trip_independently() {
    use moon_core::config::{ChartTabKind, layout::WindowLayout};
    let mut layout = WindowLayout::default();
    let main = layout.chart_graphics_for(ChartTabKind::Main);
    let shipped = layout.chart_graphics_for(ChartTabKind::Trade);
    assert!(shipped.last_price_line && !shipped.mark_price_line);
    for (last, mark) in [(false, true), (true, false), (false, false), (true, true)] {
        let mut cfg = layout.chart_graphics_for(ChartTabKind::Trade);
        cfg.last_price_line = last;
        cfg.mark_price_line = mark;
        layout.store_chart_graphics(ChartTabKind::Trade, cfg);
        let encoded = serde_json::to_string(&layout).expect("serialize trade preferences");
        layout = serde_json::from_str(&encoded).expect("reload trade preferences");
        let restored = layout.chart_graphics_for(ChartTabKind::Trade);
        assert_eq!(
            (restored.last_price_line, restored.mark_price_line),
            (last, mark)
        );
        assert_eq!(layout.chart_graphics_for(ChartTabKind::Main), main);
    }
}

/// Capture the actual production checkbox rows without constructing a backend or opening a trade.
struct PriceLinesHarness {
    boxes: std::rc::Rc<std::cell::RefCell<Vec<gpui::Bounds<gpui::Pixels>>>>,
}

impl gpui::Render for PriceLinesHarness {
    /// Render the rows at the popup's production width with inert change handlers.
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement, Styled};
        let boxes = self.boxes.clone();
        gpui::div().w(gpui::px(super::ROW_W)).child(
            super::price_line_rows(
                moon_core::config::ChartGraphicsCfg::default(),
                crate::design::ui_px(cx, 6.0),
                |_, _, _| {},
            )
            .on_children_prepainted(move |bounds, _, _| *boxes.borrow_mut() = bounds),
        )
    }
}

/// An oversized translation or shrinking row must not clip either checkbox label in the popup.
#[gpui::test]
fn price_line_popup_rows_fit_every_locale(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    for locale in ["ru", "en", "es", "uk", "tr", "pt", "vi", "zh"] {
        let _locale = crate::test_locale::force(locale);
        for mode in [
            moon_core::config::UiThemeMode::Dark,
            moon_core::config::UiThemeMode::Light,
            moon_core::config::UiThemeMode::Graphite,
            moon_core::config::UiThemeMode::Obsidian,
            moon_core::config::UiThemeMode::LightExperimental,
            moon_core::config::UiThemeMode::DarkExperimental,
        ] {
            cx.update(|cx| {
                moon_ui::MoonTheme::install_config(
                    crate::startup::moon_theme_config_for_presentation(mode, 1.0),
                    cx,
                )
            });
            let boxes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let sink = boxes.clone();
            let handle = cx.add_window(move |_, _| PriceLinesHarness { boxes: sink });
            cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
                .unwrap();
            let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
            for (index, selector) in [
                "trade-window-settings-last-price:label",
                "trade-window-settings-mark-price:label",
            ]
            .into_iter()
            .enumerate()
            {
                let label = visual
                    .debug_bounds(selector)
                    .expect("production checkbox label");
                let row = boxes.borrow()[index];
                assert!(
                    label.left() >= row.left()
                        && label.right() <= row.right() + gpui::px(0.5)
                        && label.top() >= row.top()
                        && label.bottom() <= row.bottom() + gpui::px(0.5),
                    "{locale}: price-line label clipped: {label:?} in {row:?}"
                );
            }
            let boxes = boxes.borrow();
            assert_eq!(boxes.len(), 2);
            for bounds in boxes.iter() {
                assert!(
                    bounds.origin.x >= gpui::px(0.0)
                        && bounds.right() <= gpui::px(super::ROW_W + 0.5),
                    "{locale}: price-line row clipped: {bounds:?}"
                );
                assert!(bounds.size.height > gpui::px(0.0));
            }
            assert!(
                boxes[0].bottom() <= boxes[1].origin.y,
                "{locale}: overlapping rows"
            );
            cx.update_window(handle.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}

/// The replay pin must preserve inherited zone widths in both live and saved settings.
#[test]
fn fresh_trade_window_keeps_max_candle_zones() {
    use moon_core::market::candles::CANDLE_ZONE_MAX;
    for ticks in [false, true] {
        let cfg = pinned_candle_view(CandleViewCfg::default(), ticks);
        assert_eq!(
            (cfg.trade_candles, cfg.hide_candles),
            (CANDLE_ZONE_MAX, CANDLE_ZONE_MAX)
        );
        let stored = candles_for_default(cfg, 5);
        assert_eq!(
            (stored.trade_candles, stored.hide_candles),
            (CANDLE_ZONE_MAX, CANDLE_ZONE_MAX)
        );
    }
}

fn user(mode: u8, tf_min: u32) -> CandleViewCfg {
    CandleViewCfg {
        tf_min,
        mode,
        outline_px: 2.0,
        trade_candles: 5,
        wicks_in_zone: false,
        ..CandleViewCfg::default()
    }
}

/// The pin is unconditional: whatever timeframe the user's chart is on, the window draws minutes.
///
/// Breakage: a coarser bucket resamples the minute rows under a caption that still names minutes —
/// the bug the pin was written for.
#[test]
fn the_timeframe_is_pinned_whatever_the_user_chose() {
    for tf in [1, 5, 30, 240] {
        for ticks in [false, true] {
            assert_eq!(
                pinned_candle_view(user(CANDLE_MODE_FILLED, tf), ticks).tf_min,
                PINNED_TF_MIN
            );
        }
    }
}

/// Mode Off stands only while ticks are on screen; before that the shipped mode draws instead,
/// and every other mode is left exactly as picked.
///
/// Breakage: honouring Off on a candle replay draws an empty pane under a caption naming
/// candles; forcing it once ticks are there takes the user's pure tick chart away.
#[test]
fn off_is_replaced_until_ticks_arrive_and_other_modes_pass_through() {
    let shipped = CandleViewCfg::default().mode;
    assert_ne!(
        shipped, CANDLE_MODE_OFF,
        "the shipped mode must draw candles"
    );
    assert_eq!(
        pinned_candle_view(user(CANDLE_MODE_OFF, 1), false).mode,
        shipped
    );
    assert_eq!(
        pinned_candle_view(user(CANDLE_MODE_OFF, 1), true).mode,
        CANDLE_MODE_OFF
    );
    for mode in [
        CANDLE_MODE_FILLED,
        CANDLE_MODE_OUTLINE,
        CANDLE_MODE_OUTLINE_IN_ZONE,
    ] {
        for ticks in [false, true] {
            assert_eq!(pinned_candle_view(user(mode, 1), ticks).mode, mode);
        }
    }
}

/// The pin touches nothing but the two fields it is about.
#[test]
fn the_pin_leaves_the_rest_of_the_choice_alone() {
    let pinned = pinned_candle_view(user(CANDLE_MODE_OUTLINE, 30), false);
    let expected = user(CANDLE_MODE_OUTLINE, PINNED_TF_MIN);
    assert_eq!(pinned, expected);
}

/// A candle row never writes the pinned minute into the stored set: it keeps the timeframe the
/// trade-window kind already carried.
///
/// Breakage: the stored set would silently become a one-minute one, and a main-chart ⧉ press
/// that later addressed the kind would overwrite it — the two paths disagreeing about what the
/// setting is.
#[test]
fn the_stored_default_keeps_the_kinds_own_timeframe() {
    let shown = user(CANDLE_MODE_OFF, PINNED_TF_MIN);
    let stored = candles_for_default(shown, 5);
    assert_eq!(stored.tf_min, 5);
    assert_eq!(
        stored.mode, CANDLE_MODE_OFF,
        "the user's choice, not the forced one"
    );
    assert_eq!(
        CandleViewCfg {
            tf_min: PINNED_TF_MIN,
            ..stored
        },
        shown,
        "nothing but the timeframe differs"
    );
}

/// The popup offers Off, Filled and Outline — and not "In zone", whose zone the frozen replay
/// never reaches; a stored "In zone" lights "Filled", which is what it draws as here.
///
/// Breakage: offering "In zone" gives a segment that changes nothing; lighting nothing for a
/// stored "In zone" tells the reader the window is in no mode at all.
#[test]
fn the_mode_row_offers_off_but_not_in_zone_and_maps_in_zone_to_filled() {
    assert_eq!(
        TRADE_WINDOW_MODES,
        [CANDLE_MODE_OFF, CANDLE_MODE_FILLED, CANDLE_MODE_OUTLINE]
    );
    assert!(!TRADE_WINDOW_MODES.contains(&CANDLE_MODE_OUTLINE_IN_ZONE));
    assert_eq!(
        trade_window_mode_segment(CANDLE_MODE_OUTLINE_IN_ZONE),
        CANDLE_MODE_FILLED
    );
    for mode in TRADE_WINDOW_MODES {
        assert_eq!(trade_window_mode_segment(mode), mode);
    }
    assert_eq!(
        trade_window_mode_segment(200),
        CANDLE_MODE_OFF,
        "unknown reads as Off"
    );
}
