//! A bounded, verbatim log tail with separate display classification and clipboard actions.

use super::*;

/// Build the bounded line picker, raw log, and UTC caption.
pub(super) fn build(facts: &CoreFacts, view: &ViewState) -> Page {
    let buttons = vec![
        Button::new("core_cmds.common.refresh", Action::Run),
        Button::new("core_cmds.common.copy", Action::CopyLog)
            .enabled(facts.log.as_ref().is_some_and(|l| !l.lines.is_empty())),
    ];
    let mut page = Page::new(Kind::Log, title(view.cmd))
        .block(Block::LogControls)
        .block(Block::Buttons(buttons));
    if matches!(phase(facts, view), CmdPhase::Running { .. }) {
        page.body.push(Block::State(
            Tone::Run,
            t!("core_cmds.service.running").to_string(),
            String::new(),
        ));
    }
    if let Some(log) = facts.log.as_ref().filter(|l| !l.lines.is_empty()) {
        page.body.push(Block::Log(log.lines.clone()));
    } else {
        page.body
            .push(Block::Text(t!("core_cmds.log.empty").to_string()));
    }
    page.body
        .push(Block::Text(t!("core_cmds.log.utc").to_string()));
    page
}
