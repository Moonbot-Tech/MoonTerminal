//! Shutdown-ownership guards for the group-window close handler.

/// Restoring the old `return (Vec::new(), true)` shutdown branch in `boot.rs:on_window_closed` must
/// fail. Detached panel windows are OS-owned children of the group window: when the LAST one closes
/// they die with the application, and a release that still finds itself in
/// `Backend::detached_panel_windows` queues a repin. That repin is indistinguishable from the user
/// closing the window by hand — it docks the panel and deletes its `DetachedSpec` — so the final
/// save persists every panel docked and the next launch opens with the detachment gone. The branch
/// that closes one of SEVERAL group windows already unregisters them for exactly this reason.
#[test]
fn the_last_group_window_unregisters_detached_panels_before_quitting() {
    let source = include_str!("../boot.rs");
    let branch = source
        .split("if last_group_window {")
        .nth(1)
        .and_then(|tail| tail.split("// Otherwise close detached charts").next())
        .expect("the shutdown branch of on_window_closed must exist");

    let quitting = branch
        .find("b.quitting = true;")
        .expect("the shutdown branch must mark the exit before any window is released");
    let taken = branch
        .find("detached::take_windows(b, |_| true)")
        .expect("the shutdown branch must unregister every detached panel window");
    let pruned = branch
        .find("detached::prune_requests(b, |_| true)")
        .expect("the shutdown branch must drop queued repin and detach requests");
    let returned = branch
        .find("return (")
        .expect("the shutdown branch must return the quit decision");

    assert!(quitting < taken && taken < pruned && pruned < returned);
    assert!(
        !branch.contains("return (Vec::new(), true)"),
        "the taken handles must be closed, not discarded"
    );
}
