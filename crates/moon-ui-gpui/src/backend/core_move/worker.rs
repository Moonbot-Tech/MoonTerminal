//! The threads of a move: the host-key probe, and the move itself.

use std::net::Ipv4Addr;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use moon_core::core_move::old_core::StopAnswer;
use moon_remote::core_move::plan::{KeySource, Plan, Request};
use moon_remote::core_move::{self, MoveStep};
use moon_remote::setup::{self, FirstAccess};
use moon_remote::ssh::Target;
use rust_i18n::t;

use super::{Event, text};

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
        Err(e) => {
            Event::Failed(t!("core_move.probe_failed", reason = format!("{e:#}")).to_string())
        }
    };
    let _ = tx.send(event);
}

/// Run the move of core `uid` and report its end.
pub(super) fn run(
    uid: u64,
    form: MoveForm,
    tx: &mpsc::Sender<Event>,
    answers: &mpsc::Receiver<Command>,
) {
    let mut say = |step: MoveStep| {
        let _ = tx.send(Event::Line(text::step(&step)));
    };
    let mut stage = Stage::Nothing;
    let event = match run_move(uid, &form, tx, answers, &mut say, &mut stage) {
        Ok(event) => event,
        Err(e) => {
            let reason = format!("{e:#}");
            Event::Failed(match stage {
                Stage::Nothing => t!("core_move.failed", reason = reason).to_string(),
                // Never started, the new core is of no use to anyone: it goes again.
                Stage::Installed(dir) => match remove(&form, &dir, &mut say) {
                    Ok(()) => t!("core_move.failed_removed", reason = reason).to_string(),
                    Err(e) => t!(
                        "core_move.failed_left",
                        reason = reason,
                        dir = dir,
                        remove = format!("{e:#}")
                    )
                    .to_string(),
                },
                Stage::OldStopped(dir) => {
                    t!("core_move.failed_old_stopped", reason = reason, dir = dir).to_string()
                }
                Stage::Started(dir) => {
                    t!("core_move.failed_started", reason = reason, dir = dir).to_string()
                }
            })
        }
    };
    let _ = tx.send(event);
}

/// The move proper; `stage` follows what is on the server, for a failure to report.
fn run_move(
    uid: u64,
    form: &MoveForm,
    tx: &mpsc::Sender<Event>,
    answers: &mpsc::Receiver<Command>,
    say: &mut dyn FnMut(MoveStep),
    stage: &mut Stage,
) -> anyhow::Result<Event> {
    let send = |event: Event| {
        let _ = tx.send(event);
    };
    let (conn, privilege) = setup::provider_login(&form.target, &form.first, &form.pin)?;
    send(Event::Line(
        t!("core_move.logged_in", user = conn.user()).to_string(),
    ));
    let request = Request {
        key: KeySource::Core(uid.to_string()),
        zip: None,
        exchange: None,
        from: None,
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
    core_move::check(&conn, &privilege, form.proto_port, say)?;
    let installed = core_move::install(
        &conn,
        &privilege,
        &core_move::Install {
            dir: &form.dir,
            proto_port: form.proto_port,
            package_url: core_move::TEST_PACKAGE,
            archive: &plan.archive.bytes,
            key_packet: &plan.packet_text,
            ini_flag_added: plan.archive.ini_flag_added,
        },
        say,
    )?;
    let dir = installed.dir;
    *stage = Stage::Installed(dir.clone());
    // The stop may wait for hours: no session is held open over it.
    drop(conn);
    'stop: loop {
        // Read again before every request: the old core kept trading meanwhile.
        plan.refresh_strategies(say);
        let StopAnswer::Refused { open_deals } = plan.stop_old(say)? else {
            break;
        };
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
    let (conn, privilege) = setup::provider_login(&form.target, &form.first, &form.pin)?;
    *stage = Stage::Started(dir.clone());
    core_move::start(&conn, &privilege, &dir, say)?;
    plan.carry_strategies(say)?;
    core_move::finish(&conn, &privilege, &dir, say)?;
    Ok(Event::Started {
        address: format!("{}:{}", form.public_ip, form.proto_port),
    })
}

/// Take the new core out of the server again, over a fresh login.
fn remove(form: &MoveForm, dir: &str, say: &mut dyn FnMut(MoveStep)) -> anyhow::Result<()> {
    let (conn, privilege) = setup::provider_login(&form.target, &form.first, &form.pin)?;
    core_move::remove(&conn, &privilege, dir, say)
}
