//! Errors supersede cached success; installation refusals and missing login remain distinct.

use super::*;
use crate::backend::core_cmds::CmdError;

/// Replace a command's normal page only when current facts prohibit or failed that command.
pub(super) fn build(facts: &CoreFacts, view: &ViewState) -> Option<Page> {
    if view.form == MenuForm::NoLogin {
        let mut page = Page::new(Kind::NoLogin, title(view.cmd))
            .block(Block::Text(t!("core_cmds.error.no_login").to_string()));
        page.footer_right
            .push(Button::new("core_cmds.error.add_login", Action::AddLogin).primary());
        return Some(page);
    }
    let error = match phase(facts, view) {
        CmdPhase::Failed(_) if matches!(view.confirm, Some(Confirm::Service(_))) => None,
        CmdPhase::Failed(error) => Some(error),
        _ if view.form == MenuForm::Hand
            && matches!(
                view.cmd,
                Cmd::SwitchExchange
                    | Cmd::ApiKeys
                    | Cmd::ConnectionKey
                    | Cmd::MoonProtoAddress
                    | Cmd::UpdateCore
            ) =>
        {
            Some(&CmdError::NotOurs)
        }
        _ => None,
    }?;
    let mut page = Page::new(Kind::Failed, title(view.cmd));
    let (tone, heading, why, retry) = match error {
        CmdError::Ssh(_) => (
            Tone::Bad,
            t!("core_cmds.error.ssh").to_string(),
            t!("core_cmds.error.ssh_why", ip = facts.target).to_string(),
            true,
        ),
        CmdError::NoCore { port } => (
            Tone::Warn,
            t!("core_cmds.error.no_core", port = port).to_string(),
            t!("core_cmds.error.no_core_why").to_string(),
            false,
        ),
        CmdError::NotOurs => {
            page.kind = Kind::NotOurs;
            page.body = vec![
                Block::Text(t!("core_cmds.error.refused").to_string()),
                Block::Text(t!("core_cmds.error.refused_more").to_string()),
            ];
            return Some(page);
        }
        CmdError::Timeout => (
            Tone::Warn,
            t!(
                "core_cmds.retry.timeout",
                secs = moon_remote::core_new::SETTLE_TIMEOUT.as_secs()
            )
            .to_string(),
            String::new(),
            true,
        ),
        CmdError::Refused(reason) => (
            Tone::Bad,
            if view.cmd == Cmd::RetryLogin {
                t!("core_cmds.retry.refused").to_string()
            } else {
                t!("core_cmds.error.failed", reason = reason).to_string()
            },
            reason.clone(),
            true,
        ),
    };
    page.body.push(Block::State(tone, heading, why));
    if retry {
        page.footer_right
            .push(Button::new("core_cmds.common.retry_again", Action::Run).primary());
    }
    Some(page)
}
