//! Connection-key disclosure never puts the key into persistent view state.

use super::*;

/// Reveal only after the backend supplies a key, and expose a separate forget action.
pub(super) fn build(facts: &CoreFacts, view: &ViewState) -> Page {
    let mut page = Page::new(Kind::Secret, title(view.cmd))
        .block(Block::Text(t!("core_cmds.mpkey.sub").to_string()))
        .block(Block::Secret);
    if matches!(phase(facts, view), CmdPhase::Running { .. }) {
        page.body.push(Block::State(
            Tone::Run,
            t!("core_cmds.service.running").to_string(),
            String::new(),
        ));
    }
    page
}
