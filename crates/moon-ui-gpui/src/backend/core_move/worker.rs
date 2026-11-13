//! The threads of a move: the host-key probe, and the move itself.

use std::cell::Cell;
use std::net::Ipv4Addr;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use moon_core::config::CoreTarget;
use moon_core::core_move::old_core::StopAnswer;
use moon_remote::core_move::plan::{KeySource, Plan, Request};
use moon_remote::core_move::{self, MoveStep, MoveWarning};
use moon_remote::core_new;
use moon_remote::script::Privilege;
use moon_remote::setup::{self, FirstAccess};
use moon_remote::ssh::{Conn, Target};
use rust_i18n::t;

use super::steps::{StepEvent, StepId, StepNote, StepState};
use super::{Event, FailOutcome, text};
use crate::backend::core_new::{login_failure_text, state_text};

/// How often a move that waits on its old core looks whether it went down.
const WATCH: Duration = Duration::from_secs(30);

/// What the user's form gives a move.
pub(crate) struct MoveForm {
    pub(crate) target: Target,
    /// The host key the user confirmed.
    pub(crate) pin: String,
    /// The provider's login: held in memory for the move's life — the start after a long wait
    /// logs in again — and never stored.
    pub(crate) first: FirstAccess,
    /// The server's public address, the one the new core is reached at.
    pub(crate) public_ip: Ipv4Addr,
    /// The instance folder on the server.
    pub(crate) dir: String,
    /// The new core's MoonProto UDP port.
    pub(crate) proto_port: u16,
}

/// The user's answer to an old core that refuses to stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    /// Ask it to stop again.
    Retry,
    /// Wait until its user stops it, then go on by itself.
    WaitForStop,
    /// Give the move up and take the new core out again.
    Cancel,
}

/// How far a move got on the server — what a failure leaves behind.
enum Stage {
    /// Nothing is on the server yet.
    Nothing,
    /// The new core is installed and was never started; the old one still runs.
    Installed(String),
    /// The old core is down and the new one installed, not started: the new core is all that is
    /// left, so nothing takes it out.
    OldStopped(String),
    /// The new core was started (or its start was tried).
    Started(String),
}

/// Read the server's host key and report it; sends no credential.
pub(super) fn probe(target: Target, tx: &mpsc::Sender<Event>) {
    let event = match setup::probe_host_key(target) {
        Ok(key) => Event::Probed {
            target: key.target,
            fingerprint: key.fingerprint,
        },
        Err(e) => Event::Failed {
            reason: format!("{e:#}"),
            outcome: FailOutcome::Probe,
            step: None,
        },
    };
    let _ = tx.send(event);
}

/// The move's six steps as its thread reports them, with the facts their detail lines need.
struct Progress<'a> {
    tx: &'a mpsc::Sender<Event>,
    start: Instant,
    /// The step a failure is reported at: the step under way, unless [`Progress::fail_at`] named
    /// another one for the failure about to follow.
    current: Cell<StepId>,
    /// The old core's strategy count, once read.
    strategies: Cell<Option<usize>>,
    /// Whether the settings archive carries the report database.
    history: Cell<bool>,
    /// Whether the new core's strategies went unconfirmed.
    strategies_unconfirmed: Cell<bool>,
}

impl<'a> Progress<'a> {
    fn new(tx: &'a mpsc::Sender<Event>) -> Self {
        Self {
            tx,
            start: Instant::now(),
            current: Cell::new(StepId::Settings),
            strategies: Cell::new(None),
            history: Cell::new(false),
            strategies_unconfirmed: Cell::new(false),
        }
    }

    /// Put step `id` into `state` with `note`.
    fn mark(&self, id: StepId, state: StepState, note: StepNote) {
        if state == StepState::Running {
            self.current.set(id);
        }
        let event = StepEvent::Mark { id, state, note };
        let _ = self.tx.send(Event::Step(event, self.start.elapsed()));
    }

    /// Name step `id` as where the failure about to be returned is reported, although `id` was
    /// never marked running. Sends nothing: [`run`] reports the failure at `current`.
    ///
    /// For the provider login: it runs while the settings row is under way, yet a refusal there
    /// is the new server turning the move away, not the old core failing to hand its settings
    /// over (#1319).
    fn fail_at(&self, id: StepId) {
        self.current.set(id);
    }

    /// Report `step` as a log line and note what the step rows need from it.
    fn saw(&self, step: &MoveStep) {
        match step {
            MoveStep::OldStrategies { strategies, .. } => self.strategies.set(Some(*strategies)),
            MoveStep::Composed { report_db, .. } => self.history.set(report_db.is_some()),
            MoveStep::Warning(MoveWarning::StrategiesNotConfirmed, _) => {
                self.strategies_unconfirmed.set(true)
            }
            MoveStep::Removed => self.mark(StepId::Install, StepState::Done, StepNote::Removed),
            _ => {}
        }
        let _ = self.tx.send(Event::Line(text::step(step)));
    }
}

/// Run the move of core `uid`, reached at `old` (or failing with why it cannot be), and report
/// its end.
pub(super) fn run(
    uid: u64,
    form: MoveForm,
    old: anyhow::Result<CoreTarget>,
    tx: &mpsc::Sender<Event>,
    answers: &mpsc::Receiver<Command>,
) {
    let progress = Progress::new(tx);
    let mut say = |step: MoveStep| progress.saw(&step);
    let mut stage = Stage::Nothing;
    let moved = run_move(
        uid, &form, old, tx, answers, &progress, &mut say, &mut stage,
    );
    let event = match moved {
        Ok(event) => event,
        Err(e) => {
            // A refused password names the login that was tried, in the user's language,
            // exactly as the new-core wizard says it; any other failure keeps its own text.
            let reason = login_failure_text(&e, &form.first);
            let step = Some(progress.current.get());
            let outcome = match stage {
                Stage::Nothing => FailOutcome::OldRuns,
                // Never started, the new core is of no use to anyone: it goes again.
                Stage::Installed(dir) => match remove(&form, &dir, &mut say) {
                    Ok(()) => FailOutcome::NewRemoved,
                    Err(e) => {
                        let _ = tx.send(Event::Line(
                            t!("core_move.remove_failed", reason = format!("{e:#}")).to_string(),
                        ));
                        FailOutcome::NewLeft { dir }
                    }
                },
                Stage::OldStopped(dir) => FailOutcome::OldStopped { dir },
                Stage::Started(dir) => FailOutcome::Started { dir },
            };
            Event::Failed {
                reason,
                outcome,
                step,
            }
        }
    };
    let _ = tx.send(event);
}

/// The old core's `host:port` as the move's plan takes it, from where this terminal reaches it.
///
/// Resolves a host name, which may block: call it on the move's thread.
///
/// Errors:
///     A host name that does not resolve.
pub(super) fn old_core_from(target: &CoreTarget) -> anyhow::Result<String> {
    let resolved = target
        .resolve()
        .with_context(|| format!("resolve the old core's address {}", target.text()))?;
    Ok(format!("{}:{}", resolved.client_host, target.port))
}

/// The move proper; `stage` follows what is on the server, for a failure to report.
#[allow(clippy::too_many_arguments)] // one call site; a struct would only rename the list
fn run_move(
    uid: u64,
    form: &MoveForm,
    old: anyhow::Result<CoreTarget>,
    tx: &mpsc::Sender<Event>,
    answers: &mpsc::Receiver<Command>,
    progress: &Progress,
    say: &mut dyn FnMut(MoveStep),
    stage: &mut Stage,
) -> anyhow::Result<Event> {
    let send = |event: Event| {
        let _ = tx.send(event);
    };
    progress.mark(
        StepId::Settings,
        StepState::Running,
        StepNote::TakingSettings,
    );
    // Before the login: a core with no known address has nothing to move from.
    let from = old_core_from(&old?)?;
    let (conn, privilege) = setup::provider_login(&form.target, &form.first, &form.pin)
        .inspect_err(|_| progress.fail_at(StepId::Server))?;
    send(Event::Line(
        t!("core_move.logged_in", user = conn.user()).to_string(),
    ));
    let request = Request {
        key: KeySource::Core(uid.to_string()),
        zip: None,
        exchange: None,
        from: Some(from),
        report_db: true,
        strategy_sync: true,
        strategies_off: false,
        public_ip: form.public_ip,
        proto_port: form.proto_port,
    };
    let mut plan = Plan::build(&request, say)?;
    anyhow::ensure!(
        plan.has_old_core(),
        "the old core's address is not known, so the move cannot stop it"
    );
    progress.mark(
        StepId::Settings,
        StepState::Done,
        StepNote::SettingsTaken {
            strategies: progress.strategies.get(),
            history: progress.history.get(),
        },
    );
    progress.mark(StepId::Server, StepState::Running, StepNote::CheckingServer);
    core_move::check(&conn, &privilege, form.proto_port, say)?;
    progress.mark(
        StepId::Server,
        StepState::Done,
        StepNote::ServerFits {
            port: form.proto_port,
        },
    );
    progress.mark(StepId::Install, StepState::Running, StepNote::Installing);
    let installed = core_move::install(
        &conn,
        &privilege,
        &core_move::Install {
            dir: &form.dir,
            proto_port: form.proto_port,
            package_url: core_move::RELEASE_PACKAGE,
            archive: &plan.archive.bytes,
            key_packet: &plan.packet_text,
            ini_flag_added: plan.archive.ini_flag_added,
        },
        say,
    )?;
    let dir = installed.dir;
    *stage = Stage::Installed(dir.clone());
    progress.mark(
        StepId::Install,
        StepState::Done,
        StepNote::Installed { dir: dir.clone() },
    );
    // The stop may wait for hours: no session is held open over it.
    drop(conn);
    'stop: loop {
        // Read again before every request: the old core kept trading meanwhile.
        progress.mark(StepId::StopOld, StepState::Running, StepNote::Stopping);
        plan.refresh_strategies(say);
        let StopAnswer::Refused { open_deals } = plan.stop_old(say)? else {
            break;
        };
        progress.mark(
            StepId::StopOld,
            StepState::Hold,
            StepNote::Refused { open_deals },
        );
        // An answer still queued belongs to an earlier question and is dropped — except a
        // cancel, which stands whenever it came.
        while let Ok(stale) = answers.try_recv() {
            if stale == Command::Cancel {
                remove(form, &dir, say)?;
                *stage = Stage::Nothing;
                return Ok(Event::Cancelled);
            }
        }
        send(Event::Refused { open_deals });
        match answers.recv() {
            Ok(Command::Retry) => send(Event::Resumed),
            Ok(Command::WaitForStop) => {
                send(Event::Waiting);
                loop {
                    match answers.recv_timeout(WATCH) {
                        Ok(Command::Retry) => {
                            send(Event::Resumed);
                            continue 'stop;
                        }
                        Ok(Command::WaitForStop) => {}
                        Ok(Command::Cancel) | Err(RecvTimeoutError::Disconnected) => {
                            remove(form, &dir, say)?;
                            *stage = Stage::Nothing;
                            return Ok(Event::Cancelled);
                        }
                        // The old core answered at its address before (it refused): silence
                        // there now is its user having stopped it.
                        Err(RecvTimeoutError::Timeout) => {
                            if plan.old_is_silent()? {
                                // A cancel sent while the old core was being probed stands: it
                                // was the user's last word before the move went on.
                                if answers.try_iter().any(|c| c == Command::Cancel) {
                                    remove(form, &dir, say)?;
                                    *stage = Stage::Nothing;
                                    return Ok(Event::Cancelled);
                                }
                                send(Event::Line(t!("core_move.old_stopped_by_hand").to_string()));
                                send(Event::Resumed);
                                break 'stop;
                            }
                        }
                    }
                }
            }
            Ok(Command::Cancel) | Err(_) => {
                remove(form, &dir, say)?;
                *stage = Stage::Nothing;
                return Ok(Event::Cancelled);
            }
        }
    }
    *stage = Stage::OldStopped(dir.clone());
    progress.mark(StepId::StopOld, StepState::Done, StepNote::OldStopped);
    progress.mark(StepId::StartNew, StepState::Running, StepNote::Starting);
    let (conn, privilege) = setup::provider_login(&form.target, &form.first, &form.pin)?;
    *stage = Stage::Started(dir.clone());
    core_move::start(&conn, &privilege, &dir, say)?;
    let logged_in = settle(&conn, &privilege, &dir, &send);
    plan.carry_strategies(say)?;
    core_move::finish(&conn, &privilege, &dir, say)?;
    progress.mark(
        StepId::StartNew,
        StepState::Done,
        StepNote::Started {
            logged_in,
            strategies: !progress.strategies_unconfirmed.get(),
        },
    );
    // The backend points this terminal and the station at the new core and ticks this step.
    progress.mark(StepId::Switch, StepState::Running, StepNote::Switching);
    Ok(Event::Started {
        address: format!("{}:{}", form.public_ip, form.proto_port),
    })
}

/// Wait until the new core is logged in and listens (`mb --status`) before its strategies are
/// carried and this terminal switches to it: a core still logging in skips MoonProto commands.
/// Only reported: the old core is down, so the move goes on whatever the new one says.
///
/// Returns whether the new core logged in and listens.
fn settle(conn: &Conn, privilege: &Privilege, dir: &str, send: &dyn Fn(Event)) -> bool {
    // Every change of state is a line: the wait may take minutes, and the move shows nothing else.
    let mut last = None;
    let mut seen = |status: &core_new::CoreStatus| {
        if last.as_ref() != Some(&status.state) {
            last = Some(status.state.clone());
            send(Event::Line(
                t!("core_move.settling", state = state_text(&status.state)).to_string(),
            ));
        }
    };
    let (line, logged_in) =
        match core_new::wait_settled(conn, privilege, dir, core_new::SETTLE_TIMEOUT, &mut seen) {
            Ok(status) if status.connectable() => (t!("core_move.settled").to_string(), true),
            Ok(status) => (
                t!(
                    "core_move.not_settled",
                    state = state_text(&status.state),
                    detail = &status.login_error
                )
                .to_string(),
                false,
            ),
            Err(e) => (
                t!("core_move.status_unread", reason = format!("{e:#}")).to_string(),
                false,
            ),
        };
    send(Event::Line(line));
    logged_in
}

/// Take the new core out of the server again, over a fresh login.
fn remove(form: &MoveForm, dir: &str, say: &mut dyn FnMut(MoveStep)) -> anyhow::Result<()> {
    let (conn, privilege) = setup::provider_login(&form.target, &form.first, &form.pin)?;
    core_move::remove(&conn, &privilege, dir, say)
}
