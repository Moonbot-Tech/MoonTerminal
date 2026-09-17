//! Shared, GPUI-free wording for reports-replica access and recovery notices.

use moon_core::db::report_recovery::RecoveryNotice;
use rust_i18n::t;

/// Compose the same localized title and detail for Report and Analytics.
///
/// Args:
///     notice: Startup facts; `None` is a typed access denial without a published notice yet.
///
/// Returns:
///     Human-readable title and detail, excluding the technical diagnostics kept in the log.
pub(crate) fn recovery_notice_text(notice: Option<&RecoveryNotice>) -> (String, String) {
    let (title, detail) = match notice {
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
