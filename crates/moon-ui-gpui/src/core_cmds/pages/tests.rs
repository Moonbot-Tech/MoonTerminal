//! Page-state oracles protect the actions offered after server outcomes and partial input.

use super::{Action, Block, Confirm, Kind, ViewState, address_port, build, valid_address};
use crate::backend::core_cmds::{CmdError, CmdPhase, CoreFacts, MenuForm};
use moon_remote::core_cmds::{Cmd, Located};
use moon_remote::core_new::{CoreState, CoreStatus};
use std::time::Instant;

/// Minimal neutral command facts, without a network, session, or production state.
pub(in crate::core_cmds) fn facts() -> CoreFacts {
    CoreFacts {
        target: "root@203.0.113.7".into(),
        port: 3302,
        located: Some(Located {
            dir: "/root/moonbot-a".into(),
            unit: "moonbot@a.service".into(),
            ours: true,
            has_mb: true,
            active: Some(true),
            port_known: true,
        }),
        status: Some(CoreStatus {
            state: CoreState::Ready,
            exchange: "xalpha".into(),
            exchange_name: "XALPHA".into(),
            has_api_keys: true,
            login_error: String::new(),
            moonproto_enabled: true,
            moonproto_listening: true,
            moonproto_error: String::new(),
            moonproto_port: 3302,
            emulator: None,
        }),
        info: None,
        log: None,
        key: None,
        exchanges: None,
        phases: Default::default(),
        checked_at: Some(Instant::now()),
        retry_elapsed: None,
        exchange_attempt: 0,
        exchange_switched: None,
    }
}

/// Non-secret view data lets tests exercise the same pure page builders as the window.
fn state(cmd: Cmd) -> ViewState {
    ViewState {
        cmd,
        core: "CORE_A".into(),
        form: MenuForm::Ours,
        version: "7.84".into(),
        latest: None,
        key_filled: false,
        secret_filled: false,
        address_valid: true,
        revealed: false,
        confirm: None,
        service_attempted: false,
        elapsed: "0:42".into(),
    }
}

/// Login errors must offer Retry, while successful status must never offer it as the primary action.
#[test]
fn status_login_error_offers_retry_and_loading_offers_close_only() {
    let _locale = crate::test_locale::force("en");
    let mut facts = facts();
    let state = state(Cmd::Status);
    assert_eq!(build(&facts, &state).kind, Kind::Status);
    facts.status.as_mut().expect("status").state = CoreState::LoginError;
    let page = build(&facts, &state);
    assert_eq!(page.kind, Kind::LoginError);
    assert!(
        page.footer_right
            .iter()
            .any(|b| b.primary && b.action == Action::Select(Cmd::RetryLogin))
    );
    facts.phases.insert(
        Cmd::Status,
        CmdPhase::Running {
            since: Instant::now(),
            seen: None,
        },
    );
    let page = build(&facts, &state);
    assert_eq!(page.kind, Kind::Loading);
    assert_eq!(page.footer_right.len(), 1);
    assert_eq!(page.footer_right[0].action, Action::Close);
}

/// A single credential cannot enable Save, and a running save cannot be submitted twice.
#[test]
fn api_keys_require_both_fields_and_an_idle_worker() {
    let _locale = crate::test_locale::force("en");
    let mut facts = facts();
    let mut state = state(Cmd::ApiKeys);
    state.key_filled = true;
    assert!(
        !build(&facts, &state)
            .footer_right
            .last()
            .expect("save")
            .enabled
    );
    state.secret_filled = true;
    assert!(
        build(&facts, &state)
            .footer_right
            .last()
            .expect("save")
            .enabled
    );
    facts.phases.insert(
        Cmd::ApiKeys,
        CmdPhase::Running {
            since: Instant::now(),
            seen: None,
        },
    );
    assert!(
        !build(&facts, &state)
            .footer_right
            .last()
            .expect("save")
            .enabled
    );
    facts.phases.insert(Cmd::ApiKeys, CmdPhase::Idle);
    facts.phases.insert(
        Cmd::SwitchExchange,
        CmdPhase::Running {
            since: Instant::now(),
            seen: None,
        },
    );
    assert!(
        !build(&facts, &state)
            .footer_right
            .last()
            .expect("save")
            .enabled,
        "an in-flight exchange switch must block credentials for the old exchange"
    );
}

/// Every command keeps its own page, and failures replace stale successful data.
#[test]
fn commands_and_failure_phases_select_the_expected_pages() {
    let _locale = crate::test_locale::force("en");
    for (cmd, kind) in [
        (Cmd::RetryLogin, Kind::Retry),
        (Cmd::Log, Kind::Log),
        (Cmd::Restart, Kind::Confirm),
        (Cmd::Stop, Kind::Confirm),
        (Cmd::Start, Kind::Confirm),
        (Cmd::SwitchExchange, Kind::Exchange),
        (Cmd::ApiKeys, Kind::Keys),
        (Cmd::ConnectionKey, Kind::Secret),
        (Cmd::MoonProtoAddress, Kind::Address),
        (Cmd::UpdateCore, Kind::Update),
    ] {
        let mut facts = facts();
        let mut state = state(cmd);
        if matches!(cmd, Cmd::Restart | Cmd::Stop) {
            state.confirm = Some(Confirm::Service(cmd));
        }
        assert_eq!(build(&facts, &state).kind, kind, "{cmd:?}");
        state.confirm = None;
        facts
            .phases
            .insert(cmd, CmdPhase::Failed(CmdError::Ssh("offline".into())));
        let page = build(&facts, &state);
        assert_eq!(page.kind, Kind::Failed);
        assert!(
            page.footer_right
                .iter()
                .any(|b| b.primary && b.action == Action::Run)
        );
        facts
            .phases
            .insert(cmd, CmdPhase::Failed(CmdError::NotOurs));
        assert_eq!(build(&facts, &state).kind, Kind::NotOurs);
        state.form = MenuForm::NoLogin;
        let page = build(&facts, &state);
        assert_eq!(page.kind, Kind::NoLogin);
        assert!(
            page.footer_right
                .iter()
                .any(|b| b.action == Action::AddLogin)
        );
    }
}

/// Failed saves must leave the fields editable; Retry is enabled only for a complete valid form.
#[test]
fn failed_forms_keep_inputs_and_validate_retry() {
    let _locale = crate::test_locale::force("en");
    let mut facts = facts();
    for cmd in [Cmd::ApiKeys, Cmd::MoonProtoAddress] {
        let mut state = state(cmd);
        facts
            .phases
            .insert(cmd, CmdPhase::Failed(CmdError::Ssh("offline".into())));
        state.address_valid = false;
        let page = build(&facts, &state);
        assert_eq!(page.kind, Kind::Failed);
        assert!(page.body.iter().any(|block| match cmd {
            Cmd::ApiKeys => matches!(block, Block::Keys),
            _ => matches!(block, Block::Address),
        }));
        assert!(
            !page
                .footer_right
                .iter()
                .find(|button| button.action == Action::Run)
                .expect("retry")
                .enabled
        );
        state.key_filled = true;
        state.secret_filled = true;
        state.address_valid = true;
        assert!(
            build(&facts, &state)
                .footer_right
                .iter()
                .find(|button| button.action == Action::Run)
                .expect("retry")
                .enabled
        );
    }
}

/// IPv6, zero, and out-of-range ports must keep Apply disabled just as the worker refuses them.
#[test]
fn moonproto_address_validation_requires_ipv4_and_a_real_port() {
    assert!(valid_address("0.0.0.0", "1"));
    assert!(valid_address("203.0.113.7", "65535"));
    assert!(valid_address("203.0.113.7", " 3305 \n"));
    assert_eq!(address_port(" 3305 \n"), Some(3305));
    for (host, port) in [
        ("2001:db8::7", "3302"),
        ("core.example", "3302"),
        ("203.0.113.7", "0"),
        ("203.0.113.7", "65536"),
        ("203.0.113.7", "not-a-port"),
    ] {
        assert!(!valid_address(host, port), "{host}:{port}");
    }
}

/// The status page must not invent a bind host that the core did not report.
#[test]
fn status_shows_only_the_moonproto_udp_port() {
    let _locale = crate::test_locale::force("en");
    let page = build(&facts(), &state(Cmd::Status));
    let value = page.body.iter().find_map(|block| match block {
        Block::Kv(rows) => rows
            .iter()
            .find(|(key, _)| *key == "core_cmds.status.kv_moonproto")
            .map(|(_, value)| value),
        _ => None,
    });
    assert_eq!(value.map(String::as_str), Some("UDP 3302"));
}

/// A cached Start success from an earlier attempt must not disable the next Start.
#[test]
fn cached_service_done_does_not_block_a_new_attempt() {
    let _locale = crate::test_locale::force("en");
    let mut facts = facts();
    facts.phases.insert(Cmd::Start, CmdPhase::Done);
    let mut state = state(Cmd::Start);

    let fresh = build(&facts, &state);
    assert_eq!(fresh.kind, Kind::Confirm);
    assert!(
        fresh
            .footer_right
            .iter()
            .any(|button| button.action == Action::Run && button.enabled)
    );
    state.service_attempted = true;
    assert_eq!(build(&facts, &state).kind, Kind::Done);
}

/// Changing the line count during a read would leave the selector describing the wrong log.
#[test]
fn log_picker_is_disabled_while_reading() {
    let _locale = crate::test_locale::force("en");
    let mut facts = facts();
    facts.phases.insert(
        Cmd::Log,
        CmdPhase::Running {
            since: Instant::now(),
            seen: None,
        },
    );
    let page = build(&facts, &state(Cmd::Log));
    assert!(
        page.body
            .iter()
            .any(|block| matches!(block, Block::LogControls { enabled: false }))
    );
}

/// Disabled or failed MoonProto listeners must not inherit the green exchange-ready state.
#[test]
fn status_surfaces_moonproto_disabled_and_listener_errors() {
    let _locale = crate::test_locale::force("en");
    for (enabled, tone) in [(false, super::Tone::Warn), (true, super::Tone::Bad)] {
        let mut facts = facts();
        let status = facts.status.as_mut().unwrap();
        status.moonproto_enabled = enabled;
        status.moonproto_listening = false;
        status.moonproto_error = "UDP bind failed".into();
        let page = build(&facts, &state(Cmd::Status));
        assert!(page.body.iter().any(|block| {
            matches!(block, Block::State(found, _, why) if *found == tone && why == "UDP bind failed")
        }));
    }
}
