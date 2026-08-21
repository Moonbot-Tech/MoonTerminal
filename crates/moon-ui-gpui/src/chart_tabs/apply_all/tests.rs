//! The ⧉ walk had no test of any kind before it became shared, which is why the leak it carries —
//! a press outside Main still rewrites the global default — was only ever found by reading it. These
//! tests pin the behaviour that WAS there, so the sharing cannot quietly change any popup's press.

// NOT `use super::*`: the glob would pull in the `gpui::test` macro re-exported through the parent
// module, and `#[test]` would expand into itself (recursion limit).
use super::{LayoutPopupSnapshot, MainAction, StackSetting, layout_values, plan_main};
use crate::chart_tabs::common::GlobalSlot;
use crate::persistence::chart_persist::{
    ChartBtnPos, ChartTabSpec, PriceAxisPos, StackLayoutMode, StackOrientation,
};
use moon_core::config::{ChartBucket, ChartGraphicsCfg, WindowLayout};
use moon_core::market::CandleViewCfg;

/// A spec to apply values onto, keyed like an AddToChart tab.
fn spec() -> ChartTabSpec {
    ChartTabSpec::new("g".to_string(), 3, ChartBucket::Shared)
}

/// Apply a whole value set to a spec, as the walk does for every target.
fn write_all(values: &[StackSetting], s: &mut ChartTabSpec) {
    for v in values {
        v.clone().write_spec(s);
    }
}

/// A snapshot with every setting set AWAY from its default, so a value that fails to travel shows up
/// as the default rather than coinciding with the expected one.
fn loud_snapshot() -> LayoutPopupSnapshot {
    LayoutPopupSnapshot {
        mode: StackLayoutMode::Scroll,
        orientation: StackOrientation::Horizontal,
        orderbook: false,
        liquidations: false,
        show_zone: false,
        auto_pin: true,
        cancel_pos: ChartBtnPos::Left,
        panic_pos: ChartBtnPos::Left,
        price_axis_pos: PriceAxisPos::Right,
        time_axis: false,
        line_labels: false,
        cursor_labels: false,
    }
}

// --- Which tab the press touches ---

/// A press from Main's own popup copies the source values into Main.
#[test]
fn a_press_on_main_copies_into_main() {
    assert_eq!(plan_main(true, true, false), MainAction::Copy);
    assert_eq!(plan_main(true, true, true), MainAction::Copy);
    assert_eq!(plan_main(true, false, false), MainAction::Copy);
}

/// A press from an Add/Custom/detached source PINS Main to the default it is following.
///
/// This is the load-bearing half of the walk: Main reads the global default live, so overwriting
/// that default would change Main as surely as editing it. Pinning first is what keeps a press
/// outside Main from reaching it.
#[test]
fn a_press_elsewhere_pins_main_to_the_default_it_follows() {
    assert_eq!(plan_main(false, true, false), MainAction::Pin);
}

/// Main with its own override is left alone: the default it no longer follows cannot reach it.
#[test]
fn a_press_elsewhere_leaves_an_overridden_main_alone() {
    assert_eq!(plan_main(false, true, true), MainAction::Leave);
}

/// A setting with no global default — the ⚙ layout popup — never touches Main from elsewhere.
#[test]
fn a_layout_press_elsewhere_never_touches_main() {
    assert_eq!(plan_main(false, false, false), MainAction::Leave);
    assert_eq!(plan_main(false, false, true), MainAction::Leave);
}

// --- What each popup's press carries ---

/// The layout ⧉ carries every value the popup edits, and each one reaches the spec.
#[test]
fn the_layout_press_carries_every_layout_value() {
    let values = layout_values(
        &loud_snapshot(),
        Some(40),
        Some(700),
        Some(1.5),
        Some(StackOrientation::Horizontal),
    );
    let mut s = spec();
    write_all(&values, &mut s);
    assert_eq!(s.layout_mode, Some(StackLayoutMode::Scroll));
    assert_eq!(s.layout_height_fit, Some(40));
    assert_eq!(s.layout_height_scroll, Some(700));
    assert_eq!(s.scale, Some(1.5));
    assert_eq!(s.orderbook_enabled, Some(false));
    assert_eq!(s.liquidations_enabled, Some(false));
    assert_eq!(s.show_zone, Some(false));
    assert_eq!(s.auto_pin, Some(true));
    assert_eq!(s.layout_orientation, Some(StackOrientation::Horizontal));
    assert_eq!(s.cancel_buy_pos, Some(ChartBtnPos::Left));
    assert_eq!(s.panic_sell_pos, Some(ChartBtnPos::Left));
    assert_eq!(s.price_axis_pos, Some(PriceAxisPos::Right));
    assert_eq!(s.time_axis_visible, Some(false));
    assert_eq!(s.line_labels, Some(false));
    assert_eq!(s.cursor_labels, Some(false));
    // Not the layout popup's to copy: each has its own ⧉.
    assert_eq!(s.candle_view, None);
    assert_eq!(s.chart_graphics, None);
    assert_eq!(s.x_ppm, None);
}

/// A source that never named an orientation copies "none named" rather than the resolved default.
///
/// The walk used to pass the source's raw `Option` straight into every spec; resolving it to
/// Vertical here would write an orientation into files that had none. The snapshot carries a
/// RESOLVED orientation beside the raw one, so this also pins which of the two travels.
#[test]
fn an_unset_orientation_travels_as_unset() {
    let snap = loud_snapshot();
    assert_eq!(snap.orientation, StackOrientation::Horizontal);
    let values = layout_values(&snap, None, None, None, None);
    let mut s = spec();
    s.layout_orientation = Some(StackOrientation::Horizontal);
    write_all(&values, &mut s);
    assert_eq!(
        s.layout_orientation, None,
        "the raw parameter travels, not the snapshot's resolved orientation"
    );
    // Auto scale travels as Auto too, clearing a target's stored scale.
    s.scale = Some(2.0);
    write_all(&values, &mut s);
    assert_eq!(s.scale, None);
    // And a named orientation still travels as itself.
    let named = layout_values(&snap, None, None, None, Some(StackOrientation::Vertical));
    write_all(&named, &mut s);
    assert_eq!(s.layout_orientation, Some(StackOrientation::Vertical));
}

/// The candle ⧉ carries the candle settings and nothing else; the X scale rides beside them.
#[test]
fn the_candle_press_carries_only_the_candle_settings() {
    let cfg = CandleViewCfg {
        tf_min: 30,
        ..CandleViewCfg::default()
    };
    let mut s = spec();
    write_all(&[StackSetting::CandleView(cfg)], &mut s);
    assert_eq!(s.candle_view, Some(cfg));
    assert_eq!(s.layout_mode, None);
    assert_eq!(s.chart_graphics, None);
}

/// The graphics ⧉ carries the drawing settings and nothing else.
#[test]
fn the_graphics_press_carries_only_the_drawing_settings() {
    let cfg = ChartGraphicsCfg {
        trade_arrow_scale: 1.6,
        show_emulator_trades: false,
        ..ChartGraphicsCfg::default()
    };
    let mut s = spec();
    write_all(&[StackSetting::Graphics(cfg)], &mut s);
    assert_eq!(s.chart_graphics, Some(cfg));
    assert_eq!(s.candle_view, None);
    assert_eq!(s.layout_mode, None);
}

// --- The global default each press overwrites ---

/// A value that has a global default names its own slot, and only those values have one.
///
/// The slot is derived from the value rather than passed beside it, so a press cannot overwrite one
/// default with another setting's value — the failure the earlier two-field shape allowed.
#[test]
fn only_the_two_inheritable_settings_name_a_global_slot() {
    assert_eq!(
        StackSetting::CandleView(CandleViewCfg::default()).global_slot(),
        Some(GlobalSlot::CandleView)
    );
    assert_eq!(
        StackSetting::Graphics(ChartGraphicsCfg::default()).global_slot(),
        Some(GlobalSlot::Graphics)
    );
    for v in layout_values(&loud_snapshot(), None, None, None, None) {
        assert_eq!(
            v.global_slot(),
            None,
            "no layout value may claim a global default: the ⚙ press must not touch layout.toml"
        );
    }
}

/// Every global default reads back what it wrote, and reads the CURRENT value for pinning Main.
#[test]
fn a_global_default_round_trips_through_the_layout() {
    let mut layout = WindowLayout::default();
    let candles = CandleViewCfg {
        tf_min: 240,
        ..CandleViewCfg::default()
    };
    let graphics = ChartGraphicsCfg {
        connector_thickness_px: 4.0,
        ..ChartGraphicsCfg::default()
    };
    // What Main is following right now, which is what a Pin copies into it.
    assert_eq!(
        GlobalSlot::CandleView.read(&layout),
        StackSetting::CandleView(layout.candle_view)
    );
    assert_eq!(
        GlobalSlot::Graphics.read(&layout),
        StackSetting::Graphics(layout.chart_graphics)
    );
    assert!(GlobalSlot::CandleView.write(&mut layout, StackSetting::CandleView(candles)));
    assert!(GlobalSlot::Graphics.write(&mut layout, StackSetting::Graphics(graphics)));
    assert_eq!(layout.candle_view, candles);
    assert_eq!(layout.chart_graphics, graphics);
    // Each default writes ONLY its own field.
    assert_eq!(
        GlobalSlot::CandleView.read(&layout),
        StackSetting::CandleView(candles)
    );
    // Storing what is already there reports "unchanged", which is what keeps a ⧉ press that edited
    // nothing from waking every chart in the application.
    assert!(!GlobalSlot::Graphics.write(&mut layout, StackSetting::Graphics(graphics)));
}

/// A hand-edited out-of-range default is pinned into `charts.json` as its clamp, never verbatim.
#[test]
fn pinning_main_materializes_a_normalized_value() {
    let layout = WindowLayout {
        chart_graphics: ChartGraphicsCfg {
            trade_arrow_scale: f32::NAN,
            connector_thickness_px: 999.0,
            ..ChartGraphicsCfg::default()
        },
        ..WindowLayout::default()
    };
    let StackSetting::Graphics(pinned) = GlobalSlot::Graphics.read(&layout) else {
        panic!("the graphics slot must read back a graphics value");
    };
    assert!(pinned.trade_arrow_scale.is_finite());
    assert!(pinned.connector_thickness_px < 999.0);
}

// --- Order-book demand ---

/// Only the order-book toggle rebuilds subscription demand, on both the single-setting path and
/// the ⧉ walk — one definition, so the two cannot drift apart.
#[test]
fn only_an_orderbook_value_rebuilds_demand() {
    assert!(
        layout_values(&loud_snapshot(), None, None, None, None)
            .iter()
            .any(|v| v.rebuilds_orderbook_demand()),
        "the layout press carries the order-book toggle, so its walk must rebuild demand"
    );
    assert!(!StackSetting::CandleView(CandleViewCfg::default()).rebuilds_orderbook_demand());
    assert!(!StackSetting::Graphics(ChartGraphicsCfg::default()).rebuilds_orderbook_demand());
}
