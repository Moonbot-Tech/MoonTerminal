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

/// A spec carrying its own graphics override, written by a build in which the global veto still
/// applied — so its `hide_order_move_history` is the shipped `false`.
fn overridden_spec(num: u32) -> ChartTabSpec {
    let mut s = spec(num);
    s.chart_graphics = Some(ChartGraphicsCfg::default());
    s
}

/// `orders.toml` as a user who unticked "show path" in ONE theme left it.
fn orders_hidden_in(light: bool) -> OrdersStyleSet {
    let mut set = OrdersStyleSet::default();
    set.get_mut(light).path.show = false;
    set
}

/// A user who hid the path in the dark set keeps it hidden on every tab, whichever theme is live:
/// the base default, a stored per-kind default and a tab's own override all take the flag.
#[test]
fn a_hidden_path_lands_on_every_default_and_override() {
    let mut layout = Box::new(WindowLayout::default());
    let mut compare = ChartGraphicsCfg::default();
    compare.show_real_trades = !compare.show_real_trades;
    assert!(layout.set_chart_graphics_default(ChartTabKind::Compare, compare));
    let mut specs = vec![spec(1), overridden_spec(2)];

    let carried = migrate_path_visibility(&mut layout, &mut specs, &orders_hidden_in(false))
        .expect("the pass runs once");

    assert!(carried.specs_changed, "the overriding tab was rewritten");
    for kind in ChartTabKind::ALL {
        assert!(
            layout.chart_graphics_for(kind).hide_order_move_history,
            "{kind:?} must open with the history hidden"
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
    assert!(
        specs[1]
            .chart_graphics
            .expect("the override stays")
            .hide_order_move_history,
        "the override draws what the global veto drew"
    );
}

/// The light set alone counts as well: the veto read the set of the theme that was live, and
/// the pass must not depend on which theme happens to be on at this launch.
#[test]
fn the_light_set_alone_hides_the_path() {
    let mut layout = Box::new(WindowLayout::default());
    let mut specs = vec![spec(1)];
    let carried = migrate_path_visibility(&mut layout, &mut specs, &orders_hidden_in(true))
        .expect("the pass runs once");
    assert!(!carried.specs_changed, "no override, nothing to rewrite");
    assert!(layout.chart_graphics.hide_order_move_history);
}

/// A user who never touched the toggle gets nothing written: the flags already say "shown", and a
/// tab that hid the history on its own keeps that choice.
#[test]
fn a_shown_path_writes_nothing() {
    let mut layout = Box::new(WindowLayout::default());
    let mut hidden_on_its_own = overridden_spec(1);
    hidden_on_its_own
        .chart_graphics
        .as_mut()
        .expect("fixture")
        .hide_order_move_history = true;
    let mut specs = vec![overridden_spec(2), hidden_on_its_own];
    let before: Vec<_> = ChartTabKind::ALL
        .iter()
        .map(|kind| layout.chart_graphics_for(*kind))
        .collect();

    let carried = migrate_path_visibility(&mut layout, &mut specs, &OrdersStyleSet::default())
        .expect("the pass runs once");

    assert_eq!(carried, Carried::default());
    for (kind, was) in ChartTabKind::ALL.iter().zip(before) {
        assert_eq!(
            layout.chart_graphics_for(*kind),
            was,
            "{kind:?} opens with what it opened with"
        );
    }
    assert!(
        !specs[0]
            .chart_graphics
            .expect("fixture")
            .hide_order_move_history,
        "a shown path stays shown on an overriding tab"
    );
    assert!(
        specs[1]
            .chart_graphics
            .expect("fixture")
            .hide_order_move_history,
        "a tab's own choice to hide is kept"
    );
}

/// An override that already hides the history is not counted as a rewrite, so a profile where
/// every override agrees costs no `charts.json` save.
#[test]
fn an_override_already_hidden_is_not_a_rewrite() {
    let mut layout = Box::new(WindowLayout::default());
    let mut s = overridden_spec(1);
    s.chart_graphics
        .as_mut()
        .expect("fixture")
        .hide_order_move_history = true;
    let mut specs = vec![s];
    let carried = migrate_path_visibility(&mut layout, &mut specs, &orders_hidden_in(false))
        .expect("the pass runs once");
    assert!(!carried.specs_changed);
}

/// The marker is the caller's to set, and once it is set the pass is over for good.
#[test]
fn a_finished_profile_is_left_alone() {
    let mut layout = Box::new(WindowLayout::default());
    layout.chart_path_visibility_migrated = true;
    let mut specs = vec![overridden_spec(1)];
    assert!(migrate_path_visibility(&mut layout, &mut specs, &orders_hidden_in(false)).is_none());
    assert!(
        !specs[0]
            .chart_graphics
            .expect("fixture")
            .hide_order_move_history,
        "a second pass must not re-hide what the user has since shown"
    );
}
