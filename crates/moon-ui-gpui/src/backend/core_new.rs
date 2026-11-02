//! A fresh core put on a Linux server by the connect wizard, owned by the backend: the install
//! outlives the wizard's window, and opening the wizard again shows where it stands.
//!
//! One install at a time. It runs in two threads, one after the other: the host-key probe, which
//! sends no credential, and — once the user confirmed that key — the install itself
//! ([`worker::run`] over `moon_remote::core_new`). The second thread logs in, checks the server
//! and then waits for the user's next step (install, the exchange keys, a new login, removal),
//! keeping the provider's login in memory only for as long as the install lives. Both report
//! through a channel the backend drains on a timer.
//!
//! Once the core hands out its connection key, the backend adds the core's row to the settings
//! ([`Backend::wizard_add_row`]); once the core is logged in, the wizard waits until this
//! terminal is connected to it — the one check that the core can be reached from outside.

#[cfg(test)]
mod tests;
mod worker;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui::{Context, Task};
use moon_core::diagnostics::CORE_MOVE_TARGET;
use moon_core::feed::ConnStatus;
use moon_remote::core_move::ServerCheck;
use moon_remote::core_new::{CoreStatus, Exchange};
use moon_remote::ssh::Target;
use moon_ui::MoonPalette;
use rust_i18n::t;
use zeroize::Zeroizing;

pub(crate) use worker::{NewCommand, NewForm};

use crate::Backend;
use crate::design;

/// How often the backend drains the install's channel and looks at the new core's connection.
const POLL: Duration = Duration::from_millis(250);
/// Progress lines the install keeps; the oldest go first.
const MAX_LINES: usize = 300;
/// How long a logged-in core may take to be connected to before it reads as unreachable: the
/// terminal's own connect plus the core's initialization.
const CONNECT_WAIT: Duration = Duration::from_secs(90);

/// Where the install stands, as the wizard shows it.
#[derive(Clone, Debug)]
pub(crate) enum NewPhase {
    /// The server's host key is being read; nothing has been sent to it.
    Probing,
    /// The host key is read and waits for the user's confirmation.
    Review { target: Target, fingerprint: String },
    /// Logging in and checking the server.
    Checking,
    /// The server can run the core; waits for "Install".
    Checked { check: ServerCheck },
    /// Downloading, installing and starting the core.
    Installing,
    /// The core runs and waits for its exchange and keys.
    Keys,
    /// The keys went to the core; it is logging in.
    LoggingIn,
    /// The core stopped on something only the user can change, or a step failed while the core
    /// is on the server. `status` is the core's last word, `error` the failed step's.
    Stuck {
        status: Option<CoreStatus>,
        error: Option<String>,
    },
    /// The core is logged in; this terminal is connecting to it.
    Connecting { since: Instant },
    /// This terminal did not get through to the logged-in core in time.
    Unreachable,
    /// Connected: the core works.
    Done,
    /// The install is being taken out.
    Removing,
    /// Taken out again; nothing is left on the server.
    Removed,
    /// The install stopped before anything was put on the server.
    Failed { reason: String },
}

impl NewPhase {
    /// Whether the worker is busy with a step: no other command is offered meanwhile.
    pub(crate) fn busy(&self) -> bool {
        matches!(
            self,
            Self::Probing | Self::Checking | Self::Installing | Self::LoggingIn | Self::Removing
        )
    }

    /// Whether the job is over and may be dropped for a fresh start.
    pub(crate) fn ended(&self) -> bool {
        matches!(
            self,
            Self::Done | Self::Removed | Self::Failed { .. } | Self::Review { .. }
        )
    }
}

/// The install the wizard runs.
pub(crate) struct NewJob {
    pub(crate) phase: NewPhase,
    pub(crate) lines: Vec<String>,
    /// The core's latest `--status`, shown while it settles.
    pub(crate) status: Option<CoreStatus>,
    /// The exchanges the core offers, once it runs.
    pub(crate) exchanges: Vec<Exchange>,
    /// The server's public address, the one the core is reached at.
    pub(crate) public_ip: String,
    pub(crate) proto_port: u16,
    /// The instance folder, once the core is installed.
    pub(crate) dir: Option<String>,
    /// The core's row in the settings, once it was added.
    pub(crate) uid: Option<u64>,
    /// The worker's command channel, while it lives.
    commands: Option<mpsc::Sender<NewCommand>>,
    /// The drain of the running thread's channel, held so it is not cancelled.
    _task: Task<()>,
}

impl NewJob {
    /// `ip:port` the core is reached at.
    pub(crate) fn address(&self) -> String {
        format!("{}:{}", self.public_ip, self.proto_port)
    }

    /// Whether the worker takes commands: it is alive and idle.
    pub(crate) fn takes_commands(&self) -> bool {
        self.commands.is_some() && !self.phase.busy()
    }
}

/// What the install's threads report.
pub(super) enum Event {
    Probed {
        target: Target,
        fingerprint: String,
    },
    /// A progress line, already in the user's language.
    Line(String),
    Checked(ServerCheck),
    /// The core is installed and handed out its connection key; it offers these exchanges.
    Installed {
        dir: String,
        key: Zeroizing<String>,
        exchanges: Vec<Exchange>,
    },
    /// A step began: the worker is busy until its result.
    Busy(NewPhase),
    /// One `--status` while the core settles.
    Status(CoreStatus),
    /// The core settled (or the wait ran out) with this status.
    Settled(CoreStatus),
    /// A step failed while the core is on the server; the worker waits for the next command.
    StepFailed(String),
    Removed,
    /// The install ended before anything was left on the server.
    Failed(String),
}

impl Backend {
    /// Read the host key of the server a new core goes to. Sends no credential.
    pub(crate) fn core_new_probe(&mut self, target: Target, cx: &mut Context<Self>) {
        if self.core_new.as_ref().is_some_and(|job| !job.phase.ended()) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("core-new-probe".into())
            .spawn(move || worker::probe(target, &tx))
            .ok();
        self.core_new_track(NewPhase::Probing, None, rx, String::new(), 0, cx);
    }

    /// Log in with the form the user confirmed in the review and check the server; the worker
    /// then waits for [`NewCommand`]s.
    pub(crate) fn core_new_start(&mut self, form: NewForm, cx: &mut Context<Self>) {
        if !matches!(
            self.core_new.as_ref().map(|job| &job.phase),
            Some(NewPhase::Review { .. })
        ) {
            return;
        }
        let (public_ip, proto_port) = (form.public_ip.to_string(), form.proto_port);
        let (tx, rx) = mpsc::channel();
        let (commands, answers) = mpsc::channel();
        std::thread::Builder::new()
            .name("core-new".into())
            .spawn(move || worker::run(form, &tx, &answers))
            .ok();
        self.core_new_track(
            NewPhase::Checking,
            Some(commands),
            rx,
            public_ip,
            proto_port,
            cx,
        );
    }

    /// Pass the user's next step to the install.
    pub(crate) fn core_new_command(&mut self, command: NewCommand, cx: &mut Context<Self>) {
        let Some(job) = self.core_new.as_mut() else {
            return;
        };
        if !job.takes_commands() {
            return;
        }
        let busy = match &command {
            NewCommand::Install => NewPhase::Installing,
            NewCommand::SetKeys { .. } | NewCommand::Retry => NewPhase::LoggingIn,
            NewCommand::Remove => NewPhase::Removing,
        };
        if let Some(commands) = job.commands.as_ref()
            && commands.send(command).is_ok()
        {
            job.phase = busy;
            cx.notify();
        }
    }

    /// Wait again for this terminal to reach a core that read as unreachable.
    pub(crate) fn core_new_reconnect(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.core_new.as_mut() else {
            return;
        };
        if let (NewPhase::Unreachable, Some(uid)) = (&job.phase, job.uid) {
            job.phase = NewPhase::Connecting {
                since: Instant::now(),
            };
            self.reconnect_request.push(uid);
            cx.notify();
        }
    }

    /// Drop an install that is over, so the wizard starts afresh. The worker, if alive, ends
    /// with its channel and forgets the provider's login.
    pub(crate) fn core_new_forget(&mut self, cx: &mut Context<Self>) {
        if self.core_new.as_ref().is_some_and(|job| !job.phase.busy()) {
            self.core_new = None;
            cx.notify();
        }
    }

    /// Register the install's thread and drain its channel until it ends.
    fn core_new_track(
        &mut self,
        phase: NewPhase,
        commands: Option<mpsc::Sender<NewCommand>>,
        rx: mpsc::Receiver<Event>,
        public_ip: String,
        proto_port: u16,
        cx: &mut Context<Self>,
    ) {
        // The probe opens a fresh install; the install itself continues the probe's log.
        let lines = match phase {
            NewPhase::Probing => Vec::new(),
            _ => self
                .core_new
                .take()
                .map(|job| job.lines)
                .unwrap_or_default(),
        };
        let task = cx.spawn(async move |this, cx| {
            let mut gone = false;
            loop {
                cx.background_executor().timer(POLL).await;
                let mut events = Vec::new();
                while !gone {
                    match rx.try_recv() {
                        Ok(event) => events.push(event),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => gone = true,
                    }
                }
                let alive = this.update(cx, |b, cx| {
                    let mut changed = !events.is_empty();
                    for event in events {
                        b.core_new_event(event, cx);
                    }
                    changed |= b.core_new_watch_connection();
                    // A thread gone in the middle of a step still ends it here: nothing else would.
                    // The probe thread holds no command channel, so the phase decides, not the
                    // channel.
                    if gone && let Some(job) = b.core_new.as_mut() {
                        changed |= job.commands.take().is_some();
                        if job.phase.busy() {
                            log::warn!(target: CORE_MOVE_TARGET, "core install: its thread ended without a word");
                            job.phase = NewPhase::Failed {
                                reason: t!("connect_wizard.lost").to_string(),
                            };
                            changed = true;
                        }
                    }
                    if changed {
                        cx.notify();
                    }
                    // The drain lives while its thread does, or the job has something to watch.
                    !gone
                        || b.core_new.as_ref().is_some_and(|job| {
                            job.commands.is_some()
                                || matches!(job.phase, NewPhase::Connecting { .. })
                        })
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        });
        self.core_new = Some(NewJob {
            phase,
            lines,
            status: None,
            exchanges: Vec::new(),
            public_ip,
            proto_port,
            dir: None,
            uid: None,
            commands,
            _task: task,
        });
        cx.notify();
    }

    /// Apply one event of the install.
    fn core_new_event(&mut self, event: Event, cx: &mut Context<Self>) {
        let Some(job) = self.core_new.as_mut() else {
            return;
        };
        // The app log keeps the install too; none of these lines carries a secret.
        match &event {
            Event::Probed {
                target,
                fingerprint,
            } => {
                log::info!(target: CORE_MOVE_TARGET, "core install: {} presents {fingerprint}", target.addr())
            }
            Event::Line(line) => log::info!(target: CORE_MOVE_TARGET, "core install: {line}"),
            Event::Checked(check) => {
                log::info!(target: CORE_MOVE_TARGET, "core install: server checked: {check:?}")
            }
            Event::Installed { dir, .. } => {
                log::info!(target: CORE_MOVE_TARGET, "core install: installed in {dir}")
            }
            Event::Busy(_) | Event::Status(_) => {}
            Event::Settled(status) => log::info!(
                target: CORE_MOVE_TARGET,
                "core install: settled {:?} listening={} {}",
                status.state,
                status.moonproto_listening,
                status.login_error
            ),
            Event::StepFailed(e) => {
                log::warn!(target: CORE_MOVE_TARGET, "core install: step failed: {e}")
            }
            Event::Removed => log::info!(target: CORE_MOVE_TARGET, "core install: removed"),
            Event::Failed(e) => log::warn!(target: CORE_MOVE_TARGET, "core install: failed: {e}"),
        }
        match event {
            Event::Probed {
                target,
                fingerprint,
            } => {
                job.phase = NewPhase::Review {
                    target,
                    fingerprint,
                }
            }
            Event::Line(line) => push_line(&mut job.lines, line),
            Event::Checked(check) => job.phase = NewPhase::Checked { check },
            Event::Busy(phase) => job.phase = phase,
            Event::Status(status) => job.status = Some(status),
            Event::Installed {
                dir,
                key,
                exchanges,
            } => {
                job.dir = Some(dir.clone());
                job.exchanges = exchanges;
                match self.wizard_add_row(dir.rsplit('/').next().unwrap_or(&dir), &key, cx) {
                    Ok(uid) => {
                        if let Some(job) = self.core_new.as_mut() {
                            job.uid = Some(uid);
                            job.phase = NewPhase::Keys;
                            push_line(
                                &mut job.lines,
                                t!("connect_wizard.line.row_added").to_string(),
                            );
                        }
                    }
                    Err(e) => {
                        log::warn!(target: CORE_MOVE_TARGET, "core install: the row was not added: {e:#}");
                        if let Some(job) = self.core_new.as_mut() {
                            job.phase = NewPhase::Stuck {
                                status: None,
                                error: Some(
                                    t!("connect_wizard.row_failed", reason = format!("{e:#}"))
                                        .to_string(),
                                ),
                            };
                        }
                    }
                }
            }
            Event::Settled(status) => {
                let ready = status.connectable();
                let uid = job.uid;
                job.status = Some(status.clone());
                job.phase = if ready {
                    NewPhase::Connecting {
                        since: Instant::now(),
                    }
                } else {
                    NewPhase::Stuck {
                        status: Some(status),
                        error: None,
                    }
                };
                // The session may have backed off while the core was not logged in yet.
                if ready && let Some(uid) = uid {
                    self.reconnect_request.push(uid);
                }
            }
            Event::StepFailed(error) => {
                job.phase = NewPhase::Stuck {
                    status: job.status.clone(),
                    error: Some(error),
                }
            }
            Event::Removed => {
                job.commands = None;
                job.phase = NewPhase::Removed;
                if let Some(uid) = job.uid.take()
                    && let Err(e) = self.wizard_drop_row(uid, cx)
                {
                    log::warn!(target: CORE_MOVE_TARGET, "core install: the row {uid} was not removed: {e:#}");
                }
            }
            Event::Failed(reason) => {
                job.commands = None;
                job.phase = NewPhase::Failed { reason };
            }
        }
    }

    /// Move a connecting install to done once this terminal reached the core, or to unreachable
    /// when it did not in time. Returns whether the phase changed.
    fn core_new_watch_connection(&mut self) -> bool {
        let Some(job) = self.core_new.as_mut() else {
            return false;
        };
        let (NewPhase::Connecting { since }, Some(uid)) = (&job.phase, job.uid) else {
            return false;
        };
        if matches!(self.session.status_map().get(&uid), Some(ConnStatus::Ready)) {
            push_line(
                &mut job.lines,
                t!("connect_wizard.line.connected").to_string(),
            );
            job.phase = NewPhase::Done;
            // Done: the worker ends with its channel and forgets the provider's login.
            job.commands = None;
            true
        } else if since.elapsed() >= CONNECT_WAIT {
            job.phase = NewPhase::Unreachable;
            true
        } else {
            false
        }
    }

    /// Add a wizard's core row, named `base` (numbered when taken) with connection key `key`, to
    /// the saved settings — and to an open Settings draft, so saving that draft does not drop it.
    /// Returns the row's uid.
    pub(crate) fn wizard_add_row(
        &mut self,
        base: &str,
        key: &str,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<u64> {
        let color = design::u32_to_rgb(MoonPalette::active(cx).accent);
        let mut candidate = self.config.clone();
        let name = unique_name(base, candidate.servers.iter().map(|s| s.name.as_str()));
        let id = candidate.servers.iter().map(|s| s.id).max().unwrap_or(0) + 1;
        candidate
            .servers
            .push(crate::settings::connections::table::new_server_row(
                id,
                name,
                moon_core::config::servers::default_group(),
                key,
                color,
            ));
        candidate.save()?;
        let row = candidate
            .servers
            .last()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("the new row is gone after the save"))?;
        self.config = candidate;
        if let Some(preview) = self.preview.as_mut() {
            preview.servers.push(row.clone());
            crate::settings::connections::sync_groups_from_servers(
                &preview.servers,
                &mut preview.groups,
            );
        }
        self.servers_changed_outside = self.servers_changed_outside.wrapping_add(1);
        let reports = self.reports.as_ref().map(|h| &h.tx);
        self.session.reconcile(&self.config, reports);
        cx.notify();
        Ok(row.uid)
    }

    /// Take a wizard's core row out of the saved settings and an open draft — the row of a
    /// removed install, or of a core the user gave up connecting to.
    pub(crate) fn wizard_drop_row(
        &mut self,
        uid: u64,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let mut candidate = self.config.clone();
        candidate.servers.retain(|s| s.uid != uid);
        candidate.save()?;
        self.config = candidate;
        if let Some(preview) = self.preview.as_mut() {
            preview.servers.retain(|s| s.uid != uid);
            crate::settings::connections::sync_groups_from_servers(
                &preview.servers,
                &mut preview.groups,
            );
        }
        self.servers_changed_outside = self.servers_changed_outside.wrapping_add(1);
        let reports = self.reports.as_ref().map(|h| &h.tx);
        self.session.reconcile(&self.config, reports);
        cx.notify();
        Ok(())
    }
}

/// `base`, or `base 2`, `base 3`… — the first that no row in `taken` is named.
fn unique_name<'a>(base: &str, taken: impl Iterator<Item = &'a str> + Clone) -> String {
    let free = |name: &str| !taken.clone().any(|t| t == name);
    if free(base) {
        return base.to_owned();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|name| free(name))
        .unwrap_or_else(|| base.to_owned())
}

/// Append `line`, dropping the oldest past [`MAX_LINES`].
fn push_line(lines: &mut Vec<String>, line: String) {
    lines.push(line);
    if lines.len() > MAX_LINES {
        lines.drain(..lines.len() - MAX_LINES);
    }
}
