use super::*;
use crate::config::chart_defaults::ChartTabKind;
use crate::market::candles::CandleViewCfg;

/// The shape a `layout.toml` written before the move had: switches under `candle_view`, none
/// under `chart_graphics`.
const OLD_BASE: &str =
    "[candle_view]\ntf_min = 5\nlast_price_line = false\nmoonshot_zone = false\n";

#[test]
fn main_switches_land_on_the_base_graphics_and_are_consumed() {
    let mut layout: WindowLayout = toml::from_str(OLD_BASE).expect("an old layout loads");
    assert!(layout.candle_view.carried_lines.is_some());

    assert!(layout.carry_lines_into_graphics());

    assert!(!layout.chart_graphics.last_price_line);
    assert!(
        layout.chart_graphics.mark_price_line,
        "an absent key keeps the default"
    );
    assert!(!layout.chart_graphics.moonshot_zone);
    assert!(layout.candle_view.carried_lines.is_none(), "consumed");
    // A second pass finds nothing.
    assert!(!layout.carry_lines_into_graphics());
    // ...and the save no longer writes the old keys.
    let text = toml::to_string(&layout).expect("serializes");
    let reloaded: WindowLayout = toml::from_str(&text).expect("reloads");
    assert!(reloaded.candle_view.carried_lines.is_none());
    assert!(!reloaded.chart_graphics.last_price_line);
}

/// A save that lands BEFORE the pass — nothing consumed yet — keeps the old keys, so the next
/// launch's pass still finds them.
#[test]
fn an_unconsumed_layout_keeps_the_old_keys_through_a_save() {
    let layout: WindowLayout = toml::from_str(OLD_BASE).expect("loads");
    let text = toml::to_string(&layout).expect("serializes");
    let reloaded: WindowLayout = toml::from_str(&text).expect("reloads");
    let carried = reloaded.candle_view.carried_lines.expect("still carried");
    assert_eq!(carried.last_price_line, Some(false));
    assert_eq!(carried.moonshot_zone, Some(false));
    assert_eq!(carried.mark_price_line, None);
}

#[test]
fn a_kind_with_its_own_candles_but_inherited_graphics_gets_graphics_of_its_own() {
    let doc = "[candle_view]\ntf_min = 5\nlast_price_line = false\n\
               [chart_defaults_addto.candle_view]\ntf_min = 1\nmoonshot_zone = false\n";
    let mut layout: WindowLayout = toml::from_str(doc).expect("loads");
    layout.chart_graphics.marker_scale = 2.0;

    assert!(layout.carry_lines_into_graphics());

    // Main's switch on the base.
    assert!(!layout.chart_graphics.last_price_line);
    assert!(layout.chart_graphics.moonshot_zone);
    // AddTo's own table replaced Main's whole: its own corridor switch, and the shipped default
    // for the price line it did not name — NOT Main's choice — both on a copy of the base
    // graphics, marker scale included.
    let addto = layout.chart_graphics_for(ChartTabKind::AddTo);
    assert!(layout.chart_defaults_addto.chart_graphics.is_some());
    assert!(
        addto.last_price_line,
        "unnamed in its own table: the shipped default"
    );
    assert!(!addto.moonshot_zone, "its own corridor switch");
    assert_eq!(addto.marker_scale, 2.0);
    // Compare followed Main in both chains and keeps following: nothing of its own was created.
    assert!(layout.chart_defaults_compare.chart_graphics.is_none());
}

/// A kind's own `candle_view` replaced Main's WHOLE, so a switch it never named was the shipped
/// default — not Main's. Turning a line off on Main alone must not turn it off on such a kind.
#[test]
fn a_kind_with_its_own_candles_that_named_no_switch_keeps_the_shipped_ones() {
    let doc = "[candle_view]\ntf_min = 5\nlast_price_line = false\n\
               [chart_defaults_addto.candle_view]\ntf_min = 1\n";
    let mut layout: WindowLayout = toml::from_str(doc).expect("loads");

    assert!(layout.carry_lines_into_graphics());

    assert!(!layout.chart_graphics.last_price_line, "Main's own choice");
    let addto = layout.chart_graphics_for(ChartTabKind::AddTo);
    assert!(
        addto.last_price_line,
        "drew the shipped default, keeps drawing it"
    );
    assert!(
        layout.chart_defaults_addto.chart_graphics.is_some(),
        "which takes graphics of its own, since Main's now say otherwise"
    );
}

/// The same table in a profile written AFTER the move carries nothing anywhere, and is left
/// alone: there is no old value to restore, and stamping the shipped defaults over the popup's
/// choice on every launch would be the regression the no-marker design must not have.
#[test]
fn a_new_profile_with_a_kinds_own_candles_is_left_alone() {
    let doc = "[candle_view]\ntf_min = 5\n[chart_defaults_addto.candle_view]\ntf_min = 1\n\
               [chart_graphics]\nlast_price_line = false\n";
    let mut layout: WindowLayout = toml::from_str(doc).expect("loads");
    assert!(layout.carried_lines_for(ChartTabKind::AddTo).is_none());
    assert!(!layout.carry_lines_into_graphics());
    assert!(layout.chart_defaults_addto.chart_graphics.is_none());
    assert!(!layout.chart_graphics.last_price_line);
}

#[test]
fn a_kind_with_its_own_graphics_but_inherited_candles_takes_mains_switches() {
    let doc = "[candle_view]\ntf_min = 5\nmark_price_line = false\n\
               [chart_defaults_compare.chart_graphics]\nmarker_scale = 3.0\n";
    let mut layout: WindowLayout = toml::from_str(doc).expect("loads");

    assert!(layout.carry_lines_into_graphics());

    let compare = layout.chart_graphics_for(ChartTabKind::Compare);
    assert!(
        !compare.mark_price_line,
        "drew Main's candle lines, keeps drawing them"
    );
    assert_eq!(
        compare.marker_scale, 3.0,
        "its own graphics are otherwise untouched"
    );
}

#[test]
fn a_layout_with_nothing_carried_is_left_alone() {
    let mut layout: WindowLayout = toml::from_str("[candle_view]\ntf_min = 5\n").expect("loads");
    assert!(layout.candle_view.carried_lines.is_none());
    let before = toml::to_string(&layout).expect("serializes");
    assert!(!layout.carry_lines_into_graphics());
    assert_eq!(toml::to_string(&layout).expect("serializes"), before);
}

#[test]
fn stamp_writes_only_what_the_table_named() {
    let mut cfg = ChartGraphicsCfg::default();
    let carried = CarriedLines {
        last_price_line: None,
        mark_price_line: Some(false),
        moonshot_zone: None,
    };
    assert!(stamp_carried_lines(&mut cfg, carried));
    assert!(cfg.last_price_line);
    assert!(!cfg.mark_price_line);
    assert!(cfg.moonshot_zone);
    assert!(!stamp_carried_lines(&mut cfg, carried), "idempotent");
}

#[test]
fn the_popup_value_carries_nothing() {
    // A `CandleViewCfg` the popup writes starts from `default()` or from a value already carried
    // over, so it must compare equal to its own reload: the carrier is neither set nor saved.
    let cfg = CandleViewCfg::default();
    let text = toml::to_string(&cfg).expect("serializes");
    assert!(!text.contains("carried_lines"));
    let back: CandleViewCfg = toml::from_str(&text).expect("reloads");
    assert_eq!(back, cfg);
}
