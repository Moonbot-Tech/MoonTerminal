//! Shared wording for reports-replica access and recovery notices, and the one action they
//! offer: starting a fresh replica after a full check that could not run.
//!
//! The wording stays GPUI-free so its tests need no window; the action is the only element here,
//! shared so Report and Analytics offer it the same way.

use gpui::{AnyElement, App, IntoElement, ParentElement, Styled, WeakEntity, Window, div};
use moon_core::db::report_recovery::{self, RecoveryNotice};
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonNotification, MoonPalette, MoonWindowExt as _, h_flex,
    v_flex,
};
use rust_i18n::t;

use crate::design;

/// The fresh-replica action under a recording-off notice, or `None` when it is not offered.
///
/// Offered only while the writer waits for it ([`report_recovery::fresh_replica_offered`]): after
/// a full check that could not run, before or after a failed attempt, never while a check or a
/// set-aside runs. The press opens a confirmation; only its Yes starts anything.
///
/// Args:
///     id: Element id of the button, unique within the host.
///     host: The view showing the notice, repainted once the set-aside started.
///     cx: Context for the theme.
///
/// Returns:
///     Explanation plus button, or `None`.
pub(crate) fn fresh_replica_action<V: 'static>(
    id: &'static str,
    host: WeakEntity<V>,
    cx: &App,
) -> Option<AnyElement> {
    if !report_recovery::fresh_replica_offered() {
        return None;
    }
    let p = MoonPalette::active(cx);
    Some(
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 6.0))
            .items_start()
            .child(
                div()
                    .w_full()
                    .text_size(design::t_caption(cx))
                    .text_color(design::moon(p.text_soft))
                    .child(t!("analytics.fresh_replica_offer").to_string()),
            )
            .child(
                MoonButton::new(id)
                    .outline()
                    .label(t!("analytics.fresh_replica_button").to_string())
                    .on_click(move |_, window, app| {
                        open_fresh_replica_confirmation(host.clone(), window, app);
                    })
                    .render(),
            )
            .into_any_element(),
    )
}

/// Ask before setting the replica aside; Yes starts it and repaints the host.
///
/// The offer is checked again at Yes by [`report_recovery::start_fresh_replica`] itself, so a
/// dialog left open across a start from the other surface starts nothing a second time.
///
/// Args:
///     host: The view showing the notice.
///     window: Window the dialog opens over.
///     app: Application context.
fn open_fresh_replica_confirmation<V: 'static>(
    host: WeakEntity<V>,
    window: &mut Window,
    app: &mut App,
) {
    window.open_unique_moon_dialog("fresh-replica-confirm", app, move |dialog, _, cx| {
        let host = host.clone();
        let body = t!("analytics.fresh_replica_confirm_body").to_string();
        crate::panels::order_dialog_chrome(
            dialog,
            420.0,
            t!("analytics.fresh_replica_confirm_title").to_string(),
            cx,
        )
        .content(move |content, _, _| content.child(div().w_full().child(body.clone())))
        .footer(
            h_flex()
                .w_full()
                .gap(design::ui_px(cx, 8.0))
                .justify_end()
                .child(
                    MoonButton::new("fresh-replica-no")
                        .outline()
                        .label(t!("dialogs.cancel").to_string())
                        .on_click(|_, window, cx| window.close_dialog(cx))
                        .render(),
                )
                .child(
                    MoonButton::new("fresh-replica-yes")
                        .variant(MoonButtonVariant::Danger)
                        .label(t!("analytics.fresh_replica_confirm_yes").to_string())
                        .on_click(move |_, window, app| {
                            if !report_recovery::start_fresh_replica() {
                                window.push_notification(
                                    MoonNotification::warning(
                                        t!("analytics.fresh_replica_unavailable").to_string(),
                                    ),
                                    app,
                                );
                            }
                            let _ = host.update(app, |_, cx| cx.notify());
                            window.close_dialog(app);
                        })
                        .render(),
                ),
        )
    });
}

/// Compose the same localized title and detail for Report and Analytics.
///
/// Args:
///     notice: Startup facts; `None` is a typed access denial without a published notice yet.
///
/// Returns:
///     Human-readable title and detail, excluding the technical diagnostics kept in the log.
pub(crate) fn recovery_notice_text(notice: Option<&RecoveryNotice>) -> (String, String) {
    let (title, detail) = match notice {
        // Returned whole, like a completed recovery: recording is about to start by itself, so
        // the stopped-recording paragraph below — fix the cause, restart — would be false here.
        Some(RecoveryNotice::CheckRunning) => {
            return (
                t!("analytics.integrity_running").to_string(),
                t!("analytics.integrity_running_detail").to_string(),
            );
        }
        Some(RecoveryNotice::CheckIncomplete { .. }) => (
            t!("analytics.integrity_unchecked").to_string(),
            t!("analytics.integrity_read_only_detail").to_string(),
        ),
        Some(RecoveryNotice::Recovered { snapshot_dir }) => {
            return (
                t!("analytics.recovery_done").to_string(),
                t!(
                    "analytics.recovery_done_detail",
                    path = snapshot_dir.display().to_string()
                )
                .to_string(),
            );
        }
        Some(RecoveryNotice::Blocked { snapshot_dir, .. }) => (
            t!("analytics.recovery_blocked").to_string(),
            match snapshot_dir {
                Some(snapshot) => t!(
                    "analytics.recovery_blocked_snapshot",
                    path = snapshot.display().to_string()
                )
                .to_string(),
                None => t!("analytics.recovery_blocked_detail").to_string(),
            },
        ),
        Some(RecoveryNotice::Failed { .. }) => (
            t!("analytics.recovery_failed").to_string(),
            t!("analytics.recovery_failed_detail").to_string(),
        ),
        // Returned whole: recording starts by itself once the copy is done, and history is the
        // one thing missing meanwhile, which the detail says.
        Some(RecoveryNotice::FreshReplicaRunning) => {
            return (
                t!("analytics.fresh_replica_running").to_string(),
                t!("analytics.fresh_replica_running_detail").to_string(),
            );
        }
        Some(RecoveryNotice::FreshReplicaFailed) => (
            t!("analytics.fresh_replica_failed").to_string(),
            t!("analytics.fresh_replica_failed_detail").to_string(),
        ),
        Some(RecoveryNotice::FreshReplicaStarted { snapshot_dir }) => {
            return (
                t!("analytics.fresh_replica_done").to_string(),
                t!(
                    "analytics.fresh_replica_done_detail",
                    path = snapshot_dir.display().to_string()
                )
                .to_string(),
            );
        }
        Some(RecoveryNotice::LeaseUnavailable { .. }) => (
            t!("analytics.recovery_lease_unavailable").to_string(),
            t!("analytics.recovery_lease_detail").to_string(),
        ),
        None => (
            t!("report.access_denied").to_string(),
            t!("analytics.recovery_lease_detail").to_string(),
        ),
    };
    (
        title,
        format!("{detail} {}", t!("analytics.recovery_recording_off_detail")),
    )
}

#[cfg(test)]
mod tests;
