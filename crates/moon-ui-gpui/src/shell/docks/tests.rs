// NOT `use super::*`: the glob would pull in the `gpui::test` macro, and `#[test]` would
// expand into itself (recursion limit).
use crate::panels::registry::home_ordered_names;
use crate::persistence::panel_meta::tab_label;

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

#[test]
fn every_home_tab_has_a_localized_label() {
    // Pins the link between the panel registry and the label map: a home tab whose name has no
    // `tab_label` entry would render an English caption. The broader "every registered panel has a
    // label" check lives in `panels::registry::tests`; this one guards the home strip specifically.
    for name in home_ordered_names() {
        assert!(
            tab_label(name).is_some(),
            "{name} is a home dock tab but has no entry in panel_meta::tab_label"
        );
    }
}
