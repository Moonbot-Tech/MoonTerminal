//! The one answer to "is this core running, is it detecting, and is it trading" — plus what a
//! control offering to change any of that may show.
//!
//! Everything here is pure and lives in `moon-core` rather than in a window, because more than one
//! surface asks the question — today the Profit Monitor's run column and the core-settings popup,
//! with the Core Status panel next — and each answering it from raw [`CoreData`] fields is how they
//! come to disagree. A caller reads
//! [`SessionManager::core_run_state`](super::SessionManager::core_run_state) and gets a value that
//! already knows the rules that are easy to get wrong:
//!
//! - the core reports its market runtime (`TRuntimeStateCommand`, which also carries AutoDetect)
//!   and its strategy engine (`TStratRuntimeState`) over SEPARATE commands, so either half can be
//!   known while the other is not. `None` is a real third state and never a `false`;
//! - AutoDetect is not a free-standing flag: passive mode is `is_started=true` with
//!   `auto_detect_active=false` (`feed::types::RuntimeState`), so a `false` on a core whose runtime
//!   is stopped identifies nothing and must not be read as "passive";
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
    /// Arrives in the same command as [`Self::started`], so [`Self::started_confirmed`] answers for
    /// this half too — there is no second confirmation flag to keep, and inventing one would let
    /// two fields that cannot disagree drift apart. The core-settings popup shows it as a status
    /// dot; the run control's auto slot both shows and changes it, through
    /// `Session::set_auto_detect`.
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

/// What the AutoDetect control offers for one core or one set of them.
///
/// A separate enum from [`TradingAction`] even though the shape matches: these are two different
/// commands with two different meanings, and one enum shared between them would let a caller pass
/// a trading decision to the AutoDetect action without the compiler noticing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutoAction {
    /// Detection is off (or partly off): offer to turn it on.
    Enable,
    /// Detection is active everywhere in scope: offer to turn it off.
    Disable,
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
    /// How many reported AutoDetect as active.
    pub auto_on: usize,
    /// How many reported it as off — the core is in passive mode.
    pub auto_off: usize,
    /// Of the cores reporting AutoDetect as active, how many said so on a previous connection.
    ///
    /// Split by value like the runtime counters above, and derived from the same
    /// `started_confirmed`: AutoDetect travels in the runtime-state command, so there is nothing
    /// else that could confirm it.
    pub auto_on_stale: usize,
    /// Of the cores reporting AutoDetect as off, how many said so on a previous connection.
    pub auto_off_stale: usize,
    /// How many reachable cores are not already detecting.
    ///
    /// What a press REACHES, which is deliberately wider than what votes on the control's face: a
    /// core that has reported nothing is still connected and is still commanded, exactly as the
    /// trading counters above work.
    pub needing_auto_on: usize,
    /// How many reachable cores are not already passive.
    pub needing_auto_off: usize,
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
        // AutoDetect votes asymmetrically, and deliberately: `auto_detect_active=true` says the
        // core is detecting whatever its runtime is doing, while a `false` identifies passive mode
        // ONLY together with a started runtime (`feed::types::RuntimeState`). A stopped core's
        // `false` therefore votes for nothing at all — the same reading the core-settings popup's
        // dot has always used, and what keeps the two surfaces from describing one core two ways.
        let passive = state.auto_detect == Some(false) && state.started == Some(true);
        self.auto_on += usize::from(state.auto_detect == Some(true));
        self.auto_off += usize::from(passive);
        self.auto_on_stale += usize::from(state.auto_detect == Some(true) && started_stale);
        self.auto_off_stale += usize::from(passive && started_stale);
        if state.online {
            self.needing_start +=
                usize::from(!(state.trading == Some(true) && state.trading_confirmed));
            self.needing_stop +=
                usize::from(!(state.trading == Some(false) && state.trading_confirmed));
            self.needing_auto_on +=
                usize::from(!(state.auto_detect == Some(true) && state.started_confirmed));
            self.needing_auto_off +=
                usize::from(!(state.auto_detect == Some(false) && state.started_confirmed));
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

    /// Return what the AutoDetect control offers for this scope.
    ///
    /// The same rule [`Self::trading_action`] follows, for the same reason: with a mixed set, the
    /// press that finishes the job is the one that turns the remaining cores ON, while an Off whose
    /// own state is already true for half the scope reads as a no-op. A scope where NOTHING
    /// interpretable was reported offers no control at all — not a switch whose face would be a
    /// guess. What a press then reaches is a wider set than what voted on it; see
    /// [`Self::needing_auto_on`].
    ///
    /// Returns:
    ///     The offered action.
    pub fn auto_action(self) -> AutoAction {
        match (self.auto_on, self.auto_off) {
            (0, 0) => AutoAction::Unknown,
            (_, 0) => AutoAction::Disable,
            _ => AutoAction::Enable,
        }
    }

    /// Whether only PART of the scope is detecting, which the control says in its tooltip.
    ///
    /// Returns:
    ///     Whether both a detecting and a passive core reported.
    pub fn auto_mixed(self) -> bool {
        self.auto_on > 0 && self.auto_off > 0
    }
}
