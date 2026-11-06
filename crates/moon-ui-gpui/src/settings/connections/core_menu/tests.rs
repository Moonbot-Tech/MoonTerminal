//! Exact approved command groups, service guards and eligible server addresses.

use super::{MenuCmd, MenuForm, MenuRow, core_menu_rows, eligible};
use moon_remote::core_cmds::Cmd;

/// The ownership forms must preserve all approved rows and withhold only reconfiguration.
#[test]
fn installed_forms_keep_exact_order_and_guards() {
    let commands = [
        ("status", "core_cmds.menu.status", Cmd::Status),
        ("retry", "core_cmds.menu.retry", Cmd::RetryLogin),
        ("log", "core_cmds.menu.log", Cmd::Log),
        ("restart", "core_cmds.menu.restart", Cmd::Restart),
        ("stop", "core_cmds.menu.stop", Cmd::Stop),
        ("start", "core_cmds.menu.start", Cmd::Start),
        ("exchange", "core_cmds.menu.exchange", Cmd::SwitchExchange),
        ("keys", "core_cmds.menu.keys", Cmd::ApiKeys),
        ("mpkey", "core_cmds.menu.mpkey", Cmd::ConnectionKey),
        ("addr", "core_cmds.menu.addr", Cmd::MoonProtoAddress),
        ("update", "core_cmds.menu.update", Cmd::UpdateCore),
    ];
    for form in [MenuForm::Ours, MenuForm::Hand, MenuForm::Checking] {
        for active in [None, Some(true), Some(false)] {
            let rows = core_menu_rows(form, active);
            assert_eq!(rows.len(), if form == MenuForm::Hand { 14 } else { 13 });
            assert_eq!(rows[3], MenuRow::Separator);
            assert_eq!(rows[7], MenuRow::Separator);
            for (index, &(id, label_key, cmd)) in commands.iter().enumerate() {
                let row_index = index + usize::from(index >= 3) + usize::from(index >= 6);
                let hint = if index == 5 && active == Some(true) {
                    Some("core_cmds.menu.running_hint")
                } else if [3, 4].contains(&index) && active == Some(false) {
                    Some("core_cmds.menu.stopped_hint")
                } else if index >= 6 && form == MenuForm::Checking {
                    Some("core_cmds.menu.checking_hint")
                } else {
                    None
                };
                assert_eq!(
                    rows[row_index],
                    MenuRow::Item {
                        id,
                        label_key,
                        action: MenuCmd::Run(cmd),
                        enabled: hint.is_none() && (index < 6 || form == MenuForm::Ours),
                        hint_key: hint,
                        accent: false,
                    },
                    "{form:?} {active:?} {id}"
                );
            }
            if form == MenuForm::Hand {
                assert_eq!(
                    rows.last(),
                    Some(&MenuRow::Note("core_cmds.menu.hand_note"))
                );
            }
        }
    }
}

/// Without a login, exposing any service or configuration command bypasses the login-only form.
#[test]
fn no_login_offers_only_note_and_accent_login() {
    assert_eq!(
        core_menu_rows(MenuForm::NoLogin, Some(true)),
        vec![
            MenuRow::Note("core_cmds.menu.nologin_note"),
            MenuRow::Item {
                id: "add-login",
                label_key: "core_cmds.menu.add_login",
                action: MenuCmd::AddLogin,
                enabled: true,
                hint_key: None,
                accent: true,
            },
        ]
    );
}

/// A public-IP-only gate hides LAN commands, while name or loopback admission targets local apps.
#[test]
fn addresses_allow_lan_and_ipv6_but_reject_loopback_and_names() {
    for host in ["203.0.113.7", "192.168.1.5", "2001:db8::7", "fe80::7"] {
        assert!(eligible(host), "{host}");
    }
    for host in [
        "127.0.0.1",
        "127.12.3.4",
        "localhost",
        "::1",
        "core.example",
        "",
    ] {
        assert!(!eligible(host), "{host}");
    }
}
