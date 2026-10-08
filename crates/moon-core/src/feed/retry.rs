//! Connection retry loop, with an injected attempt so its timing can be tested without networking.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Instant;

use super::{
    BACKOFF_MAX, BACKOFF_MIN, ConnStatus, FeedMsg, FeedTx, STABLE_AFTER, jittered, live,
    retry_can_help,
};
use crate::config::ServerConfig;

/// Run attempts until success, an unfixable failure, or feed-handle cancellation.
///
/// The handle owns the only stop sender. Reconnect replaces that handle and starts a fresh feed;
/// routine commands remain queued for the next attempt and cannot shorten this loop's delay.
pub(super) fn run(
    server: &ServerConfig,
    tx: &FeedTx,
    stop: &Receiver<()>,
    mut attempt: impl FnMut() -> anyhow::Result<()>,
) {
    let mut backoff = BACKOFF_MIN;
    loop {
        let started = Instant::now();
        let Err(e) = attempt() else {
            break;
        };
        // A long-lived operational connection resets the ladder. A stalled startup can exceed
        // STABLE_AFTER without ever working, so its marker must preserve escalation.
        let never_worked = e.downcast_ref::<live::NeverOperational>().is_some();
        if !never_worked && started.elapsed() >= STABLE_AFTER {
            backoff = BACKOFF_MIN;
        }
        let can_retry = retry_can_help(&e);
        let wait = jittered(backoff);
        if can_retry {
            log::error!(
                "live backend {} failed: {e:#}; planned reconnect delay {:?}",
                server.name,
                wait
            );
        } else {
            log::error!(
                "live backend {} failed: {e:#}; attempts stopped until settings are corrected",
                server.name
            );
        }
        // Preserve the whole error chain for the UI's typed-fault fallback, without a localized
        // reconnect suffix: the UI derives retry state from the lifecycle status.
        if tx
            .send(FeedMsg::Status(ConnStatus::Failed(format!("{e:#}"))))
            .is_err()
        {
            break;
        }
        // Settings edits and explicit reconnect create a new feed; waiting cannot fix this one.
        if !can_retry {
            break;
        }
        let pause_started = Instant::now();
        let retry = matches!(stop.recv_timeout(wait), Err(RecvTimeoutError::Timeout));
        log::info!(
            "live backend {} reconnect wait ended after {:?}; retry={retry}",
            server.name,
            pause_started.elapsed()
        );
        if !retry {
            break;
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}
