//! Resolving a report row into a log-file scan, and running that scan.

use super::TradeLogRequest;
use moon_core::applog::{self, LogLine};

/// Milliseconds in one day, the step between rotated log files.
const DAY_MS: i64 = 86_400_000;

/// Maximum number of daily files scanned for one trade.
///
/// A trade held open for weeks would otherwise stream every day of a core's log — tens of megabytes
/// each — to find the handful of lines at its two ends. Beyond this many days the middle is dropped
/// and the ends are kept, which is where a trade's lines actually are.
const MAX_DAYS: usize = 8;

/// Builds the scan request for one report row.
///
/// Args:
///     row_core_name: `core_name` as the report stored it, which matches the file names written
///         while that row was recorded even if the core has since been renamed.
///     config_core_name: Current configured name of the same core, or `None` when it is gone.
///     coin: Coin as the report stored it, for the dialog title.
///     task_id: Core task number from the row's `taskid`.
///     buy_ms: Trade entry time in unix milliseconds.
///     close_ms: Trade exit time in unix milliseconds; not after `buy_ms` means still open, and the
///         span then ends at `now_ms`.
///     now_ms: Current time in unix milliseconds.
///
/// Returns:
///     The labels and UTC days to scan, plus the title fields.
pub(crate) fn trade_log_request(
    row_core_name: &str,
    config_core_name: Option<&str>,
    coin: &str,
    task_id: i64,
    buy_ms: i64,
    close_ms: i64,
    now_ms: i64,
) -> TradeLogRequest {
    let mut labels: Vec<String> = Vec::new();
    for name in [config_core_name.unwrap_or(""), row_core_name] {
        let label = applog::sanitize_label(name);
        if !name.is_empty() && !labels.contains(&label) {
            labels.push(label);
        }
    }
    // A row can carry a task number and no usable entry date; scanning from the epoch would burn
    // the whole day budget on 1970. Fall back to the exit, then to now.
    let start = if buy_ms > 0 {
        buy_ms
    } else if close_ms > 0 {
        close_ms
    } else {
        now_ms
    };
    let end = if close_ms > start { close_ms } else { now_ms };
    TradeLogRequest {
        core_name: if row_core_name.is_empty() {
            config_core_name.unwrap_or_default().to_string()
        } else {
            row_core_name.to_string()
        },
        coin: coin.to_string(),
        task_id,
        labels,
        dates: log_dates(start, end, MAX_DAYS),
    }
}

/// UTC days a trade spans, as the rotated log files name them.
///
/// A span longer than `max_days` keeps the first and last halves and drops the middle: the entry
/// and the exit are where a trade's lines are, and the days between would cost a full file read
/// each. Returns at least the entry day, even when the times are nonsense.
fn log_dates(from_ms: i64, to_ms: i64, max_days: usize) -> Vec<String> {
    let to_ms = to_ms.max(from_ms);
    let head_days = max_days.div_ceil(2);
    // Days from the entry forward.
    let mut days: Vec<String> = Vec::new();
    // Saturating steps: these times come from the database and a corrupt one can sit at the end of
    // the i64 range, where a wrapping step would walk the scan onto unrelated days.
    let mut at = from_ms;
    while at <= to_ms && days.len() < head_days {
        days.push(applog::split_unix_ms(at).0);
        let next = at.saturating_add(DAY_MS);
        if next == at {
            break;
        }
        at = next;
    }
    // Days from the exit backward, so a long trade keeps the exit rather than an arbitrary middle.
    let mut at = to_ms;
    while at >= from_ms && days.len() < max_days {
        days.push(applog::split_unix_ms(at).0);
        let next = at.saturating_sub(DAY_MS);
        if next == at {
            break;
        }
        at = next;
    }
    // A short trade produces the same days from both ends; dates sort lexicographically.
    days.sort();
    days.dedup();
    days
}

/// Scans the request's files for lines carrying the task number.
///
/// Runs on a background thread: each file is a day of one core's log. Lines from several files (a
/// multi-day trade, or a core renamed mid-way) are merged into one chronological list; both file
/// names and line times are UTC, so ordering by the stored timestamp is exact.
///
/// Args:
///     request: Labels, days, and task number resolved by [`trade_log_request`].
///     max: Maximum number of lines to return.
///
/// Returns:
///     The matching lines oldest first, and whether the cap cut them short.
pub(super) fn scan_trade_log(request: &TradeLogRequest, max: usize) -> applog::ScanResult {
    let marker = applog::task_marker(request.task_id);
    let mut lines: Vec<LogLine> = Vec::new();
    let mut truncated = false;
    'files: for date in &request.dates {
        for label in &request.labels {
            if lines.len() >= max {
                // Leave both loops: continuing would stream every remaining day's file in full
                // only to throw the result away.
                truncated = true;
                break 'files;
            }
            let found =
                applog::scan_file(&applog::file_name(date, label), &marker, max - lines.len());
            truncated |= found.truncated;
            lines.extend(found.lines);
        }
    }
    lines.sort_by(|a, b| a.ts.cmp(&b.ts));
    applog::ScanResult { lines, truncated }
}

#[cfg(test)]
/// Tests for resolving a trade into the files and days to scan.
mod tests;
