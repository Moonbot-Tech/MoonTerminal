//! Telegram HTML for one notification: a closed-trade card, a down or back
//! line, and the daily summary.
//!
//! Names are cut to 64 chars before [`crate::html::escape`]. The outbox refuses
//! oversized bodies rather than cutting HTML inside a tag or entity.

use crate::t;
use chrono::NaiveDate;
use chrono_tz::Tz;
use moon_core::feed::order_math::MONEY_DECIMALS;
use moon_core::util::{display_time, fmt};

use super::daily::DaySummary;
use super::trades::ClosedTrade;
use crate::html::escape;

/// Longest coin, core, or strategy kept on a card, in Unicode scalars.
const NAME_CHARS: usize = 64;

/// One closed trade as a three-line HTML card.
///
/// Line 1 is a sign mark, the coin, the signed profit, an optional whole-dollar
/// entry volume, and the duration. Line 2 is the core name. Line 3 is the
/// strategy, in italics.
///
/// Args:
///     t: The trade to announce. `profit_usd: None` is unvalued and draws the
///         white mark plus the unvalued word. `volume_usd: None`, or a
///         non-finite volume, drops the volume segment.
///
/// Returns:
///     HTML using only `<b>` and `<i>`. Under a thousand chars for ordinary
///     names, and far under Telegram's 4096 UTF-16 limit even when every name
///     is 64 escaped characters.
pub(crate) fn trade_card(t: &ClosedTrade) -> String {
    let mut parts = vec![
        format!("{} <b>{}</b>", sign_mark(t.profit_usd), name(&t.coin)),
        profit_text(t.profit_usd, t.profit_pct),
    ];
    if let Some(volume) = whole_dollar_volume(t.volume_usd) {
        parts.push(volume);
    }
    parts.push(human_duration(t.close_utc.saturating_sub(t.open_utc)));
    format!(
        "{}\n{}\n{}",
        parts.join(" \u{00b7} "),
        name(&t.core_name),
        strategy_line(&t.strategy)
    )
}

/// One line: the core lost its link, and the local time that outage started.
///
/// Args:
///     core_name: Configured core name. Cut to 64 chars, then escaped.
///     since_utc: UTC Unix seconds when the loss was first observed.
///     zone: Host report zone. The clock is that zone's `HH:MM`.
///
/// Returns:
///     A single plain line with a leading loss mark. An instant chrono cannot
///     represent leaves the clock empty; the phrase still names the outage.
pub(crate) fn down_line(core_name: &str, since_utc: i64, zone: Tz) -> String {
    format!(
        "\u{1f534} {} \u{2014} {} {}",
        name(core_name),
        escape(&t!("telegram.notify_down")),
        local_hhmm(since_utc, zone)
    )
}

/// One line: the core is back, with how long the announced outage lasted.
///
/// Args:
///     core_name: Configured core name. Cut to 64 chars, then escaped.
///     down_for_secs: Seconds from the recorded loss to now. Negative becomes
///         zero minutes.
///
/// Returns:
///     A single plain line with a leading gain mark. The duration sits in
///     parentheses.
pub(crate) fn back_line(core_name: &str, down_for_secs: i64) -> String {
    format!(
        "\u{1f7e2} {} \u{2014} {} ({})",
        name(core_name),
        escape(&t!("telegram.notify_back")),
        human_duration(down_for_secs)
    )
}

/// The day's profit, trade count, and valued extremes.
///
/// Args:
///     date: Local calendar date the summary covers.
///     s: Totals from [`super::daily::summarize`]. `best` and `worst` of
///         `None` are omitted. `unvalued` is omitted when it is zero.
///
/// Returns:
///     HTML whose only tag is `<b>` around the title and the date.
pub(crate) fn daily_summary(date: NaiveDate, s: &DaySummary) -> String {
    let mut lines = vec![
        format!(
            "<b>{} {}</b>",
            escape(&t!("telegram.notify_daily_title")),
            date.format("%Y-%m-%d")
        ),
        format!(
            "{}: {}",
            escape(&t!("telegram.notify_profit")),
            signed_money(s.profit_usd)
        ),
        format!("{}: {}", escape(&t!("telegram.notify_trades")), s.count),
    ];
    if s.unvalued > 0 {
        lines.push(format!(
            "{}: {}",
            escape(&t!("telegram.notify_unvalued_trades")),
            s.unvalued
        ));
    }
    if let Some(trade) = &s.best {
        lines.push(extreme_line(&t!("telegram.notify_best"), trade));
    }
    if let Some(trade) = &s.worst {
        lines.push(extreme_line(&t!("telegram.notify_worst"), trade));
    }
    lines.join("\n")
}

/// Strategy line: `<i>source · name</i>`, or the manual word when the row stored none.
///
/// A stored channel name of the form `Source: (strategy <NAME>)` is split.
/// The 64-char cut applies to the parsed name and to the source separately,
/// then each piece is escaped. A string that is not that shape is cut and
/// escaped whole, still inside `<i>`.
///
/// Args:
///     strategy: Stored `channelname`. Empty means a manual close.
///
/// Returns:
///     One italic HTML line.
fn strategy_line(strategy: &str) -> String {
    if strategy.is_empty() {
        return format!("<i>{}</i>", escape(&t!("telegram.notify_manual")));
    }
    let body = match parse_stored_strategy(strategy) {
        Some((source, strategy_name)) => {
            let shown = name(&strategy_name);
            if source.is_empty() {
                shown
            } else {
                format!("{} \u{00b7} {shown}", name(&source))
            }
        }
        None => name(strategy),
    };
    format!("<i>{body}</i>")
}

/// Split `Source: (strategy <NAME>)` into the source and the name.
///
/// The chart finds this tail on a live detect line so it can drop it
/// (`strategy_tail_start` in `moon-ui-gpui`). That helper is private, returns
/// only the cut point, and lives in a crate this one must not depend on.
/// `moon_core::db::trade_meta` documents the same spelling and deliberately
/// keeps the tail on a stored comment. The card needs the two pieces, so it
/// reads them here.
///
/// The group has to close the string, and the name between the angle brackets
/// has to contain neither bracket. Anything else is not this shape.
///
/// Args:
///     raw: Stored channel name, possibly with surrounding whitespace.
///
/// Returns:
///     Source (trailing colon and surrounding whitespace removed; empty when
///     the group is the whole string) and the inner name. `None` when `raw`
///     does not end in `(strategy <NAME>)`.
fn parse_stored_strategy(raw: &str) -> Option<(String, String)> {
    const OPEN: &str = "(strategy <";
    let raw = raw.trim();
    let at = raw.rfind(OPEN)?;
    let inner = raw.get(at + OPEN.len()..)?.strip_suffix(">)")?;
    let strategy_name = inner.trim();
    if strategy_name.is_empty() || strategy_name.contains(['<', '>']) {
        return None;
    }
    let source = raw[..at].trim().trim_end_matches(':').trim().to_string();
    Some((source, strategy_name.to_string()))
}

/// Entry notional as grouped whole dollars with a `$` suffix.
///
/// Cents round to the nearest dollar, half away from zero, through
/// [`fmt::round_to`]. A non-finite amount is omitted, the same as a missing
/// volume: the card must not print `0$` for a figure the row could not value.
///
/// Args:
///     value: Entry notional in USDT, when the row has one.
///
/// Returns:
///     Text such as `5 992$`, or `None` when there is nothing to print.
fn whole_dollar_volume(value: Option<f64>) -> Option<String> {
    let rounded = fmt::round_to(value?, 0)?;
    let digits = format!("{rounded:.0}");
    Some(format!("{}$", fmt::group_thousands(&digits)))
}

/// `label: coin money` for one extreme. The coin is a name; the money is the
/// row's profit, or the unvalued word when that profit is absent.
fn extreme_line(label: &str, trade: &ClosedTrade) -> String {
    let profit = match trade.profit_usd {
        Some(value) => signed_money(value),
        None => escape(&t!("telegram.notify_unvalued")),
    };
    format!("{}: {} {profit}", escape(label), name(&trade.coin))
}

/// Profit in dollars and percent. `None` dollars is the unvalued word alone,
/// never a zero. A missing percent keeps the bold dollars and marks the
/// percent unvalued. The dollar figure is bold; the percent is not.
///
/// Args:
///     usd: Closed profit in USDT. `None` is unvalued.
///     pct: Profit percent already scaled by 100, when the row has one.
///
/// Returns:
///     Either the escaped unvalued word, or `<b>+1.25$</b> (+0.40%)`.
fn profit_text(usd: Option<f64>, pct: Option<f64>) -> String {
    let Some(usd) = usd else {
        return escape(&t!("telegram.notify_unvalued"));
    };
    let money = signed_money(usd);
    let pct_text = match pct.and_then(|value| fmt::signed_pct(value, MONEY_DECIMALS)) {
        Some((pct, _)) => pct,
        None => escape(&t!("telegram.notify_unvalued")),
    };
    format!("<b>{money}</b> ({pct_text})")
}

/// `🟢` for a gain, `🔴` for a loss, `⚪` for zero or an unvalued profit.
///
/// Chosen from the same rounded cents as [`signed_money`], so a value that
/// prints as `0.00$` is white even when the raw profit is a tiny non-zero.
///
/// Args:
///     usd: Closed profit in USDT. `None` is unvalued.
///
/// Returns:
///     One sign mark, with no trailing space.
fn sign_mark(usd: Option<f64>) -> &'static str {
    let Some(usd) = usd else {
        return "\u{26aa}";
    };
    let text = fmt::usd_grouped_cents(usd);
    if text.starts_with('-') {
        "\u{1f534}"
    } else if text == "0.00" {
        "\u{26aa}"
    } else {
        "\u{1f7e2}"
    }
}

/// Grouped cents with `$`. A positive rounded amount gets `+`; zero and
/// minus stay as [`fmt::usd_grouped_cents`] prints them.
fn signed_money(value: f64) -> String {
    let text = fmt::usd_grouped_cents(value);
    if text.starts_with('-') || text == "0.00" {
        format!("{text}$")
    } else {
        format!("+{text}$")
    }
}

/// First 64 Unicode scalars, then HTML-escaped.
fn name(value: &str) -> String {
    escape(&value.chars().take(NAME_CHARS).collect::<String>())
}

/// `HH:MM` in `zone`, or empty when `since_utc` is outside chrono's range.
fn local_hhmm(since_utc: i64, zone: Tz) -> String {
    display_time::at(since_utc, zone)
        .map(|value| value.format("%H:%M").to_string())
        .unwrap_or_default()
}

/// Largest two units, dropping seconds: `3d 4h`, `2h 5m`, or `2m`.
///
/// A day hides the minutes, an hour hides nothing below it except seconds,
/// and a span under an hour is whole minutes (zero when shorter than one).
/// No duration helper in `moon_core::util::fmt` or the report renderer speaks
/// this shape; the station uptime helper is a different phrase set.
fn human_duration(secs: i64) -> String {
    let secs = u64::try_from(secs).unwrap_or(0);
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests;
