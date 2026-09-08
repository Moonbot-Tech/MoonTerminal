use super::*;
use moon_core::config::{ChartLabelField, ChartTabKind, LabelFlow, LabelZone};

/// One spec, as `charts.json` holds it, with the two legacy keys set.
///
/// A spec carries a whole caption set inline, so these live on the HEAP in every test below: a
/// handful of them on a test thread's stack is enough to overflow it.
fn spec(num: u32, cancel: Option<ChartBtnPos>, panic: Option<ChartBtnPos>) -> ChartTabSpec {
    let mut spec = ChartTabSpec::new(
        "main".to_string(),
        num,
        moon_core::config::ChartBucket::Shared,
    );
    spec.cancel_buy_pos = cancel;
    spec.panic_sell_pos = panic;
    spec
}

/// A layout as the PREVIOUS build left it: the shipped captions, with no button among them.
///
/// The state every existing profile is actually in, and the one the pass exists for.
/// `WindowLayout::default()` is NOT that state any more — the shipped set now carries the pair —
/// so a test starting from it would prove only that the pass leaves a finished profile alone.
fn old_layout() -> Box<WindowLayout> {
    let mut layout = Box::new(WindowLayout::default());
    let mut cfg = ChartLabelsCfg::default();
    for row in cfg.rows.iter_mut() {
        if row.holds_action() {
            *row = ChartLabelRow::default();
        }
    }
    cfg.sanitize();
    assert!(
        !holds_a_button(&cfg),
        "the fixture must start where an existing profile does"
    );
    layout.chart_labels = cfg;
    layout
}

/// A caption set filled to the module ceiling, so the pass has nowhere to put a button.
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

/// The buttons a caption set places, as `(field, zone, align, placement)`, in configured order.
fn buttons(cfg: &ChartLabelsCfg) -> Vec<(ChartLabelField, LabelZone, LabelAlign, LabelFlow)> {
    cfg.rows
        .iter()
        .filter(|row| row.holds_action())
        .map(|row| (row.parts[0].field, row.zone, row.align, row.placement))
        .collect()
}

/// A tab that never touched the setting keeps following its default, which already draws the pair.
///
/// The one case that must NOT produce an override: freezing a follower's captions into a copy is
/// how a later build's improved default stops reaching the reader.
#[test]
fn an_untouched_tab_is_left_following_its_default() {
    let mut layout = old_layout();
    let mut specs = vec![spec(1, None, None)];
    let carried = migrate_action_buttons(&mut layout, &mut specs).expect("the pass runs once");
    assert!(
        !carried.specs_changed,
        "nothing in any tab changed, so charts.json is not rewritten"
    );
    assert!(
        specs[0].chart_labels.is_none(),
        "a tab that stated nothing keeps no override of its own"
    );
    assert_eq!(
        buttons(&layout.chart_labels),
        vec![
            (
                ChartLabelField::ActPanicSell,
                LabelZone::ChartBottom,
                LabelAlign::Right,
                LabelFlow::Column
            ),
            (
                ChartLabelField::ActCancelBuy,
                LabelZone::ChartBottom,
                LabelAlign::Right,
                LabelFlow::Row
            ),
        ],
        "and the default it follows gains the pair, where the old layout drew it"
    );
}

/// A tab that MOVED a button gets an override built from what it was drawing, buttons included.
#[test]
fn a_moved_button_becomes_a_caption_where_it_stood() {
    let mut layout = old_layout();
    let mut specs = vec![spec(1, Some(ChartBtnPos::Left), Some(ChartBtnPos::Left))];
    migrate_action_buttons(&mut layout, &mut specs);
    let cfg = specs[0]
        .chart_labels
        .as_ref()
        .expect("a moved button needs a per-tab home");
    assert_eq!(
        buttons(cfg),
        vec![
            (
                ChartLabelField::ActCancelBuy,
                LabelZone::ChartBottom,
                LabelAlign::Left,
                LabelFlow::Column
            ),
            (
                ChartLabelField::ActPanicSell,
                LabelZone::ChartBottom,
                LabelAlign::Left,
                LabelFlow::Row
            ),
        ],
        "both on the left edge, on ONE line, cancel before panic"
    );
    assert!(
        cfg.contains(ChartLabelField::Coin),
        "and the captions the tab was already drawing survive"
    );
    assert_eq!(
        (specs[0].cancel_buy_pos, specs[0].panic_sell_pos),
        (None, None),
        "the legacy keys are cleared, so a second pass cannot re-add anything"
    );
}

/// A hidden button stays hidden: no row is written for it at all.
#[test]
fn a_hidden_button_writes_no_row() {
    let mut layout = old_layout();
    let mut specs = vec![spec(1, Some(ChartBtnPos::Hide), Some(ChartBtnPos::Right))];
    migrate_action_buttons(&mut layout, &mut specs);
    let cfg = specs[0].chart_labels.as_ref().expect("panic still moved");
    assert_eq!(
        buttons(cfg),
        vec![(
            ChartLabelField::ActPanicSell,
            LabelZone::ChartBottom,
            LabelAlign::Right,
            LabelFlow::Column
        )],
        "only the button that was drawn becomes a caption"
    );
}

/// An absent key meant the shipped position, not "no button".
#[test]
fn one_stated_key_leaves_the_other_at_its_shipped_place() {
    let mut layout = old_layout();
    let mut specs = vec![spec(1, Some(ChartBtnPos::Left), None)];
    migrate_action_buttons(&mut layout, &mut specs);
    let cfg = specs[0].chart_labels.as_ref().expect("cancel moved");
    assert_eq!(
        buttons(cfg),
        vec![
            (
                ChartLabelField::ActCancelBuy,
                LabelZone::ChartBottom,
                LabelAlign::Left,
                LabelFlow::Column
            ),
            (
                ChartLabelField::ActPanicSell,
                LabelZone::ChartBottom,
                LabelAlign::Right,
                LabelFlow::Column
            ),
        ],
        "panic keeps the right edge the absent key meant"
    );
}

/// A tab with its OWN caption set keeps every module it had and gains the buttons.
#[test]
fn a_tab_with_its_own_captions_keeps_them() {
    let mut layout = old_layout();
    let mut own = ChartLabelsCfg::empty();
    own.push_row(
        ChartLabelField::LastPrice,
        LabelZone::ChartTop,
        LabelAlign::Left,
    );
    let mut specs = vec![spec(
        2,
        Some(ChartBtnPos::Center),
        Some(ChartBtnPos::Center),
    )];
    specs[0].chart_labels = Some(own);
    migrate_action_buttons(&mut layout, &mut specs);
    let cfg = specs[0].chart_labels.as_ref().expect("its own set");
    assert!(cfg.contains(ChartLabelField::LastPrice), "its module stays");
    assert_eq!(
        buttons(cfg),
        vec![
            (
                ChartLabelField::ActCancelBuy,
                LabelZone::ChartBottom,
                LabelAlign::Center,
                LabelFlow::Column
            ),
            (
                ChartLabelField::ActPanicSell,
                LabelZone::ChartBottom,
                LabelAlign::Center,
                LabelFlow::Row
            ),
        ],
        "and both buttons land centred, on one line"
    );
}

/// A stored per-kind default gets the pair too — it is what its tabs were drawing.
#[test]
fn a_stored_kind_default_gains_the_pair() {
    let mut layout = old_layout();
    let mut own = ChartLabelsCfg::empty();
    own.push_row(
        ChartLabelField::LastPrice,
        LabelZone::ChartTop,
        LabelAlign::Left,
    );
    layout.store_chart_labels(ChartTabKind::AddTo, own);
    migrate_action_buttons(&mut layout, &mut Vec::new());
    let cfg = layout
        .stored_chart_labels(ChartTabKind::AddTo)
        .expect("the kind holds its own set");
    assert_eq!(buttons(cfg).len(), 2, "its tabs keep both buttons");
    assert!(
        layout.stored_chart_labels(ChartTabKind::Compare).is_none(),
        "a kind that stored nothing is not given a copy to follow"
    );
}

/// The pass is one-shot: a set that already places a button is never given a second one.
#[test]
fn a_second_pass_adds_nothing() {
    let mut layout = old_layout();
    let mut specs = vec![spec(1, Some(ChartBtnPos::Left), Some(ChartBtnPos::Left))];
    migrate_action_buttons(&mut layout, &mut specs);
    let after_first = specs[0].chart_labels.clone();
    // The marker is the caller's to commit, so the guard that matters here is the one INSIDE the
    // pass: a caption set that already holds a button is left alone.
    migrate_action_buttons(&mut layout, &mut specs);
    assert_eq!(
        specs[0].chart_labels, after_first,
        "a repeat pass must not duplicate the buttons it already placed"
    );
    assert_eq!(
        buttons(&layout.chart_labels).len(),
        2,
        "nor double the global default's pair"
    );
}

/// At the module ceiling the pass gives up LOUDLY rather than quietly rewriting a profile.
///
/// Two things must not happen: a caption of the reader's must not be dropped to make room, and a
/// tab that follows a default must not be frozen into a copy that carries no button either — it
/// would lose the buttons AND stop following.
#[test]
fn a_full_profile_is_left_exactly_as_it_was() {
    let mut layout = old_layout();
    layout.chart_labels = full_of_modules();
    let before = layout.chart_labels.clone();
    let mut specs = vec![spec(1, Some(ChartBtnPos::Left), Some(ChartBtnPos::Left))];
    let carried = migrate_action_buttons(&mut layout, &mut specs).expect("the pass runs");
    assert_eq!(
        layout.chart_labels, before,
        "every module the reader had stays"
    );
    assert!(
        specs[0].chart_labels.is_none(),
        "and the tab is not frozen into a copy with no buttons in it"
    );
    assert!(
        !carried.specs_changed,
        "so charts.json has nothing to record"
    );
}

/// A tab whose OWN set is full keeps it, buttons or not.
#[test]
fn a_full_tab_keeps_its_own_captions() {
    let mut layout = old_layout();
    let mut specs = vec![spec(1, Some(ChartBtnPos::Left), Some(ChartBtnPos::Left))];
    specs[0].chart_labels = Some(full_of_modules());
    let before = specs[0].chart_labels.clone();
    migrate_action_buttons(&mut layout, &mut specs);
    assert_eq!(
        specs[0].chart_labels, before,
        "no module of the reader's is dropped to fit a button"
    );
}

/// Both buttons hidden is a CHOICE, and it survives a default that now draws them.
#[test]
fn a_tab_that_hid_both_buttons_keeps_them_hidden() {
    let mut layout = old_layout();
    let mut specs = vec![spec(1, Some(ChartBtnPos::Hide), Some(ChartBtnPos::Hide))];
    migrate_action_buttons(&mut layout, &mut specs);
    let cfg = specs[0]
        .chart_labels
        .as_ref()
        .expect("hiding both needs a per-tab home, since the default now draws them");
    assert!(buttons(cfg).is_empty(), "no button is drawn on that tab");
    assert!(
        cfg.contains(ChartLabelField::Coin),
        "and everything else it drew is still there"
    );
}
