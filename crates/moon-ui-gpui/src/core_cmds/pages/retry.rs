//! Login retry progress keeps the server operation independent of the window lifetime.

use super::*;

/// Present running, settled, and unstarted retries; failures are routed by the shared error page.
pub(super) fn build(facts: &CoreFacts, view: &ViewState) -> Page {
    let mut page = Page::new(Kind::Retry, title(view.cmd));
    match phase(facts, view) {
        CmdPhase::Running { seen, .. } => {
            let state = seen.as_ref().map(state_name).unwrap_or("initializing");
            page.body = vec![
                Block::Text(t!("core_cmds.retry.sub").to_string()),
                Block::State(
                    Tone::Run,
                    t!("core_cmds.retry.running", elapsed = view.elapsed).to_string(),
                    t!(
                        "core_cmds.retry.running_why",
                        state = state,
                        exchange = exchange_name(facts)
                    )
                    .to_string(),
                ),
            ];
            page.footer_left = Some(t!("core_cmds.retry.close_hint").to_string());
        }
        CmdPhase::Done => {
            page.kind = Kind::Done;
            page.body.push(Block::State(
                Tone::Ok,
                t!("core_cmds.retry.done").to_string(),
                t!(
                    "core_cmds.retry.done_why",
                    exchange = exchange_name(facts),
                    took = view.elapsed
                )
                .to_string(),
            ));
        }
        _ => {
            page.body
                .push(Block::Text(t!("core_cmds.retry.sub").to_string()));
            page.footer_right
                .push(Button::new("core_cmds.status.retry_login", Action::Run).primary());
        }
    }
    page
}
