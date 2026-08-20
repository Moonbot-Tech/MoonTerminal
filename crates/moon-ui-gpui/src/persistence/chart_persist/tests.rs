//! Persistence compatibility for the chart-tab schema: detached window geometry and the
//! per-tab display overrides that arrived after files were already on disk.

use super::*;

/// A `charts.json` written before chart geometry carried a display identity must still load, and a
/// geometry that names no display must not start writing the key.
///
/// `detached` inside this file is what reopens a chart tab as its own window; a decode broken by
/// the new field would turn every detached chart back into an in-window tab on the next launch.
#[test]
fn detached_chart_geometry_without_a_display_identity_still_loads() {
    let legacy = r#"{"x":400,"y":260,"w":900,"h":620}"#;
    let geom: WinGeom = serde_json::from_str(legacy).expect("a pre-display WinGeom must decode");
    assert_eq!((geom.x, geom.y, geom.w, geom.h), (400, 260, 900, 620));
    assert_eq!(geom.display_uuid, None);

    let re_encoded = serde_json::to_string(&geom).expect("geometry must re-encode");
    assert!(
        !re_encoded.contains("display_uuid"),
        "a geometry with no display must keep the file's previous shape"
    );

    let identity = uuid::Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);
    let saved = WinGeom {
        display_uuid: Some(identity),
        ..geom
    };
    let back: WinGeom = serde_json::from_str(&serde_json::to_string(&saved).unwrap())
        .expect("geometry must round-trip");
    assert_eq!(back.display_uuid, Some(identity));
    assert_eq!(
        back, saved,
        "equality must cover the display: on macOS the same x/y on another monitor is a real move"
    );
}

/// A `charts.json` written before chart-drawing settings were per tab must still load, and its tabs
/// must come back with NO override — that is what makes them keep following `layout.chart_graphics`
/// and leaves every existing installation looking exactly as it did.
#[test]
fn a_spec_without_chart_graphics_loads_as_inheriting_the_global_default() {
    let legacy = r#"{"group":"main","num":2,"bucket":"Shared","candle_view":null}"#;
    let spec: ChartTabSpec = serde_json::from_str(legacy).expect("a pre-graphics spec must decode");
    assert_eq!(
        spec.chart_graphics, None,
        "no override means: follow the global default"
    );
    assert_eq!(spec.num, 2);

    let saved = ChartTabSpec {
        chart_graphics: Some(moon_core::config::ChartGraphicsCfg {
            trade_arrow_scale: 1.6,
            connector_thickness_px: 3.0,
            show_real_trades: true,
            show_emulator_trades: false,
            hide_closed_sell_line: false,
            ..moon_core::config::ChartGraphicsCfg::default()
        }),
        ..ChartTabSpec::new("main".to_string(), 2, ChartBucket::Shared)
    };
    let back: ChartTabSpec = serde_json::from_str(&serde_json::to_string(&saved).unwrap())
        .expect("a spec with an override must round-trip");
    assert_eq!(back.chart_graphics, saved.chart_graphics);
}
