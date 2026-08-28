//! The give-up clock for a core whose FIRST startup stops making progress.
//!
//! MoonProto's init spine has no terminal failure of its own. The three heavy steps re-send
//! themselves forever on timeout — `GetMarketsList` every 20 s, `UpdateMarketsList` every 15 s,
//! `StrategySchema` every 30 s (`client/init/machine.rs`) — and the phases that wait for
//! authorization park indefinitely on `!client.is_authorized()`. Neither path returns an error, so
//! `live::run` is never told to rebuild the client and the app-level reconnect in `feed::spawn` never
//! starts. A core can therefore sit half-initialized — authorized, no market list, no strategies,
//! no runtime state — for the whole session.
//!
//! That is not theory: on 2026-08-27 seven of twenty-two cores took a mid-init link blip and then
//! reported nothing further for seventy-one minutes, while the connection badge read "connected"
//! because a re-handshake had been mistaken for a completed startup.
//!
//! This watches the startup snapshot the feed already polls and reports a stall so the caller can
//! fail the run, which rebuilds the client from scratch under the existing backoff. It watches the
//! FIRST startup only: its caller stops feeding it once `LifecycleEvent::Ready` has landed, after
//! which MoonProto's own reconnect owns the connection. That gate is the single owner of "init has
//! finished" — the snapshot cannot serve as a second one, because after startup MoonProto publishes
//! `Ready`/`Reconnecting` in step with authorization rather than freezing
//! (`active_runtime/runtime_loop/mod.rs`, `StartupStatusPublisher::publish`).
//!
//! The rule is deliberately ONE thing: init has ACHIEVED nothing for a budget longer than any
//! recovery the library itself still intends to finish. Liveness counters are not a second opinion
//! on it — see [`Progress`].

use std::fmt;
use std::time::{Duration, Instant};

use crate::feed::{CoreInitStep, CoreStartupStatus};

#[cfg(test)]
mod tests;

/// How long a startup may show no progress at all before the client is rebuilt.
///
/// Measured against progress rather than against total startup time, so a core that keeps advancing
/// keeps its connection however long the whole startup takes.
///
/// Sized past MoonProto's own longest legitimate SILENCE rather than past a healthy startup, which
/// costs under five seconds here. Every rebuild throws away the steps already done, so cutting a
/// recovery the library still intends to finish is the expensive mistake, not waiting.
///
/// The worst case in the pinned library is the Delphi BaseCheck update ladder, and it advances on
/// TIMEOUTS — nothing is received for any of it (`client/init/machine.rs`, the
/// `PendingEnginePoll::Timeout` arm): 34 × 300 ms of auth wait, a 12 s first attempt, then ten
/// retries of 2 s + 12 s ≈ 162 s in total (`DELPHI_BASE_CHECK_UPDATE_AUTH_WAITS`,
/// `DELPHI_BASE_CHECK_UPDATE_RETRIES`, `DEFAULT_PENDING_TIMEOUT_MS`). Five minutes clears that with
/// margin, and is about ten full `StrategySchema` attempts. Against an outage that used to last the
/// whole session, the detection delay costs nothing worth trading a false kill for.
pub(super) const STARTUP_STALL: Duration = Duration::from_secs(300);

/// What init has ACHIEVED, as the watchdog compares it.
///
/// Deliberately not the whole snapshot. [`CoreStartupStatus::progress_eq`] exists to gate a UI
/// repaint and counts `elapsed_ms` as change, so it would reset this clock every second and the
/// watchdog would never fire.
///
/// Two fields, and the two omissions matter more than the fields:
///
/// - `completed` and `step` are the achievement itself, and `step` also moves on the retry paths
///   that walk backwards (`BaseCheck ⇄ AuthCheck`).
/// - the PHASE is out. It follows MoonProto's authorization flag, flipping
///   `Initializing ⇄ Reconnecting` on every blip, so including it would let a link that
///   re-handshakes more often than the budget hide a zero-progress init forever — the very incident
///   this module exists for.
/// - the SLICED counters are out, for the same reason read the other way round. They are taken from
///   `transport.recv_slicer` (`active_runtime/runtime_loop/mod.rs`), which sits below the
///   domain-ready dispatch filter, so a step that keeps timing out while its answer keeps partly
///   arriving — and the pushes MoonProto allows before `domain_ready` — move them without init ever
///   advancing. Counting that as progress would silence the watchdog on exactly its own case. They
///   would only ever protect a single step slower than the budget, and at five minutes such a step
///   has already timed out and been re-sent ten times over: that is failing, not slow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Progress {
    /// The step being run now.
    step: Option<CoreInitStep>,
    /// Bit per completed step.
    completed: u16,
}

impl Progress {
    /// Read the achieved progress out of one polled snapshot.
    fn of(snap: &CoreStartupStatus) -> Self {
        Self {
            step: snap.current_step,
            completed: snap.completed_mask,
        }
    }
}

/// Watches one core's first startup and reports when it has stopped advancing.
#[derive(Default)]
pub(super) struct StartupWatchdog {
    /// The last progress seen and when it was first seen, or `None` before the first observation.
    ///
    /// One field rather than two: a mark with no instant, or an instant with no mark, is not a
    /// state this watchdog has.
    mark: Option<(Progress, Instant)>,
}

impl StartupWatchdog {
    /// Feed one polled startup snapshot.
    ///
    /// Takes the caller's "initialization has finished" latch rather than trusting the call site to
    /// guard the call: the module's central rule is that it watches the FIRST startup only, and a
    /// rule enforced by an `&&` at one call site is a rule a second call site can drop in silence.
    /// One latch, checked inside the type that claims it.
    ///
    /// Args:
    ///     snap: The snapshot just read from MoonProto.
    ///     now: Instant that snapshot was read at.
    ///     init_completed: Whether `LifecycleEvent::Ready` has already landed on this client.
    ///
    /// Returns:
    ///     Whether startup has shown no progress for [`STARTUP_STALL`] and the client should be
    ///     rebuilt.
    pub(super) fn observe(
        &mut self,
        snap: &CoreStartupStatus,
        now: Instant,
        init_completed: bool,
    ) -> bool {
        if init_completed {
            return false;
        }
        let progress = Progress::of(snap);
        match self.mark {
            Some((seen, since)) if seen == progress => now.duration_since(since) >= STARTUP_STALL,
            _ => {
                self.mark = Some((progress, now));
                false
            }
        }
    }
}

/// A first startup that stopped advancing, as `live::run` reports it.
///
/// It says WHERE the startup stopped, for the log and for the reconnect loop's own error line.
/// Whether the attempt ever became operational — the fact the backoff turns on — is a separate,
/// more general marker (`super::NeverOperational`), because a stall is only one of the ways a run
/// can end without ever having worked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct StartupStalled {
    /// The step it stopped on, when the snapshot named one.
    step: Option<CoreInitStep>,
    /// Steps completed, and the total they are shown against.
    progress: (u8, u8),
}

impl StartupStalled {
    /// Read where a startup stopped out of the snapshot that decided it had.
    ///
    /// Args:
    ///     snap: The snapshot the stall was detected on.
    ///
    /// Returns:
    ///     The reportable record of that stall.
    pub(super) fn of(snap: &CoreStartupStatus) -> Self {
        Self {
            step: snap.current_step,
            progress: snap.progress_pair(),
        }
    }
}

impl fmt::Display for StartupStalled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (done, total) = self.progress;
        write!(
            f,
            "startup stalled: no init progress for {}s at {:?} ({done}/{total} steps done)",
            STARTUP_STALL.as_secs(),
            self.step
        )
    }
}

impl std::error::Error for StartupStalled {}
