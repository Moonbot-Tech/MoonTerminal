//! A move's steps in the user's language: `moon_remote` reports them as data and says nothing.

use moon_remote::core_move::{MoveStep, MoveWarning};
use rust_i18n::t;

/// One progress line for `step`.
pub(super) fn step(step: &MoveStep) -> String {
    match step {
        MoveStep::SnapshotTaken { bytes, exchange } => t!(
            "core_move.step.snapshot",
            size = megabytes(*bytes as u64),
            exchange = exchange.as_deref().unwrap_or("—")
        ),
        MoveStep::Composed {
            exchange,
            report_db,
            ..
        } => match report_db {
            Some(_) => t!("core_move.step.composed_db", exchange = exchange),
            None => t!("core_move.step.composed", exchange = exchange),
        },
        MoveStep::ReportDb {
            rows, open_rows, ..
        } => t!("core_move.step.report_db", rows = rows, open = open_rows),
        MoveStep::OldStrategies {
            strategies,
            folders,
        } => t!(
            "core_move.step.strategies_read",
            n = strategies,
            folders = folders
        ),
        MoveStep::Checked => t!("core_move.step.checked"),
        MoveStep::Uploaded => t!("core_move.step.uploaded"),
        MoveStep::Installed => t!("core_move.step.installed"),
        MoveStep::StopRequested => t!("core_move.step.stop_requested"),
        MoveStep::OldStopped => t!("core_move.step.old_stopped"),
        MoveStep::OldRefused { open_deals } => match open_deals {
            Some(n) => t!("core_move.step.old_refused", n = n),
            None => t!("core_move.step.old_refused_unknown"),
        },
        MoveStep::Started => t!("core_move.step.started"),
        MoveStep::StrategiesWritten { written, deleted } => t!(
            "core_move.step.strategies_written",
            n = written,
            deleted = deleted
        ),
        MoveStep::Finished => t!("core_move.step.finished"),
        MoveStep::Removed => t!("core_move.step.removed"),
        MoveStep::Warning(kind, detail) => {
            let key = match kind {
                MoveWarning::ExchangeNotAsked => "core_move.warn.exchange",
                MoveWarning::StrategiesNotRead => "core_move.warn.strategies_read",
                MoveWarning::StrategiesNotRefreshed => "core_move.warn.strategies_refresh",
                MoveWarning::StrategiesNotConfirmed => "core_move.warn.strategies_written",
            };
            t!(key, detail = detail)
        }
    }
    .to_string()
}

/// `bytes` as megabytes with one decimal.
fn megabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_048_576.0)
}
