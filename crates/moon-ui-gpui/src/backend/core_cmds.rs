//! Backend queues, per-core workers and cached results for core server commands.

mod model;
mod worker;
pub(crate) use model::*;

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui::Context;
use moon_core::config::{parse_endpoint_override, target_from_key};
use moon_remote::core_cmds::{Cmd, Login};
use moon_remote::hosts::Hosts;
use moon_remote::setup::FirstAccess;
use moon_remote::ssh::Target;

use crate::Backend;

/// How often pending commands and worker events reach the backend.
const POLL: Duration = Duration::from_millis(250);
/// How long a status check suppresses another automatic status request.
const STATUS_REFRESH: Duration = Duration::from_secs(60);

#[allow(dead_code)] // frozen methods are called by the later window and Connections batches
impl Backend {
    /// Queue `req` for the command worker associated with core `uid`.
    pub(crate) fn core_cmds_send(&mut self, uid: u64, req: CmdRequest) {
        self.core_cmds.pending.push((uid, req));
    }

    /// Store a confirmed provider login for `ip` for the rest of this terminal session.
    pub(crate) fn core_cmds_add_login(&mut self, ip: String, login: SessionLogin) {
        self.core_cmds.logins.insert(ip, login);
    }

    /// Return the cached server facts for core `uid`, if a request has created them.
    pub(crate) fn core_cmds_facts(&self, uid: u64) -> Option<&CoreFacts> {
        self.core_cmds.facts.get(&uid)
    }

    /// Queue a status request unless a successful answer is less than 60 seconds old.
    pub(crate) fn core_cmds_request_status(&mut self, uid: u64) {
        self.core_cmds.request_status(uid);
    }

    /// Prefer a confirmed session login, then fall back to the administrator pinned for `ip`.
    pub(crate) fn core_cmds_login_for(&self, ip: &str) -> Option<Login> {
        if let Some(login) = self.core_cmds.logins.get(ip) {
            let first = match &login.first {
                FirstAccess::Password { user, password } => FirstAccess::Password {
                    user: user.clone(),
                    password: password.clone(),
                },
                FirstAccess::Key {
                    user,
                    key,
                    sudo_password,
                } => FirstAccess::Key {
                    user: user.clone(),
                    key: key.clone(),
                    sudo_password: sudo_password.clone(),
                },
            };
            return Some(Login::Provider {
                first,
                pin: login.pin.clone(),
            });
        }
        Hosts::load(&Hosts::path())
            .ok()?
            .admin_by_ip(ip)
            .map(|_| Login::Admin)
    }

    /// Start the persistent 250 ms drain for all per-core command workers.
    pub(crate) fn core_cmds_track(&mut self, cx: &mut Context<Self>) {
        let (events, answers) = mpsc::channel::<(u64, worker::Event)>();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this
                    .update(cx, |backend, cx| {
                        backend.core_cmds_tick(&events, &answers, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Dispatch queued requests and apply every event workers have published.
    fn core_cmds_tick(
        &mut self,
        events: &mpsc::Sender<(u64, worker::Event)>,
        answers: &mpsc::Receiver<(u64, worker::Event)>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for (uid, request) in std::mem::take(&mut self.core_cmds.pending) {
            changed = true;
            if matches!(request, CmdRequest::Forget) {
                self.core_cmds.workers.remove(&uid);
                self.core_cmds.facts.remove(&uid);
                continue;
            }
            let cmd = match &request {
                CmdRequest::Run { cmd, .. } => *cmd,
                CmdRequest::Refresh => Cmd::Status,
                CmdRequest::Forget => unreachable!(),
            };
            let config = self.core_cmds_worker_config(uid);
            let Some((target, display, port, login)) = config else {
                if let Some(facts) = self.core_cmds.facts.get_mut(&uid) {
                    facts.phases.insert(
                        cmd,
                        CmdPhase::Failed(CmdError::Ssh(
                            "no SSH login is available for this server".into(),
                        )),
                    );
                }
                continue;
            };
            let facts = self
                .core_cmds
                .facts
                .entry(uid)
                .or_insert_with(|| core_facts(display, port));
            if !self.core_cmds.workers.contains_key(&uid)
                && let Some(sender) = worker::spawn(
                    uid,
                    target,
                    login,
                    port,
                    facts.located.clone(),
                    events.clone(),
                )
            {
                self.core_cmds.workers.insert(uid, sender);
            }
            facts.phases.insert(
                cmd,
                CmdPhase::Running {
                    since: Instant::now(),
                    seen: None,
                },
            );
            let sent = self
                .core_cmds
                .workers
                .get(&uid)
                .is_some_and(|worker| worker.send(request).is_ok());
            if !sent {
                self.core_cmds.workers.remove(&uid);
                if let Some(facts) = self.core_cmds.facts.get_mut(&uid) {
                    facts.phases.insert(
                        cmd,
                        CmdPhase::Failed(CmdError::Ssh(
                            "the core command worker could not be started".into(),
                        )),
                    );
                }
            }
        }
        while let Ok((uid, event)) = answers.try_recv() {
            changed |= self.core_cmds_event(uid, event);
        }
        if changed {
            cx.notify();
        }
    }

    /// Resolve the core address and login a worker needs without contacting the server.
    fn core_cmds_worker_config(&self, uid: u64) -> Option<(Target, String, u16, Login)> {
        let server = self
            .config
            .servers
            .iter()
            .find(|server| server.uid == uid)?;
        let endpoint = parse_endpoint_override(&server.endpoint_override).ok()?;
        let core = target_from_key(server.key.expose(), endpoint.as_ref())?;
        let host = core.host.to_string();
        let login = self.core_cmds_login_for(&host)?;
        let fallback = format!("{host}:22");
        let display = match &login {
            Login::Provider { .. } => self
                .core_cmds
                .logins
                .get(&host)
                .map(|login| login.target.clone())
                .filter(|target| !target.is_empty())
                .unwrap_or(fallback),
            Login::Admin => Hosts::load(&Hosts::path())
                .ok()
                .and_then(|hosts| {
                    hosts
                        .admin_by_ip(&host)
                        .and_then(|host| host.admin.as_deref())
                        .map(|admin| format!("{admin}@{host}:22"))
                })
                .unwrap_or(fallback),
        };
        Some((Target { host, port: 22 }, display, core.port, login))
    }

    /// Apply one worker event and return whether it changed a cached core.
    fn core_cmds_event(&mut self, uid: u64, event: worker::Event) -> bool {
        match event {
            worker::Event::AddressChanged { host, port } => {
                self.core_cmds_address_changed(uid, &host, port)
            }
            event => self
                .core_cmds
                .facts
                .get_mut(&uid)
                .is_some_and(|facts| facts.apply(event)),
        }
    }

    /// Save a changed MoonProto endpoint through the same settings edit as a completed core move.
    fn core_cmds_address_changed(&mut self, uid: u64, host: &str, port: u16) -> bool {
        let address = format!("{host}:{port}");
        let result = (|| {
            let point = |servers: &mut Vec<moon_core::config::ServerConfig>| -> bool {
                match servers.iter_mut().find(|server| server.uid == uid) {
                    Some(server) => {
                        server.endpoint_override = address.clone();
                        server.endpoint_to_station = true;
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
            let reports = self.reports.as_ref().map(|handle| &handle.tx);
            self.session.reconcile(&self.config, reports);
            self.servers_changed_outside = self.servers_changed_outside.wrapping_add(1);
            Ok::<(), anyhow::Error>(())
        })();
        if let Some(facts) = self.core_cmds.facts.get_mut(&uid) {
            if let Err(error) = result {
                facts.phases.insert(
                    Cmd::MoonProtoAddress,
                    CmdPhase::Failed(CmdError::Ssh(format!("{error:#}"))),
                );
            } else {
                facts.port = port;
                facts.located = None;
                facts.status = None;
                facts.info = None;
                facts.checked_at = None;
            }
        }
        true
    }
}

impl CoreCmdsState {
    /// Queue one status check unless a recent or pending check already covers this core.
    fn request_status(&mut self, uid: u64) {
        let fresh = self.facts.get(&uid).is_some_and(|facts| {
            facts
                .checked_at
                .is_some_and(|checked| checked.elapsed() < STATUS_REFRESH)
        });
        let pending = self.pending.iter().any(|(pending_uid, request)| {
            *pending_uid == uid
                && matches!(
                    request,
                    CmdRequest::Run {
                        cmd: Cmd::Status,
                        ..
                    } | CmdRequest::Refresh
                )
        });
        if fresh || pending {
            return;
        }
        if let Some(facts) = self.facts.get_mut(&uid) {
            facts.checked_at = Some(Instant::now());
        }
        self.pending.push((
            uid,
            CmdRequest::Run {
                cmd: Cmd::Status,
                args: CmdArgs::None,
            },
        ));
    }
}

impl CoreFacts {
    /// Apply a worker event that does not alter the terminal's saved endpoint.
    fn apply(&mut self, event: worker::Event) -> bool {
        match event {
            worker::Event::Phase(cmd, phase) => {
                self.phases.insert(cmd, phase);
            }
            worker::Event::Located(located) => self.located = Some(located),
            worker::Event::Status(status) => {
                self.status = Some(status);
                self.checked_at = Some(Instant::now());
                self.phases.insert(Cmd::Status, CmdPhase::Done);
            }
            worker::Event::Settling(status) => {
                let since = match self.phases.get(&Cmd::RetryLogin) {
                    Some(CmdPhase::Running { since, .. }) => *since,
                    _ => Instant::now(),
                };
                self.phases.insert(
                    Cmd::RetryLogin,
                    CmdPhase::Running {
                        since,
                        seen: Some(status.state.clone()),
                    },
                );
                self.status = Some(status);
            }
            worker::Event::Info(info) => {
                self.located = Some(info.located.clone());
                self.info = Some(info);
            }
            worker::Event::Log(log) => self.log = Some(log),
            worker::Event::Key(key) => self.key = Some(key),
            worker::Event::Exchanges(exchanges) => self.exchanges = Some(exchanges),
            worker::Event::AddressChanged { .. } => return false,
        }
        true
    }
}

/// Create empty cached facts for a core's first queued request.
fn core_facts(target: String, port: u16) -> CoreFacts {
    CoreFacts {
        target,
        port,
        located: None,
        status: None,
        info: None,
        log: None,
        key: None,
        exchanges: None,
        phases: HashMap::new(),
        checked_at: None,
    }
}

#[cfg(test)]
mod tests;
