//! Commands that flip a core's run switches, gated so a press never outlives the connection.
//!
//! The per-core command channel survives a disconnect — the feed thread retries in place — so a
//! command queued for a core that is down is not dropped but replayed whenever the core comes
//! back, which for a trading action can be an hour later and nothing like what the press meant.
//! Every surface that starts or stops trading or AutoDetect (the terminal's run controls, the Mini
//! App) therefore goes through [`SessionManager::dispatch_run`]. The raw senders are crate-private,
//! so no window and no bot can skip the gate.

use super::SessionManager;
use super::run_state::CoreRunState;
use super::store::CoreId;

#[cfg(test)]
mod tests;

/// Which run switch a command flips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunSwitch {
    /// The global strategy engine (Moonbot's own Start/Stop).
    Trading,
    /// AutoDetect, carried by the runtime-state command.
    AutoDetect,
}

/// What a run command does with one core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunTarget {
    /// The core is connected and not yet in the asked state: send.
    Send,
    /// The core is not connected: a queued command would replay when it comes back.
    Offline,
    /// The core already reports the asked state on this connection: an unchanged repeat gets no
    /// answer, so a control would wait for the whole timeout.
    Already,
}

impl RunSwitch {
    /// Decide what a command setting this switch to `on` does with a core in `state`.
    ///
    /// AutoDetect reads `started_confirmed` rather than its own flag: it travels inside the
    /// runtime-state command, so that is what says the value came from this connection.
    ///
    /// Args:
    ///     state: The core's projected run state.
    ///     on: The asked state.
    ///
    /// Returns:
    ///     Whether to send, or why not.
    pub fn target(self, state: &CoreRunState, on: bool) -> RunTarget {
        if !state.online {
            return RunTarget::Offline;
        }
        let already = match self {
            RunSwitch::Trading => state.trading == Some(on) && state.trading_confirmed,
            RunSwitch::AutoDetect => state.auto_detect == Some(on) && state.started_confirmed,
        };
        if already {
            RunTarget::Already
        } else {
            RunTarget::Send
        }
    }
}

/// Outcome of one run command over a scope.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunDispatch {
    /// Cores whose command channel accepted the intent, in the order given. A caller that shows
    /// "waiting for the core" arms exactly these.
    pub sent: Vec<CoreId>,
    /// Cores that needed the command: `sent` plus those whose channel refused it.
    pub needed: usize,
    /// Cores skipped because they are not connected.
    pub offline: usize,
    /// Cores skipped because they already report the asked state.
    pub already: usize,
}

impl RunDispatch {
    /// Cores that needed the command but whose command channel is gone.
    pub fn refused(&self) -> usize {
        self.needed.saturating_sub(self.sent.len())
    }
}

impl SessionManager {
    /// Set one run switch on a scope, commanding only the cores that are connected and not
    /// already in the asked state.
    ///
    /// The single entry point for starting or stopping trading and AutoDetect; see the module note
    /// for why an unreachable core is skipped rather than queued.
    ///
    /// Args:
    ///     cores: Cores the pressed control stands for, without repeats.
    ///     switch: Which switch to set.
    ///     on: The asked state.
    ///
    /// Returns:
    ///     Which cores accepted the command and how many were skipped, and why.
    pub fn dispatch_run(&self, cores: &[CoreId], switch: RunSwitch, on: bool) -> RunDispatch {
        let mut outcome = RunDispatch::default();
        let mut targets = Vec::with_capacity(cores.len());
        for &core in cores {
            match switch.target(&self.core_run_state(core), on) {
                RunTarget::Send => targets.push(core),
                RunTarget::Offline => outcome.offline += 1,
                RunTarget::Already => outcome.already += 1,
            }
        }
        outcome.needed = targets.len();
        if !targets.is_empty() {
            outcome.sent = match switch {
                RunSwitch::Trading => self.set_trading_many(&targets, on),
                RunSwitch::AutoDetect => self.set_auto_detect_many(&targets, on),
            };
        }
        outcome
    }
}
