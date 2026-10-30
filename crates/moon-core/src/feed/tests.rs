//! Unit tests for feed retry lifetime and order rules shared by display and packet assembly.

use super::{live, retry_can_help, stop_inherited_from_strategy};
use anyhow::anyhow;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

mod order_queue;

/// Observations made by each injected connection attempt, independently of the retry timer.
pub(crate) type Attempt = (Instant, Vec<super::QueuedCmd>, usize);

/// Run the production retry loop with a failing first attempt and a successful second attempt.
/// No client, network, filesystem, or shortened production backoff is involved.
pub(crate) fn backoff_feed(
    server: crate::config::ServerConfig,
) -> (super::FeedHandle, Receiver<Attempt>, Receiver<()>) {
    let (commands, commands_rx) = mpsc::channel();
    let (wake, wake_rx) = mpsc::channel();
    let (stop, stop_rx) = mpsc::channel();
    let (data, rx) = mpsc::channel();
    let (attempt_tx, attempts) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let join = std::thread::spawn(move || {
        let tx = super::FeedTx::new(data, None);
        let mut first = true;
        super::retry::run(&server, &tx, &stop_rx, || {
            let queued = commands_rx.try_iter().collect();
            let wakes = wake_rx.try_iter().count();
            attempt_tx.send((Instant::now(), queued, wakes)).unwrap();
            if first {
                first = false;
                Err(anyhow!("fixture connection failure"))
            } else {
                Ok(())
            }
        });
        done_tx.send(()).unwrap();
    });
    (
        super::FeedHandle {
            rx,
            cmd_tx: super::CoreCmdTx::new(commands, wake, super::LatestMarketRole::default()),
            client: super::SharedMoonClient::default(),
            _stop: stop,
            _join: join,
        },
        attempts,
        done,
    )
}

/// Waiting on command doorbells again would retry early and consume fleet jitter.
/// Commands sent during the pause must remain queued until the full delay expires.
#[test]
fn backoff_commands_do_not_shorten_the_pause() {
    let server = serde_json::from_str(r#"{"id": 91001, "name": "retry fixture"}"#).unwrap();
    let (feed, attempts, done) = backoff_feed(server);
    let (first_at, _, _) = attempts.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(
        feed.rx.recv_timeout(Duration::from_secs(2)),
        Ok(super::FeedMsg::Status(super::ConnStatus::Failed(_)))
    ));
    for _ in 0..5 {
        feed.cmd_tx
            .send(super::CoreCmd::RefreshTransferAssets)
            .unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
    let (second_at, commands, wakes) = attempts.recv_timeout(Duration::from_secs(5)).unwrap();
    let elapsed = second_at.duration_since(first_at);
    assert!(
        elapsed >= Duration::from_millis(1500),
        "commands shortened the retry delay: {elapsed:?}"
    );
    assert_eq!(commands.len(), 5);
    assert_eq!(wakes, 5);
    for command in commands {
        assert!(matches!(command.cmd, super::CoreCmd::RefreshTransferAssets));
    }
    done.recv_timeout(Duration::from_secs(2)).unwrap();
}

/// Waiting only for a timeout would leave removed cores alive throughout the maximum backoff.
/// Shutdown must cancel the wait without needing another command or a live client event.
#[test]
fn backoff_shutdown_cancels_the_wait() {
    let server = serde_json::from_str(r#"{"id": 91002, "name": "shutdown fixture"}"#).unwrap();
    let (feed, attempts, done) = backoff_feed(server);
    attempts.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(
        feed.rx.recv_timeout(Duration::from_secs(2)),
        Ok(super::FeedMsg::Status(super::ConnStatus::Failed(_)))
    ));
    drop(feed);
    done.recv_timeout(Duration::from_millis(500)).unwrap();
    assert!(attempts.try_recv().is_err(), "stopped feed retried");
}

/// `feed::retry_can_help` is the error-side half of the retry decision paired with
/// `FailureClass::retry_can_help`: dropping its `KeyUnreadable` downcast makes an unfixable key
/// failure spin its backoff forever, while widening it stops real reconnects.
#[test]
fn only_an_unreadable_key_stops_the_feed_retry() {
    assert!(!retry_can_help(&anyhow::Error::new(live::KeyUnreadable {
        empty: true
    })));
    assert!(retry_can_help(&anyhow!("connect timeout")));
}

/// A typed address that is not an address cannot become one by waiting, so retrying it would spin
/// the backoff forever; a name the resolver did not answer for can, so stopping there would leave
/// a dynamic-DNS core down until the next launch.
#[test]
fn only_an_invalid_endpoint_override_stops_the_retry() {
    assert!(!retry_can_help(&anyhow::Error::new(
        live::EndpointUnusable { unresolved: false }
    )));
    assert!(retry_can_help(&anyhow::Error::new(
        live::EndpointUnusable { unresolved: true }
    )));
}

/// A stop switched off after the entry filled must not be re-supplied by its strategy.
///
/// The core materializes a stop INTO the order at the fill, so from then on the order's own flag is
/// the state. Answering with the strategy past that point is the whole bug: the table redrew a
/// hand-disabled SL as ON one frame after the strategy snapshot arrived and on every restart, and
/// `resolve_stop_group` re-armed it in the core when the neighbouring stop was clicked.
///
/// Mutation: drop the fill check. Both defects return at once, in display and in packets.
///
/// Returns:
///     Nothing; a filled entry ends inheritance whatever the strategy says.
#[test]
fn a_filled_entry_ends_inheritance_from_the_strategy() {
    assert!(!stop_inherited_from_strategy(true, true));
    assert!(!stop_inherited_from_strategy(true, false));
}

/// An order whose entry has not filled inherits its strategy's stops, because it has none of its
/// own.
///
/// This is what the Orders table shows for a working order: the stop the core will apply at the
/// fill, not a bare OFF suggesting the trader is unprotected by choice. It also covers the order
/// that holds a position with no buy leg at all — a sale of an already-held asset, which the core
/// never ran `CheckBuyOrder` for, so nothing materialized a stop into it.
///
/// Mutation: return `false` before the fill. Every waiting order would read OFF — SL in red —
/// while the strategy is about to arm the stop anyway.
///
/// Returns:
///     Nothing; inheritance follows the strategy flag until the entry fills.
#[test]
fn an_unfilled_entry_inherits_what_its_strategy_enables() {
    assert!(stop_inherited_from_strategy(false, true));
    assert!(!stop_inherited_from_strategy(false, false));
}

/// A burst must leave one wake, and a notify after clear must still enqueue.
///
/// Sending on every `notify`, or leaving the latch set across `clear`, would either
/// wake the UI once per message or drop the message that arrives after the UI has
/// started its drain. `clear` must not discard a token already queued.
#[test]
fn wake_latch_coalesces_until_clear_and_keeps_the_next_send() {
    let (tx, rx) = super::feed_wake_pair();
    let other = tx.clone();
    for i in 0..1000 {
        if i % 2 == 0 {
            tx.notify();
        } else {
            other.notify();
        }
    }
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err());
    rx.clear();
    tx.notify();
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err());
    rx.clear();
    tx.notify();
    rx.clear();
    other.notify();
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err());
}

/// Removing the parked-owner wake leaves a queued token unread until the housekeeping timer.
#[test]
fn a_parked_owner_wakes_when_a_feed_token_is_admitted() {
    let (ready, sender) = std::sync::mpsc::channel();
    let (done, awakened) = std::sync::mpsc::channel();
    let owner = std::thread::spawn(move || {
        let (tx, rx) = super::feed_wake_pair_for_thread(std::thread::current());
        ready.send(tx).unwrap();
        std::thread::park_timeout(std::time::Duration::from_secs(5));
        done.send(rx.try_recv().is_ok()).unwrap();
    });
    sender.recv().unwrap().notify();
    assert!(
        awakened
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
    );
    owner.join().unwrap();
}

/// Startup staggering must leave one core instant and spread ten initial attempts over 1.5 seconds.
///
/// Returning zero for every core restores the connection burst; applying the last core's delay to
/// the first needlessly slows the common single-core launch.
#[test]
fn startup_delay_spreads_only_multi_core_launch() {
    assert_eq!(super::retry::startup_delay(0, 1), Duration::ZERO);
    let delays: Vec<u128> = (0..10)
        .map(|index| super::retry::startup_delay(index, 10).as_millis())
        .collect();
    assert_eq!(
        delays,
        vec![0, 166, 333, 500, 666, 833, 1000, 1166, 1333, 1500]
    );
}

/// Background destruction must release exactly the queue count owned by the retired handle;
/// losing that accounting would make an idle terminal report a permanent feed backlog.
#[test]
fn retired_feed_backlog_releases_all_queue_accounting() {
    let queued_before = super::queued_messages();
    let (tx, handle) = super::trade_sound_test_feed();
    for _ in 0..1_000 {
        tx.send(super::FeedMsg::Status(super::ConnStatus::Connecting))
            .unwrap();
    }
    let before = super::retired_for_test();
    drop(handle);
    while super::retired_for_test() < before + 1_000 {
        std::thread::yield_now();
    }
    assert!(super::retired_for_test() >= before + 1_000);
    assert!(super::queued_messages() <= queued_before);
}

/// Measure caller-thread teardown of a full feed FIFO. The post-change probe also waits for the
/// background drain so its two timings distinguish handoff latency from total disposal work.
#[test]
#[ignore = "synthetic 100000-message feed-drop timing probe"]
fn feed_handle_drop_scale_probe() {
    for _ in 0..3 {
        let (tx, handle) = super::trade_sound_test_feed();
        for _ in 0..100_000 {
            tx.send(super::FeedMsg::Status(super::ConnStatus::Connecting))
                .unwrap();
        }
        let before = super::retired_for_test();
        let started = Instant::now();
        drop(handle);
        let caller = started.elapsed();
        while super::retired_for_test() < before + 100_000 {
            std::thread::yield_now();
        }
        eprintln!(
            "feed drop 100000 messages: caller {:.3} ms, total {:.3} ms",
            caller.as_secs_f64() * 1000.0,
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
}
