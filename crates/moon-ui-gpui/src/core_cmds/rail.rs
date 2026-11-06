//! Command groups and availability, shared by navigation and action gating.

use crate::backend::core_cmds::MenuForm;
use moon_remote::core_cmds::Cmd;

/// A rail caption or a selectable command, with its frozen dictionary key.
pub(super) enum Row {
    Caption(&'static str),
    Command(Cmd, &'static str),
}

/// Ordered navigation from the approved command-window mockup.
pub(super) const RAIL: &[Row] = &[
    Row::Caption("core_cmds.rail.cap_state"),
    Row::Command(Cmd::Status, "core_cmds.rail.status"),
    Row::Command(Cmd::RetryLogin, "core_cmds.rail.retry"),
    Row::Command(Cmd::Log, "core_cmds.rail.log"),
    Row::Caption("core_cmds.rail.cap_service"),
    Row::Command(Cmd::Restart, "core_cmds.rail.restart"),
    Row::Command(Cmd::Stop, "core_cmds.rail.stop"),
    Row::Command(Cmd::Start, "core_cmds.rail.start"),
    Row::Caption("core_cmds.rail.cap_settings"),
    Row::Command(Cmd::SwitchExchange, "core_cmds.rail.exchange"),
    Row::Command(Cmd::ApiKeys, "core_cmds.rail.keys"),
    Row::Command(Cmd::ConnectionKey, "core_cmds.rail.mpkey"),
    Row::Command(Cmd::MoonProtoAddress, "core_cmds.rail.addr"),
    Row::Command(Cmd::UpdateCore, "core_cmds.rail.update"),
];

/// Refuse unavailable commands before a click can enqueue a server-side change.
pub(super) fn available(form: MenuForm, active: Option<bool>, cmd: Cmd) -> bool {
    if form == MenuForm::NoLogin {
        return false;
    }
    if form != MenuForm::Ours
        && matches!(
            cmd,
            Cmd::SwitchExchange
                | Cmd::ApiKeys
                | Cmd::ConnectionKey
                | Cmd::MoonProtoAddress
                | Cmd::UpdateCore
        )
    {
        return false;
    }
    !matches!(
        (active, cmd),
        (Some(true), Cmd::Start) | (Some(false), Cmd::Stop | Cmd::Restart)
    )
}

#[cfg(test)]
mod tests;
