//! Startup, reconnection and terminal-failure predicates.

use super::*;

/// Whether the startup poll may stop for this core.
///
/// BOTH halves are required and each has already caused its own bug. Dropping the snapshot half
/// re-opens a real race: MoonProto fires `Connected{fresh:false}` from inside `run_protocol_step`
/// but publishes the matching `Ready` status LATER in the same iteration, so this thread — woken by
/// that event — can read a stale non-terminal snapshot, and stopping there would freeze the UI
/// showing progress on a core that is actually up, forever. Dropping the `is_ready` half makes it
/// LATCH: `Ready` is terminal forever, so a later reconnect episode would never be polled again.
///
/// Extracted so both the poll gate and the wake-deadline fold read ONE predicate: two hand-written
/// copies could drift, and a loop that sleeps past a poll it means to make is invisible until a
/// core is slow to start.
///
/// Args:
///     is_ready: Whether the lifecycle side currently reports `ConnStatus::Ready`.
///     sent: The last startup snapshot actually sent, if any.
///
/// Returns:
///     Whether the poll may stop.
pub(super) fn startup_poll_settled(is_ready: bool, sent: Option<CoreStartupStatus>) -> bool {
    is_ready && sent.is_some_and(|prev| prev.state.is_terminal())
}

/// Whether a MoonProto re-handshake resumes an OPERATIONAL connection.
///
/// `Connected { fresh }` does not answer this on its own. MoonProto derives `fresh` from
/// `was_ever_connected`, which it sets on the FIRST AuthDone — before the init spine runs at all
/// (`client/lifecycle.rs`) — so a link blip during the first startup arrives with `fresh: false`
/// too. Reading that flag alone is what left cores with no market list, no strategies and no
/// runtime state reading "connected" for a whole session. Only a startup that reached `Ready` makes
/// the next re-handshake a resumption of something that works.
///
/// Extracted so the status mapping and the licence re-request answer it from ONE place: two
/// hand-written copies of this rule would drift, and the wrong half stays invisible until a core
/// happens to reconnect mid-init.
///
/// Args:
///     fresh: MoonProto's own flag from `LifecycleEvent::Connected`.
///     init_completed: Whether `LifecycleEvent::Ready` has already landed on this client.
///
/// Returns:
///     Whether the core may be treated as up.
pub(super) fn reconnect_is_operational(fresh: bool, init_completed: bool) -> bool {
    !fresh && init_completed
}

/// Marker attached to every `run` failure from an attempt that never became operational.
///
/// [`crate::feed::spawn`]'s reconnect loop resets its backoff for a connection that lasted long
/// enough to look stable. Lifetime alone cannot tell "worked for an hour, then dropped" from "spent
/// three minutes failing to come up", and the second one is exactly what must escalate: without
/// this, a core that can never finish initialization rebuilds on a fixed cadence for the life of
/// the session and never backs off.
///
/// One marker for the general fact rather than a downcast per failing kind: the startup watchdog is
/// the first exit that needs it, MoonProto's own `ConnectFailed` after a long init ladder is the
/// second, and the next one should only have to route through [`run_failed`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::feed) struct NeverOperational;

impl std::fmt::Display for NeverOperational {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the connection never became operational")
    }
}

/// `run` failed before any client existed because the configured key could not be decoded.
///
/// `feed::spawn` reads it to stop the backoff loop: no retry can succeed until the key is
/// edited, and both Save and Reconnect spawn a NEW thread
/// (`session::lifecycle::respawn_session`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::feed) struct KeyUnreadable {
    pub(in crate::feed) empty: bool,
}

impl std::fmt::Display for KeyUnreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.empty {
            f.write_str("key empty")
        } else {
            f.write_str("key unparsable")
        }
    }
}

impl std::error::Error for KeyUnreadable {}

/// The Connections row's hand-typed address could not be used: the attempt ended before any client
/// was built.
///
/// `feed::spawn` stops the backoff loop for an `unresolved: false` field — it is not an address,
/// and no retry can change that until it is edited — and keeps retrying an unresolved name, which
/// a resolver or a network that comes back can fix on its own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::feed) struct EndpointUnusable {
    pub(in crate::feed) unresolved: bool,
}

impl std::fmt::Display for EndpointUnusable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.unresolved {
            f.write_str("endpoint host did not resolve")
        } else {
            f.write_str("endpoint override is not an address")
        }
    }
}

impl std::error::Error for EndpointUnusable {}

/// Tag one `run` failure with whether the attempt had ever become operational.
///
/// Args:
///     init_completed: Whether `LifecycleEvent::Ready` landed before this failure.
///     cause: What ended the run.
///
/// Returns:
///     The error to return from `run`, carrying [`NeverOperational`] when it never worked.
pub(super) fn run_failed(init_completed: bool, cause: impl Into<anyhow::Error>) -> anyhow::Error {
    let cause = cause.into();
    if init_completed {
        cause
    } else {
        cause.context(NeverOperational)
    }
}
