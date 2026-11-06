//! Page-state oracles protect the actions offered after server outcomes and partial input.

use super::{Action, Block, Confirm, Kind, ViewState, build};
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
