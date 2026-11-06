use std::cell::RefCell;
use std::net::Ipv4Addr;

use moon_remote::core_cmds::{
    Cmd, LocateResult, Located, LogLines, LogTail, ServiceAction, ServiceInfo,
};
use moon_remote::core_new::{CoreState, CoreStatus, Exchange};
use zeroize::Zeroizing;

use super::*;

/// Build a located core with neutral public fixture data.
fn located(ours: bool) -> Located {
    Located {
        dir: "/root/moonbot-a".into(),
        unit: "UNIT_01.service".into(),
        ours,
        has_mb: true,
        active: Some(true),
        port_known: true,
    }
}

/// Build a core status in `state`, ready to accept a terminal connection when requested.
fn status(state: CoreState) -> CoreStatus {
    CoreStatus {
        state,
        exchange: "EX-XX".into(),
        exchange_name: "CORE_A".into(),
        has_api_keys: true,
        login_error: String::new(),
        moonproto_enabled: true,
        moonproto_listening: true,
        moonproto_error: String::new(),
        moonproto_port: 3302,
        emulator: None,
    }
}

/// A runner that records call order and returns deterministic successful fixtures.
struct FakeRunner {
    locate_result: LocateResult,
    wait_status: CoreStatus,
    calls: RefCell<Vec<String>>,
    retarget_error: Option<String>,
    info_available: bool,
}

impl FakeRunner {
    /// Build a fake with a located MoonTerminal-installed core and a connected retry result.
    fn new() -> Self {
        Self {
            locate_result: LocateResult::Found(located(true)),
            wait_status: status(CoreState::Ready),
            calls: RefCell::new(Vec::new()),
            retarget_error: None,
            info_available: true,
        }
    }

    /// Build a fake whose locate operation returns `result`.
    fn with_locate(result: LocateResult) -> Self {
        Self {
            locate_result: result,
            ..Self::new()
        }
    }

    /// Return the cached location used by the current fixture.
    fn located(&self) -> Located {
        match &self.locate_result {
            LocateResult::Found(located) => located.clone(),
            _ => located(true),
        }
    }

    /// Record a call without recording credentials or server output.
    fn record(&self, call: impl Into<String>) {
        self.calls.borrow_mut().push(call.into());
    }
}

impl Runner for FakeRunner {
    fn locate(&self, port: u16) -> anyhow::Result<LocateResult> {
        self.record(format!("locate:{port}"));
        Ok(self.locate_result.clone())
    }

    fn info(&self, port: u16) -> anyhow::Result<Option<ServiceInfo>> {
        self.record(format!("info:{port}"));
        Ok(self.info_available.then(|| ServiceInfo {
            located: self.located(),
            active: "active".into(),
            uptime_s: Some(60),
            memory: Some(1024),
        }))
    }

    fn service(
        &self,
        action: ServiceAction,
        port: u16,
        unit_hint: Option<&str>,
    ) -> anyhow::Result<String> {
        self.record(format!(
            "service:{}:{port}:{}",
            action.as_str(),
            unit_hint.unwrap_or("-")
        ));
        Ok("active".into())
    }

    fn log_tail(&self, port: u16, n: LogLines, dir_hint: Option<&str>) -> anyhow::Result<LogTail> {
        self.record(format!(
            "log_tail:{port}:{}:{}",
            n.count(),
            dir_hint.unwrap_or("-")
        ));
        Ok(LogTail {
            lines: vec!["CORE_A ready".into()],
        })
    }

    fn status(&self, dir: &str) -> anyhow::Result<CoreStatus> {
        self.record(format!("status:{dir}"));
        Ok(status(CoreState::Ready))
    }

    fn retry_login(&self, dir: &str) -> anyhow::Result<()> {
        self.record(format!("retry_login:{dir}"));
        Ok(())
    }

    fn wait_settled(
        &self,
        dir: &str,
        seen: &mut dyn FnMut(CoreStatus),
    ) -> anyhow::Result<CoreStatus> {
        self.record(format!("wait_settled:{dir}"));
        seen(self.wait_status.clone());
        Ok(self.wait_status.clone())
    }

    fn exchanges(&self, dir: &str) -> anyhow::Result<Vec<Exchange>> {
        self.record(format!("exchanges:{dir}"));
        Ok(Vec::new())
    }

    fn select_exchange(&self, dir: &str, name: &str) -> anyhow::Result<()> {
        self.record(format!("select_exchange:{dir}:{name}"));
        Ok(())
    }

    fn set_api_keys(
        &self,
        dir: &str,
        _key: Zeroizing<String>,
        _secret: Zeroizing<String>,
    ) -> anyhow::Result<()> {
        self.record(format!("set_api_keys:{dir}"));
        Ok(())
    }

    fn set_moonproto_address(
        &self,
        dir: &str,
        host: Ipv4Addr,
        port: u16,
    ) -> anyhow::Result<(Zeroizing<String>, Option<String>)> {
        self.record(format!("set_moonproto_address:{dir}:{host}:{port}"));
        Ok((
            Zeroizing::new("TEST_CONNECTION_KEY".into()),
            self.retarget_error.clone(),
        ))
    }

    fn moonproto_key(&self, dir: &str) -> anyhow::Result<Zeroizing<String>> {
        self.record(format!("moonproto_key:{dir}"));
        Ok(Zeroizing::new("TEST_CONNECTION_KEY".into()))
    }
}

/// Build a request snapshot with no cached location.
fn snapshot() -> FactsSnapshot {
    FactsSnapshot {
        port: 3302,
        located: None,
    }
}

/// Build a request snapshot with the fake's current location cached.
fn cached_snapshot(fake: &FakeRunner) -> FactsSnapshot {
    FactsSnapshot {
        port: 3302,
        located: Some(fake.located()),
    }
}

/// Return the terminal phase emitted for `cmd`.
fn phase(events: &[Event], cmd: Cmd) -> &CmdPhase {
    events
        .iter()
        .find_map(|event| match event {
            Event::Phase(found, phase) if *found == cmd => Some(phase),
            _ => None,
        })
        .expect("command must emit a terminal phase")
}

/// Treating an absent locate result as success would run later commands against an invented path.
#[test]
fn locate_none_fails_with_the_requested_port() {
    let fake = FakeRunner::with_locate(LocateResult::None);
    let events = run(
        &fake,
        snapshot(),
        CmdRequest::Run {
            cmd: Cmd::Status,
            args: CmdArgs::None,
        },
    );

    assert!(matches!(
        phase(&events, Cmd::Status),
        CmdPhase::Failed(CmdError::NoCore { port: 3302 })
    ));
    assert_eq!(fake.calls.borrow().as_slice(), ["locate:3302"]);
}

/// Reconfiguration must stop at the ownership gate before any exchange call reaches the server.
#[test]
fn hand_installed_switch_exchange_refuses_without_exchange_calls() {
    let fake = FakeRunner::with_locate(LocateResult::Found(located(false)));
    let events = run(
        &fake,
        snapshot(),
        CmdRequest::Run {
            cmd: Cmd::SwitchExchange,
            args: CmdArgs::None,
        },
    );

    assert!(matches!(
        phase(&events, Cmd::SwitchExchange),
        CmdPhase::Failed(CmdError::NotOurs)
    ));
    assert_eq!(fake.calls.borrow().as_slice(), ["locate:3302"]);
}

/// A connectable retry must retain its progress event before its successful terminal phase.
#[test]
fn retry_emits_settling_then_done_when_connectable() {
    let fake = FakeRunner::new();
    let events = run(
        &fake,
        cached_snapshot(&fake),
        CmdRequest::Run {
            cmd: Cmd::RetryLogin,
            args: CmdArgs::None,
        },
    );

    assert!(matches!(events.first(), Some(Event::Settling(_))));
    assert!(matches!(events.get(1), Some(Event::Status(_))));
    assert!(matches!(
        events.last(),
        Some(Event::Phase(Cmd::RetryLogin, CmdPhase::Done))
    ));
}

/// A user-action state returned by wait_settled is a refusal, not a successful retry or a timeout.
#[test]
fn retry_login_error_fails_refused_with_server_reason() {
    let mut fake = FakeRunner::new();
    fake.wait_status = status(CoreState::LoginError);
    fake.wait_status.login_error = "429 Too Many Requests".into();
    let events = run(
        &fake,
        cached_snapshot(&fake),
        CmdRequest::Run {
            cmd: Cmd::RetryLogin,
            args: CmdArgs::None,
        },
    );

    assert!(matches!(
        phase(&events, Cmd::RetryLogin),
        CmdPhase::Failed(CmdError::Refused(reason)) if reason == "429 Too Many Requests"
    ));
}

/// wait_settled also returns normally on elapsed time, so an unfinished state must be a timeout.
#[test]
fn retry_unfinished_after_wait_fails_timeout() {
    let mut fake = FakeRunner::new();
    fake.wait_status = status(CoreState::Initializing);
    let events = run(
        &fake,
        cached_snapshot(&fake),
        CmdRequest::Run {
            cmd: Cmd::RetryLogin,
            args: CmdArgs::None,
        },
    );

    assert!(matches!(
        phase(&events, Cmd::RetryLogin),
        CmdPhase::Failed(CmdError::Timeout)
    ));
}

/// Losing the cached unit when post-stop locate finds nothing would make Start impossible.
#[test]
fn start_after_stop_passes_cached_unit_hint() {
    let mut fake = FakeRunner::new();
    fake.locate_result = LocateResult::Found(located(false));
    fake.info_available = false;
    let stopped = run(
        &fake,
        cached_snapshot(&fake),
        CmdRequest::Run {
            cmd: Cmd::Stop,
            args: CmdArgs::None,
        },
    );
    assert!(matches!(phase(&stopped, Cmd::Stop), CmdPhase::Done));
    let stopped = stopped
        .iter()
        .find_map(|event| match event {
            Event::Info(info) => Some(info.located.clone()),
            _ => None,
        })
        .expect("stopped location survives missing post-stop info");
    assert_eq!(stopped.active, Some(false));

    fake.info_available = true;
    let started = run(
        &fake,
        FactsSnapshot {
            port: 3302,
            located: Some(stopped),
        },
        CmdRequest::Run {
            cmd: Cmd::Start,
            args: CmdArgs::None,
        },
    );
    assert!(matches!(phase(&started, Cmd::Start), CmdPhase::Done));
    let services: Vec<_> = fake
        .calls
        .borrow()
        .iter()
        .filter(|call| call.starts_with("service:"))
        .cloned()
        .collect();
    assert_eq!(
        services,
        ["service:stop:3302:-", "service:start:3302:UNIT_01.service"]
    );
}

/// Every menu command must cross the Runner boundary instead of becoming a UI-only success.
#[test]
fn every_command_reaches_its_runner_method() {
    let cases = vec![
        (Cmd::Status, CmdArgs::None, "status:"),
        (Cmd::RetryLogin, CmdArgs::None, "retry_login:"),
        (Cmd::Log, CmdArgs::Log(LogLines::L500), "log_tail:"),
        (Cmd::Restart, CmdArgs::None, "service:restart:"),
        (Cmd::Stop, CmdArgs::None, "service:stop:"),
        (Cmd::Start, CmdArgs::None, "service:start:"),
        (
            Cmd::SwitchExchange,
            CmdArgs::Exchange("EX-XX".into()),
            "select_exchange:",
        ),
        (
            Cmd::ApiKeys,
            CmdArgs::ApiKeys {
                key: Zeroizing::new("TEST_API_KEY".into()),
                secret: Zeroizing::new("TEST_API_SECRET".into()),
            },
            "set_api_keys:",
        ),
        (Cmd::ConnectionKey, CmdArgs::None, "moonproto_key:"),
        (
            Cmd::MoonProtoAddress,
            CmdArgs::Address {
                host: "203.0.113.7".into(),
                port: 4402,
            },
            "set_moonproto_address:",
        ),
        (Cmd::UpdateCore, CmdArgs::None, "status:"),
    ];

    for (cmd, args, expected) in cases {
        let fake = FakeRunner::new();
        let events = run(&fake, cached_snapshot(&fake), CmdRequest::Run { cmd, args });
        assert!(
            fake.calls
                .borrow()
                .iter()
                .any(|call| call.starts_with(expected)),
            "{cmd:?} did not call {expected}"
        );
        assert!(matches!(phase(&events, cmd), CmdPhase::Done), "{cmd:?}");
    }
}

/// Opening the exchange picker must fetch choices rather than selecting an empty exchange name.
#[test]
fn switch_exchange_without_selection_lists_exchanges() {
    let fake = FakeRunner::new();
    let events = run(
        &fake,
        cached_snapshot(&fake),
        CmdRequest::Run {
            cmd: Cmd::SwitchExchange,
            args: CmdArgs::None,
        },
    );

    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Exchanges(_)))
    );
    assert!(
        fake.calls
            .borrow()
            .iter()
            .any(|call| call.starts_with("exchanges:"))
    );
    assert!(
        !fake
            .calls
            .borrow()
            .iter()
            .any(|call| call.starts_with("select_exchange:"))
    );
}

/// A retarget refusal follows a successful core move, so saving the new endpoint cannot be skipped.
#[test]
fn retarget_failure_saves_address_and_refuses_without_ssh_failure() {
    let mut fake = FakeRunner::new();
    fake.retarget_error = Some("retarget refused".into());
    let events = run(
        &fake,
        cached_snapshot(&fake),
        CmdRequest::Run {
            cmd: Cmd::MoonProtoAddress,
            args: CmdArgs::Address {
                host: "203.0.113.7".into(),
                port: 4402,
            },
        },
    );
    let address = events
        .iter()
        .position(|event| {
            matches!(event,
                Event::AddressChanged { host, port: 4402 } if host == "203.0.113.7"
            )
        })
        .expect("the core already moved");
    let failure = events
        .iter()
        .position(|event| {
            matches!(event,
                Event::Phase(Cmd::MoonProtoAddress, CmdPhase::Failed(CmdError::Refused(reason)))
                    if reason == "retarget refused"
            )
        })
        .expect("retarget failure remains a core refusal");
    assert!(address < failure);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Phase(_, CmdPhase::Failed(CmdError::Ssh(_)))))
    );
}

/// Retaining a failed location skips discovery after a core is removed or the SSH session dies.
#[test]
fn no_core_and_ssh_failures_invalidate_location_before_next_request() {
    for error in [
        CmdError::NoCore { port: 3302 },
        CmdError::Ssh("closed".into()),
    ] {
        let fake = FakeRunner::new();
        let mut location = Some(located(true));
        update_location(
            &mut location,
            &Event::Phase(Cmd::Status, CmdPhase::Failed(error)),
        );
        let events = run(
            &fake,
            FactsSnapshot {
                port: 3302,
                located: location,
            },
            CmdRequest::Refresh,
        );
        assert_eq!(
            fake.calls.borrow().first().map(String::as_str),
            Some("locate:3302")
        );
        assert!(matches!(phase(&events, Cmd::Status), CmdPhase::Done));
    }
}

/// Service information carries a new active state and unit which the following Start must use.
#[test]
fn info_refreshes_cached_active_state_and_unit() {
    let fake = FakeRunner::new();
    let mut found = located(true);
    found.active = Some(false);
    found.unit = "UNIT_02.service".into();
    let mut location = Some(located(true));
    update_location(
        &mut location,
        &Event::Info(ServiceInfo {
            located: found.clone(),
            active: "inactive".into(),
            uptime_s: None,
            memory: None,
        }),
    );
    assert_eq!(location.as_ref().unwrap().active, Some(false));
    run(
        &fake,
        FactsSnapshot {
            port: 3302,
            located: location,
        },
        CmdRequest::Run {
            cmd: Cmd::Start,
            args: CmdArgs::None,
        },
    );
    assert!(
        fake.calls
            .borrow()
            .iter()
            .any(|call| call == "service:start:3302:UNIT_02.service")
    );
}

/// Selecting an exchange without refreshing would leave current status and key captions stale.
#[test]
fn selected_exchange_refreshes_status_and_choices_before_done() {
    let fake = FakeRunner::new();
    let events = run(
        &fake,
        cached_snapshot(&fake),
        CmdRequest::Run {
            cmd: Cmd::SwitchExchange,
            args: CmdArgs::Exchange("EX-XX".into()),
        },
    );

    assert_eq!(
        fake.calls.borrow().as_slice(),
        [
            "select_exchange:/root/moonbot-a:EX-XX",
            "status:/root/moonbot-a",
            "exchanges:/root/moonbot-a",
        ]
    );
    assert!(events.iter().any(|event| matches!(event, Event::Status(_))));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Exchanges(_)))
    );
    assert!(matches!(
        phase(&events, Cmd::SwitchExchange),
        CmdPhase::Done
    ));
}

/// A service refusal is not a transport failure and must not tear down the SSH session.
#[test]
fn service_failure_is_a_refusal_not_an_ssh_failure() {
    let error = ssh_failure(ServiceFailed { tail: Vec::new() }.into());
    assert!(matches!(error, CmdError::Refused(_)));
}
