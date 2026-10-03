//! Telegram HTML for one notification: a closed-trade card, a down or back
//! line, and the daily summary.
//!
//! Names are cut to 64 chars before [`crate::html::escape`]. The outbox refuses
//! oversized bodies rather than cutting HTML inside a tag or entity.

use chrono::NaiveDate;
use chrono_tz::Tz;
use moon_core::feed::order_math::MONEY_DECIMALS;
use moon_core::util::{display_time, fmt};
use rust_i18n::t;

use super::daily::DaySummary;
use super::trades::ClosedTrade;
use crate::html::escape;

/// Longest coin, core, or strategy kept on a card, in Unicode scalars.
const NAME_CHARS: usize = 64;

/// One closed trade as a short HTML card.
///
/// Args:
///     t: The trade to announce. `profit_usd: None` is unvalued. `volume_usd:
///         None` drops the volume line.
///
/// Returns:
///     HTML using only `<b>`. Under a thousand chars for ordinary names, and
///     far under Telegram's 4096 UTF-16 limit even when every name is 64
///     escaped characters.
pub(crate) fn trade_card(t: &ClosedTrade) -> String {
    let mut lines = vec![
        format!("<b>{}</b> \u{00b7} {}", name(&t.coin), name(&t.core_name)),
        strategy_text(&t.strategy),
    ];
    if let Some(volume) = t.volume_usd {
        lines.push(format!(
            "{}: {}$",
            escape(&t!("telegram.notify_volume")),
            fmt::usd_grouped_cents(volume)
        ));
    }
    lines.push(format!(
        "{}: {}",
        escape(&t!("telegram.notify_profit")),
        profit_text(t.profit_usd, t.profit_pct)
    ));
    lines.push(format!(
        "{}: {}",
        escape(&t!("telegram.notify_duration")),
        human_duration(t.close_utc.saturating_sub(t.open_utc))
    ));
    lines.join("\n")
}

/// One line: the core lost its link, and the local time that outage started.
///
/// Args:
///     core_name: Configured core name. Cut to 64 chars, then escaped.
///     since_utc: UTC Unix seconds when the loss was first observed.
///     zone: Host report zone. The clock is that zone's `HH:MM`.
///
/// Returns:
///     A single plain line. An instant chrono cannot represent leaves the
///     clock empty; the phrase still names the outage.
pub(crate) fn down_line(core_name: &str, since_utc: i64, zone: Tz) -> String {
    format!(
        "{} \u{2014} {} {}",
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
///     A single plain line. The duration sits in parentheses.
pub(crate) fn back_line(core_name: &str, down_for_secs: i64) -> String {
    format!(
        "{} \u{2014} {} ({})",
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

/// Locale strategy name when the row stored none.
fn strategy_text(strategy: &str) -> String {
    if strategy.is_empty() {
        escape(&t!("telegram.notify_manual"))
    } else {
        name(strategy)
    }
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
/// never a zero. A missing percent keeps the dollars and marks the percent
/// unvalued.
fn profit_text(usd: Option<f64>, pct: Option<f64>) -> String {
    let Some(usd) = usd else {
        return escape(&t!("telegram.notify_unvalued"));
    };
    let money = signed_money(usd);
    match pct.and_then(|value| fmt::signed_pct(value, MONEY_DECIMALS)) {
        Some((pct, _)) => format!("{money} ({pct})"),
        None => format!("{money} ({})", escape(&t!("telegram.notify_unvalued"))),
    }
}

/// Grouped cents with `$`. A positive rounded amount gets `+`; zero and
/// minus stay as [`fmt::usd_grouped_cents`] prints them. The card has no
/// colour, so the sign is the only gain or loss mark.
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
