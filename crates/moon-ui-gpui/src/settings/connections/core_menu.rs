//! Command-menu rows and address eligibility, independent of the Settings window.

use std::net::Ipv4Addr;

use moon_remote::core_cmds::Cmd;

use crate::backend::core_cmds::MenuForm;

/// A localized row in the core's context menu; the caller supplies the caption.
#[derive(Debug, PartialEq)]
pub(crate) enum MenuRow {
    Caption(String),
    Item {
        id: &'static str,
        label_key: &'static str,
        action: MenuCmd,
        enabled: bool,
        hint_key: Option<&'static str>,
        accent: bool,
    },
    Separator,
    Note(&'static str),
}

/// The action dispatched by a menu item.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MenuCmd {
    Run(Cmd),
    AddLogin,
}

/// Build the approved menu, withholding reconfiguration until ownership is established.
pub(crate) fn core_menu_rows(form: MenuForm, active: Option<bool>) -> Vec<MenuRow> {
    if form == MenuForm::NoLogin {
        return vec![
            MenuRow::Note("core_cmds.menu.nologin_note_1"),
            MenuRow::Note("core_cmds.menu.nologin_note_2"),
            MenuRow::Item {
                id: "add-login",
                label_key: "core_cmds.menu.add_login",
                action: MenuCmd::AddLogin,
                enabled: true,
                hint_key: None,
                accent: true,
            },
        ];
    }
    let mut rows = Vec::new();
    for (group, commands) in [
        &[
            ("status", "core_cmds.menu.status", Cmd::Status),
            ("retry", "core_cmds.menu.retry", Cmd::RetryLogin),
            ("log", "core_cmds.menu.log", Cmd::Log),
        ][..],
        &[
            ("restart", "core_cmds.menu.restart", Cmd::Restart),
            ("stop", "core_cmds.menu.stop", Cmd::Stop),
            ("start", "core_cmds.menu.start", Cmd::Start),
        ][..],
        &[
            ("exchange", "core_cmds.menu.exchange", Cmd::SwitchExchange),
            ("keys", "core_cmds.menu.keys", Cmd::ApiKeys),
            ("mpkey", "core_cmds.menu.mpkey", Cmd::ConnectionKey),
            ("addr", "core_cmds.menu.addr", Cmd::MoonProtoAddress),
            ("update", "core_cmds.menu.update", Cmd::UpdateCore),
        ][..],
    ]
    .into_iter()
    .enumerate()
    {
        if group != 0 {
            rows.push(MenuRow::Separator);
        }
        for &(id, label_key, cmd) in commands {
            let hint_key = match (cmd, active) {
                (Cmd::Start, Some(true)) => Some("core_cmds.menu.running_hint"),
                (Cmd::Stop | Cmd::Restart, Some(false)) => Some("core_cmds.menu.stopped_hint"),
                _ if group == 2 && form == MenuForm::Checking => {
                    Some("core_cmds.menu.checking_hint")
                }
                _ => None,
            };
            rows.push(MenuRow::Item {
                id,
                label_key,
                action: MenuCmd::Run(cmd),
                enabled: hint_key.is_none() && (group != 2 || form == MenuForm::Ours),
                hint_key,
                accent: false,
            });
        }
    }
    if form == MenuForm::Hand {
        rows.extend([
            MenuRow::Note("core_cmds.menu.hand_note_1"),
            MenuRow::Note("core_cmds.menu.hand_note_2"),
            MenuRow::Note("core_cmds.menu.hand_note_3"),
        ]);
    }
    rows
}

/// Offer server commands for non-loopback literal IPv4 hosts, including LAN addresses.
pub(crate) fn eligible(addr_host: &str) -> bool {
    addr_host
        .parse::<Ipv4Addr>()
        .is_ok_and(|ip| !ip.is_loopback())
}

#[cfg(test)]
mod tests;
