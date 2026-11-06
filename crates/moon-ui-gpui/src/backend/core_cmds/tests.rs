use std::time::{Duration, Instant};

use moon_remote::core_cmds::{Cmd, Located};
use moon_remote::core_new::{CoreState, CoreStatus};
use zeroize::Zeroizing;

use super::{CmdArgs, CmdPhase, CoreCmdsState, MenuForm, core_facts, menu_form, worker};

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
    facts.apply(worker::Event::Key(Zeroizing::new(
        "TEST_CONNECTION_KEY".into(),
    )));

    let debug = format!("{facts:?}");
    assert!(!debug.contains("TEST_CONNECTION_KEY"));
    assert!(debug.contains("<hidden>"));
}
