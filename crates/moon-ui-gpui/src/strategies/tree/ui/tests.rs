//! Paste/Create target precedence plus UI-only folder occupancy, observer wiring, and
//! strategy-drag confinement oracles.

use gpui::{Bounds, WindowId, point, px, size};
use moon_core::feed::StrategyRow;
use moon_core::session::CoreId;

use super::{
    StratDrag, drag_chip_should_paint, footer_labels_fit, keep_ui_folder, resolve_paste_target,
    strat_drag_event_should_stop, strat_drag_move_should_stop,
};

/// The source file, read at COMPILE time so the guard below cannot drift from it.
const SRC: &str = include_str!("../ui.rs");
/// The observer must retire UI-only folders when the live strategy snapshot changes.
const STATE_SRC: &str = include_str!("../../state.rs");

/// Build the only strategy fields the folder-occupancy decision reads.
fn strategy(id: u64, folder_path: &str) -> StrategyRow {
    StrategyRow {
        id,
        name: format!("strategy-{id}"),
        kind: "Demo".to_string(),
        kind_ordinal: 1,
        folder_path: folder_path.to_string(),
        checked: false,
        is_short: false,
        fields: Vec::new(),
    }
}

/// Build a selected folder or strategy location for precedence tests.
fn at(core: CoreId, path: &str) -> Option<(CoreId, String)> {
    Some((core, path.to_string()))
}

/// `default_target` must actually DELEGATE, or the precedence proven below protects nothing.
///
/// Plausible edit this catches: inlining a strategy-first
/// `if let Some((core, id)) = self.selected` body in `default_target` would bypass the shared
/// precedence while every pure-function assertion in this file stayed green.
#[test]
fn default_target_resolves_through_the_shared_precedence() {
    let start = SRC
        .find("fn default_target")
        .expect("default_target must exist");
    let body = &SRC[start..];
    let end = body.find("\n    }").expect("its body must end");
    let body = &body[..end];
    assert!(
        body.contains("resolve_paste_target("),
        "default_target must delegate to resolve_paste_target"
    );
    assert!(
        !body.contains("return (core,"),
        "default_target must not re-implement its own precedence"
    );
}

/// A clicked folder wins over whatever strategy happens to still be the primary selection.
///
/// Plausible edit this catches: the two arms are reordered, or the folder arm is dropped
/// because "`selected` is always set anyway" — and Ctrl+V after clicking a folder lands in an
/// unrelated strategy's folder, or in the core root, building a second folder tree beside the
/// one the user was looking at.
#[test]
fn a_selected_folder_outranks_a_stale_strategy() {
    assert_eq!(
        resolve_paste_target(at(7, "grid/live"), at(3, "old"), Some(1)),
        (7, "grid/live".to_string()),
        "the folder was the last thing clicked, so it is the target — core included"
    );
}

/// With no folder selected, the primary strategy's own folder is the target.
#[test]
fn without_a_folder_the_strategy_supplies_the_target() {
    assert_eq!(
        resolve_paste_target(None, at(3, "old"), Some(1)),
        (3, "old".to_string())
    );
}

/// Nothing selected at all falls back to the first core's root — and to core 0 when there is
/// not even a core, rather than panicking on an empty tree.
#[test]
fn nothing_selected_falls_back_to_the_first_cores_root() {
    assert_eq!(
        resolve_paste_target(None, None, Some(1)),
        (1, String::new())
    );
    assert_eq!(resolve_paste_target(None, None, None), (0, String::new()));
}

/// A folder selected at the core ROOT is still a real answer, not an absent one.
#[test]
fn a_root_folder_selection_is_not_mistaken_for_nothing() {
    assert_eq!(
        resolve_paste_target(at(9, ""), at(3, "old"), Some(1)),
        (9, String::new()),
        "an empty path on core 9 means that core's root, not 'no selection'"
    );
}

/// `strategies/tree/ui.rs:keep_ui_folder`: replacing subtree occupancy with direct equality would
/// preserve a parent marker and reveal a ghost folder after the child strategy is later deleted.
#[test]
fn a_live_strategy_occupies_its_folder_and_every_ui_only_ancestor() {
    let rows = vec![strategy(1, "desk/live")];

    assert!(!keep_ui_folder("desk", Some(&rows)));
    assert!(!keep_ui_folder("desk/live", Some(&rows)));
    assert!(keep_ui_folder("desk/archive", Some(&rows)));
    assert!(keep_ui_folder("des", Some(&rows)));
    assert!(keep_ui_folder("desk", Some(&[])));
    assert!(keep_ui_folder("desk", None));
}

/// `strategies/state.rs:StrategiesView::observe_state`: removing the reconciliation call would
/// leave occupied markers cached and expose a ghost folder after an Analytics purge.
#[test]
fn strategy_snapshot_changes_reconcile_ui_only_folders() {
    let state_code: String = STATE_SRC
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        state_code.contains("this.reconcile_ui_folders(b.session.store());"),
        "the live snapshot observer must retire newly occupied UI-only folders"
    );
}

/// `tree/ui.rs:footer_labels_fit`: changing the inclusive threshold would either hide all labels
/// at the exact fitting width or show a mixed/clipped footer one pixel below its full requirement.
#[test]
fn footer_labels_switch_atomically_at_the_inclusive_boundary() {
    let fixed_chrome = 40.0;
    let buttons_and_staged_label = 60.0;

    assert!(!footer_labels_fit(
        99.0,
        fixed_chrome,
        buttons_and_staged_label
    ));
    assert!(footer_labels_fit(
        100.0,
        fixed_chrome,
        buttons_and_staged_label
    ));
    assert!(footer_labels_fit(
        101.0,
        fixed_chrome,
        buttons_and_staged_label
    ));
}

/// Independent tree rectangle used by the confinement oracles. Origin (10, 20), size 100×200, so
/// the half-open far edge is x = 110 and y = 220.
fn tree_field() -> Bounds<gpui::Pixels> {
    Bounds::new(point(px(10.0), px(20.0)), size(px(100.0), px(200.0)))
}

/// Origin window id for confinement oracles; every same-window sample uses this.
fn origin() -> WindowId {
    WindowId::from(1)
}

/// Foreign window id; a sample here is the chart/second-window leak, never the origin.
fn other_window() -> WindowId {
    WindowId::from(2)
}

/// `drag_chip_should_paint`: same window + pointer inside the tree paints. Replacing the body
/// with `true` reintroduces the chart duplicate and the params-pane leak.
#[test]
fn drag_chip_paints_only_inside_the_origin_tree_field() {
    let tree = tree_field();
    let origin = origin();

    assert!(
        drag_chip_should_paint(origin, origin, point(px(10.0), px(20.0)), Some(tree)),
        "the inclusive origin corner is inside the folders-and-strategies field"
    );
    assert!(drag_chip_should_paint(
        origin,
        origin,
        point(px(60.0), px(100.0)),
        Some(tree)
    ));
    assert!(
        !drag_chip_should_paint(origin, origin, point(px(120.0), px(100.0)), Some(tree)),
        "a pointer to the right of the tree (versions/sections/params) must hide the chip"
    );
    assert!(
        !drag_chip_should_paint(
            origin,
            other_window(),
            point(px(60.0), px(100.0)),
            Some(tree)
        ),
        "another window id with a pointer inside the rectangle is the chart leak"
    );
    assert!(
        !drag_chip_should_paint(origin, origin, point(px(60.0), px(100.0)), None),
        "missing bounds must hide, never leak, on the first frame before prepaint"
    );
    assert!(
        !drag_chip_should_paint(origin, origin, point(px(110.0), px(20.0)), Some(tree)),
        "x == origin.x + width is the half-open far edge and must hide"
    );
    assert!(!drag_chip_should_paint(
        origin,
        origin,
        point(px(10.0), px(220.0)),
        Some(tree)
    ));
}

/// Deleting the helper call from `DragChip::render` would keep every oracle green while the
/// production overlay painted globally again.
#[test]
fn drag_chip_render_consults_the_paint_gate() {
    let start = SRC
        .find("impl Render for DragChip")
        .expect("DragChip::render must exist");
    let body = &SRC[start..];
    assert!(
        body.contains("drag_chip_should_paint("),
        "DragChip::render must call drag_chip_should_paint so the overlay cannot paint globally"
    );
    assert!(
        body.contains("window.defer("),
        "outside StratDrag must defer stop_active_drag until after overlay restore"
    );
    let before_defer = body
        .split("window.defer(")
        .next()
        .expect("the deferred stop must exist");
    assert!(
        !before_defer.contains("stop_active_drag("),
        "DragChip::render must not call stop_active_drag directly during overlay prepaint"
    );
}

/// Every confined DragChip hides only through the same flag that schedules cancellation.
///
/// Removing `stop_when_outside` would let an unconfined payload disappear while staying active;
/// removing the paint predicate would restore the cross-window duplicate artifact.
#[test]
fn confined_drag_preview_pairs_its_paint_gate_with_cancellation() {
    let start = SRC
        .find("impl Render for DragChip")
        .expect("DragChip::render must exist");
    let body = &SRC[start..];
    let hide = body
        .find("return div()")
        .expect("the empty-chip hide path must exist");
    let last_if = body[..hide]
        .rfind("if ")
        .expect("the empty-chip return must be conditioned");
    let cond = &body[last_if..hide];
    assert!(
        cond.contains("stop_when_outside"),
        "returning an empty chip must require the same flag that schedules drag cancellation"
    );
    assert!(
        cond.contains("drag_chip_should_paint("),
        "StratDrag must still hide via drag_chip_should_paint when outside the origin tree"
    );
}

/// Interior StratDrag samples must keep the session so a same-core move or cross-core copy can
/// still complete on a folder or core row.
#[test]
fn strat_drag_survives_inside_the_tree_field() {
    let tree = tree_field();
    let origin = origin();
    assert!(
        !strat_drag_move_should_stop(origin, origin, point(px(60.0), px(100.0)), Some(tree)),
        "a pointer still inside strat-tree-scroll must not cancel the drag"
    );
    assert!(
        !strat_drag_move_should_stop(origin, origin, point(px(10.0), px(20.0)), Some(tree)),
        "the inclusive origin corner is a live interior sample"
    );
    assert!(
        !strat_drag_move_should_stop(origin, origin, point(px(60.0), px(100.0)), None),
        "missing bounds must not abort a drag that started before the first prepaint"
    );
}

/// Leaving the live tree field, or sampling another window, must cancel StratDrag. Returning
/// `false` here would hide the chip while the global drag continued across the screen.
#[test]
fn strat_drag_cancels_outside_the_tree_field() {
    let tree = tree_field();
    let origin = origin();
    assert!(
        strat_drag_move_should_stop(origin, origin, point(px(120.0), px(100.0)), Some(tree)),
        "a move over params/versions must stop the strategy drag"
    );
    assert!(
        strat_drag_move_should_stop(origin, origin, point(px(110.0), px(20.0)), Some(tree)),
        "the half-open far edge is already outside the field"
    );
    assert!(
        strat_drag_move_should_stop(
            origin,
            other_window(),
            point(px(60.0), px(100.0)),
            Some(tree)
        ),
        "a sample in another window must stop even when the pointer sits inside the rectangle"
    );
    assert!(
        strat_drag_move_should_stop(origin, other_window(), point(px(60.0), px(100.0)), None),
        "an origin-window mismatch must cancel even before tree bounds exist"
    );
}

/// Event-time cancellation must read origin from the StratDrag payload. Passing the receiving
/// window as both arguments would keep a second Strategies window from ever seeing a mismatch.
#[test]
fn strat_drag_event_uses_payload_origin_not_the_receiver() {
    let tree = tree_field();
    let drag = StratDrag {
        core: 7,
        ids: vec![9],
        origin_window: origin(),
    };
    assert!(
        strat_drag_event_should_stop(&drag, other_window(), point(px(60.0), px(100.0)), None),
        "payload origin vs receiving window must cancel independently of bounds"
    );
    assert!(
        !strat_drag_event_should_stop(&drag, origin(), point(px(60.0), px(100.0)), Some(tree)),
        "same-window interior samples must keep the session"
    );
    assert!(
        strat_drag_event_should_stop(&drag, origin(), point(px(120.0), px(100.0)), Some(tree)),
        "same-window samples past the tree field must cancel"
    );
}
