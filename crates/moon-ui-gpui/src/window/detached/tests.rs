//! Which detached panels get a window back, and which records simply wait for one.

// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::{DetachedSpec, should_reopen};
use std::collections::HashSet;

/// A detached panel is reopened only against a live owner window, and never on top of a state that
/// already accounts for it: an open window, a queued repin, or a dock that will restore it.
#[test]
fn a_panel_is_reopened_only_when_nothing_else_accounts_for_it() {
    let spec = |group: &str, panel: &str| DetachedSpec::new(group.to_string(), panel.to_string());
    let live: HashSet<&str> = ["G1"].into_iter().collect();
    let open: HashSet<(&str, &str)> = [("G1", "log")].into_iter().collect();
    let repins: HashSet<(&str, &str)> = [("G1", "report")].into_iter().collect();
    let docked: HashSet<(&str, &str)> = [("G1", "alerts")].into_iter().collect();

    assert!(should_reopen(
        &spec("G1", "orders"),
        &live,
        &open,
        &repins,
        &docked
    ));
    // A window is already open for it: a second one would orphan the first, which could then never
    // repin, because the handle map would name only the newer window.
    assert!(!should_reopen(
        &spec("G1", "log"),
        &live,
        &open,
        &repins,
        &docked
    ));
    // Its group has no live window to own it — `spawn` would produce a top-level window that no
    // longer dies with its group.
    assert!(!should_reopen(
        &spec("G2", "orders"),
        &live,
        &open,
        &repins,
        &docked
    ));
    // The user just closed it and its repin has not drained yet: that request means "put the panel
    // back in the dock", so reopening the window would fight it.
    assert!(!should_reopen(
        &spec("G1", "report"),
        &live,
        &open,
        &repins,
        &docked
    ));
    // A stale spec whose panel the restorable dock will bring back anyway; reopening it would show
    // the panel twice, once as a tab and once as a window.
    assert!(!should_reopen(
        &spec("G1", "alerts"),
        &live,
        &open,
        &repins,
        &docked
    ));
}

/// Whatever a settings save does to windows, the detachment RECORD survives it: a deactivated or
/// renamed group must not cost the user a panel, because the dock no longer holds it either. This
/// is the regression that motivated the rule — the teardown used to repin every detached panel and
/// delete its spec from `detached.json`.
#[test]
fn a_detached_record_outlives_a_windowless_group() {
    let orders = DetachedSpec::new("G3".to_string(), "orders".to_string());
    let nothing = HashSet::new();

    assert!(!should_reopen(
        &orders,
        &HashSet::new(),
        &nothing,
        &nothing,
        &nothing
    ));
    // ...and once its group has a window again, the same spec is what brings the panel back.
    let live: HashSet<&str> = ["G3"].into_iter().collect();
    assert!(should_reopen(&orders, &live, &nothing, &nothing, &nothing));
}
