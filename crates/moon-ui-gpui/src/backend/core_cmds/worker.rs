//! One blocking SSH worker per core, with a pure event-collecting command runner for tests.

use std::net::Ipv4Addr;
use std::sync::mpsc;
use std::time::Instant;

use moon_remote::core_cmds::{
    self, Cmd, LocateResult, Located, LogLines, LogTail, Login, Refusal, ServiceAction, ServiceInfo,
};
use moon_remote::core_new::{self, ApiKeys, CoreStatus, Exchange};
use moon_remote::script::Privilege;
use moon_remote::ssh::{Conn, Target};
use zeroize::Zeroizing;

use super::{CmdArgs, CmdError, CmdPhase, CmdRequest};

#[cfg(test)]
mod tests;

/// A fact or lifecycle transition produced by a core command.
pub(crate) enum Event {
    Phase(Cmd, CmdPhase),
    Located(Located),
    Status(CoreStatus),
    Settling(CoreStatus),
    Info(ServiceInfo),
    Log(LogTail),
    Key(Zeroizing<String>),
    Exchanges(Vec<Exchange>),
    AddressChanged { host: String, port: u16 },
}

/// The server calls needed by the command runner, isolated from SSH for deterministic tests.
pub(crate) trait Runner {
    /// Resolve the core that owns `port`.
    fn locate(&self, port: u16) -> anyhow::Result<LocateResult>;
    /// Read service state and resource use for `port`.
    fn info(&self, port: u16) -> anyhow::Result<Option<ServiceInfo>>;
    /// Change a core's service state, with an optional unit hint for start.
    fn service(
        &self,
        action: ServiceAction,
        port: u16,
        unit_hint: Option<&str>,
    ) -> anyhow::Result<String>;
    /// Read a bounded core log tail.
    fn log_tail(&self, port: u16, n: LogLines) -> anyhow::Result<LogTail>;
    /// Read the core's exchange and MoonProto state.
    fn status(&self, dir: &str) -> anyhow::Result<CoreStatus>;
    /// Ask the core to retry its exchange login.
    fn retry_login(&self, dir: &str) -> anyhow::Result<()>;
    /// Wait for a retry to connect, require user action, or exhaust its deadline.
    fn wait_settled(
        &self,
        dir: &str,
        seen: &mut dyn FnMut(CoreStatus),
    ) -> anyhow::Result<CoreStatus>;
    /// List exchanges offered by the core.
    fn exchanges(&self, dir: &str) -> anyhow::Result<Vec<Exchange>>;
    /// Select an exchange by its core identifier.
    fn select_exchange(&self, dir: &str, name: &str) -> anyhow::Result<()>;
    /// Send an exchange key pair as JSON over stdin.
    fn set_api_keys(
        &self,
        dir: &str,
        key: Zeroizing<String>,
        secret: Zeroizing<String>,
    ) -> anyhow::Result<()>;
    /// Configure MoonProto and return its key plus any retarget failure.
    fn set_moonproto_address(
        &self,
        dir: &str,
        host: Ipv4Addr,
        port: u16,
    ) -> anyhow::Result<(Zeroizing<String>, Option<String>)>;
    /// Read the existing MoonProto connection key.
    fn moonproto_key(&self, dir: &str) -> anyhow::Result<Zeroizing<String>>;
}

/// The cached location facts needed to execute one request.
pub(super) struct FactsSnapshot {
    port: u16,
    located: Option<Located>,
}

/// A live SSH connection and its detected privilege path.
struct SshRunner {
    conn: Conn,
    privilege: Privilege,
}

impl SshRunner {
    /// Open one authenticated SSH session for a worker.
    fn open(target: &Target, login: &Login) -> anyhow::Result<Self> {
        let (conn, privilege) = core_cmds::open(target, login)?;
        Ok(Self { conn, privilege })
    }
}

impl Runner for SshRunner {
    fn locate(&self, port: u16) -> anyhow::Result<LocateResult> {
        core_cmds::locate(&self.conn, &self.privilege, port)
    }

    fn info(&self, port: u16) -> anyhow::Result<Option<ServiceInfo>> {
        core_cmds::info(&self.conn, &self.privilege, port)
    }

    fn service(
        &self,
        action: ServiceAction,
        port: u16,
        unit_hint: Option<&str>,
    ) -> anyhow::Result<String> {
        core_cmds::service(&self.conn, &self.privilege, action, port, unit_hint)
    }

    fn log_tail(&self, port: u16, n: LogLines) -> anyhow::Result<LogTail> {
        core_cmds::log_tail(&self.conn, &self.privilege, port, n)
    }

    fn status(&self, dir: &str) -> anyhow::Result<CoreStatus> {
        core_cmds::status(&self.conn, &self.privilege, dir)
    }

    fn retry_login(&self, dir: &str) -> anyhow::Result<()> {
        core_cmds::retry_login(&self.conn, &self.privilege, dir)
    }

    fn wait_settled(
        &self,
        dir: &str,
        seen: &mut dyn FnMut(CoreStatus),
    ) -> anyhow::Result<CoreStatus> {
        core_cmds::wait_settled(
            &self.conn,
            &self.privilege,
            dir,
            core_new::SETTLE_TIMEOUT,
            &mut |status| seen(status.clone()),
        )
    }

    fn exchanges(&self, dir: &str) -> anyhow::Result<Vec<Exchange>> {
        core_cmds::exchanges(&self.conn, &self.privilege, dir)
    }

    fn select_exchange(&self, dir: &str, name: &str) -> anyhow::Result<()> {
        core_cmds::select_exchange(&self.conn, &self.privilege, dir, name)
    }

    fn set_api_keys(
        &self,
        dir: &str,
        key: Zeroizing<String>,
        secret: Zeroizing<String>,
    ) -> anyhow::Result<()> {
        core_cmds::set_api_keys(
            &self.conn,
            &self.privilege,
            dir,
            &ApiKeys::Pair {
                api_key: key,
                api_secret: secret,
            },
        )
    }

    fn set_moonproto_address(
        &self,
        dir: &str,
        host: Ipv4Addr,
        port: u16,
    ) -> anyhow::Result<(Zeroizing<String>, Option<String>)> {
        core_cmds::set_moonproto_address(&self.conn, &self.privilege, dir, host, port)
            .map(|changed| (changed.key, changed.retarget.err()))
    }

    fn moonproto_key(&self, dir: &str) -> anyhow::Result<Zeroizing<String>> {
        core_cmds::moonproto_key(&self.conn, &self.privilege, dir)
    }
}

/// Start a named worker thread and return its request channel.
pub(super) fn spawn(
    uid: u64,
    target: Target,
    login: Login,
    port: u16,
    located: Option<Located>,
    events: mpsc::Sender<(u64, Event)>,
) -> Option<mpsc::Sender<CmdRequest>> {
    let (commands, answers) = mpsc::channel();
    std::thread::Builder::new()
        .name(format!("core-cmds-{uid}"))
        .spawn(move || serve(uid, target, login, port, located, answers, events))
        .ok()?;
    Some(commands)
}

/// Serve requests until the backend forgets the core or drops its channel.
fn serve(
    uid: u64,
    target: Target,
    login: Login,
    mut port: u16,
    mut located: Option<Located>,
    commands: mpsc::Receiver<CmdRequest>,
    events: mpsc::Sender<(u64, Event)>,
) {
    let mut runner = None;
    while let Ok(request) = commands.recv() {
        let cmd = match &request {
            CmdRequest::Run { cmd, .. } => *cmd,
            CmdRequest::Refresh => Cmd::Status,
            CmdRequest::Forget => break,
        };
        let send = |event: Event| {
            let _ = events.send((uid, event));
        };
        send(Event::Phase(
            cmd,
            CmdPhase::Running {
                since: Instant::now(),
                seen: None,
            },
        ));
        if runner.is_none() {
            match SshRunner::open(&target, &login) {
                Ok(opened) => runner = Some(opened),
                Err(error) => {
                    send(Event::Phase(cmd, CmdPhase::Failed(ssh_failure(error))));
                    log::info!("core-cmds {uid} {cmd:?}: failed");
                    continue;
                }
            }
        }
        let mut reconnect = false;
        let ok = run_with(
            runner.as_ref().expect("runner opened"),
            FactsSnapshot {
                port,
                located: located.clone(),
            },
            request,
            &mut |event| {
                match &event {
                    Event::Located(found) => located = Some(found.clone()),
                    Event::Info(info) => located = Some(info.located.clone()),
                    Event::AddressChanged { port: changed, .. } => port = *changed,
                    Event::Phase(_, CmdPhase::Failed(CmdError::Ssh(_))) => reconnect = true,
                    _ => {}
                }
                send(event);
            },
        );
        if reconnect {
            runner = None;
        }
        log::info!(
            "core-cmds {uid} {cmd:?}: {}",
            if ok { "ok" } else { "failed" }
        );
    }
}

/// Execute one request against a fake or real runner and collect its events.
#[cfg(test)]
fn run(runner: &dyn Runner, facts_snapshot: FactsSnapshot, request: CmdRequest) -> Vec<Event> {
    let mut events = Vec::new();
    run_with(runner, facts_snapshot, request, &mut |event| {
        events.push(event);
    });
    events
}

/// Execute one request, publishing facts and progress as they arrive.
fn run_with(
    runner: &dyn Runner,
    facts_snapshot: FactsSnapshot,
    request: CmdRequest,
    emit: &mut dyn FnMut(Event),
) -> bool {
    let (cmd, args) = match request {
        CmdRequest::Run { cmd, args } => (cmd, args),
        CmdRequest::Refresh => (Cmd::Status, CmdArgs::None),
        CmdRequest::Forget => return true,
    };
    let port = facts_snapshot.port;
    let located = match facts_snapshot.located {
        Some(located) => located,
        None => match runner.locate(port) {
            Ok(LocateResult::Found(located)) => {
                emit(Event::Located(located.clone()));
                located
            }
            Ok(LocateResult::None | LocateResult::Candidates(_)) => {
                return fail(cmd, CmdError::NoCore { port }, emit);
            }
            Err(error) => return fail(cmd, ssh_failure(error), emit),
        },
    };
    if let Err(refusal) = core_cmds::gate(&located, cmd) {
        let error = match refusal {
            Refusal::NotOurs => CmdError::NotOurs,
            Refusal::NoMb => CmdError::Refused("the core has no command wrapper".into()),
        };
        return fail(cmd, error, emit);
    }

    match cmd {
        Cmd::Status => match runner.status(&located.dir) {
            Ok(status) => {
                emit(Event::Status(status));
                info_after(runner, cmd, port, emit)
            }
            Err(error) => fail(cmd, ssh_failure(error), emit),
        },
        Cmd::RetryLogin => {
            if let Err(error) = runner.retry_login(&located.dir) {
                return fail(cmd, ssh_failure(error), emit);
            }
            match runner.wait_settled(&located.dir, &mut |status| {
                emit(Event::Settling(status));
            }) {
                Ok(status) => {
                    emit(Event::Status(status.clone()));
                    if status.connectable() {
                        done(cmd, emit)
                    } else if status.needs_user() {
                        let reason = if status.login_error.is_empty() {
                            format!("{:?}", status.state)
                        } else {
                            status.login_error
                        };
                        fail(cmd, CmdError::Refused(reason), emit)
                    } else {
                        fail(cmd, CmdError::Timeout, emit)
                    }
                }
                Err(error) => fail(cmd, ssh_failure(error), emit),
            }
        }
        Cmd::Log => {
            let lines = match args {
                CmdArgs::Log(lines) => lines,
                _ => LogLines::L200,
            };
            match runner.log_tail(port, lines) {
                Ok(log) => {
                    emit(Event::Log(log));
                    done(cmd, emit)
                }
                Err(error) => fail(cmd, ssh_failure(error), emit),
            }
        }
        Cmd::Restart | Cmd::Stop | Cmd::Start => {
            let action = match cmd {
                Cmd::Restart => ServiceAction::Restart,
                Cmd::Stop => ServiceAction::Stop,
                Cmd::Start => ServiceAction::Start,
                _ => unreachable!(),
            };
            let hint = (cmd == Cmd::Start).then_some(located.unit.as_str());
            match runner.service(action, port, hint) {
                Ok(_) => info_after(runner, cmd, port, emit),
                Err(error) => fail(cmd, ssh_failure(error), emit),
            }
        }
        Cmd::SwitchExchange => match args {
            CmdArgs::None => match runner.exchanges(&located.dir) {
                Ok(exchanges) => {
                    emit(Event::Exchanges(exchanges));
                    done(cmd, emit)
                }
                Err(error) => fail(cmd, ssh_failure(error), emit),
            },
            CmdArgs::Exchange(name) => match runner.select_exchange(&located.dir, &name) {
                Ok(()) => done(cmd, emit),
                Err(error) => fail(cmd, ssh_failure(error), emit),
            },
            _ => invalid_args(cmd, emit),
        },
        Cmd::ApiKeys => match args {
            CmdArgs::ApiKeys { key, secret } => {
                match runner.set_api_keys(&located.dir, key, secret) {
                    Ok(()) => done(cmd, emit),
                    Err(error) => fail(cmd, ssh_failure(error), emit),
                }
            }
            _ => invalid_args(cmd, emit),
        },
        Cmd::ConnectionKey => match runner.moonproto_key(&located.dir) {
            Ok(key) => {
                emit(Event::Key(key));
                done(cmd, emit)
            }
            Err(error) => fail(cmd, ssh_failure(error), emit),
        },
        Cmd::MoonProtoAddress => match args {
            CmdArgs::Address { host, port } => match host.parse::<Ipv4Addr>() {
                Ok(host) => match runner.set_moonproto_address(&located.dir, host, port) {
                    Ok((key, retarget)) => {
                        emit(Event::Key(key));
                        if let Some(error) = retarget {
                            fail(cmd, CmdError::Ssh(error), emit)
                        } else {
                            done(cmd, emit);
                            emit(Event::AddressChanged {
                                host: host.to_string(),
                                port,
                            });
                            true
                        }
                    }
                    Err(error) => fail(cmd, ssh_failure(error), emit),
                },
                Err(_) => invalid_args(cmd, emit),
            },
            _ => invalid_args(cmd, emit),
        },
        Cmd::UpdateCore => match runner.status(&located.dir) {
            Ok(status) => {
                emit(Event::Status(status));
                done(cmd, emit)
            }
            Err(error) => fail(cmd, ssh_failure(error), emit),
        },
    }
}

/// Publish refreshed service information and finish a service or status command.
fn info_after(runner: &dyn Runner, cmd: Cmd, port: u16, emit: &mut dyn FnMut(Event)) -> bool {
    match runner.info(port) {
        Ok(Some(info)) => {
            emit(Event::Info(info));
            done(cmd, emit)
        }
        Ok(None) => fail(cmd, CmdError::NoCore { port }, emit),
        Err(error) => fail(cmd, ssh_failure(error), emit),
    }
}

/// Publish a successful command result.
fn done(cmd: Cmd, emit: &mut dyn FnMut(Event)) -> bool {
    emit(Event::Phase(cmd, CmdPhase::Done));
    true
}

/// Publish a typed command failure.
fn fail(cmd: Cmd, error: CmdError, emit: &mut dyn FnMut(Event)) -> bool {
    emit(Event::Phase(cmd, CmdPhase::Failed(error)));
    false
}

/// Refuse a malformed request without making a server call for the command.
fn invalid_args(cmd: Cmd, emit: &mut dyn FnMut(Event)) -> bool {
    fail(
        cmd,
        CmdError::Refused("the command arguments are not valid".into()),
        emit,
    )
}

/// Convert an SSH-layer error to the window's non-secret failure form.
fn ssh_failure(error: anyhow::Error) -> CmdError {
    CmdError::Ssh(format!("{error:#}"))
}
