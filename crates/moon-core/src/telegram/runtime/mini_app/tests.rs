//! What a change of the saved Mini App intent asks of a running owner.

use std::collections::BTreeMap;

use super::{Change, change_of};

/// Labels in one language.
fn labels(open: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("menu_miniapp".to_string(), open.to_string())])
}

/// A new pairing or a language switch keeps the running listener and its tunnel; switching the
/// Mini App on or off, or the first pass, starts over (STATION.md §9, question 36).
#[test]
fn only_the_switch_restarts_the_tunnel() {
    let on = (true, vec![7]);
    let en = labels("Open");
    assert_eq!(change_of(None, &en, &on, &en), Change::Restart);
    assert_eq!(change_of(Some(&on), &en, &on, &en), Change::None);
    assert_eq!(
        change_of(Some(&on), &en, &(true, vec![7, 8]), &en),
        Change::InPlace
    );
    assert_eq!(
        change_of(Some(&on), &en, &(true, Vec::new()), &en),
        Change::InPlace
    );
    assert_eq!(
        change_of(Some(&on), &en, &on, &labels("Abrir")),
        Change::InPlace
    );
    assert_eq!(
        change_of(Some(&on), &en, &(false, vec![7]), &en),
        Change::Restart
    );
    assert_eq!(
        change_of(Some(&(false, vec![7])), &en, &on, &en),
        Change::Restart
    );
}

/// The owner loop polls fast only while the Mini App is on, serving, or due a retry.
#[test]
fn idle_interval_is_fast_only_while_something_can_happen() {
    use std::time::Duration;
    let fast = Duration::from_millis(50);
    assert_eq!(super::idle_interval(true, false, false), fast);
    assert_eq!(super::idle_interval(false, true, false), fast);
    assert_eq!(super::idle_interval(false, false, true), fast);
    assert_eq!(
        super::idle_interval(false, false, false),
        Duration::from_millis(250)
    );
}
