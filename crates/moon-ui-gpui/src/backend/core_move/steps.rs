//! The six steps a move shows the user, as data: the move's thread reports each step's state as
//! a [`StepEvent`], and [`Steps::apply`] folds those events into the six rows the wizard draws.
//!
//! Nothing here is worded: a row carries a [`StepNote`], and the wizard puts it in the user's
//! language, so the fold stays a pure function a test drives without a locale or a window.

use std::time::Duration;

/// One of the six steps, in the order a move takes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StepId {
    /// The settings snapshot, the strategies and the report database are taken from the old core.
    Settings,
    /// The server is checked: it runs MoonBot and the port is free.
    Server,
    /// The new core is uploaded and installed, not started.
    Install,
    /// The old core is asked to stop, and is down.
    StopOld,
    /// The new core starts, logs in, and gets the old core's strategies.
    StartNew,
    /// This terminal and the station switch to the new core.
    Switch,
}

impl StepId {
    /// Every step, in the order a move takes them.
    pub(crate) const ALL: [StepId; 6] = [
        StepId::Settings,
        StepId::Server,
        StepId::Install,
        StepId::StopOld,
        StepId::StartNew,
        StepId::Switch,
    ];

    /// The step's place in [`StepId::ALL`].
    fn index(self) -> usize {
        self as usize
    }
}

/// Where one step stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StepState {
    /// Not reached yet.
    Waiting,
    /// Under way.
    Running,
    /// Finished.
    Done,
    /// Stopped on something the user has to answer: the old core refuses to stop.
    Hold,
    /// The move failed here.
    Failed,
    /// The move ended before this step, so it never ran.
    Skipped,
}

/// What a step's detail line says; the wizard words it in the user's language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StepNote {
    /// The settings are being taken from the old core.
    TakingSettings,
    /// The settings are taken: the strategy count (`None`: not read) and whether the trade
    /// history goes along.
    SettingsTaken {
        strategies: Option<usize>,
        history: bool,
    },
    /// The server is being checked.
    CheckingServer,
    /// The server runs MoonBot and `port` is free.
    ServerFits { port: u16 },
    /// The new core is being uploaded and installed.
    Installing,
    /// The new core is installed in `dir`, not started.
    Installed { dir: String },
    /// The new core was taken off the server again.
    Removed,
    /// The old core is being asked to stop.
    Stopping,
    /// The old core refuses to stop, with `open_deals` unfinished deals (`None`: not read).
    Refused { open_deals: Option<usize> },
    /// The old core is down; its folder stays.
    OldStopped,
    /// The new core is being started.
    Starting,
    /// The new core runs: whether it logged in to the exchange, and whether its strategies were
    /// confirmed.
    Started { logged_in: bool, strategies: bool },
    /// This terminal and the station are being pointed at the new core.
    Switching,
    /// This terminal and the station work with the new core.
    Switched,
    /// The failure's own reason, as the error said it.
    Reason(String),
    /// The user cancelled the move here.
    Cancelled,
    /// The move ended before this step, which was not needed then.
    NotNeeded,
}

/// One report of a move's progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StepEvent {
    /// Step `id` is now in `state`, with `note` as its detail.
    Mark {
        id: StepId,
        state: StepState,
        note: StepNote,
    },
    /// The move failed at `step` for `reason`.
    Failed { step: StepId, reason: String },
    /// The user cancelled the move.
    Cancelled,
}

/// One step as the wizard draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StepRow {
    pub(crate) id: StepId,
    pub(crate) state: StepState,
    /// The detail line, once the step has one.
    pub(crate) note: Option<StepNote>,
    /// When the step finished (or failed), counted from the move's start.
    pub(crate) at: Option<Duration>,
}

/// The six steps of one move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Steps {
    rows: [StepRow; 6],
}

impl Default for Steps {
    fn default() -> Self {
        Self {
            rows: StepId::ALL.map(|id| StepRow {
                id,
                state: StepState::Waiting,
                note: None,
                at: None,
            }),
        }
    }
}

impl Steps {
    /// The six rows, in the order a move takes them.
    pub(crate) fn rows(&self) -> &[StepRow; 6] {
        &self.rows
    }

    /// The step under way or on hold, else the first not reached yet, else the last.
    pub(crate) fn current(&self) -> StepId {
        self.rows
            .iter()
            .find(|r| matches!(r.state, StepState::Running | StepState::Hold))
            .or_else(|| self.rows.iter().find(|r| r.state == StepState::Waiting))
            .map_or(StepId::Switch, |r| r.id)
    }

    /// Apply `event`, which happened `at` after the move's start.
    ///
    /// A failure or a cancel ends the move: every step that has not finished by then is shown
    /// as not needed, so the list never shows a step still waiting on a move that is over.
    pub(crate) fn apply(&mut self, event: StepEvent, at: Duration) {
        match event {
            StepEvent::Mark { id, state, note } => {
                let row = &mut self.rows[id.index()];
                row.state = state;
                row.note = Some(note);
                row.at = matches!(state, StepState::Done | StepState::Failed).then_some(at);
            }
            StepEvent::Failed { step, reason } => {
                let row = &mut self.rows[step.index()];
                row.state = StepState::Failed;
                row.note = Some(StepNote::Reason(reason));
                row.at = Some(at);
                self.skip_unfinished(StepNote::NotNeeded);
            }
            // The step the user cancelled on says so; the ones after it were not needed.
            StepEvent::Cancelled => self.skip_unfinished(StepNote::Cancelled),
        }
    }

    /// Mark every step that has not finished as skipped: one under way or on hold says
    /// `current`, the ones not reached say "not needed".
    fn skip_unfinished(&mut self, current: StepNote) {
        for row in &mut self.rows {
            match row.state {
                StepState::Running | StepState::Hold => {
                    row.state = StepState::Skipped;
                    row.note = Some(current.clone());
                    row.at = None;
                }
                StepState::Waiting => {
                    row.state = StepState::Skipped;
                    row.note = Some(StepNote::NotNeeded);
                }
                StepState::Done | StepState::Failed | StepState::Skipped => {}
            }
        }
    }
}

/// Fold `events`, each with its time since the move's start, into the six rows.
pub(crate) fn fold(events: impl IntoIterator<Item = (StepEvent, Duration)>) -> Steps {
    let mut steps = Steps::default();
    for (event, at) in events {
        steps.apply(event, at);
    }
    steps
}

#[cfg(test)]
mod tests;
