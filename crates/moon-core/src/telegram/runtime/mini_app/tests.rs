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

/// A synthetic signed identity for queue-scale requests.
fn identity() -> crate::telegram::init_data::SignedInitData {
    crate::telegram::init_data::SignedInitData {
        auth_date: 1,
        user: crate::telegram::init_data::SignedUser {
            id: 7,
            first_name: "UNIT_01".into(),
            last_name: None,
            username: None,
            language_code: None,
        },
        chat: None,
        query_id: None,
    }
}

/// Measure how much of an identical polling burst fits the unchanged bounded Work queue.
#[test]
#[ignore = "manual Mini App queue saturation measurement"]
fn mini_app_identical_poll_burst_scale() {
    use std::sync::mpsc;
    use std::time::Instant;

    let (tx, rx) = mpsc::sync_channel::<super::super::Work>(64);
    let mut polls = super::poll::MiniAppPolls::default();
    let mut replies = Vec::new();
    let started = Instant::now();
    for _ in 0..128 {
        let (reply, receiver) = mpsc::sync_channel(1);
        replies.push(receiver);
        polls.relay(
            &tx,
            crate::telegram::web::MiniAppApiRequest::Orders {
                identity: identity(),
                chat_id: 7,
                reply,
            },
        );
    }
    let queued = rx.try_iter().count();
    println!(
        "[scale] identical poll burst: {queued}/128 queued in {:.3} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
}
