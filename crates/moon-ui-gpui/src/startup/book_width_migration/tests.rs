use super::*;
use moon_core::config::{ChartGraphicsCfg, ChartTabKind};

use crate::persistence::chart_persist::ChartTabSpec;

/// One spec, as `charts.json` holds it.
fn spec(num: u32) -> ChartTabSpec {
    ChartTabSpec::new(
        "main".to_string(),
        num,
        moon_core::config::ChartBucket::Shared,
    )
}

/// A spec carrying its own graphics override, written before the width moved per tab — so its
/// `book_width_px` is the serde default.
fn overridden_spec(num: u32) -> ChartTabSpec {
    let mut s = spec(num);
    s.chart_graphics = Some(ChartGraphicsCfg::default());
    s
}

/// A user who widened the book keeps that width on every tab: the base default, a stored per-kind
/// default and a tab's own override all take it, and a tab following its default keeps following.
#[test]
fn a_stored_width_lands_on_every_default_and_override() {
    let mut layout = Box::new(WindowLayout::default());
    let mut compare = ChartGraphicsCfg::default();
    compare.show_real_trades = !compare.show_real_trades;
    assert!(layout.set_chart_graphics_default(ChartTabKind::Compare, compare));
    let mut specs = vec![spec(1), overridden_spec(2)];

    let carried =
        migrate_book_width(&mut layout, &mut specs, Some(340.0)).expect("the pass runs once");

    assert!(carried.specs_changed, "the overriding tab was rewritten");
    for kind in ChartTabKind::ALL {
        assert_eq!(
            layout.chart_graphics_for(kind).book_width_px,
            340.0,
            "{kind:?} must open with the stored width"
        );
    }
    assert_eq!(
        layout
            .chart_graphics_for(ChartTabKind::Compare)
            .show_real_trades,
        compare.show_real_trades,
        "the stored default keeps its other values"
    );
    assert!(
        specs[0].chart_graphics.is_none(),
        "a tab following its default keeps following it"
    );
    assert_eq!(
        specs[1]
            .chart_graphics
            .expect("the override stays")
            .book_width_px,
        340.0,
        "the override draws what the global width drew"
    );
}

/// A profile with no stored width, or with the shipped one, gets nothing written: every per-tab
/// field already reads 220.
#[test]
fn no_width_or_the_shipped_one_writes_nothing() {
    for legacy in [None, Some(book_width::DEFAULT)] {
        let mut layout = Box::new(WindowLayout::default());
        let before = layout.chart_graphics;
        let mut specs = vec![overridden_spec(1)];
        let carried =
            migrate_book_width(&mut layout, &mut specs, legacy).expect("the pass runs once");
        assert_eq!(carried, Carried::default(), "{legacy:?}");
        assert_eq!(layout.chart_graphics, before, "{legacy:?}");
    }
}

/// The marker is the caller's to set, and once it is set the pass is over for good: the retired
/// key may still be in `settings.toml`, and a width chosen per tab since must survive the launch.
#[test]
fn a_finished_profile_is_left_alone() {
    let mut layout = Box::new(WindowLayout::default());
    layout.chart_book_width_migrated = true;
    let mut specs = vec![overridden_spec(1)];
    assert!(migrate_book_width(&mut layout, &mut specs, Some(340.0)).is_none());
    assert_eq!(
        specs[0].chart_graphics.expect("fixture").book_width_px,
        book_width::DEFAULT,
        "a second pass must not overwrite a width set since"
    );
}
