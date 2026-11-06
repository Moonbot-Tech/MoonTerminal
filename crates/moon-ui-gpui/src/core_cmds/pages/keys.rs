//! The two exchange credentials leave the view only as zeroizing request arguments.

use super::*;

/// Require both credentials, and disable submission while a save or exchange switch is in progress.
pub(super) fn build(facts: &CoreFacts, view: &ViewState) -> Page {
    let running = matches!(phase(facts, view), CmdPhase::Running { .. });
    let switching = matches!(
        facts.phases.get(&Cmd::SwitchExchange),
        Some(CmdPhase::Running { .. })
    );
    let mut page = Page::new(Kind::Keys, title(view.cmd))
        .block(Block::Text(
            t!("core_cmds.keys.sub", exchange = exchange_name(facts)).to_string(),
        ))
        .block(Block::Keys);
    if matches!(phase(facts, view), CmdPhase::Done) {
        page.body.push(Block::State(
            Tone::Ok,
            t!("core_cmds.keys.saved").to_string(),
            String::new(),
        ));
    } else if running || switching {
        page.body.push(Block::State(
            Tone::Run,
            t!("core_cmds.service.running").to_string(),
            String::new(),
        ));
    }
    page.footer_right = vec![
        Button::new("core_cmds.common.cancel", Action::Close),
        Button::new("core_cmds.keys.save", Action::Run)
            .primary()
            .enabled(view.key_filled && view.secret_filled && !running && !switching),
    ];
    page
}
