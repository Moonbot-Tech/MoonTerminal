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
