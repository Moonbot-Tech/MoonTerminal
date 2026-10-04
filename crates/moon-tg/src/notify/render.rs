//! Telegram HTML for one notification: a closed-trade card, or a down or back
//! line.
//!
//! Names are cut to 64 chars before [`crate::html::escape`]. The outbox refuses
//! oversized bodies rather than cutting HTML inside a tag or entity.

use crate::t;
use chrono_tz::Tz;
use moon_core::feed::order_math::MONEY_DECIMALS;
use moon_core::util::{display_time, fmt};

use super::trades::ClosedTrade;
use crate::html::{coin_tag, escape};

/// Longest coin, core, or strategy kept on a card, in Unicode scalars.
const NAME_CHARS: usize = 64;

/// One closed trade as an HTML card, in the trade's own currency, laid out as
/// the cores' own bot writes it (LinKvo, 04.10): the core first.
///
/// Line 1 is the core name and a colon. Line 2 is a sign mark, the coin as a
/// hashtag, and the signed profit. Line 3 is the entry and exit price, so the
/// card stands on its own when the entry was never announced. The last line is
/// the strategy, in italics. An unchecked card adds a line saying its
/// thresholds were not checked.
///
/// The profit prints in the trade's own currency (`+0.00012 BTC`,
/// `+3.30 USDC`) the moment the row lands, without a valuation. Outside a USD
/// stablecoin the dollar value follows once the valuation has it
/// (`≈ +7.50$`). A row without its own amount falls back to the dollars alone.
///
/// Args:
///     t: The trade to announce. No amount at all is unvalued and draws the
///         white mark plus the unvalued word. A row without both prices drops
///         their line.
///     unchecked: The rule's thresholds could not be checked.
///
/// Returns:
///     HTML using only `<b>` and `<i>`. Under a thousand chars for ordinary
///     names, and far under Telegram's 4096 UTF-16 limit even when every name
///     is 64 escaped characters.
pub(crate) fn trade_card(t: &ClosedTrade, unchecked: bool) -> String {
    let native = native_profit(t);
    let mark = match native {
        Some((_, sign)) => sign.pick("\u{1f7e2}", "\u{1f534}", "\u{26aa}"),
        None => sign_mark(t.profit_usd),
    };
    let mut card = format!(
        "{}:\n{mark} {} {}",
        name(&t.core_name),
        coin_tag(&t.coin, NAME_CHARS),
        profit_text(t, native.map(|(text, _)| text)),
    );
    if let Some(prices) = prices_text(t.buy_price, t.sell_price) {
        card.push('\n');
        card.push_str(&prices);
    }
    card.push('\n');
    card.push_str(&strategy_line(&t.strategy));
    if unchecked {
        card.push('\n');
        card.push_str(&escape(&t!("telegram.notify_unchecked")));
    }
    card
}

/// Whether a card for `t` shows a dollar value beside its own currency: not for a USD
/// stablecoin, whose amount already is one.
pub(crate) fn shows_dollars(t: &ClosedTrade) -> bool {
    !t.stable_quote()
}

/// The trade's own signed profit with its ticker, and the sign that text shows. A profit that
/// rounds to zero prints unsigned, as the dollar figure does: a `+` there would claim a gain.
fn native_profit(t: &ClosedTrade) -> Option<(String, fmt::DeltaSign)> {
    let total = moon_core::db::QuoteTotal {
        currency: t.quote?,
        profit: t.profit_native?,
        orders: 1,
    };
    let (text, sign) = total.signed_display();
    let text = match sign {
        fmt::DeltaSign::Zero => text.trim_start_matches('+').to_string(),
        _ => text,
    };
    Some((text, sign))
}

/// Significant digits a card's prices keep: one more than the terminal's
/// [`fmt::adaptive`], since an exit a tick away from its entry has to read as a
/// different price.
const PRICE_DIGITS: i32 = 6;

/// Most decimals a card's price prints: a satoshi-quoted coin at `0.00000001` still keeps six
/// significant digits.
const MAX_PRICE_DECIMALS: i32 = 16;

/// `entry → exit`, both at the same decimals: enough for [`PRICE_DIGITS`]
/// significant digits of the larger, and more while two different prices would
/// still print alike. `None` unless both are finite and positive.
///
/// The row carries no exchange tick, so the decimals follow the magnitude, and
/// trailing zeros are trimmed.
fn prices_text(buy: Option<f64>, sell: Option<f64>) -> Option<String> {
    let valid = |value: Option<f64>| value.filter(|v| v.is_finite() && *v > 0.0);
    let (buy, sell) = (valid(buy)?, valid(sell)?);
    let exp = buy.max(sell).log10().floor() as i32;
    let mut decimals = (PRICE_DIGITS - 1 - exp).clamp(0, MAX_PRICE_DECIMALS) as usize;
    while decimals < MAX_PRICE_DECIMALS as usize
        && buy != sell
        && fmt::compact(buy, decimals) == fmt::compact(sell, decimals)
    {
        decimals += 1;
    }
    Some(format!(
        "{} \u{2192} {}",
        fmt::compact(buy, decimals),
        fmt::compact(sell, decimals)
    ))
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

/// Profit and percent. The trade's own amount is bold, with the dollars after it once known
/// outside a USD stablecoin; a row without its own amount shows the dollars bold; neither is
/// the unvalued word alone, never a zero. A missing percent is marked unvalued.
///
/// Args:
///     t: The trade: dollars, percent and quote.
///     native: The trade's own signed profit with its ticker, when the row has it.
///
/// Returns:
///     The escaped unvalued word, `<b>+0.00012 BTC</b> ≈ +7.50$ (+0.40%)`,
///     `<b>+3.30 USDC</b> (+0.40%)` or `<b>+1.25$</b> (+0.40%)`.
fn profit_text(t: &ClosedTrade, native: Option<String>) -> String {
    let money = match (native, t.profit_usd) {
        (Some(native), Some(usd)) if shows_dollars(t) => {
            format!("<b>{native}</b> \u{2248} {}", signed_money(usd))
        }
        (Some(native), _) => format!("<b>{native}</b>"),
        (None, Some(usd)) => format!("<b>{}</b>", signed_money(usd)),
        (None, None) => return escape(&t!("telegram.notify_unvalued")),
    };
    let pct_text = match t
        .profit_pct
        .and_then(|value| fmt::signed_pct(value, MONEY_DECIMALS))
    {
        Some((pct, _)) => pct,
        None => escape(&t!("telegram.notify_unvalued")),
    };
    format!("{money} ({pct_text})")
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
