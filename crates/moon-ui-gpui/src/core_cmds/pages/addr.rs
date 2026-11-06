//! MoonProto endpoint editing and save result.

use super::*;

/// Keep malformed addresses and zero ports out of the command queue.
pub(super) fn build(facts: &CoreFacts, view: &ViewState) -> Page {
    let running = matches!(phase(facts, view), CmdPhase::Running { .. });
    let mut page = Page::new(Kind::Address, title(view.cmd))
        .block(Block::Text(t!("core_cmds.addr.sub").to_string()))
        .block(Block::Address);
    if matches!(phase(facts, view), CmdPhase::Done) {
        page.body.push(Block::State(
            Tone::Ok,
            t!("core_cmds.addr.done").to_string(),
            String::new(),
        ));
    } else if running {
        page.body.push(Block::State(
            Tone::Run,
            t!("core_cmds.service.running").to_string(),
            String::new(),
        ));
    }
    page.footer_right = vec![
        Button::new("core_cmds.common.cancel", Action::Close),
        Button::new("core_cmds.addr.apply", Action::Run)
            .primary()
            .enabled(view.address_valid && !running),
    ];
    page
}
