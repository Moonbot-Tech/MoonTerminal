//! Exchange selection and the explicit confirmation required before stopping strategies.

use super::*;

/// Show the server's exchange catalogue without inventing unsupported venues.
pub(super) fn build(facts: &CoreFacts, view: &ViewState) -> Page {
    let current = facts
        .status
        .as_ref()
        .map(|s| s.exchange.as_str())
        .unwrap_or_default();
    let exchanges = facts
        .exchanges
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|ex| {
            (
                ex.id.clone(),
                ex.name.clone(),
                ex.id == current,
                !ex.required_passphrase.is_empty(),
            )
        })
        .collect();
    let running = matches!(phase(facts, view), CmdPhase::Running { .. });
    let mut page = Page::new(Kind::Exchange, title(view.cmd)).block(Block::Exchanges(exchanges));
    if let Some(Confirm::Exchange { name, .. }) = &view.confirm {
        page.body.push(Block::Confirm {
            danger: false,
            title: t!("core_cmds.exchange.confirm", name = name).to_string(),
            why: t!("core_cmds.exchange.confirm_why").to_string(),
            buttons: vec![
                Button::new("core_cmds.exchange.do", Action::Confirm)
                    .primary()
                    .enabled(!running),
                Button::new("core_cmds.common.cancel", Action::CancelConfirm),
            ],
        });
    }
    if running {
        page.body.push(Block::State(
            Tone::Run,
            t!("core_cmds.service.running").to_string(),
            String::new(),
        ));
    }
    page
}
