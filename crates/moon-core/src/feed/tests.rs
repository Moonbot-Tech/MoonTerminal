//! Unit tests for feed retry lifetime and order rules shared by display and packet assembly.

use super::{live, retry_can_help, stop_inherited_from_strategy};
use anyhow::anyhow;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

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
