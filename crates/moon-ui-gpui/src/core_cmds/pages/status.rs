//! Status, loading, and exchange-login diagnosis from the cached core snapshot.

use super::*;

/// Build status facts without treating a failed exchange login as a stopped service.
pub(super) fn build(facts: &CoreFacts, view: &ViewState) -> Page {
    if matches!(phase(facts, view), CmdPhase::Running { .. }) || facts.status.is_none() {
        let mut page = Page::new(Kind::Loading, title(view.cmd)).block(Block::State(
            Tone::Run,
            t!("core_cmds.status.loading").to_string(),
            t!("core_cmds.status.loading_why", ip = facts.target).to_string(),
        ));
        page.footer_right = vec![Button::new("core_cmds.common.cancel", Action::Close)];
        return page;
    }
    let status = facts.status.as_ref().expect("status checked above");
    let login_error = status.state == CoreState::LoginError;
    let (tone, heading, why) = if login_error {
        (
            Tone::Bad,
            t!("core_cmds.status.login_error").to_string(),
            t!(
                "core_cmds.status.login_error_why",
                reason = status.login_error
            )
            .to_string(),
        )
    } else if status.state == CoreState::Ready {
        (
            Tone::Ok,
            t!("core_cmds.status.ready").to_string(),
            t!("core_cmds.status.ready_why").to_string(),
        )
    } else {
        (
            Tone::Warn,
            t!(
                "core_cmds.status.other_state",
                state = state_name(&status.state)
            )
            .to_string(),
            status.login_error.clone(),
        )
    };
    let unknown = || t!("core_cmds.status.unknown").to_string();
    let service = facts
        .info
        .as_ref()
        .map(|info| {
            if info.active == "active" {
                let uptime = info
                    .uptime_s
                    .map(|s| crate::display_text::fmt_duration_short(s as f64))
                    .unwrap_or_else(unknown);
                t!("core_cmds.status.service_running", uptime = uptime).to_string()
            } else {
                t!("core_cmds.status.service_stopped").to_string()
            }
        })
        .unwrap_or_else(unknown);
    let memory = facts
        .info
        .as_ref()
        .and_then(|i| i.memory)
        .map(|m| t!("core_cmds.status.memory_mb", mb = m / (1024 * 1024)).to_string())
        .unwrap_or_else(unknown);
    let mut page = Page::new(
        if login_error {
            Kind::LoginError
        } else {
            Kind::Status
        },
        title(view.cmd),
    )
    .block(Block::State(tone, heading, why))
    .block(Block::Kv(vec![
        ("core_cmds.status.kv_exchange", exchange_name(facts)),
        ("core_cmds.status.kv_service", service),
        ("core_cmds.status.kv_memory", memory),
        (
            "core_cmds.status.kv_moonproto",
            format!(
                "UDP {}",
                if status.moonproto_port > 0 {
                    status.moonproto_port
                } else {
                    facts.port
                }
            ),
        ),
        ("core_cmds.status.kv_version", view.version.clone()),
        (
            "core_cmds.status.kv_dir",
            facts
                .located
                .as_ref()
                .map(|l| l.dir.clone())
                .unwrap_or_else(unknown),
        ),
    ]));
    page.footer_left = facts.checked_at.map(|checked| {
        let elapsed = checked.elapsed().as_secs();
        if elapsed < 5 {
            t!("core_cmds.status.checked_now").to_string()
        } else {
            t!(
                "core_cmds.status.checked_ago",
                ago = format!("{}:{:02}", elapsed / 60, elapsed % 60)
            )
            .to_string()
        }
    });
    page.footer_right = vec![Button::new("core_cmds.common.refresh", Action::Run)];
    if login_error {
        page.footer_right.push(
            Button::new(
                "core_cmds.status.retry_login",
                Action::Select(Cmd::RetryLogin),
            )
            .primary(),
        );
    }
    page
}
