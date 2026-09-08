//! Normalizes arbitrary feed and database text for single-line UI surfaces, and prints the
//! shared compact duration every surface that shows an elapsed or remaining time uses.
//!
//! GPUI lays out one visual line per hard `\n`, including in fixed-height rows that disable
//! soft wrapping — `whitespace_nowrap` and `truncate` suppress only SOFT wrapping. Such a
//! row centres the stacked lines and clips them, so the neighbours show up as half-cut
//! glyphs that read as an extra, broken row; that is the symptom this module exists for.
//! [`flatten_lines`] folds hard breaks for report cells, log rows, and single-line
//! diagnostics. Report exports use raw database values.

/// Visible marker inserted for a folded line break.
///
/// U+00B6 is available in both bundled fonts; U+23CE would require a system fallback font.
/// The marker is not localized because it contains no translatable text.
const BREAK_MARK: &str = " ¶ ";

/// Folds embedded CRLF, lone CR, and lone LF separators into [`BREAK_MARK`].
///
/// Leading and trailing line-break characters are removed. The returned string contains no
/// raw line breaks, preserves all other whitespace, and is unchanged by another call.
pub(crate) fn flatten_lines(text: &str) -> String {
    let text = text.trim_matches(['\r', '\n']);
    // Avoid the two replacement allocations when the text is already a single line.
    if !text.contains(['\n', '\r']) {
        return text.to_string();
    }
    // Normalize CRLF first so each pair produces one marker.
    text.replace("\r\n", "\n").replace(['\r', '\n'], BREAK_MARK)
}

use rust_i18n::t;

/// Format a short duration as at most two units: "45s", "1m 29s", "2h 15m", "3d 4h".
///
/// Callers give it one line — a calendar cell, a menu row — so the second unit is dropped when it
/// is zero and there is never a third. The unit names come from `analytics.cal.dur_*`, which is
/// where they were first written; they are not calendar-specific and every caller reads the same
/// four.
///
/// Args:
///     secs: Duration in seconds; negative and non-finite inputs yield an em dash.
///
/// Returns:
///     Localized compact duration.
pub(crate) fn fmt_duration_short(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "—".to_string();
    }
    let total = secs.round() as i64;
    let (s, m, h, d) = (
        t!("analytics.cal.dur_s"),
        t!("analytics.cal.dur_m"),
        t!("analytics.cal.dur_h"),
        t!("analytics.cal.dur_d"),
    );
    // Each arm picks the largest unit that fits and one below it, so precision falls away with
    // scale instead of printing "3d 4h 12m 6s" into a cell that has room for eight characters.
    let pair = |big: i64, big_unit: &str, small: i64, small_unit: &str| {
        if small > 0 {
            format!("{big}{big_unit} {small}{small_unit}")
        } else {
            format!("{big}{big_unit}")
        }
    };
    match total {
        ..=59 => format!("{total}{s}"),
        60..=3_599 => pair(total / 60, &m, total % 60, &s),
        3_600..=86_399 => pair(total / 3_600, &h, total % 3_600 / 60, &m),
        _ => pair(total / 86_400, &d, total % 86_400 / 3_600, &h),
    }
}
/// Format what is left of a temporary ban, to the MINUTE and rounded UP.
///
/// The ONE rule for a figure three surfaces print for the same ban — the chart's caption beside its
/// lock, the coin menu's row, and the coin dropdown's ban tab. A second copy of it is how the
/// caption and the menu come to disagree by a minute about the same coin.
///
/// Minutes rather than the raw remainder, and that is accuracy rather than taste: the core does not
/// republish a remainder merely for counting down (`feed::live::temp_blacklist::DECAY_SLACK` lets
/// it drift for up to a minute), so a figure printed to the second would claim a precision the
/// value does not have. Rounded UP, and floored at ONE minute, because a row the core still lists
/// is a ban a press can still lift: "0" beside a working lift button reads as a dead row.
///
/// Args:
///     remaining_ms: Deadline minus now; a negative value is a countdown that has run out locally.
///
/// Returns:
///     The localized figure, never below one minute.
pub(crate) fn fmt_ban_left(remaining_ms: i64) -> String {
    let left = remaining_ms.max(0);
    let minutes = (left / 60_000 + i64::from(left % 60_000 > 0)).max(1);
    fmt_duration_short((minutes * 60) as f64)
}

#[cfg(test)]
mod tests;
