//! Formatting one news card for the clipboard, shaped for pasting into Telegram.
//!
//! Plain text, no markup: Telegram does not parse Markdown out of a paste, so anything clever would
//! arrive as literal asterisks. What DOES survive is Telegram's own linkification — `#topic` becomes
//! a hashtag and `$COIN` a cashtag — so the tickers and tags are written in those forms and stay
//! clickable on the other side.
//!
//! Layout follows how a forwarded post is read: source · absolute UTC time with the service delay
//! in brackets, then the tickers it concerns, then the body — and the topic hashtags last, under
//! the text, where they file the item without standing between the headline and what it says.

use rust_i18n::t;

use moon_core::feed::NewsItem;

/// Build the clipboard text for `item`.
///
/// `translate` picks the same body the card is showing, so what lands in the clipboard is what the
/// user was reading — not a different language they never saw.
pub(super) fn telegram_text(item: &NewsItem, translate: bool) -> String {
    // Head block: what the item is, on consecutive lines. Body and topics follow after a blank
    // line. Assembled from the parts that exist, so an item missing any of them cannot leave a
    // stray separator or a trailing newline behind.
    let head: Vec<String> = [header(item), tickers(item)]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();

    // Topics last, on the line right after the body: they file the item, they are not what it
    // says, and a wall of hashtags between the headline and the text is how a forwarded post reads
    // worst. The body is the one the card is showing — `render::body_text` owns that rule.
    let tail: Vec<String> = [
        super::render::body_text(item, translate).to_string(),
        tags(item),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect();

    match (head.is_empty(), tail.is_empty()) {
        (true, true) => String::new(),
        (false, true) => head.join("\n"),
        (true, false) => tail.join("\n"),
        (false, false) => format!("{}\n\n{}", head.join("\n"), tail.join("\n")),
    }
}

/// `SOURCE · DD.MM.YYYY HH:MM:SS UTC (+2349 мс)`, dropping any part the item lacks.
fn header(item: &NewsItem) -> String {
    let source = item.source.to_uppercase();
    let time = match (stamp(item.time_ms), delay(item)) {
        (s, _) if s.is_empty() => String::new(),
        (s, Some(d)) => format!("{s} ({d})"),
        (s, None) => s,
    };
    match (source.is_empty(), time.is_empty()) {
        (true, true) => String::new(),
        (false, true) => source,
        (true, false) => time,
        (false, false) => format!("{source} · {time}"),
    }
}

/// How long the news service held the item, as `+2349 мс`, or `None` when it cannot be measured.
///
/// The service's receive and send stamps are absolute Unix ms, like publication, so each becomes an
/// offset by subtracting the same anchor — two marks on one scale, not two legs of a journey. The
/// delay end to end is therefore the LATEST of them, never their sum. The terminal's own receipt is
/// deliberately excluded: it is stamped by this PC's clock, so pulling it into the same figure
/// would measure the gap between two machines' clocks, and for news backfilled from the core's ring
/// on connect it is the moment of connect rather than of delivery.
///
/// Saturating arithmetic, like `rel_time`: these stamps are service-controlled, and this build runs
/// with overflow checks off, so a rescaled value must clamp rather than wrap into a nonsense delay.
fn delay(item: &NewsItem) -> Option<String> {
    let anchor = (item.time_ms > 0).then_some(item.time_ms)?;
    let last = [item.recv_time_ms, item.send_time_ms]
        .into_iter()
        .flatten()
        .filter(|t| *t > 0)
        .max()?;
    let d = last.saturating_sub(anchor);
    let sign = if d >= 0 { "+" } else { "−" };
    Some(format!(
        "{sign}{} {}",
        d.saturating_abs(),
        t!("news.lat.unit")
    ))
}

/// The item's tickers as cashtags on one line: `$AERGO $BTC`.
fn tickers(item: &NewsItem) -> String {
    item.coins
        .iter()
        .map(|c| format!("${}", c.trim().to_uppercase()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The item's topics as hashtags on one line: `#Crypto #Exchange`.
///
/// The stored tags carry no `#` — `feed::news::strip_hash` removes it at parse — so this only adds
/// the one Telegram needs.
fn tags(item: &NewsItem) -> String {
    item.tags
        .iter()
        .map(|t| format!("#{t}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Publication time as `DD.MM.YYYY HH:MM:SS UTC`, or empty when the item carries no usable stamp.
///
/// Absolute, not the card's "5 min ago": a pasted message outlives the moment it was copied, and a
/// relative age would be read against the wrong clock on the other end.
fn stamp(time_ms: i64) -> String {
    if time_ms <= 0 {
        return String::new();
    }
    chrono::DateTime::from_timestamp_millis(time_ms)
        .map(|dt| dt.format("%d.%m.%Y %H:%M:%S UTC").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
