// NOT `use super::*`: the glob would pull in the `gpui::test` macro, and `#[test]` would
// expand into itself (recursion limit).
use super::dock_placement_key_belongs_to;

/// Removing the `quitting` gate from `docks.rs:drain_repin_requests` must fail: a detached panel
/// window dies with the application, and draining its release-queued repin at that moment docks the
/// panel and consumes its `DetachedSpec`, so the next launch starts with the detachment lost.
#[test]
fn repin_drain_refuses_to_dock_panels_while_quitting() {
    let source = include_str!("../docks.rs");
    let body = source
        .split("pub(super) fn drain_repin_requests")
        .nth(1)
        .and_then(|tail| tail.split("pub(super) fn defer_detach_panel").next())
        .expect("drain_repin_requests must exist");
    let gate = body
        .find("if self.backend.read(cx).quitting")
        .expect("the repin drain must refuse to run during application quit");
    let consume = body
        .find("b.repin_request.retain")
        .expect("the repin drain must consume queued requests");
    assert!(
        gate < consume,
        "the quit gate must precede consuming the queue, or the requests are lost either way"
    );
    // The restore itself runs a deferred turn later, so one check is not enough: the quit can land
    // between the two, and on Linux — where a release cannot be attributed to the user — this
    // second read is the only thing left standing between the exit and a deleted `DetachedSpec`.
    let deferred = body
        .split("cx.defer(")
        .nth(1)
        .expect("the restore must stay deferred out of the Backend observer");
    assert!(
        deferred.contains("quitting"),
        "the deferred restore must re-read the exit flag before docking the panel"
    );
}

/// Replacing `shell/docks.rs:dock_placement_key_belongs_to` with a plain prefix check must fail:
/// resetting group `g` would erase `gx` or `g:sub` placement state and silently damage another
/// group's layout.
#[test]
fn dock_placement_key_matches_only_its_own_group() {
    assert!(dock_placement_key_belongs_to("g:Report", "g"));
    assert!(dock_placement_key_belongs_to("g:Orders", "g"));
    assert!(!dock_placement_key_belongs_to("gx:Report", "g"));
    assert!(!dock_placement_key_belongs_to("g:sub:Report", "g"));
    assert!(!dock_placement_key_belongs_to("g-archive:Report", "g"));
    assert!(!dock_placement_key_belongs_to("g", "g"));
}
