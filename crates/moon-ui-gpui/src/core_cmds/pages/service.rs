//! Explicit service-change confirmation, followed by progress and completion.

use super::*;

/// Keep Restart and Stop gated by an explicit user confirmation on each new invocation.
pub(super) fn build(facts: &CoreFacts, view: &ViewState) -> Page {
    let mut page = Page::new(Kind::Confirm, title(view.cmd));
    match phase(facts, view) {
        CmdPhase::Running { .. } => {
            page.kind = Kind::Loading;
            page.body.push(Block::State(
                Tone::Run,
                t!("core_cmds.service.running").to_string(),
                String::new(),
            ));
        }
        CmdPhase::Done if view.confirm.is_none() => {
            page.kind = Kind::Done;
            let state = facts
                .info
                .as_ref()
                .map(|i| i.active.as_str())
                .unwrap_or_default();
            page.body.push(Block::State(
                Tone::Ok,
                t!("core_cmds.service.done", state = state).to_string(),
                String::new(),
            ));
        }
        _ if view.cmd == Cmd::Start => {
            page.footer_right
                .push(Button::new("core_cmds.service.start_do", Action::Run).primary());
        }
        _ => {
            let stop = view.cmd == Cmd::Stop;
            let mut action = Button::new(
                if stop {
                    "core_cmds.service.stop_do"
                } else {
                    "core_cmds.service.restart_do"
                },
                Action::Confirm,
            )
            .primary();
            if stop {
                action = action.danger();
            }
            page.body.push(Block::Confirm {
                danger: stop,
                title: t!(
                    if stop {
                        "core_cmds.service.stop_confirm"
                    } else {
                        "core_cmds.service.restart_confirm"
                    },
                    core = view.core
                )
                .to_string(),
                why: t!(if stop {
                    "core_cmds.service.stop_why"
                } else {
                    "core_cmds.service.restart_why"
                })
                .to_string(),
                buttons: vec![
                    action,
                    Button::new("core_cmds.common.cancel", Action::Close),
                ],
            });
        }
    }
    page
}
