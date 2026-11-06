//! Backend queues, per-core workers and cached results for core server commands.

mod model;
mod worker;
pub(crate) use model::*;

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui::Context;
use moon_core::config::{AppConfig, ServerConfig, parse_endpoint_override, target_from_key};
use moon_remote::core_cmds::{Cmd, Login};
use moon_remote::core_new::CoreState;
use moon_remote::hosts::Hosts;
use moon_remote::ssh::Target;

use crate::Backend;

/// How often pending commands and worker events reach the backend.
const POLL: Duration = Duration::from_millis(250);
/// How long a status check suppresses another automatic status request.
const STATUS_REFRESH: Duration = Duration::from_secs(60);

/// Side effects left for the backend after the shared worker-event handler runs.
#[derive(Default)]
struct EventEffects {
    /// Whether cached facts changed and observers should be notified.
    changed: bool,
    /// A current-worker address result that still needs endpoint persistence.
    address: Option<(String, u16)>,
}

/// Resolve a saved core's current connection host and MoonProto port without DNS.
pub(crate) fn core_host_port(server: &ServerConfig) -> Option<(String, u16)> {
    let endpoint = parse_endpoint_override(&server.endpoint_override).ok()?;
    let target = target_from_key(server.key.expose(), endpoint.as_ref())?;
    Some((target.host.to_string(), target.port))
}

impl Backend {
    /// Queue `req` for the command worker associated with core `uid`.
    pub(crate) fn core_cmds_send(&mut self, uid: u64, req: CmdRequest) {
        self.core_cmds.pending.push((uid, req));
    }

    /// Store a confirmed provider login for `ip` for the rest of this terminal session.
    pub(crate) fn core_cmds_add_login(&mut self, ip: String, login: SessionLogin) {
        self.core_cmds.refresh_hosts();
        self.core_cmds.logins.insert(ip.clone(), login);
        let uids: Vec<_> = self
            .config
            .servers
            .iter()
            .filter_map(|server| {
                core_host_port(server)
                    .is_some_and(|(host, _)| host == ip)
                    .then_some(server.uid)
            })
            .collect();
        for uid in uids {
            self.core_cmds.invalidate_worker(uid);
            self.core_cmds.request_status(uid);
        }
    }

    /// Return the cached server facts for core `uid`, if a request has created them.
    pub(crate) fn core_cmds_facts(&self, uid: u64) -> Option<&CoreFacts> {
        self.core_cmds.facts.get(&uid)
    }

    /// Queue a status request unless a successful answer is less than 60 seconds old.
    pub(crate) fn core_cmds_request_status(&mut self, uid: u64) {
        self.core_cmds.request_status(uid);
    }

    /// Invalidate command workers whose saved endpoint changed or disappeared.
    pub(crate) fn core_cmds_reconcile(&mut self, before: &AppConfig) -> bool {
        self.core_cmds.reconcile_workers(before, &self.config)
    }

    /// Prefer a confirmed session login, then fall back to the cached administrator for `ip`.
    pub(crate) fn core_cmds_login_for(&self, ip: &str) -> Option<Login> {
        self.core_cmds_login_from(ip, &self.core_cmds.hosts)
    }

    /// Choose a session or administrator login using one already loaded host snapshot.
    fn core_cmds_login_from(&self, ip: &str, hosts: &Hosts) -> Option<Login> {
        if let Some(login) = self.core_cmds.logins.get(ip) {
            return Some(Login::Provider {
                first: login.first.clone(),
                pin: login.pin.clone(),
            });
        }
        hosts.admin_by_ip(ip).map(|_| Login::Admin)
    }

    /// Start the persistent 250 ms drain for all per-core command workers.
    pub(crate) fn core_cmds_track(&mut self, cx: &mut Context<Self>) {
        self.core_cmds.refresh_hosts();
        let (events, answers) = mpsc::channel::<(u64, u64, worker::Event)>();
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

    /// Dispatch queued requests and apply every current-generation worker event.
    fn core_cmds_tick(
        &mut self,
        events: &mpsc::Sender<(u64, u64, worker::Event)>,
        answers: &mpsc::Receiver<(u64, u64, worker::Event)>,
        cx: &mut Context<Self>,
    ) {
        if !self.core_cmds.pending.is_empty() {
            self.core_cmds.refresh_hosts();
        }
        let mut changed = false;
        for (uid, request) in std::mem::take(&mut self.core_cmds.pending) {
            changed = true;
            let cmd = match &request {
                CmdRequest::Run { cmd, .. } => *cmd,
                CmdRequest::Refresh => Cmd::Status,
                CmdRequest::Forget => {
                    self.core_cmds.forget_key(uid);
                    continue;
                }
            };
            let config = self.core_cmds_worker_config(uid);
            let Some((target, display, port, login)) = config else {
                self.core_cmds.invalidate_worker(uid);
                let (target, port) = self
                    .config
                    .servers
                    .iter()
                    .find(|s| s.uid == uid)
                    .and_then(core_host_port)
                    .unwrap_or_default();
                self.core_cmds.no_login(uid, cmd, target, port);
                continue;
            };
            let endpoint_changed = self.core_cmds.workers.get(&uid).is_some_and(|worker| {
                worker.target.host != target.host
                    || worker.target.port != target.port
                    || worker.port != port
            });
            if endpoint_changed {
                self.core_cmds.invalidate_worker(uid);
            }
            let facts = self
                .core_cmds
                .facts
                .entry(uid)
                .or_insert_with(|| core_facts(display.clone(), port));
            facts.target = display;
            facts.port = port;
            let generation = self
                .core_cmds
                .workers
                .get(&uid)
                .map(|worker| worker.generation)
                .unwrap_or_else(|| {
                    self.core_cmds.next_generation = self.core_cmds.next_generation.wrapping_add(1);
                    self.core_cmds.next_generation
                });
            if !self.core_cmds.workers.contains_key(&uid)
                && let Some(sender) = worker::spawn(
                    uid,
                    generation,
                    target.clone(),
                    login,
                    port,
                    facts.located.clone(),
                    events.clone(),
                )
            {
                self.core_cmds.workers.insert(
                    uid,
                    CoreWorker {
                        target,
                        port,
                        generation,
                        sender,
                    },
                );
            }
            if matches!(
                &request,
                CmdRequest::Run {
                    cmd: Cmd::SwitchExchange,
                    args: CmdArgs::Exchange(_),
                }
            ) {
                facts.begin_exchange_attempt();
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
                .is_some_and(|worker| worker.sender.send(request).is_ok());
            if !sent {
                self.core_cmds.invalidate_worker(uid);
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
        while let Ok((uid, generation, event)) = answers.try_recv() {
            changed |= self.core_cmds_event(uid, generation, event);
        }
        if changed {
            cx.notify();
        }
    }

    /// Resolve the core address and real SSH login endpoint without contacting the server.
    fn core_cmds_worker_config(&self, uid: u64) -> Option<(Target, String, u16, Login)> {
        let server = self
            .config
            .servers
            .iter()
            .find(|server| server.uid == uid)?;
        let (host, core_port) = core_host_port(server)?;
        let login = self.core_cmds_login_from(&host, &self.core_cmds.hosts)?;
        let fallback = format!("{host}:22");
        let (display, ssh_port) = match &login {
            Login::Provider { .. } => {
                let display = self
                    .core_cmds
                    .logins
                    .get(&host)
                    .map(|login| login.target.clone())
                    .filter(|target| !target.is_empty())
                    .unwrap_or(fallback);
                let port = ssh_target_port(&display).unwrap_or(22);
                (display, port)
            }
            Login::Admin => {
                let known = self.core_cmds.hosts.admin_by_ip(&host);
                let display = known
                    .map(|known| {
                        known
                            .admin
                            .as_deref()
                            .map(|admin| format!("{admin}@{}", known.addr))
                            .unwrap_or(fallback)
                    })
                    .unwrap_or_else(|| format!("{host}:22"));
                let port = known
                    .and_then(|known| ssh_target_port(&known.addr))
                    .unwrap_or(22);
                (display, port)
            }
        };
        Some((
            Target {
                host,
                port: ssh_port,
            },
            display,
            core_port,
            login,
        ))
    }

    /// Apply one current-generation worker event and return whether it changed a cached core.
    fn core_cmds_event(&mut self, uid: u64, generation: u64, event: worker::Event) -> bool {
        let endpoint = self
            .config
            .servers
            .iter()
            .find(|server| server.uid == uid)
            .and_then(core_host_port);
        let mut effects = self.core_cmds.apply_event(
            uid,
            generation,
            event,
            endpoint,
            &mut self.reconnect_request,
        );
        if let Some((host, port)) = effects.address.take() {
            let (changed, saved) = self.core_cmds_address_changed(uid, &host, port);
            if saved && let Some(worker) = self.core_cmds.workers.get_mut(&uid) {
                worker.address_changed(port);
            }
            effects.changed |= changed;
        }
        effects.changed
    }

    /// Save the current connect host with the new port through the completed core-move edit.
    fn core_cmds_address_changed(
        &mut self,
        uid: u64,
        _listen_host: &str,
        port: u16,
    ) -> (bool, bool) {
        let connect_host = self
            .config
            .servers
            .iter()
            .find(|server| server.uid == uid)
            .and_then(core_host_port)
            .map(|(host, _)| host)
            .or_else(|| {
                self.core_cmds
                    .facts
                    .get(&uid)
                    .and_then(|facts| ssh_display_host(&facts.target))
            });
        let result = (|| {
            let host = connect_host
                .filter(|host| !host.is_empty())
                .ok_or_else(|| anyhow::anyhow!("the current core address is unavailable"))?;
            self.core_move_switch(uid, &format!("{host}:{port}"))
        })();
        let saved = result.is_ok();
        if let Some(facts) = self.core_cmds.facts.get_mut(&uid) {
            if let Err(error) = result {
                facts.phases.insert(
                    Cmd::MoonProtoAddress,
                    CmdPhase::Failed(CmdError::Ssh(format!("{error:#}"))),
                );
            } else {
                facts.address_changed(port);
            }
        }
        (true, saved)
    }
}

impl CoreCmdsState {
    /// Route one worker event through generation and endpoint admission into facts and side effects.
    fn apply_event(
        &mut self,
        uid: u64,
        generation: u64,
        event: worker::Event,
        endpoint: Option<(String, u16)>,
        reconnect_request: &mut Vec<u64>,
    ) -> EventEffects {
        if !worker_generation_matches(self.workers.get(&uid), generation) {
            return EventEffects::default();
        }
        match event {
            worker::Event::AddressChanged { host, port } => {
                if !self
                    .workers
                    .get(&uid)
                    .is_some_and(|worker| worker_endpoint_matches(worker, endpoint))
                {
                    return EventEffects::default();
                }
                EventEffects {
                    address: Some((host, port)),
                    ..EventEffects::default()
                }
            }
            worker::Event::ExchangeSwitched => {
                let changed = self
                    .facts
                    .get_mut(&uid)
                    .is_some_and(|facts| facts.apply(worker::Event::ExchangeSwitched));
                if changed && !reconnect_request.contains(&uid) {
                    reconnect_request.push(uid);
                }
                EventEffects {
                    changed,
                    ..EventEffects::default()
                }
            }
            event => EventEffects {
                changed: self
                    .facts
                    .get_mut(&uid)
                    .is_some_and(|facts| facts.apply(event)),
                ..EventEffects::default()
            },
        }
    }

    /// Drop a replaced worker and every cached fact that belongs to its previous endpoint.
    fn invalidate_worker(&mut self, uid: u64) -> bool {
        let mut changed = self.workers.remove(&uid).is_some();
        if let Some(facts) = self.facts.get_mut(&uid) {
            facts.invalidate_endpoint();
            changed = true;
        }
        changed
    }

    /// Invalidate workers whose endpoint differs between two saved configuration snapshots.
    fn reconcile_workers(&mut self, before: &AppConfig, after: &AppConfig) -> bool {
        let endpoint = |config: &AppConfig, uid| {
            config
                .servers
                .iter()
                .find(|server| server.uid == uid)
                .and_then(core_host_port)
        };
        let changed: Vec<_> = self
            .workers
            .keys()
            .copied()
            .filter(|uid| endpoint(before, *uid) != endpoint(after, *uid))
            .collect();
        changed
            .into_iter()
            .fold(false, |changed, uid| self.invalidate_worker(uid) | changed)
    }

    /// Refresh the cached host pins once per active backend tick or confirmed login.
    fn refresh_hosts(&mut self) {
        self.hosts = Hosts::load(&Hosts::path()).unwrap_or_default();
    }

    /// Hide only the connection key and cancel its reveal so late results cannot expose it.
    fn forget_key(&mut self, uid: u64) {
        if let Some(facts) = self.facts.get_mut(&uid) {
            facts.key = None;
            facts.phases.insert(Cmd::ConnectionKey, CmdPhase::Idle);
        }
    }

    /// Record a first-request refusal even when no worker configuration could be constructed.
    fn no_login(&mut self, uid: u64, cmd: Cmd, target: String, port: u16) {
        let facts = self
            .facts
            .entry(uid)
            .or_insert_with(|| core_facts(target, port));
        facts.phases.insert(
            cmd,
            CmdPhase::Failed(CmdError::Refused(
                rust_i18n::t!("core_cmds.error.no_login").to_string(),
            )),
        );
    }

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
                if cmd == Cmd::ConnectionKey
                    && matches!(phase, CmdPhase::Running { .. })
                    && !matches!(self.phases.get(&cmd), Some(CmdPhase::Running { .. }))
                {
                    return false;
                }
                if cmd == Cmd::MoonProtoAddress
                    && matches!(phase, CmdPhase::Done)
                    && matches!(self.phases.get(&cmd), Some(CmdPhase::Failed(_)))
                {
                    return false;
                }
                if cmd == Cmd::RetryLogin {
                    match &phase {
                        CmdPhase::Running { .. } => self.retry_elapsed = None,
                        CmdPhase::Done => {
                            self.retry_elapsed = match self.phases.get(&cmd) {
                                Some(CmdPhase::Running { since, .. }) => Some(since.elapsed()),
                                _ => None,
                            };
                        }
                        _ => {}
                    }
                }
                self.phases.insert(cmd, phase);
            }
            worker::Event::Located(located) => {
                self.located = Some(located);
                self.retarget_recovery = false;
            }
            worker::Event::Status(status) => {
                let login_error = status.state == CoreState::LoginError;
                self.status = Some(status);
                self.checked_at = Some(Instant::now());
                self.phases.insert(Cmd::Status, CmdPhase::Done);
                if login_error && matches!(self.phases.get(&Cmd::RetryLogin), Some(CmdPhase::Done))
                {
                    self.phases.insert(Cmd::RetryLogin, CmdPhase::Idle);
                    self.retry_elapsed = None;
                }
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
            worker::Event::Info(mut info) => {
                let ownership_confirmed = info.located.ours;
                if self.retarget_recovery
                    && let Some(recovery) = self.located.clone()
                {
                    worker::preserve_retarget_location(&recovery, &mut info);
                }
                self.located = Some(info.located.clone());
                self.info = Some(info);
                if ownership_confirmed {
                    self.retarget_recovery = false;
                }
            }
            worker::Event::Log(log) => self.log = Some(log),
            worker::Event::Key(key) => {
                if matches!(
                    self.phases.get(&Cmd::ConnectionKey),
                    Some(CmdPhase::Running { .. })
                ) {
                    self.key = Some(key);
                }
            }
            worker::Event::Exchanges(exchanges) => self.exchanges = Some(exchanges),
            worker::Event::ExchangeSwitched => {
                self.exchange_switched = Some(self.exchange_attempt);
            }
            worker::Event::AddressChanged { .. } => return false,
        }
        true
    }

    /// Start a new confirmed exchange selection, retiring earlier switch and key-save results.
    fn begin_exchange_attempt(&mut self) {
        self.exchange_attempt = self.exchange_attempt.wrapping_add(1);
        self.exchange_switched = None;
        self.phases.insert(Cmd::ApiKeys, CmdPhase::Idle);
    }

    /// Advance the saved port while retaining the owned installation needed for retarget recovery.
    fn address_changed(&mut self, port: u16) {
        self.port = port;
        self.status = None;
        self.info = None;
        self.checked_at = None;
        self.retarget_recovery = true;
    }

    /// Clear old-core facts while keeping attempt IDs monotonic for windows that outlive the edit.
    fn invalidate_endpoint(&mut self) {
        self.located = None;
        self.status = None;
        self.info = None;
        self.log = None;
        self.key = None;
        self.exchanges = None;
        self.checked_at = None;
        self.retry_elapsed = None;
        self.retarget_recovery = false;
        self.exchange_switched = None;
        for phase in self.phases.values_mut() {
            *phase = CmdPhase::Idle;
        }
    }
}

/// Accept an event only from the worker generation currently registered for the core.
fn worker_generation_matches(worker: Option<&CoreWorker>, generation: u64) -> bool {
    worker.is_some_and(|worker| worker.generation == generation)
}

/// Check that an address event still targets the saved endpoint its worker was created for.
fn worker_endpoint_matches(worker: &CoreWorker, endpoint: Option<(String, u16)>) -> bool {
    endpoint.is_some_and(|(host, port)| worker.target.host == host && worker.port == port)
}

/// Read the SSH port from a decorated `user@host:port` login target.
fn ssh_target_port(target: &str) -> Option<u16> {
    let address = target
        .rsplit_once('@')
        .map_or(target, |(_, address)| address);
    address
        .rsplit_once(':')?
        .1
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
}

/// Recover the current connect IP from the worker's decorated display target.
fn ssh_display_host(target: &str) -> Option<String> {
    let address = target
        .rsplit_once('@')
        .map_or(target, |(_, address)| address);
    if address.parse::<std::net::IpAddr>().is_ok() {
        return Some(address.to_owned());
    }
    let (host, port) = address.rsplit_once(':')?;
    port.parse::<u16>().ok()?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    (!host.is_empty()).then(|| host.to_owned())
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
        retry_elapsed: None,
        retarget_recovery: false,
        exchange_attempt: 0,
        exchange_switched: None,
    }
}

#[cfg(test)]
mod tests;
