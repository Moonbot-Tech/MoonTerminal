use super::*;
use moon_core::config::{ChartBucket, ChartGraphicsCfg};
use moon_core::market::candles::CandleViewCfg;

use crate::persistence::chart_persist::ChartTabSpec;

/// A tab spec exactly as a `charts.json` written BEFORE the move holds it: the switches under
/// `candle_view`, the liquidations toggle beside it, and no `chart_graphics` unless asked for.
fn old_spec(
    num: u32,
    candle_json: &str,
    liquidations: Option<bool>,
    graphics: bool,
) -> ChartTabSpec {
    let liq = liquidations.map_or(String::new(), |v| format!(r#","liquidations_enabled":{v}"#));
    let gfx = if graphics {
        r#","chart_graphics":{"marker_scale":2.0}"#
    } else {
        ""
    };
    let text = format!(
        r#"{{"group":"main","num":{num},"bucket":"Shared","candle_view":{candle_json}{liq}{gfx}}}"#
    );
    serde_json::from_str(&text).expect("an old spec loads")
}

/// A layout whose Main default carried `last_price_line = false` — the shape an old `layout.toml`
/// has, decoded through JSON because this crate carries no TOML reader and the serde shape is the
/// same — and the same layout after its own pass, with the kinds' carriers read first.
fn migrated_layout() -> (WindowLayout, KindCarried) {
    let mut layout: WindowLayout =
        serde_json::from_str(r#"{"candle_view":{"tf_min":5,"last_price_line":false}}"#)
            .expect("loads");
    let kinds = KindCarried::snapshot(&layout);
    assert!(layout.carry_lines_into_graphics());
    (layout, kinds)
}

#[test]
fn a_spec_with_its_own_candle_switches_gets_graphics_of_its_own() {
    let (layout, kinds) = migrated_layout();
    let mut spec = old_spec(1, r#"{"tf_min":1,"moonshot_zone":false}"#, None, false);

    assert!(carry_spec(&layout, &kinds, &mut spec));

    let cfg = spec.chart_graphics.expect("graphics of its own now");
    assert!(!cfg.moonshot_zone, "its own corridor switch");
    assert!(
        cfg.last_price_line,
        "unnamed in its own table, which replaced the kind's whole: the shipped default"
    );
    assert!(cfg.liquidations, "never named: the default");
    assert!(
        spec.candle_view.unwrap().carried_lines.is_none(),
        "consumed"
    );
    // Idempotent: a second pass finds nothing and changes nothing.
    assert!(!carry_spec(&layout, &kinds, &mut spec));
}

#[test]
fn a_spec_with_its_own_graphics_but_inherited_candles_takes_the_kinds_old_switches() {
    let (layout, kinds) = migrated_layout();
    let mut spec = old_spec(2, r#"{"tf_min":1}"#, None, true);
    spec.candle_view = None;

    assert!(carry_spec(&layout, &kinds, &mut spec));

    let cfg = spec.chart_graphics.expect("kept");
    assert!(
        !cfg.last_price_line,
        "drew Main's candle lines, keeps drawing them"
    );
    assert_eq!(
        cfg.marker_scale, 2.0,
        "its own graphics are otherwise untouched"
    );
}

#[test]
fn the_liquidations_toggle_lands_on_the_graphics_and_is_consumed() {
    let (layout, kinds) = migrated_layout();
    // Off with inherited graphics: needs graphics of its own.
    let mut off = old_spec(3, r#"{"tf_min":5}"#, Some(false), false);
    assert!(carry_spec(&layout, &kinds, &mut off));
    assert!(!off.chart_graphics.expect("created").liquidations);
    assert!(off.liquidations_enabled.is_none(), "consumed");
    // Explicitly ON with inherited graphics: what the default already says, so no override is
    // created — only the old key is dropped.
    let mut on = old_spec(4, r#"{"tf_min":5}"#, Some(true), false);
    assert!(carry_spec(&layout, &kinds, &mut on));
    assert!(on.chart_graphics.is_none());
    assert!(on.liquidations_enabled.is_none());
    // Off with graphics of its own: stamped in place.
    let mut own = old_spec(5, r#"{"tf_min":5}"#, Some(false), true);
    assert!(carry_spec(&layout, &kinds, &mut own));
    let cfg = own.chart_graphics.expect("kept");
    assert!(!cfg.liquidations);
    assert_eq!(cfg.marker_scale, 2.0);
}

/// The reason there is no marker: a spec written by THIS build carries nothing old, and the pass
/// must not stamp the kind's current default over what the popup wrote — or a line turned off in
/// the popup would come back on every launch.
#[test]
fn a_spec_written_after_the_move_is_left_alone() {
    let (layout, kinds) = migrated_layout();
    let mut spec = ChartTabSpec::new("main".to_string(), 6, ChartBucket::Shared);
    spec.candle_view = Some(CandleViewCfg::default());
    spec.chart_graphics = Some(ChartGraphicsCfg {
        last_price_line: true,
        marker_scale: 3.0,
        ..ChartGraphicsCfg::default()
    });
    let before = serde_json::to_string(&spec).expect("serializes");

    assert!(!carry_spec(&layout, &kinds, &mut spec));
    assert_eq!(serde_json::to_string(&spec).expect("serializes"), before);
}

/// Candles of a tab's own that name no switch were written after the move: the kind's carrier
/// says nothing about what THIS tab drew, so the tab is left alone even while the layout still
/// carries — which is exactly the state a launch after a failed layout save is in.
#[test]
fn a_spec_with_its_own_candles_that_carry_nothing_is_left_alone_while_the_kind_carries() {
    let (layout, kinds) = migrated_layout();
    let mut spec = old_spec(9, r#"{"tf_min":1}"#, None, false);
    assert!(!carry_spec(&layout, &kinds, &mut spec));
    assert!(spec.chart_graphics.is_none());
    // A carried switch that agrees with the kind's needs no graphics of its own either: only the
    // old key is consumed.
    let mut spec = old_spec(10, r#"{"tf_min":1,"last_price_line":false}"#, None, false);
    assert!(
        carry_spec(&layout, &kinds, &mut spec),
        "the old key is consumed"
    );
    assert!(
        spec.chart_graphics.is_none(),
        "but nothing differed from the kind's"
    );
}

#[test]
fn a_spec_following_both_chains_needs_nothing() {
    let (layout, kinds) = migrated_layout();
    // Old file, but this tab holds neither candles nor graphics of its own: its kind's migrated
    // default already draws what it drew, and the kind's carrier — which it DID draw — agrees
    // with that default by construction.
    let mut spec = old_spec(7, r#"{"tf_min":5}"#, None, false);
    spec.candle_view = None;
    assert!(!carry_spec(&layout, &kinds, &mut spec));
    assert!(spec.chart_graphics.is_none());
}

#[test]
fn the_old_keys_are_gone_after_one_save() {
    let (layout, kinds) = migrated_layout();
    let mut specs = vec![old_spec(
        8,
        r#"{"tf_min":1,"mark_price_line":false}"#,
        Some(false),
        false,
    )];
    assert!(carry_specs(&layout, &kinds, &mut specs));
    let text = serde_json::to_string(&specs).expect("serializes");
    assert!(!text.contains("liquidations_enabled"));
    assert!(!text.contains("carried_lines"));
    // The candle table's old key is gone; the switch lives on the graphics now.
    let value: serde_json::Value = serde_json::from_str(&text).expect("parses");
    assert!(value[0]["candle_view"].get("mark_price_line").is_none());
    assert_eq!(value[0]["chart_graphics"]["mark_price_line"], false);
    let back: Vec<ChartTabSpec> = serde_json::from_str(&text).expect("reloads");
    assert!(back[0].candle_view.unwrap().carried_lines.is_none());
    assert!(back[0].liquidations_enabled.is_none());
    let cfg = back[0].chart_graphics.expect("kept");
    assert!(!cfg.mark_price_line);
    assert!(!cfg.liquidations);
}
