//! The "asked, waiting for the core to agree" register behind every run control.
//!
//! `restart_now`, `strategies start/stop` and `set_auto_detect_active` are INTENTS: MoonProto
//! queues the command and the core answers later with a fresh state. Without this register a
//! pressed button looks like a button that did nothing for as long as that round trip takes, and
//! the user presses it again.
//!
//! It lives on `Backend` — one register per process, not one per window — so every surface drawing
//! a run cell shows the same outstanding intent, and none of them offers to send it twice. The
//! core-settings popup arms it through the same action but does not render it yet; that is the next
//! consumer, not a promise this module keeps today.
//!
//! An ask is answered by the STATE the intent asked for, confirmed by the live connection — not by
//! a revision counter. A counter also moves when the connection drops, when the other half reports,
//! and when a value is merely re-confirmed, and every one of those would hand the button back while
//! the core has still said nothing about what was asked. The timeout below is the only other exit.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use moon_core::session::{CoreId, CoreRunState};

use super::RunKey;

#[cfg(test)]
mod tests;

/// How long an unanswered intent keeps its control in the waiting state.
///
/// Long enough to cover a slow core round trip, short enough that a core which will never answer
/// gives the button back rather than freezing it for the session. A core that was ALREADY in the
/// asked-for state answers nothing at all, which is why the callers do not send to those.
pub(crate) const PENDING_TIMEOUT: Duration = Duration::from_secs(5);

/// What was asked of one core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunAsk {
    /// Start or restart the market runtime.
    Restart,
    /// Start (`true`) or stop (`false`) the global strategy engine.
    Trading(bool),
    /// Turn AutoDetect on (`true`) or off (`false`).
    AutoDetect(bool),
}

impl RunAsk {
    /// Which independently reported half this ask waits on.
    pub(crate) fn half(self) -> RunHalf {
        match self {
            Self::Restart => RunHalf::Runtime,
            Self::Trading(_) => RunHalf::Trading,
            Self::AutoDetect(_) => RunHalf::Auto,
        }
    }

    /// Whether one core's current state already satisfies this ask.
    ///
    /// Confirmation is part of the answer: a value carried over an unreported reconnect describes
    /// the connection before the press, so it cannot report on a command sent after it.
    ///
    /// Args:
    ///     state: The core's current run state.
    ///
    /// Returns:
    ///     Whether the core has said it is now in the asked-for state.
    fn satisfied_by(self, state: CoreRunState) -> bool {
        match self {
            // Restart asks the market runtime to come up; the core answers by reporting it started.
            Self::Restart => state.started == Some(true) && state.started_confirmed,
            Self::Trading(on) => state.trading == Some(on) && state.trading_confirmed,
            // AutoDetect rides the runtime-state command, so the runtime flag is what confirms it.
            Self::AutoDetect(on) => state.auto_detect == Some(on) && state.started_confirmed,
        }
    }
}

/// Which SLOT of a run cell an intent belongs to.
///
/// Not quite "which command reported it": AutoDetect arrives inside the same
/// `TRuntimeStateCommand` as the runtime itself, yet it needs its own key here. A core can be
/// waiting on a restart and on an AutoDetect flip at the same time, drawn by two different slots,
/// and one shared key would let either press erase the other's waiting face.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum RunHalf {
    /// The market runtime (`TRuntimeStateCommand`).
    Runtime,
    /// The global strategy engine (`TStratRuntimeState`).
    Trading,
    /// AutoDetect / passive mode, carried by the runtime-state command.
    Auto,
}

/// One outstanding intent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Ask {
    /// What was asked.
    pub(crate) kind: RunAsk,
    /// WHICH control sent it, by the identity of the line it sits on.
    ///
    /// A group caption, the rows under it and the table's own heading all command the same cores,
    /// so without this a press on one of them would put the others — which nobody touched — into
    /// the waiting face, while a press that skipped the cores already in the asked-for state would
    /// leave the pressed control looking idle. A bare "came from a group" bit was not enough once
    /// two group-level controls could overlap. The press says who is waiting; the register only
    /// carries it.
    pub(crate) from: RunKey,
    at: Instant,
}

/// Outstanding run intents, keyed by core and half.
///
/// Keyed by half as well as by core because a core can have one outstanding intent per SLOT at
/// once, and each slot may only show the wait it owns.
///
/// ONE ask per key, and that is the register's stated bound: when two controls that overlap on a
/// core — a group caption and the table-wide heading — are pressed within the timeout, the newer
/// press owns the waiting face and the older control returns to pressable while its own command is
/// still in flight. Pressing it again re-sends the same value, which the core folds into the state
/// it is already moving to; the cost is a duplicate datagram, not a wrong action. Holding both
/// would mean a list per key and a lookup per drawn cell, which is the wrong trade for a 5 s
/// cosmetic window.
#[derive(Default)]
pub(crate) struct RunPending {
    asks: HashMap<(CoreId, RunHalf), Ask>,
    /// Advances whenever an entry is added or dropped, so cached surfaces can gate on it.
    rev: u64,
    /// Whether an expiry sweep is already scheduled.
    ///
    /// One timer at a time, however many times a control is pressed: each press would otherwise
    /// leave its own detached task behind, and every one of them ends in a backend wake. The sweep
    /// re-arms itself while entries remain, so a press that arrived under a running timer still
    /// gets one of its own.
    sweep_armed: bool,
}

impl RunPending {
    /// Record an intent just sent to one core.
    ///
    /// Args:
    ///     core: Core the command went to.
    ///     kind: What was asked.
    ///     from: Identity of the control that pressed it.
    ///     now: Current instant.
    pub(crate) fn arm(&mut self, core: CoreId, kind: RunAsk, from: RunKey, now: Instant) {
        self.asks.insert(
            (core, kind.half()),
            Ask {
                kind,
                from,
                at: now,
            },
        );
        self.rev = self.rev.wrapping_add(1);
    }

    /// Claim the right to schedule the next expiry sweep.
    ///
    /// Returns:
    ///     Whether the caller should start a timer; `false` when one is already pending.
    pub(crate) fn claim_sweep(&mut self) -> bool {
        if self.sweep_armed {
            return false;
        }
        self.sweep_armed = true;
        true
    }

    /// Return what one core is still waiting for on ONE half, if anything.
    ///
    /// Pure: an answered or expired entry reports nothing but is not removed here, because this
    /// runs during render. [`Self::sweep`] does the removal on the paths that already write.
    ///
    /// Args:
    ///     core: Core being drawn.
    ///     half: Which half the drawn slot commands.
    ///     state: That core's current run state.
    ///     now: Current instant.
    ///
    /// Returns:
    ///     The outstanding ask — including which kind of control sent it — or `None` once the core
    ///     reached the asked-for state or the wait expired.
    pub(crate) fn active(
        &self,
        core: CoreId,
        half: RunHalf,
        state: CoreRunState,
        now: Instant,
    ) -> Option<Ask> {
        let ask = *self.asks.get(&(core, half))?;
        let expired = now.duration_since(ask.at) >= PENDING_TIMEOUT;
        (!expired && !ask.kind.satisfied_by(state)).then_some(ask)
    }

    /// Drop entries that expired purely by time, and release the sweep claim.
    ///
    /// Answered entries are deliberately NOT swept here: sweeping needs each core's current state,
    /// which the caller of [`Self::active`] already has and this one does not. They cost one stale
    /// map entry per core and half until that core is asked again, which is bounded by the fleet.
    ///
    /// Args:
    ///     now: Current instant.
    ///
    /// Returns:
    ///     Whether anything was dropped, so the caller can skip an otherwise pointless repaint.
    ///     Whether a FURTHER sweep is owed is [`Self::is_empty`]: entries armed under the finished
    ///     timer have not reached their own timeout yet.
    pub(crate) fn sweep(&mut self, now: Instant) -> bool {
        self.sweep_armed = false;
        let before = self.asks.len();
        self.asks
            .retain(|_, ask| now.duration_since(ask.at) < PENDING_TIMEOUT);
        let dropped = self.asks.len() != before;
        if dropped {
            self.rev = self.rev.wrapping_add(1);
        }
        dropped
    }

    /// Whether nothing is outstanding.
    ///
    /// A renderer checks this before asking about individual cores: with no intent in flight there
    /// is nothing to answer, and the per-cell clock read that answer needs is pure waste. The
    /// expiry sweep checks it too, to decide whether it owes itself another run.
    ///
    /// Returns:
    ///     Whether the register holds no entry at all.
    pub(crate) fn is_empty(&self) -> bool {
        self.asks.is_empty()
    }

    /// Return the register's change token, folded into a surface's own repaint gate.
    ///
    /// Returns:
    ///     A value that changes whenever an intent is armed or swept.
    pub(crate) fn rev(&self) -> u64 {
        self.rev
    }
}
