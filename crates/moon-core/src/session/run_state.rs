//! The one answer to "is this core running, and is it trading" — plus what a control offering to
//! change that may show.
//!
//! Everything here is pure and lives in `moon-core` rather than in a window, because more than one
//! surface asks the question — today the Profit Monitor's run column and the core-settings popup,
//! with the Core Status panel next — and each answering it from raw [`CoreData`] fields is how they
//! come to disagree. A caller reads
//! [`SessionManager::core_run_state`](super::SessionManager::core_run_state) and gets a value that
//! already knows the two rules that are easy to get wrong:
//!
//! - the core reports its market runtime (`TRuntimeStateCommand`) and its strategy engine
//!   (`TStratRuntimeState`) over SEPARATE commands, so either half can be known while the other is
//!   not. `None` is a real third state and never a `false`;
//! - a core that is not `Ready` has only STALE knowledge, and an offline core reports every half as
//!   unknown — a control must not offer "Stop trading" for a core it cannot reach;
//! - a core that came BACK is a third case. MoonProto repeats neither init nor its post-init
//!   resync on a reconnect and the protocol has no request for either half; the feed re-publishes
//!   both from the library's retained snapshot, but until that lands — or when the client itself
//!   was replaced and the snapshot is empty — the values a reconnected core carries are the last
//!   ones it volunteered. They are then reported with
//!   [`CoreRunState::started_confirmed`] / [`CoreRunState::trading_confirmed`] cleared, so a
//!   control can draw them muted instead of pretending the core said nothing at all.
//!
//! The set-shaped answer ([`RunSummary`]) exists for the same reason: a group caption and an
//! exchange row both act on many cores at once, and "what does the button say when four of six are
//! trading" is a decision, not an implementation detail.

use crate::feed::{ConnStatus, RuntimeState};

use super::store::{CoreData, CoreId};
use super::SessionManager;

#[cfg(test)]
mod tests;

impl SessionManager {
    /// Return one core's run state, or the fully unknown state when no such core is live.
    ///
    /// The single read path for every surface that shows or changes a core's run state.
    ///
    /// Args:
    ///     core: Core whose state is required.
    ///
    /// Returns:
    ///     The projected run state.
    pub fn core_run_state(&self, core: CoreId) -> CoreRunState {
        self.store()
            .core(core)
            .map(CoreRunState::from_core)
            .unwrap_or_default()
    }

    /// Return a change token covering everything [`Self::core_run_state`] reads.
    ///
    /// A cached surface compares this instead of re-projecting and diffing the states themselves.
    /// It folds the connection status in because an offline core reports nothing: without it, a
    /// core going down would leave a live Stop button on screen until some other revision moved.
    ///
    /// The two halves are MIXED rather than summed, and the online flag is a whole term rather
    /// than a low bit: a plain sum lets one core coming up cancel another going down inside the
    /// same coalesced notification, and the surface then never repaints.
    ///
    /// Args:
    ///     core: Core whose token is required.
    ///
    /// Returns:
    ///     A value that changes whenever that core's run state can have changed.
    pub fn core_run_rev(&self, core: CoreId) -> u64 {
        let Some(data) = self.store().core(core) else {
            return 0;
        };
        // One lookup, three terms: either half reporting, and the connection itself — an offline
        // core reports nothing, so a control has to redraw when that changes too. Mixed rather
        // than summed, or one core coming up would cancel another going down inside the same
        // coalesced notification and the surface would never repaint.
        mix(
            mix(data.runtime_state_rev, data.strategies_running_rev),
            u64::from(data.status == ConnStatus::Ready),
        )
    }

    /// Return one change token for a whole SCOPE of cores.
    ///
    /// The counterpart of [`Self::core_run_rev`] for a surface that draws many cells and gates one
    /// cached body on all of them. The per-core tokens are MIXED rather than summed for the reason
    /// stated there, and `seed` folds in whatever the caller tracks beside the session — the UI
    /// passes its pending-intent revision, so a pressed button repaints even though nothing in the
    /// store moved yet.
    ///
    /// Args:
    ///     cores: Every core the surface draws a cell for.
    ///     seed: Caller-side change token folded in first.
    ///
    /// Returns:
    ///     A value that changes whenever any of those cells would draw differently.
    pub fn run_scope_rev(&self, cores: impl IntoIterator<Item = CoreId>, seed: u64) -> u64 {
        cores
            .into_iter()
            .fold(seed, |acc, core| mix(acc, self.core_run_rev(core)))
    }
}

/// Combine two change counters so a decrease in one cannot cancel an increase in the other.
///
/// Not a checksum and not a hash of the values themselves — only a token whose equality means
/// "nothing observed here moved". The odd multiplier is what makes the two terms non-commutative
/// enough for that.
///
/// Args:
///     acc: Accumulated token.
///     value: Next counter to fold in.
///
/// Returns:
///     The combined token.
fn mix(acc: u64, value: u64) -> u64 {
    acc.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(value)
}

/// Everything a run control needs to know about ONE core.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CoreRunState {
    /// Whether the core is reachable right now, which is what makes the rest of this trustworthy.
    pub online: bool,
    /// Whether the market runtime is started (`MarketActive`), or `None` when unknown or offline.
    pub started: Option<bool>,
    /// Whether [`Self::started`] was reported by the CURRENT connection.
    ///
    /// Meaningless while `started` is `None`. False means "this is what the core last said, before
    /// a reconnect nobody re-reported through".
    pub started_confirmed: bool,
    /// Whether automatic detection is active (`not PassiveMode`), or `None` when unknown.
    ///
    /// Carried because it arrives in the same command as [`Self::started`] and a reader that needed
    /// it would otherwise reach past this type into the raw snapshot. The core-settings popup shows
    /// it as a status dot; no control changes it yet — the protocol's
    /// `settings().set_auto_detect_active` has no terminal command behind it.
    pub auto_detect: Option<bool>,
    /// Whether the global strategy engine is running (`IsRunningStrat`), or `None` when unknown.
    pub trading: Option<bool>,
    /// Whether [`Self::trading`] was reported by the current connection; see
    /// [`Self::started_confirmed`].
    pub trading_confirmed: bool,
}

impl CoreRunState {
    /// Project one core's retained data into the run state a control may act on.
    ///
    /// Args:
    ///     data: The core's retained store entry.
    ///
    /// Returns:
    ///     The run state, with every half unknown while the core is not `Ready`.
    pub fn from_core(data: &CoreData) -> Self {
        let online = data.status == ConnStatus::Ready;
        if !online {
            return Self::default();
        }
        Self {
            // Invariably true past the guard above; spelled out so the value cannot read as
            // "whatever the status happened to be".
            online: true,
            started: data
                .runtime_state
                .map(|state: RuntimeState| state.is_started),
            started_confirmed: data.runtime_state_confirmed,
            auto_detect: data.runtime_state.map(|state| state.auto_detect_active),
            trading: data.strategies_running,
            trading_confirmed: data.strategies_running_confirmed,
        }
    }

    /// Whether this core needs the start/restart action offered.
    ///
    /// Only a core that has SAID its runtime is stopped — which, coming out of
    /// [`Self::from_core`], also means it is reachable, since an offline core reports nothing. An
    /// unknown runtime does
    /// not get the offer: the protocol's restart is not idempotent for the user's purposes — it
    /// also leaves passive mode and starts checked strategies — so guessing at it is a trade
    /// action taken on a shrug.
    ///
    /// Returns:
    ///     Whether to show the restart control.
    pub fn needs_restart(self) -> bool {
        self.started == Some(false)
    }
}

/// What the trading control offers for one core or one set of them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TradingAction {
    /// Trading is stopped (or partly stopped): offer to start it.
    Start,
    /// Trading is running everywhere in scope: offer to stop it.
    Stop,
    /// Nothing in scope has reported: offer nothing actionable.
    Unknown,
}

/// The folded run state of a SET of cores, as a group caption or an exchange row acts on it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RunSummary {
    /// How many of the scope's cores are reachable.
    pub online: usize,
    /// How many reported their strategy engine as running.
    pub trading_on: usize,
    /// How many reported it as stopped.
    pub trading_off: usize,
    /// How many reported a RUNNING market runtime.
    pub started_on: usize,
    /// How many reported a stopped market runtime.
    pub stopped: usize,
    /// Of the cores reporting a RUNNING runtime, how many said so on a previous connection.
    ///
    /// Split by the value rather than counted once for the half, because a control fades the state
    /// it is DRAWING: a group whose stop is confirmed must not be dimmed by an unrelated core whose
    /// start is stale.
    pub started_on_stale: usize,
    /// Of the cores reporting a STOPPED runtime, how many said so on a previous connection.
    pub stopped_stale: usize,
    /// Of the cores reporting trading as running, how many said so on a previous connection.
    pub trading_on_stale: usize,
    /// Of the cores reporting trading as stopped, how many said so on a previous connection.
    pub trading_off_stale: usize,
    /// How many reachable cores are not already trading in the given direction.
    ///
    /// One counter per direction, because a control has to name what its own press would reach:
    /// "start trading on N cores" is the cores that are online and not confirmed to be trading
    /// already, which is neither `online` nor `trading_off`.
    pub needing_start: usize,
    /// How many reachable cores are not already stopped.
    pub needing_stop: usize,
}

impl RunSummary {
    /// Fold every core in scope into one answer.
    ///
    /// Args:
    ///     states: Run state of each core the control stands for.
    ///
    /// Returns:
    ///     The folded counts.
    pub fn fold(states: impl IntoIterator<Item = CoreRunState>) -> Self {
        let mut summary = Self::default();
        for state in states {
            summary.add(state);
        }
        summary
    }

    /// Count one more core into this summary.
    ///
    /// Exposed beside [`Self::fold`] so a caller already walking its cores — reading each one's
    /// sample for other reasons — folds as it goes instead of collecting the states first.
    ///
    /// Args:
    ///     state: The core's run state.
    pub fn add(&mut self, state: CoreRunState) {
        self.online += usize::from(state.online);
        match state.trading {
            Some(true) => self.trading_on += 1,
            Some(false) => self.trading_off += 1,
            None => {}
        }
        self.started_on += usize::from(state.started == Some(true));
        self.stopped += usize::from(state.needs_restart());
        let started_stale = !state.started_confirmed;
        self.started_on_stale += usize::from(state.started == Some(true) && started_stale);
        self.stopped_stale += usize::from(state.needs_restart() && started_stale);
        let trading_stale = !state.trading_confirmed;
        self.trading_on_stale += usize::from(state.trading == Some(true) && trading_stale);
        self.trading_off_stale += usize::from(state.trading == Some(false) && trading_stale);
        if state.online {
            self.needing_start +=
                usize::from(!(state.trading == Some(true) && state.trading_confirmed));
            self.needing_stop +=
                usize::from(!(state.trading == Some(false) && state.trading_confirmed));
        }
    }

    /// Return what the trading control offers for this scope.
    ///
    /// Stop is offered ONLY when every core that reported is trading: with a mixed set, "Stop"
    /// would be a button whose own label is already true for half its targets, while Start
    /// finishes the job the user is visibly asking for. Nothing reported means nothing to offer —
    /// a disabled control, never a Start that would fire blind at cores that may already trade.
    ///
    /// Returns:
    ///     The offered action.
    pub fn trading_action(self) -> TradingAction {
        match (self.trading_on, self.trading_off) {
            (0, 0) => TradingAction::Unknown,
            (_, 0) => TradingAction::Stop,
            _ => TradingAction::Start,
        }
    }

    /// Whether only PART of the scope is trading, which the control says in its tooltip.
    ///
    /// Returns:
    ///     Whether both a trading and a stopped core reported.
    pub fn trading_mixed(self) -> bool {
        self.trading_on > 0 && self.trading_off > 0
    }
}
