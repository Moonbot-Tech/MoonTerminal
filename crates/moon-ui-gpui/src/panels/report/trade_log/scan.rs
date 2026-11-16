//! Resolving a report row into a log-file scan, and running that scan.

use std::collections::HashMap;

use super::TradeLogRequest;
use moon_core::applog::server_day::server_file_name;
use moon_core::applog::{self, LogLine};

/// Milliseconds in one day, the step between rotated log files.
const DAY_MS: i64 = 86_400_000;

/// Maximum number of daily files scanned for one trade.
///
/// A trade held open for weeks would otherwise stream every day of a core's log — tens of megabytes
/// each — to find the handful of lines at its two ends. Beyond this many days the middle is dropped
/// and the ends are kept, which is where a trade's lines actually are.
const MAX_DAYS: usize = 8;

/// How far apart, in seconds, the two copies of one line may be stamped.
///
/// A pulled line carries the clock the core wrote it with; a live line carries the clock of the
/// stream that delivered it, which runs behind. Measured on a day of one core's live copy
/// (16 349 lines, 2026-11-14): 0.36 s on average, 254 lines over a second, 52 over three, and
/// 10.9 s at the worst. The slack sits above that worst case; matching is one for one, so a wide
/// window does not merge two real events — it only decides which copy pairs with which.
const CLOCK_SLACK_S: i64 = 15;

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
///     The labels and UTC days to scan, plus title fields and an explicitly unscoped workspace
///     identity for the Report caller to retain or bind.
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
        workspace: None,
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
/// A day can exist twice: the live copy, which holds the hours this terminal was connected, and
/// the whole day pulled from the core's server. Both are read, and a line the two share is shown
/// once ([`only_in_live`]) — so a trade made while the terminal was closed has its log as soon as
/// that day is pulled, and one made while it was open does not read double.
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
            let budget = max - lines.len();
            let pulled = applog::scan_file(&server_file_name(date, label), &marker, budget);
            let live = applog::scan_file(&applog::file_name(date, label), &marker, budget);
            truncated |= pulled.truncated || live.truncated;
            let live = only_in_live(live.lines, &pulled.lines);
            lines.extend(pulled.lines);
            lines.extend(live);
        }
    }
    lines.sort_by(|a, b| a.ts.cmp(&b.ts));
    // Each of a day's two files had the whole remaining budget, so together they can overshoot it.
    if lines.len() > max {
        lines.truncate(max);
        truncated = true;
    }
    applog::ScanResult { lines, truncated }
}

/// Lines of the live copy that the pulled copy of the same day does not already hold.
///
/// Two lines are the same line when their text is equal and their clocks are within
/// [`CLOCK_SLACK_S`]. The text is compared without the clock a core writes in front of most of its
/// messages ([`without_core_clock`]): the stream delivers that clock as part of the message, while
/// a pulled line has it in its time column. Matching is one for one and takes the nearest clock: a
/// message the core really wrote twice is in the pulled file twice and consumes two live lines,
/// never three.
///
/// Args:
///     live: Matching lines of the live copy.
///     pulled: Matching lines of the day pulled from the core's server.
///
/// Returns:
///     The live lines with no counterpart in `pulled`, in their original order.
fn only_in_live(live: Vec<LogLine>, pulled: &[LogLine]) -> Vec<LogLine> {
    if pulled.is_empty() {
        return live;
    }
    let mut unmatched: HashMap<&str, Vec<i64>> = HashMap::new();
    for line in pulled {
        if let Some(second) = line_second(line) {
            unmatched
                .entry(without_core_clock(&line.msg))
                .or_default()
                .push(second);
        }
    }
    live.into_iter()
        .filter(|line| {
            let Some(second) = line_second(line) else {
                return true;
            };
            let Some(seconds) = unmatched.get_mut(without_core_clock(&line.msg)) else {
                return true;
            };
            let twin = seconds
                .iter()
                .enumerate()
                .filter(|(_, pulled)| (**pulled - second).abs() <= CLOCK_SLACK_S)
                .min_by_key(|(_, pulled)| (**pulled - second).abs())
                .map(|(at, _)| at);
            match twin {
                Some(at) => {
                    seconds.swap_remove(at);
                    false
                }
                None => true,
            }
        })
        .collect()
}

/// A message without the `hh:nn:ss` or `hh:nn:ss.zzz` clock a core starts most of its lines with,
/// and without the blanks around it.
///
/// A message that merely begins with digits is left whole: only a clock followed by a blank — or
/// by nothing — is a clock.
fn without_core_clock(msg: &str) -> &str {
    let msg = msg.trim();
    let bytes = msg.as_bytes();
    let digit = |at: usize| bytes.get(at).is_some_and(u8::is_ascii_digit);
    let colon = |at: usize| bytes.get(at) == Some(&b':');
    if !(digit(0)
        && digit(1)
        && colon(2)
        && digit(3)
        && digit(4)
        && colon(5)
        && digit(6)
        && digit(7))
    {
        return msg;
    }
    let end = if bytes.get(8) == Some(&b'.') && digit(9) && digit(10) && digit(11) {
        12
    } else {
        8
    };
    // Every byte up to `end` is ASCII, so it is a character boundary.
    let rest = &msg[end..];
    if rest.is_empty() || rest.starts_with([' ', '\t']) {
        rest.trim_start()
    } else {
        msg
    }
}

/// A stored line's timestamp in whole seconds, or `None` when it carries no full timestamp.
fn line_second(line: &LogLine) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(line.ts.get(..19)?, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|at| at.and_utc().timestamp())
}

#[cfg(test)]
mod tests;
