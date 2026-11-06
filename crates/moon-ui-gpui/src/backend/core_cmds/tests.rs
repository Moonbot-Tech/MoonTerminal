use std::time::{Duration, Instant};

use moon_remote::core_cmds::{Cmd, Located};
use moon_remote::core_new::{CoreState, CoreStatus};
use zeroize::Zeroizing;

use super::{
    CmdArgs, CmdError, CmdPhase, CoreCmdsState, CoreWorker, MenuForm, core_facts, menu_form,
    ssh_display_host, ssh_target_port, worker,
};

/// Build a located core with only its installation ownership varied.
fn located(ours: bool) -> Located {
    Located {
        dir: "/root/moonbot-a".into(),
        unit: "UNIT_01.service".into(),
        ours,
        has_mb: true,
        active: None,
        port_known: true,
    }
}

/// Build a status whose exchange login failed with the server's own reason.
fn login_error() -> CoreStatus {
    CoreStatus {
        state: CoreState::LoginError,
        exchange: "EX-XX".into(),
        exchange_name: "CORE_A".into(),
        has_api_keys: true,
        login_error: "429 Too Many Requests".into(),
        moonproto_enabled: true,
        moonproto_listening: true,
        moonproto_error: String::new(),
        moonproto_port: 3302,
        emulator: None,
    }
}

/// Reordering these branches could offer reconfiguration before identity or login is known.
#[test]
fn menu_form_covers_login_checking_ours_and_hand() {
    assert_eq!(menu_form(false, None), MenuForm::NoLogin);
    assert_eq!(menu_form(false, Some(&located(true))), MenuForm::NoLogin);
    assert_eq!(menu_form(true, None), MenuForm::Checking);
    assert_eq!(menu_form(true, Some(&located(true))), MenuForm::Ours);
    assert_eq!(menu_form(true, Some(&located(false))), MenuForm::Hand);
}

/// A status event must feed both the Connections card and the Status page's terminal phase.
#[test]
fn status_event_caches_login_error_and_finishes_status() {
    let mut state = CoreCmdsState::default();
    state
        .facts
        .insert(7, core_facts("root@203.0.113.7:22".into(), 3302));

    state
        .facts
        .get_mut(&7)
        .unwrap()
        .apply(worker::Event::Status(login_error()));

    let facts = state.facts.get(&7).unwrap();
    assert_eq!(facts.status.as_ref(), Some(&login_error()));
    assert!(matches!(
        facts.phases.get(&Cmd::Status),
        Some(CmdPhase::Done)
    ));
    assert!(facts.checked_at.is_some());
}

/// Ignoring the freshness timestamp would turn every card selection into another SSH login.
#[test]
fn request_status_inside_sixty_seconds_queues_nothing() {
    let mut state = CoreCmdsState::default();
    let mut facts = core_facts("203.0.113.7:22".into(), 3302);
    facts.checked_at = Some(Instant::now() - Duration::from_secs(61));
    state.facts.insert(7, facts);

    state.request_status(7);
    state.request_status(7);

    assert_eq!(state.pending.len(), 1);
    assert!(matches!(
        state.pending.first(),
        Some((
            7,
            super::CmdRequest::Run {
                cmd: Cmd::Status,
                args: CmdArgs::None,
            }
        ))
    ));
}

/// Deriving or forwarding Debug must never expose a fetched MoonProto connection key.
#[test]
fn facts_debug_hides_connection_key() {
    let mut facts = core_facts("203.0.113.7:22".into(), 3302);
    facts.phases.insert(
        Cmd::ConnectionKey,
        CmdPhase::Running {
            since: Instant::now(),
            seen: None,
        },
    );
    facts.apply(worker::Event::Key(Zeroizing::new(
        "TEST_CONNECTION_KEY".into(),
    )));

    let debug = format!("{facts:?}");
    assert!(!debug.contains("TEST_CONNECTION_KEY"));
    assert!(debug.contains("<hidden>"));
}

/// Hiding a key must retain the worker and cached status and reject a reply already in flight.
#[test]
fn forget_erases_only_key_and_drops_late_key() {
    let mut state = CoreCmdsState::default();
    let mut facts = core_facts("203.0.113.7:22".into(), 3302);
    facts.apply(worker::Event::Status(login_error()));
    facts.phases.insert(
        Cmd::ConnectionKey,
        CmdPhase::Running {
            since: Instant::now(),
            seen: None,
        },
    );
    facts.apply(worker::Event::Key(Zeroizing::new(
        "TEST_CONNECTION_KEY".into(),
    )));
    assert!(facts.key.is_some());
    state.facts.insert(7, facts);
    let (sender, _receiver) = std::sync::mpsc::channel();
    state.workers.insert(
        7,
        CoreWorker {
            target: moon_remote::ssh::Target {
                host: "203.0.113.7".into(),
                port: 22,
            },
            port: 3302,
            generation: 1,
            sender,
        },
    );

    state.forget_key(7);
    let facts = state.facts.get_mut(&7).expect("facts survive Forget");
    assert_eq!(facts.status.as_ref(), Some(&login_error()));
    assert!(facts.key.is_none());
    facts.apply(worker::Event::Phase(
        Cmd::ConnectionKey,
        CmdPhase::Running {
            since: Instant::now(),
            seen: None,
        },
    ));
    facts.apply(worker::Event::Key(Zeroizing::new("LATE_TEST_KEY".into())));
    assert!(facts.key.is_none());
    assert!(state.workers.contains_key(&7));
}

/// A first request without credentials must leave an actionable failure, not an endless loader.
#[test]
fn first_request_without_login_records_refused_phase() {
    let _locale = crate::test_locale::force("en");
    let mut state = CoreCmdsState::default();
    state.no_login(7, Cmd::Status, "203.0.113.7".into(), 3302);
    let facts = state.facts.get(&7).expect("first request creates facts");
    assert_eq!(facts.target, "203.0.113.7");
    assert!(matches!(
        facts.phases.get(&Cmd::Status),
        Some(CmdPhase::Failed(CmdError::Refused(_)))
    ));
}

/// Hardcoding SSH port 22 would make pinned and session logins unreachable on their real port.
#[test]
fn ssh_targets_keep_their_login_port_and_connect_host() {
    for target in [
        "root@203.0.113.7:2222",
        "203.0.113.7:2222",
        "root@[2001:db8::7]:2222",
    ] {
        assert_eq!(ssh_target_port(target), Some(2222));
    }
    assert_eq!(ssh_target_port("root@203.0.113.7"), None);
    assert_eq!(
        ssh_display_host("root@203.0.113.7:2222").as_deref(),
        Some("203.0.113.7")
    );
}

/// A worker's later Done must not erase a local endpoint-save failure shown to the user.
#[test]
fn address_save_failure_survives_late_worker_done() {
    let mut facts = core_facts("root@203.0.113.7:22".into(), 3302);
    facts.phases.insert(
        Cmd::MoonProtoAddress,
        CmdPhase::Failed(CmdError::Ssh("save failed".into())),
    );

    assert!(!facts.apply(worker::Event::Phase(Cmd::MoonProtoAddress, CmdPhase::Done,)));
    assert!(matches!(
        facts.phases.get(&Cmd::MoonProtoAddress),
        Some(CmdPhase::Failed(CmdError::Ssh(reason))) if reason == "save failed"
    ));
}
