//! Core updates use the existing fleet update surface rather than a second updater.

use super::*;

/// Show the known version and route the user to the shared Updates window.
pub(super) fn build(_: &CoreFacts, view: &ViewState) -> Page {
    let why = view
        .latest
        .as_ref()
        .map(|latest| t!("core_cmds.update.why_available", latest = latest).to_string())
        .unwrap_or_else(|| t!("core_cmds.update.why").to_string());
    let mut page = Page::new(Kind::Update, title(view.cmd)).block(Block::State(
        Tone::Ok,
        t!("core_cmds.update.version", version = view.version).to_string(),
        why,
    ));
    page.footer_right
        .push(Button::new("core_cmds.update.open", Action::OpenUpdates).primary());
    page
}
