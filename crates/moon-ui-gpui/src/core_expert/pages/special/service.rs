//! The banner both service blocks of the Special page draw about this window's last request — the
//! hang watchdog and the Remote block's outgoing bot. The verdict itself is decided in
//! `core_expert::service`; this only words it.

use gpui::*;
use moon_ui::MoonAlert;
use rust_i18n::t;

use moon_core::feed::{ServiceAction, ServiceVerdict};

use super::super::super::service::Verdict;

/// The banner for a block's last request.
///
/// Args:
///     id: The block's own element id for the banner.
///     v: What to say.
pub(super) fn verdict_alert(id: &'static str, v: &Verdict<'_>) -> AnyElement {
    match v {
        Verdict::Waiting => MoonAlert::info(id, t!("core_expert.svc_waiting").to_string()),
        Verdict::Settled(action, ServiceVerdict::Applied) => {
            MoonAlert::success(id, applied_text(*action))
        }
        Verdict::Settled(_, ServiceVerdict::AppliedNotSaved(error)) => MoonAlert::warning(
            id,
            t!("core_expert.svc_not_saved", error = error.as_str()).to_string(),
        ),
        Verdict::Settled(_, ServiceVerdict::Refused(error)) if error.is_empty() => {
            MoonAlert::error(id, t!("core_expert.svc_refused_plain").to_string())
        }
        Verdict::Settled(_, ServiceVerdict::Refused(error)) => MoonAlert::error(
            id,
            t!("core_expert.svc_refused", error = error.as_str()).to_string(),
        ),
        Verdict::Settled(_, ServiceVerdict::NotSent(_)) => {
            MoonAlert::error(id, t!("core_expert.svc_not_sent").to_string())
        }
        Verdict::Settled(_, ServiceVerdict::Lost) => {
            MoonAlert::warning(id, t!("core_expert.svc_lost").to_string())
        }
    }
    .into_any_element()
}

/// What a confirmed action did, in the words that fit it.
fn applied_text(action: ServiceAction) -> String {
    match action {
        ServiceAction::WatchdogApply => t!("core_expert.wd_applied").to_string(),
        ServiceAction::WatchdogRemove => t!("core_expert.wd_removed").to_string(),
        ServiceAction::WatchdogCancelBuys => t!("core_expert.wd_cancel_accepted").to_string(),
        ServiceAction::TelegramOutgoingBot => t!("core_expert.remote_bot_saved").to_string(),
        ServiceAction::TelegramChannelPin => t!("core_expert.remote_pin_ready").to_string(),
        ServiceAction::TelegramResetChannel => t!("core_expert.remote_channel_reset").to_string(),
    }
}
