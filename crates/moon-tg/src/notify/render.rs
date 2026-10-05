//! Telegram HTML for one notification: a closed-trade card, or a down or back
//! line.
//!
//! Names are cut to 64 chars before [`crate::html::escape`]. The outbox refuses
//! oversized bodies rather than cutting HTML inside a tag or entity.

use crate::t;
use chrono_tz::Tz;
use moon_core::config::telegram_layout::{CardField, CardLayout};
use moon_core::feed::order_math::MONEY_DECIMALS;
use moon_core::util::{display_time, fmt};

use super::trades::ClosedTrade;
use crate::html::{TAG_NAME_CHARS, escape, name_tag};

/// Longest coin, core, or strategy kept on a card, in Unicode scalars.
const NAME_CHARS: usize = 64;

/// One closed trade as layout-ordered HTML lines, in the trade's own currency.
/// Capture sites sanitize layouts once. Missing segments and unfamiliar fields are omitted;
/// a mark joins the next drawable segment with a space. A layout with no drawable segment
/// falls back to the default card, which preserves the original format.
///
/// The profit and the volume print in the trade's own currency (`+0.00012 BTC`,
/// `+3.30$` for a USD stablecoin) the moment the row lands, without a valuation. Outside a USD
/// stablecoin the dollar value follows once the valuation has it
/// (`≈ +7.50$`). A row without its own amount falls back to the dollars alone.
///
/// Args:
///     t: The trade to announce. No amount at all is unvalued and draws the
///         white mark plus the unvalued word. No volume drops its segment.
///     unchecked: The rule's thresholds could not be checked.
///     layout: Saved line order and independent coin/core hashtag switches.
///
/// Returns:
///     HTML using only `<b>` and `<i>`. Under a thousand chars for ordinary
///     names, and far under Telegram's 4096 UTF-16 limit even when every name
///     is 64 escaped characters.
pub(crate) fn trade_card(t: &ClosedTrade, unchecked: bool, layout: &CardLayout) -> String {
    card_lines(t, unchecked, layout)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(crate::preview::Span::html)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Resolve card fields and separators for HTML and previews, retaining hashtags on fallback.
pub(crate) fn card_lines(
    t: &ClosedTrade,
    unchecked: bool,
    layout: &CardLayout,
) -> Vec<crate::preview::PreviewLine> {
    use crate::preview::{PreviewLine, Span};
    let native = native_profit(t);
    let mark = match native {
        Some((_, sign)) => sign.pick("\u{1f7e2}", "\u{1f534}", "\u{26aa}"),
        None => sign_mark(t.profit_usd),
    };
    let mut lines = Vec::new();
    for (index, fields) in layout.lines.iter().enumerate() {
        let mut spans = Vec::new();
        let mut after_mark = false;
        for field in fields {
            let mut segment = match field {
                CardField::Mark => vec![Span::plain(mark)],
                CardField::Coin => {
                    let mut span = card_name_span(&t.coin, layout.coin_hashtag);
                    span.bold = true;
                    vec![span]
                }
                CardField::Profit => profit_spans(t, native.clone()),
                CardField::Volume => volume_text(t).map(Span::plain).into_iter().collect(),
                CardField::Duration => vec![Span::plain(human_duration(
                    t.close_utc.saturating_sub(t.open_utc),
                ))],
                CardField::Core => {
                    let mut span = card_name_span(&t.core_name, layout.core_hashtag);
                    if index == 0 && fields == &[CardField::Core] {
                        span.text = t!("telegram.notify_core_named", name = span.text).to_string();
                    }
                    vec![span]
                }
                CardField::Strategy => {
                    let mut span = Span::plain(strategy_text(&t.strategy));
                    span.italic = true;
                    vec![span]
                }
                CardField::Prices => t
                    .buy_price
                    .zip(t.sell_price)
                    .map(|(entry, exit)| {
                        Span::plain(format!(
                            "{} \u{2192} {}",
                            fmt::compact(entry, 8),
                            fmt::compact(exit, 8)
                        ))
                    })
                    .into_iter()
                    .collect(),
                CardField::Other(_) => Vec::new(),
            };
            segment.retain(|span| !span.text.is_empty() || span.bold || span.italic);
            if segment.is_empty() {
                continue;
            }
            if !spans.is_empty() {
                spans.push(Span::plain(if after_mark { " " } else { " \u{00b7} " }));
            }
            spans.append(&mut segment);
            after_mark = *field == CardField::Mark;
        }
        if !spans.is_empty() {
            lines.push(PreviewLine { spans });
        }
    }
    if lines.is_empty() {
        return card_lines(
            t,
            unchecked,
            &CardLayout {
                coin_hashtag: layout.coin_hashtag,
                core_hashtag: layout.core_hashtag,
                ..CardLayout::default()
            },
        );
    }
    if unchecked {
        lines.push(PreviewLine {
            spans: vec![Span::plain(t!("telegram.notify_unchecked").to_string())],
        });
    }
    lines
}

/// Reuse the shared hashtag mapper; decode only its four escaped name entities, never message HTML.
fn card_name_span(value: &str, hashtag: bool) -> crate::preview::Span {
    use crate::preview::{Span, Tone};
    let text = if hashtag {
        name_tag(value, TAG_NAME_CHARS, false)
            .replace("&quot;", "\"")
            .replace("&gt;", ">")
            .replace("&lt;", "<")
            .replace("&amp;", "&")
    } else {
        bounded_name(value)
    };
    let link = hashtag && text.starts_with('#') && text.chars().any(char::is_alphabetic);
    Span {
        text,
        tone: if link { Tone::Link } else { Tone::Plain },
        ..Span::plain("")
    }
}

/// Whether a card for `t` shows a dollar value beside its own currency: not for a USD
/// stablecoin, whose amount already is one.
pub(crate) fn shows_dollars(t: &ClosedTrade) -> bool {
    !t.stable_quote()
}

/// The trade's own signed profit with `$` for a USD stablecoin or its ticker otherwise, and
/// the sign that text shows. Rounded zero stays unsigned so it does not claim a gain.
fn native_profit(t: &ClosedTrade) -> Option<(String, fmt::DeltaSign)> {
    let total = moon_core::db::QuoteTotal {
        currency: t.quote?,
        profit: t.profit_native?,
        orders: 1,
    };
    let (text, sign) = total.signed_display();
    if t.stable_quote() {
        return Some((signed_money(total.profit), sign));
    }
    let text = match sign {
        fmt::DeltaSign::Zero => text.trim_start_matches('+').to_string(),
        _ => text,
    };
    Some((text, sign))
}

/// Entry volume: in the trade's own currency, grouped whole units for a stablecoin (cents under
/// one unit) and the currency's places otherwise; else the valued whole dollars; else nothing.
fn volume_text(t: &ClosedTrade) -> Option<String> {
    match (t.quote, t.volume_native) {
        (Some(quote), Some(volume)) => native_volume_text(quote, volume),
        _ => whole_dollar_volume(t.volume_usd),
    }
}

/// Native volume shared by cards and reports: whole stablecoin units with `$`, cents below one,
/// and the quote currency's own decimal precision and ticker otherwise.
pub(crate) fn native_volume_text(
    quote: moon_core::db::QuoteCurrency,
    volume: f64,
) -> Option<String> {
    let stable = moon_core::symbol::is_usd_stable(quote.ticker());
    let amount = if stable && volume < 1.0 {
        fmt::compact(volume, 2)
    } else if stable {
        let rounded = fmt::round_to(volume, 0)?;
        fmt::group_thousands(&format!("{rounded:.0}"))
    } else {
        fmt::compact(volume, quote.display_decimals())
    };
    Some(if stable {
        format!("{amount}$")
    } else {
        format!("{amount} {}", quote.ticker())
    })
}

/// One line: the core lost its link, and the local time that outage started.
///
/// Args:
///     core_name: Configured core name, mapped to the card's hashtag or escaped fallback.
///     since_utc: UTC Unix seconds when the loss was first observed.
///     zone: Host report zone. The clock is that zone's `HH:MM`.
///
/// Returns:
///     A single plain line with a leading loss mark. An instant chrono cannot
///     represent leaves the clock empty; the phrase still names the outage.
pub(crate) fn down_line(core_name: &str, since_utc: i64, zone: Tz) -> String {
    format!(
        "\u{1f534} {} \u{2014} {} {}",
        name_tag(core_name, TAG_NAME_CHARS, false),
        escape(&t!("telegram.notify_down")),
        local_hhmm(since_utc, zone)
    )
}

/// One line: the core is back, with how long the announced outage lasted.
///
/// Args:
///     core_name: Configured core name, mapped to the card's hashtag or escaped fallback.
///     down_for_secs: Seconds from the recorded loss to now. Negative becomes
///         zero minutes.
///
/// Returns:
///     A single plain line with a leading gain mark. The duration sits in
///     parentheses.
pub(crate) fn back_line(core_name: &str, down_for_secs: i64) -> String {
    format!(
        "\u{1f7e2} {} \u{2014} {} ({})",
        name_tag(core_name, TAG_NAME_CHARS, false),
        escape(&t!("telegram.notify_back")),
        human_duration(down_for_secs)
    )
}

/// Raw strategy text shared by italic card spans and their production HTML encoding.
/// Stored source and strategy names retain their separate 64-scalar caps.
fn strategy_text(strategy: &str) -> String {
    if strategy.is_empty() {
        return t!("telegram.notify_manual").to_string();
    }
    match parse_stored_strategy(strategy) {
        Some((source, strategy_name)) => {
            let shown = bounded_name(&strategy_name);
            if source.is_empty() {
                shown
            } else {
                format!("{} \u{00b7} {shown}", bounded_name(&source))
            }
        }
        None => bounded_name(strategy),
    }
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
pub(crate) fn whole_dollar_volume(value: Option<f64>) -> Option<String> {
    let rounded = fmt::round_to(value?, 0)?;
    let digits = format!("{rounded:.0}");
    Some(format!("{}$", fmt::group_thousands(&digits)))
}

/// Profit and percent as typed runs, preserving native-currency and valuation formatting.
fn profit_spans(
    t: &ClosedTrade,
    native: Option<(String, fmt::DeltaSign)>,
) -> Vec<crate::preview::Span> {
    use crate::preview::{Span, Tone};
    let mut spans = Vec::new();
    let money = match (native, t.profit_usd) {
        (Some((text, sign)), usd) => {
            let span = Span {
                text,
                bold: true,
                tone: sign.pick(Tone::Gain, Tone::Loss, Tone::Plain),
                ..Span::plain("")
            };
            spans.push(span);
            usd.filter(|_| shows_dollars(t))
        }
        (None, Some(usd)) => Some(usd),
        (None, None) => return vec![Span::plain(t!("telegram.notify_unvalued").to_string())],
    };
    if let Some(usd) = money {
        let text = signed_money(usd);
        let tone = fmt::signed_fixed(usd, 2).map_or(Tone::Plain, |(_, sign)| {
            sign.pick(Tone::Gain, Tone::Loss, Tone::Plain)
        });
        if spans.is_empty() {
            spans.push(Span {
                text,
                bold: true,
                tone,
                ..Span::plain("")
            });
        } else {
            spans.push(Span {
                text: format!(" \u{2248} {text}"),
                tone,
                ..Span::plain("")
            });
        }
    }
    let pct = t
        .profit_pct
        .and_then(|value| fmt::signed_pct(value, MONEY_DECIMALS))
        .map(|(text, _)| text)
        .unwrap_or_else(|| t!("telegram.notify_unvalued").to_string());
    spans.push(Span::plain(format!(" ({pct})")));
    spans
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

/// Bound raw user text before its presentation-specific encoding.
fn bounded_name(value: &str) -> String {
    value.chars().take(NAME_CHARS).collect()
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
