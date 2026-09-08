use super::*;
use moon_core::config::{ChartLabelField, ChartLabelsCfg, ChartTabKind, LabelAlign, LabelZone};

use crate::persistence::chart_persist::ChartTabSpec;

/// One spec, as `charts.json` holds it.
fn spec(num: u32) -> ChartTabSpec {
    ChartTabSpec::new(
        "main".to_string(),
        num,
        moon_core::config::ChartBucket::Shared,
    )
}

/// A layout as the PREVIOUS build left it: the shipped captions, without the filter column.
///
/// `WindowLayout::default()` is NOT that state any more — the shipped set now carries the module —
/// so a test starting from it would prove only that the pass leaves a finished profile alone.
fn old_layout() -> Box<WindowLayout> {
    let mut layout = Box::new(WindowLayout::default());
    let mut cfg = ChartLabelsCfg::default();
    for row in cfg.rows.iter_mut() {
        if row.holds_strategy_filters() {
            *row = ChartLabelRow::default();
        }
    }
    cfg.sanitize();
    assert!(
        !holds_filters(&cfg),
        "the fixture must start where an existing profile does"
    );
    layout.chart_labels = cfg;
    layout.chart_strategy_filters_migrated = false;
    layout
}

/// A caption set filled to the module ceiling, so the pass has nowhere to put a row.
fn full_of_modules() -> ChartLabelsCfg {
    let mut cfg = ChartLabelsCfg::empty();
    while cfg
        .push_row(
            ChartLabelField::LastPrice,
            LabelZone::ChartTop,
            LabelAlign::Left,
        )
        .is_some()
    {}
    assert_eq!(cfg.used_rows(), cfg.rows.len(), "filled to the ceiling");
    cfg
}

/// A tab that never touched captions keeps following its default, which already draws the column
/// after the pass has repaired that default.
#[test]
fn an_untouched_tab_is_left_following_its_default() {
    let mut layout = old_layout();
    let mut specs = vec![spec(1)];
    let carried = migrate_strategy_filters(&mut layout, &mut specs).expect("the pass runs once");
    assert!(
        !carried.specs_changed,
        "nothing in any tab changed, so charts.json is not rewritten"
    );
    assert!(
        specs[0].chart_labels.is_none(),
        "a tab that stated nothing keeps no override of its own"
    );
    assert!(
        holds_filters(&layout.chart_labels),
        "the live default gained the column"
    );
}

/// A live tab that froze its own captions gets the column appended to THAT set.
#[test]
fn a_tab_with_its_own_captions_gains_the_column() {
    let mut layout = old_layout();
    let mut tab = spec(1);
    tab.chart_labels = Some(layout.chart_labels.clone());
    let mut specs = vec![tab];
    let carried = migrate_strategy_filters(&mut layout, &mut specs).expect("the pass runs once");
    assert!(carried.specs_changed);
    assert!(
        holds_filters(specs[0].chart_labels.as_ref().expect("override kept")),
        "the tab's own set gained the column"
    );
}

/// Compare and Trade are not live views of a coin's skip reasons; the pass must not plant a module
/// there that those shipped sets never included.
#[test]
fn compare_and_trade_sets_are_left_alone() {
    let mut layout = old_layout();
    layout.store_chart_labels(ChartTabKind::Compare, ChartLabelsCfg::compare_default());
    layout.store_chart_labels(ChartTabKind::Trade, ChartLabelsCfg::trade_default());
    let mut compare_tab = spec(2);
    compare_tab.compare_anchor = Some((1, "BTCUSDT".into()));
    compare_tab.chart_labels = Some(ChartLabelsCfg::compare_default());
    let mut specs = vec![compare_tab];
    migrate_strategy_filters(&mut layout, &mut specs).expect("the pass runs once");
    assert!(
        !holds_filters(layout.stored_chart_labels(ChartTabKind::Compare).unwrap()),
        "the comparison default must not gain the column"
    );
    assert!(
        !holds_filters(layout.stored_chart_labels(ChartTabKind::Trade).unwrap()),
        "the trade default must not gain the column"
    );
    assert!(
        !holds_filters(specs[0].chart_labels.as_ref().unwrap()),
        "a comparison tab's override must not gain the column"
    );
}

/// A set that already places the column — hidden included — is not given a second one.
#[test]
fn a_set_that_already_holds_the_column_is_left_alone() {
    let mut layout = Box::new(WindowLayout::default());
    layout.chart_strategy_filters_migrated = false;
    assert!(holds_filters(&layout.chart_labels));
    let before = layout.chart_labels.clone();
    let mut specs = Vec::new();
    migrate_strategy_filters(&mut layout, &mut specs).expect("the pass runs once");
    assert_eq!(layout.chart_labels, before);
}

/// Out of room is not a failure: the reader is already at the sixteen-module ceiling.
#[test]
fn a_full_set_is_not_rewritten() {
    let mut layout = old_layout();
    layout.chart_labels = full_of_modules();
    let mut tab = spec(1);
    tab.chart_labels = Some(full_of_modules());
    let mut specs = vec![tab];
    let carried = migrate_strategy_filters(&mut layout, &mut specs).expect("the pass runs once");
    assert!(!carried.specs_changed);
    assert!(!holds_filters(&layout.chart_labels));
    assert!(!holds_filters(specs[0].chart_labels.as_ref().unwrap()));
}

/// The marker is what makes a repeated pass a no-op, including after the reader deleted the module.
#[test]
fn a_second_pass_does_not_resurrect_a_deleted_module() {
    let mut layout = old_layout();
    let mut specs = Vec::new();
    assert!(migrate_strategy_filters(&mut layout, &mut specs).is_some());
    for row in layout.chart_labels.rows.iter_mut() {
        if row.holds_strategy_filters() {
            *row = ChartLabelRow::default();
        }
    }
    layout.chart_labels.sanitize();
    layout.chart_strategy_filters_migrated = true;
    assert!(migrate_strategy_filters(&mut layout, &mut specs).is_none());
    assert!(
        !holds_filters(&layout.chart_labels),
        "deleting the module after migration must not bring it back"
    );
}
