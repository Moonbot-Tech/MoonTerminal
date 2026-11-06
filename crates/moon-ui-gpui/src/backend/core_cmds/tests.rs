use std::time::{Duration, Instant};

use moon_remote::core_cmds::{Cmd, Located};
use moon_remote::core_new::{CoreState, CoreStatus};
use zeroize::Zeroizing;

use super::{
    CmdArgs, CmdError, CmdPhase, CoreCmdsState, CoreWorker, MenuForm, core_facts, menu_form,
    ssh_display_host, ssh_target_port, worker, worker_endpoint_matches, worker_generation_matches,
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
    state.pending.clear();
    state.facts.get_mut(&7).unwrap().checked_at = Some(Instant::now());
    state.request_status(7);
    assert!(
        state.pending.is_empty(),
        "a fresh answer with no pending request must suppress another SSH status check"
    );
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

/// Build a worker pinned to one SSH host and MoonProto port.
fn command_worker(host: &str, port: u16) -> CoreWorker {
    let (sender, _receiver) = std::sync::mpsc::channel();
    CoreWorker {
        target: moon_remote::ssh::Target {
            host: host.into(),
            port: 22,
        },
        port,
        generation: 1,
        sender,
    }
}

/// Accepting an old worker's address event after an endpoint edit would overwrite the newer edit.
#[test]
fn stale_worker_address_event_requires_the_original_endpoint() {
    let worker = command_worker("203.0.113.7", 3302);
    assert!(worker_endpoint_matches(
        &worker,
        Some(("203.0.113.7".into(), 3302))
    ));
    assert!(!worker_endpoint_matches(
        &worker,
        Some(("203.0.113.8".into(), 3302))
    ));
    assert!(!worker_endpoint_matches(
        &worker,
        Some(("203.0.113.7".into(), 5502))
    ));
    assert!(!worker_endpoint_matches(&worker, None));
}

/// A replaced worker must not persist an address event even when its endpoint still matches.
#[test]
fn stale_generation_event_is_rejected_before_endpoint_handling() {
    let worker = command_worker("203.0.113.7", 3302);
    assert!(worker_generation_matches(Some(&worker), 1));
    assert!(!worker_generation_matches(Some(&worker), 2));
    assert!(!worker_generation_matches(None, 1));
}

/// A successful address change must let the same worker accept a later change on the new port.
#[test]
fn accepted_address_change_advances_worker_endpoint() {
    let mut worker = command_worker("203.0.113.7", 3302);
    worker.address_changed(4402);
    assert!(worker_endpoint_matches(
        &worker,
        Some(("203.0.113.7".into(), 4402))
    ));
}

/// Replacing a worker must not strand an unrelated RetryLogin phase forever.
#[test]
fn worker_invalidation_releases_running_phases() {
    let mut state = CoreCmdsState::default();
    let mut facts = core_facts("203.0.113.7:22".into(), 3302);
    facts.phases.insert(
        Cmd::RetryLogin,
        CmdPhase::Running {
            since: Instant::now(),
            seen: None,
        },
    );
    facts.phases.insert(Cmd::Start, CmdPhase::Done);
    state.facts.insert(7, facts);
    state.workers.insert(7, command_worker("203.0.113.7", 3302));

    state.invalidate_worker(7);

    assert!(!state.workers.contains_key(&7));
    let facts = state.facts.get(&7).unwrap();
    assert!(facts.located.is_none());
    assert!(matches!(
        facts.phases.get(&Cmd::RetryLogin),
        Some(CmdPhase::Idle)
    ));
    assert!(matches!(
        facts.phases.get(&Cmd::Start),
        Some(CmdPhase::Done)
    ));
}

/// Saving a different endpoint must invalidate the worker before any later command is queued.
#[test]
fn settings_reconciliation_invalidates_a_changed_endpoint_worker() {
    fn config(endpoint: &str) -> moon_core::config::AppConfig {
        let mut server: moon_core::config::ServerConfig =
            serde_json::from_value(serde_json::json!({"id": 7, "uid": 7})).unwrap();
        server.key = moon_core::config::Secret::new(
            "sX85BQAAAAD4HMdln7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA==",
        );
        server.endpoint_override = endpoint.into();
        moon_core::config::AppConfig::headless(vec![server])
    }
    let before = config("203.0.113.7:3302");
    let after = config("203.0.113.8:5502");
    let mut state = CoreCmdsState::default();
    state.workers.insert(7, command_worker("203.0.113.7", 3302));
    state
        .facts
        .insert(7, core_facts("203.0.113.7:22".into(), 3302));

    state.reconcile_workers(&before, &after);

    assert!(!state.workers.contains_key(&7));
    let unchanged = state.reconcile_workers(&after, &after);
    assert!(!unchanged);
}

/// A partial retarget failure must leave the owned directory available to Retry.
#[test]
fn address_change_preserves_location_for_retarget_retry() {
    let mut facts = core_facts("203.0.113.7:22".into(), 3302);
    facts.located = Some(located(true));

    facts.address_changed(4402);

    assert_eq!(facts.port, 4402);
    assert_eq!(facts.located.as_ref(), Some(&located(true)));
    assert!(facts.checked_at.is_none());
}

/// Retry completion must retain its own duration after the window is recreated.
#[test]
fn retry_done_keeps_the_attempt_duration() {
    let mut facts = core_facts("203.0.113.7:22".into(), 3302);
    facts.phases.insert(
        Cmd::RetryLogin,
        CmdPhase::Running {
            since: Instant::now() - Duration::from_secs(42),
            seen: None,
        },
    );

    facts.apply(worker::Event::Phase(Cmd::RetryLogin, CmdPhase::Done));

    let elapsed = facts.retry_elapsed.expect("completed retry duration");
    assert!(elapsed >= Duration::from_secs(42));
    assert!(elapsed < Duration::from_secs(43));
}

/// Only a confirmed hot switch, not an exchange-list read, requests a feed identity refresh.
#[test]
fn exchange_switch_marks_the_completed_identity_change() {
    let mut facts = core_facts("203.0.113.7:22".into(), 3302);
    assert_eq!(facts.exchange_switched, None);

    facts.begin_exchange_attempt();
    facts.apply(worker::Event::ExchangeSwitched);
    assert_eq!(facts.exchange_switched, Some(1));

    facts.begin_exchange_attempt();
    assert_eq!(facts.exchange_switched, None);
    facts.apply(worker::Event::ExchangeSwitched);
    assert_eq!(facts.exchange_switched, Some(2));
}
