//! Moves of cores onto Linux servers — the window "Move to VPS…" — owned by the backend, so a
//! move outlives its window and several run at once, one per core.
//!
//! A move runs in two threads, one after the other: the host-key probe, which sends no
//! credential, and — once the user confirmed that key and the secrets leaving the machine — the
//! move itself ([`worker::run`] over `moon_remote::core_move::plan`): install the new core without
//! starting it, stop the old one, start the new one. Each thread reports through a channel the
//! backend drains on a timer; a refusing old core waits on the user's answer through a second
//! channel. Once the new core runs, the backend points this core's row at it and has the station
//! follow ([`Backend::core_move_switch`]).

#[cfg(test)]
mod tests;
mod text;
mod worker;

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use gpui::{Context, Task};
use moon_core::diagnostics::CORE_MOVE_TARGET;
use moon_remote::ssh::Target;
use rust_i18n::t;

pub(crate) use worker::{Command, MoveForm};

use crate::Backend;

/// How often the backend drains a move's channel.
const POLL: Duration = Duration::from_millis(250);
/// Progress lines a move keeps; the oldest go first.
const MAX_LINES: usize = 300;

/// Where a move stands, as its window shows it.
#[derive(Clone, Debug)]
pub(crate) enum MovePhase {
    /// The server's host key is being read; nothing has been sent to it.
    Probing,
    /// The host key is read and waits for the user's confirmation.
    Review { target: Target, fingerprint: String },
    /// The move runs.
    Running,
    /// The old core refuses to stop; `open_deals` unfinished deals (`None`: not read).
    Refused { open_deals: Option<usize> },
    /// Waiting until the old core's user stops it, then the move goes on by itself.
    Waiting,
    /// The new core runs at `address` and this core's row points at it.
    Done { address: String },
    /// The move stopped on this error; the reason says what is left on the server.
    Failed { reason: String },
    /// The user cancelled; the new core is removed again.
    Cancelled,
}

impl MovePhase {
    /// Whether a thread of the move is alive — the window offers no new start then.
    pub(crate) fn busy(&self) -> bool {
        matches!(
            self,
            Self::Probing | Self::Running | Self::Refused { .. } | Self::Waiting
        )
    }
}

/// One core's move.
pub(crate) struct MoveJob {
    pub(crate) phase: MovePhase,
    pub(crate) lines: Vec<String>,
    /// An answer was sent and the thread has not reported since: the answer buttons stay hidden,
    /// so one question takes one answer.
    pub(crate) answering: bool,
    /// The running move's answer channel, while it may wait on one.
    commands: Option<mpsc::Sender<Command>>,
    /// The drain of the running thread's channel, held so it is not cancelled.
    _task: Task<()>,
}

/// Every core's move, by core uid.
#[derive(Default)]
pub(crate) struct CoreMoves {
    jobs: HashMap<u64, MoveJob>,
}

impl CoreMoves {
    /// The move of core `uid`, if one was started.
    pub(crate) fn get(&self, uid: u64) -> Option<&MoveJob> {
        self.jobs.get(&uid)
    }
}

/// What a move's threads report.
pub(super) enum Event {
    /// The server's host key, for the user to confirm.
    Probed {
        target: Target,
        fingerprint: String,
    },
    /// A progress line, already in the user's language.
    Line(String),
    /// The old core refuses to stop.
    Refused {
        open_deals: Option<usize>,
    },
    /// The move waits until the old core is stopped by hand.
    Waiting,
    /// The move goes on after an answer.
    Resumed,
    /// The new core runs at `address`; `uid`'s row is to point there.
    Started {
        address: String,
    },
    Failed(String),
    Cancelled,
}

impl Backend {
    /// Read the host key of the server a move of core `uid` goes to. Sends no credential.
    pub(crate) fn core_move_probe(&mut self, uid: u64, target: Target, cx: &mut Context<Self>) {
        if self.core_moves.get(uid).is_some_and(|job| job.phase.busy()) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("core-move-probe".into())
            .spawn(move || worker::probe(target, &tx))
            .ok();
        self.core_move_track(uid, MovePhase::Probing, None, rx, cx);
    }

    /// Start the move of core `uid` the user confirmed in its review.
    pub(crate) fn core_move_start(&mut self, uid: u64, form: MoveForm, cx: &mut Context<Self>) {
        if !matches!(
            self.core_moves.get(uid).map(|job| &job.phase),
            Some(MovePhase::Review { .. })
        ) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (commands, answers) = mpsc::channel();
        std::thread::Builder::new()
            .name("core-move".into())
            .spawn(move || worker::run(uid, form, &tx, &answers))
            .ok();
        self.core_move_track(uid, MovePhase::Running, Some(commands), rx, cx);
    }

    /// Pass the user's answer to a move that waits on its old core.
    ///
    /// The phase stays the thread's to move; until the thread reports, the job is `answering`
    /// and the window offers no answer, so a second click cannot queue one to a question not
    /// asked yet.
    pub(crate) fn core_move_command(&mut self, uid: u64, command: Command, cx: &mut Context<Self>) {
        let Some(job) = self.core_moves.jobs.get_mut(&uid) else {
            return;
        };
        let answerable = match command {
            Command::Retry | Command::Cancel => {
                matches!(job.phase, MovePhase::Refused { .. } | MovePhase::Waiting)
            }
            Command::WaitForStop => matches!(job.phase, MovePhase::Refused { .. }),
        };
        if job.answering || !answerable {
            return;
        }
        if let Some(commands) = job.commands.as_ref()
            && commands.send(command).is_ok()
        {
            job.answering = true;
            cx.notify();
        }
    }

    /// Forget a finished move, so its window starts over.
    pub(crate) fn core_move_forget(&mut self, uid: u64, cx: &mut Context<Self>) {
        if self
            .core_moves
            .get(uid)
            .is_some_and(|job| !job.phase.busy())
        {
            self.core_moves.jobs.remove(&uid);
            cx.notify();
        }
    }

    /// Add a line to core `uid`'s move log — the station's answer comes in this way.
    pub(crate) fn core_move_note(&mut self, uid: u64, line: String) {
        log::info!(target: CORE_MOVE_TARGET, "core move {uid}: {line}");
        if let Some(job) = self.core_moves.jobs.get_mut(&uid) {
            push_line(&mut job.lines, line);
        }
    }

    /// Register a move's thread and drain its channel until it ends.
    fn core_move_track(
        &mut self,
        uid: u64,
        phase: MovePhase,
        commands: Option<mpsc::Sender<Command>>,
        rx: mpsc::Receiver<Event>,
        cx: &mut Context<Self>,
    ) {
        let lines = match phase {
            // The probe opens a fresh move; the move itself continues the probe's log.
            MovePhase::Probing => Vec::new(),
            _ => self
                .core_moves
                .jobs
                .remove(&uid)
                .map(|job| job.lines)
                .unwrap_or_default(),
        };
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let mut events = Vec::new();
                let mut gone = false;
                loop {
                    match rx.try_recv() {
                        Ok(event) => events.push(event),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            gone = true;
                            break;
                        }
                    }
                }
                if events.is_empty() && !gone {
                    continue;
                }
                let alive = this.update(cx, |b, cx| {
                    for event in events {
                        b.core_move_event(uid, event, cx);
                    }
                    // A thread gone without its end still ends the move here: nothing else would.
                    if gone
                        && let Some(job) = b.core_moves.jobs.get_mut(&uid)
                        && job.phase.busy()
                    {
                        log::warn!(target: CORE_MOVE_TARGET, "core move {uid}: its thread ended without a word");
                        job.phase = MovePhase::Failed {
                            reason: t!("core_move.lost").to_string(),
                        };
                    }
                    cx.notify();
                });
                if alive.is_err() || gone {
                    break;
                }
            }
        });
        self.core_moves.jobs.insert(
            uid,
            MoveJob {
                phase,
                lines,
                answering: false,
                commands,
                _task: task,
            },
        );
        cx.notify();
    }

    /// Apply one event of core `uid`'s move.
    fn core_move_event(&mut self, uid: u64, event: Event, cx: &mut Context<Self>) {
        let Some(job) = self.core_moves.jobs.get_mut(&uid) else {
            return;
        };
        // Any report but a log line answers the last answer: the thread has moved on.
        if !matches!(event, Event::Line(_)) {
            job.answering = false;
        }
        // The app log keeps the move too: a window closed, or the terminal quit, takes its own
        // log with it. None of these lines carries a secret.
        match &event {
            Event::Probed {
                target,
                fingerprint,
            } => {
                log::info!(target: CORE_MOVE_TARGET, "core move {uid}: {} presents {fingerprint}", target.addr())
            }
            Event::Line(line) => log::info!(target: CORE_MOVE_TARGET, "core move {uid}: {line}"),
            Event::Refused { open_deals } => {
                log::info!(target: CORE_MOVE_TARGET, "core move {uid}: the old core refuses to stop ({open_deals:?} open)")
            }
            Event::Waiting => {
                log::info!(target: CORE_MOVE_TARGET, "core move {uid}: waiting for the old core to stop")
            }
            Event::Resumed => log::info!(target: CORE_MOVE_TARGET, "core move {uid}: going on"),
            Event::Started { address } => {
                log::info!(target: CORE_MOVE_TARGET, "core move {uid}: new core runs at {address}")
            }
            Event::Failed(reason) => {
                log::warn!(target: CORE_MOVE_TARGET, "core move {uid}: failed: {reason}")
            }
            Event::Cancelled => log::info!(target: CORE_MOVE_TARGET, "core move {uid}: cancelled"),
        }
        match event {
            Event::Probed {
                target,
                fingerprint,
            } => {
                job.phase = MovePhase::Review {
                    target,
                    fingerprint,
                }
            }
            Event::Line(line) => push_line(&mut job.lines, line),
            Event::Refused { open_deals } => job.phase = MovePhase::Refused { open_deals },
            Event::Waiting => job.phase = MovePhase::Waiting,
            Event::Resumed => job.phase = MovePhase::Running,
            Event::Failed(reason) => {
                job.commands = None;
                job.phase = MovePhase::Failed { reason };
            }
            Event::Cancelled => {
                job.commands = None;
                job.phase = MovePhase::Cancelled;
            }
            Event::Started { address } => {
                job.commands = None;
                match self.core_move_switch(uid, &address) {
                    Ok(()) => {
                        if let Some(job) = self.core_moves.jobs.get_mut(&uid) {
                            push_line(&mut job.lines, t!("core_move.switched").to_string());
                            job.phase = MovePhase::Done { address };
                        }
                        self.station_follow_core(uid, cx);
                    }
                    Err(e) => {
                        log::warn!(target: CORE_MOVE_TARGET, "core move {uid}: the row was not switched to {address}: {e:#}");
                        if let Some(job) = self.core_moves.jobs.get_mut(&uid) {
                            job.phase = MovePhase::Failed {
                                reason: t!(
                                    "core_move.switch_failed",
                                    address = address,
                                    reason = format!("{e:#}")
                                )
                                .to_string(),
                            };
                        }
                    }
                }
            }
        }
    }

    /// Point core `uid`'s row at its moved core at `address` (`ip:port`).
    ///
    /// The key stays as it is — the moved core runs on the same MoonProto key — and the row gets
    /// the new address as its typed endpoint, ticked for the station too. That is the edit the
    /// station reads as "this core moved" (it matches cores by address and finds the unchanged
    /// key), so its history goes on; a replaced key would read as a new core there. The saved
    /// settings and an open Settings draft change alike, or saving the draft would point the row
    /// back at the old core.
    fn core_move_switch(&mut self, uid: u64, address: &str) -> anyhow::Result<()> {
        let point = |servers: &mut Vec<moon_core::config::ServerConfig>| -> bool {
            match servers.iter_mut().find(|s| s.uid == uid) {
                Some(s) => {
                    s.endpoint_override = address.to_owned();
                    s.endpoint_to_station = true;
                    true
                }
                None => false,
            }
        };
        let mut candidate = self.config.clone();
        anyhow::ensure!(
            point(&mut candidate.servers),
            "the core is no longer in the settings"
        );
        candidate.save()?;
        self.config = candidate;
        if let Some(preview) = self.preview.as_mut() {
            point(&mut preview.servers);
        }
        let reports = self.reports.as_ref().map(|h| &h.tx);
        self.session.reconcile(&self.config, reports);
        self.core_moves_switched = self.core_moves_switched.wrapping_add(1);
        Ok(())
    }
}

/// Append `line`, dropping the oldest past [`MAX_LINES`].
fn push_line(lines: &mut Vec<String>, line: String) {
    lines.push(line);
    if lines.len() > MAX_LINES {
        lines.drain(..lines.len() - MAX_LINES);
    }
}
