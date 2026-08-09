//! Structural regression coverage for the retained Report scope control.

/// Return the source between two stable function anchors.
///
/// Args:
///     source: Complete source file.
///     start: Opening function signature fragment.
///     end: Following function signature fragment.
///
/// Returns:
///     The bounded source slice containing the requested function.
fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let from = source.find(start).expect("start function must exist");
    let tail = &source[from..];
    let to = tail.find(end).expect("end function must exist");
    &tail[..to]
}

/// Adding an owner update to `ReportScopeControl::set_menu_open` must fail this assertion; popup
/// open/close would then invalidate the full Report data owner and restore the multi-second stall.
#[test]
fn menu_visibility_updates_only_the_retained_child() {
    let source = include_str!("../controls.rs");
    let set_open = between(source, "fn set_menu_open", "fn select_side");
    let set_open_code = set_open
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let render = between(
        source,
        "impl Render for ReportScopeControl",
        "impl ReportPanel",
    );

    assert!(set_open_code.contains("self.menu_open = open"));
    assert!(set_open_code.contains("cx.notify()"));
    assert!(!set_open_code.contains("owner"));
    assert!(render.contains(".open(self.menu_open)"));
    assert!(render.contains("this.set_menu_open(open, cx)"));
}

/// Removing the guarded owner setters from `ReportScopeControl` must fail this assertion; actual
/// side, kind, and deleted selections would otherwise repaint only the child without a new query.
#[test]
fn real_scope_selections_reach_guarded_owner_invalidation() {
    let controls = include_str!("../controls.rs");
    let actions = include_str!("../actions.rs");
    let side = between(controls, "fn select_side", "fn select_kind");
    let kind = between(controls, "fn select_kind", "fn toggle_deleted");
    let deleted = between(controls, "fn toggle_deleted", "fn toggle_comment");

    assert!(side.contains("owner.read(cx).side != side"));
    assert!(side.contains("panel.set_side(side, cx)"));
    assert!(kind.contains("owner.read(cx).kind != kind"));
    assert!(kind.contains("panel.set_kind(kind, cx)"));
    assert!(deleted.contains("panel.set_deleted_only(deleted_only, cx)"));
    assert!(actions.matches("self.request_requery(cx)").count() >= 3);
}
