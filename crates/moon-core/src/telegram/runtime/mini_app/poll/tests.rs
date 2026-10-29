//! Typed relay regressions for queue coalescing and response fanout.

use std::sync::mpsc::{Receiver, sync_channel};

use crate::telegram::web::MiniAppApiError;

use super::{MiniAppPolls, PollKey};
use crate::telegram::init_data::{SignedInitData, SignedUser};
use crate::telegram::runtime::Work;
use crate::telegram::web::MiniAppApiRequest;
use crate::telegram::web::dto::OrdersDto;

/// Synthetic verified identity; no transport or real account is involved.
fn identity() -> SignedInitData {
    SignedInitData {
        auth_date: 1,
        user: SignedUser {
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

/// Build one Orders poll and its HTTP-side receiver.
fn orders(
    chat: i64,
) -> (
    MiniAppApiRequest,
    Receiver<Result<OrdersDto, MiniAppApiError>>,
) {
    let (reply, receiver) = sync_channel(1);
    (
        MiniAppApiRequest::Orders {
            identity: identity(),
            chat_id: chat,
            reply,
        },
        receiver,
    )
}

/// Respond to the only queued Orders read with an empty synthetic page.
fn answer_orders(work: Work) {
    let Work::MiniApp(MiniAppApiRequest::Orders { reply, .. }) = work else {
        panic!("expected one Orders Work item")
    };
    reply
        .send(Ok(OrdersDto {
            orders: Vec::new().into(),
            can_control: true,
        }))
        .expect("shared receiver");
}

/// Dropping duplicate Work items would return 503 to a burst even though one read can answer all.
#[test]
fn identical_polls_share_one_work_item_and_fan_out_the_result() {
    let (tx, rx) = sync_channel(64);
    let mut polls = MiniAppPolls::default();
    let mut waiters = Vec::new();
    for _ in 0..128 {
        let (request, receiver) = orders(7);
        polls.relay(&tx, request);
        waiters.push(receiver);
    }
    assert_eq!(polls.pending.len(), 1);
    let queued: Vec<_> = rx.try_iter().collect();
    assert_eq!(queued.len(), 1);
    answer_orders(queued.into_iter().next().expect("one read"));
    polls.drain();
    assert!(polls.pending.is_empty());
    for waiter in waiters {
        assert_eq!(
            waiter.try_recv().expect("shared answer"),
            Ok(OrdersDto {
                orders: Vec::new().into(),
                can_control: true,
            })
        );
    }
}

/// Coalescing must never merge another chat, and a full queue must not retain a failed new read.
#[test]
fn distinct_charts_and_failed_admission_keep_their_own_lifecycle() {
    let (tx, rx) = sync_channel::<Work>(2);
    let mut polls = MiniAppPolls::default();
    let (first, _first_rx) = orders(7);
    let (second, _second_rx) = orders(8);
    polls.relay(&tx, first);
    polls.relay(&tx, second);
    assert_eq!(polls.pending.len(), 2);
    let (third, third_rx) = orders(9);
    polls.relay(&tx, third);
    assert!(third_rx.try_recv().is_err());
    assert_eq!(polls.pending.len(), 2);
    let (duplicate, _duplicate_rx) = orders(7);
    polls.relay(&tx, duplicate);
    assert_eq!(rx.try_iter().count(), 2);
    assert_eq!(polls.pending.len(), 2);
}

/// A new polling interval is a different chart read even on the same core and market.
#[test]
fn chart_arguments_are_part_of_the_poll_key() {
    let (reply, _rx) = sync_channel(1);
    let first = MiniAppApiRequest::CoinTicks {
        identity: identity(),
        chat_id: 7,
        core: 1,
        market: "XALPHAUSDT".into(),
        tf: crate::telegram::web::dto::CoinTfDto::M1,
        since_ms: Some(100),
        reply,
    };
    let super::PollSplit::Poll((first_key, _, _)) = super::Poll::split(first) else {
        panic!("chart poll")
    };
    let (reply, _rx) = sync_channel(1);
    let second = MiniAppApiRequest::CoinTicks {
        identity: identity(),
        chat_id: 7,
        core: 1,
        market: "XALPHAUSDT".into(),
        tf: crate::telegram::web::dto::CoinTfDto::M1,
        since_ms: Some(101),
        reply,
    };
    let super::PollSplit::Poll((second_key, _, _)) = super::Poll::split(second) else {
        panic!("chart poll")
    };
    assert_ne!(first_key, second_key);
    assert!(matches!(
        first_key,
        PollKey::CoinTicks {
            since_ms: Some(100),
            ..
        }
    ));
}

/// Money commands must remain one Work item per request even when their arguments match.
#[test]
fn commands_are_never_coalesced() {
    let (tx, rx) = sync_channel::<Work>(4);
    let mut polls = MiniAppPolls::default();
    for _ in 0..2 {
        let (reply, _rx) = sync_channel(1);
        polls.relay(
            &tx,
            MiniAppApiRequest::CancelOrder {
                identity: identity(),
                chat_id: 7,
                core: 1,
                uid: 2,
                reply,
            },
        );
    }
    assert!(polls.pending.is_empty());
    assert_eq!(rx.try_iter().count(), 2);
}

/// Coalescing across an intervening write could answer the second read with pre-write state.
#[test]
fn writes_split_identical_poll_generations() {
    let (tx, rx) = sync_channel::<Work>(4);
    let mut polls = MiniAppPolls::default();
    let (first, first_rx) = orders(7);
    polls.relay(&tx, first);
    let (reply, _command_rx) = sync_channel(1);
    polls.relay(
        &tx,
        MiniAppApiRequest::CancelOrder {
            identity: identity(),
            chat_id: 7,
            core: 1,
            uid: 2,
            reply,
        },
    );
    let (second, second_rx) = orders(7);
    polls.relay(&tx, second);
    assert_eq!(polls.pending.len(), 2);
    let queued: Vec<_> = rx.try_iter().collect();
    assert_eq!(queued.len(), 3);
    let mut queued = queued.into_iter();
    answer_orders(queued.next().expect("first read"));
    answer_orders(queued.last().expect("second read"));
    polls.drain();
    assert!(first_rx.try_recv().is_ok());
    assert!(second_rx.try_recv().is_ok());
}

/// A command dropped by a full queue must not make a later identical poll miss its existing read.
#[test]
fn failed_write_does_not_split_an_inflight_poll() {
    let (tx, rx) = sync_channel::<Work>(1);
    let mut polls = MiniAppPolls::default();
    let (first, first_rx) = orders(7);
    polls.relay(&tx, first);
    let (reply, _command_rx) = sync_channel(1);
    polls.relay(
        &tx,
        MiniAppApiRequest::CancelOrder {
            identity: identity(),
            chat_id: 7,
            core: 1,
            uid: 2,
            reply,
        },
    );
    let (second, second_rx) = orders(7);
    polls.relay(&tx, second);
    assert_eq!(polls.pending.len(), 1);
    let queued: Vec<_> = rx.try_iter().collect();
    assert_eq!(queued.len(), 1);
    answer_orders(queued.into_iter().next().expect("existing read"));
    polls.drain();
    assert!(first_rx.try_recv().is_ok());
    assert!(second_rx.try_recv().is_ok());
}

/// Pending replies must not sit behind the ordinary 50 ms owner-loop wait.
#[test]
fn pending_replies_cap_the_idle_wait() {
    let (tx, _rx) = sync_channel::<Work>(1);
    let mut polls = MiniAppPolls::default();
    let base = std::time::Duration::from_millis(50);
    assert_eq!(polls.idle_interval(base), base);
    let (request, _receiver) = orders(7);
    polls.relay(&tx, request);
    assert_eq!(
        polls.idle_interval(base),
        std::time::Duration::from_millis(1)
    );
}
