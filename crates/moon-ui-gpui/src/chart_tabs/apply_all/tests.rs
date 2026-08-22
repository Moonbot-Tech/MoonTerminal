//! The ⧉ walk had no test of any kind before it became shared, which is why the leak it carried —
//! a press outside Main rewriting the one global default, and through it the main chart — was only
//! ever found by reading it. These tests pin what each press carries, which kinds it reaches, and
//! what a stored default does to the kinds it does not address.

// NOT `use super::*`: the glob would pull in the `gpui::test` macro re-exported through the parent
// module, and `#[test]` would expand into itself (recursion limit).
use super::{
    KindTargets, LayoutPopupSnapshot, StackSetting, layout_values, override_counts, spec_kind,
};
use crate::chart_tabs::apply_row::ApplyPress;
use crate::chart_tabs::common::GlobalSlot;
use crate::persistence::chart_persist::{
    ChartBtnPos, ChartTabSpec, PriceAxisPos, StackLayoutMode, StackOrientation, WinGeom,
};
use moon_core::config::{ChartBucket, ChartGraphicsCfg, ChartTabKind, WindowLayout};
use moon_core::market::CandleViewCfg;
use moon_core::session::CoreId;

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

// --- Which tabs the press touches ---

/// A press starts by addressing the kind it came FROM, and nothing else.
///
/// That is what makes the common case one click — the reader adjusting a torn-off window means the
/// torn-off windows — and it is why the press can no longer reach Main from an AddToChart popup by
/// accident, which is what the old walk did through the shared default.
#[test]
fn a_press_starts_on_its_own_kind() {
    for source in ChartTabKind::ALL {
        let targets = KindTargets::only(source);
        assert!(targets.has(source));
        for other in ChartTabKind::ALL.into_iter().filter(|k| *k != source) {
            assert!(!targets.has(other), "{source:?} must not reach {other:?}");
        }
        assert!(targets.any());
    }
}

/// An empty set is a press that does nothing, and the row must be able to say so.
#[test]
fn an_empty_target_set_addresses_nothing() {
    let mut targets = KindTargets::only(ChartTabKind::Main);
    targets.set(ChartTabKind::Main, false);
    assert!(!targets.any());
}

/// A press SETS A DEFAULT only when every value it carries has one; the ⚙ layout press writes tabs.
///
/// Derived from the values at press time rather than stored beside the choice, so a popup cannot ask
/// to store a value that has nowhere to be stored — and cannot perform a press another popup armed.
#[test]
fn only_values_that_have_a_default_can_become_one() {
    let storable = vec![StackSetting::CandleView(CandleViewCfg::default())];
    assert!(storable.iter().all(|v| v.global_slot().is_some()));
    let layout = layout_values(&loud_snapshot(), None, None, None, None);
    assert!(
        layout.iter().all(|v| v.global_slot().is_none()),
        "the layout values have no default to set: the press must write them into tabs"
    );
}

/// Arming toggles: the second press on ⧉ closes the row instead of re-opening it.
#[test]
fn a_second_press_closes_the_row() {
    let mut press = ApplyPress::default();
    press.arm(ChartTabKind::AddTo);
    assert!(press.open);
    assert!(press.targets.has(ChartTabKind::AddTo));
    press.arm(ChartTabKind::AddTo);
    assert!(!press.open);
}

/// What kind a STORED tab is, for the closed tabs whose override a default-setting press must drop.
///
/// The anchor lock outranks the window, exactly as it does for a live stack: a comparison torn off
/// into its own window is a comparison.
#[test]
fn a_stored_tab_names_its_kind() {
    let plain = spec();
    assert_eq!(spec_kind(&plain), ChartTabKind::Main);

    let mut detached = spec();
    detached.detached = Some(WinGeom {
        x: 0,
        y: 0,
        w: 800,
        h: 600,
        display_uuid: None,
    });
    assert_eq!(spec_kind(&detached), ChartTabKind::AddTo);

    let mut compare = detached.clone();
    compare.compare_anchor = Some((1 as CoreId, "USDT-BTC".to_string()));
    assert_eq!(spec_kind(&compare), ChartTabKind::Compare);
}

/// The count the row states is what the press OVERWRITES — tabs holding their own value.
///
/// A tab that follows the default already is not changed by a press that moves it, and counting it
/// would overstate the damage the reader is being asked to accept.
#[test]
fn the_count_names_only_the_tabs_that_hold_an_override() {
    let mut follows = spec();
    follows.candle_view = None;
    let mut holds = spec();
    holds.candle_view = Some(CandleViewCfg::default());
    let mut holds_other = spec();
    holds_other.chart_labels = Some(Default::default());

    let counts = override_counts(
        &[follows, holds, holds_other],
        &[StackSetting::CandleView(CandleViewCfg::default())],
    );
    assert_eq!(counts[KindTargets::index(ChartTabKind::Main)], 1);
    assert_eq!(counts[KindTargets::index(ChartTabKind::AddTo)], 0);
    assert_eq!(counts[KindTargets::index(ChartTabKind::Compare)], 0);
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

/// A value that has a default of its own names its slot, and only those values have one.
///
/// The slot is derived from the value rather than passed beside it, so a press cannot overwrite one
/// default with another setting's value — the failure the earlier two-field shape allowed.
#[test]
fn only_the_inheritable_settings_name_a_slot() {
    assert_eq!(
        StackSetting::CandleView(CandleViewCfg::default()).global_slot(),
        Some(GlobalSlot::CandleView)
    );
    assert_eq!(
        StackSetting::Graphics(ChartGraphicsCfg::default()).global_slot(),
        Some(GlobalSlot::Graphics)
    );
    assert_eq!(
        StackSetting::Labels(Default::default()).global_slot(),
        Some(GlobalSlot::Labels)
    );
    for v in layout_values(&loud_snapshot(), None, None, None, None) {
        assert_eq!(
            v.global_slot(),
            None,
            "no layout value may claim a global default: the ⚙ press must not touch layout.toml"
        );
    }
}

/// A default is stored for ONE kind and read back by that kind alone.
#[test]
fn a_default_is_stored_per_kind() {
    let mut layout = WindowLayout::default();
    let candles = CandleViewCfg {
        tf_min: 240,
        ..CandleViewCfg::default()
    };
    assert!(GlobalSlot::CandleView.write_default(
        &mut layout,
        ChartTabKind::AddTo,
        StackSetting::CandleView(candles)
    ));
    assert_eq!(layout.candle_view_for(ChartTabKind::AddTo), candles);
    assert_ne!(layout.candle_view_for(ChartTabKind::Main), candles);
    // Storing what is already there reports "unchanged", which keeps a press that changed nothing
    // from marking the file dirty.
    assert!(!GlobalSlot::CandleView.write_default(
        &mut layout,
        ChartTabKind::AddTo,
        StackSetting::CandleView(candles)
    ));
    // And only its own setting moved.
    assert_eq!(
        layout.chart_graphics_for(ChartTabKind::AddTo),
        layout.chart_graphics
    );
}

/// Until the first press the kinds share one default; the first press is what separates them.
///
/// Without this, setting the Main default would still drag the windows along — the reader who just
/// separated them would watch them move together anyway — and a profile that never used the feature
/// would have to carry three copies of the same value for nothing.
#[test]
fn the_first_press_freezes_the_kinds_it_is_not_addressing() {
    let mut layout = WindowLayout::default();
    let before = layout.candle_view;
    let main = CandleViewCfg {
        tf_min: 240,
        ..CandleViewCfg::default()
    };
    assert!(GlobalSlot::CandleView.write_default(
        &mut layout,
        ChartTabKind::Main,
        StackSetting::CandleView(main)
    ));
    assert_eq!(layout.candle_view_for(ChartTabKind::Main), main);
    assert_eq!(
        layout.candle_view_for(ChartTabKind::AddTo),
        before,
        "the windows kept what they were showing when the kinds were separated"
    );
    assert_eq!(layout.candle_view_for(ChartTabKind::Compare), before);
    // Per SETTING: separating the candles must not freeze the captions as well.
    assert_eq!(
        layout.chart_labels_for(ChartTabKind::AddTo),
        &layout.chart_labels
    );

    // And the freeze IS a change, even when the pressed value is what the file already held: the
    // kinds were separated, and reporting "nothing moved" would leave that split in memory only,
    // to be lost on the next launch.
    let mut fresh = WindowLayout::default();
    let unchanged = fresh.candle_view;
    assert!(fresh.set_candle_view_default(ChartTabKind::Main, unchanged));
    // The second press stores the same value into an already-split file and moves nothing.
    assert!(!fresh.set_candle_view_default(ChartTabKind::Main, unchanged));
}

/// A hand-edited out-of-range value is stored as its clamp, never verbatim.
///
/// `layout.toml` is hand-editable and this value is COMPARED on every notification: an unrepaired
/// impossibility would look like a change for the rest of time.
#[test]
fn a_stored_default_is_normalized_on_the_way_in() {
    let mut layout = WindowLayout::default();
    GlobalSlot::Graphics.write_default(
        &mut layout,
        ChartTabKind::Compare,
        StackSetting::Graphics(ChartGraphicsCfg {
            trade_arrow_scale: f32::NAN,
            connector_thickness_px: 999.0,
            ..ChartGraphicsCfg::default()
        }),
    );
    let stored = layout.chart_graphics_for(ChartTabKind::Compare);
    assert!(stored.trade_arrow_scale.is_finite());
    assert!(stored.connector_thickness_px < 999.0);
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
