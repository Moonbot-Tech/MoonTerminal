//! Read the cores' own log files without writing them.
//!
//! Two line shapes are kept: a stop fire and a sell placement. The TSV event
//! clock is the fire time. The leading clock in the message is the core's
//! whole-second clock, kept only as a cross-check.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use chrono::NaiveDate;

#[cfg(test)]
mod tests;

/// Split one stored TSV line into the event clock and the message.
///
/// Args:
///     line: One log line, `HH:MM:SS` or `HH:MM:SS.mmm`, then level, source, and message.
///
/// Returns:
///     Milliseconds since UTC midnight and the message. The message keeps any
///     further tabs. None when the line has fewer than four fields or the clock
///     is not one of those two shapes.
pub(super) fn split_tsv(line: &str) -> Option<(i64, &str)> {
    let line = line.trim_end_matches(['\r', '\n']);
    let mut fields = line.splitn(4, '\t');
    let clock = fields.next()?;
    let _level = fields.next()?;
    let _target = fields.next()?;
    let msg = fields.next()?;
    Some((ms_of_day(clock)?, msg))
}

/// UTC day start in milliseconds, and the core label, from `YYYY-MM-DD_<label>.log`.
///
/// Args:
///     name: Bare file name, not a path.
///
/// Returns:
///     The UTC midnight of that date and the label. None for any other name,
///     for an invalid calendar day, and for the terminal log whose label is `app`.
pub(super) fn date_of_file(name: &str) -> Option<(i64, &str)> {
    let stem = name.strip_suffix(".log")?;
    let (date, label) = stem.split_once('_')?;
    if label.is_empty() || label == "app" || date.len() != 10 {
        return None;
    }
    let mut parts = date.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let midnight = NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(0, 0, 0)?;
    Some((midnight.and_utc().timestamp_millis(), label))
}

/// First `(<digits>)` after the first `[<digits>]` counter.
///
/// The spelling is [`crate::applog::task_marker`]: `(345)`.
///
/// Args:
///     msg: The log message, including its leading clock when the core wrote one.
///
/// Returns:
///     The task id. None when the counter or the parenthesized id is absent.
pub(super) fn task_of(msg: &str) -> Option<i64> {
    let after_counter = after_first_counter(msg)?;
    first_parenthesized_i64(after_counter)
}

/// Text between `(strategy <` and the next `>`.
///
/// Args:
///     msg: The log message.
///
/// Returns:
///     The strategy name, which may be empty. None when the marker is absent.
pub(super) fn strategy_of(msg: &str) -> Option<&str> {
    let start = msg.find("(strategy <")?;
    let after = &msg[start + "(strategy <".len()..];
    let end = after.find('>')?;
    Some(&after[..end])
}

/// Coin token after the leading clock, before the side tag and `: [` counter.
pub(super) fn coin_of(msg: &str) -> Option<&str> {
    let (clock, rest) = msg.split_once(char::is_whitespace)?;
    ms_of_day(clock)?;
    let prefix = rest.trim_start().split_once(": [")?.0;
    let end = prefix.find(['<', ' ', ':']).unwrap_or(prefix.len());
    let coin = &prefix[..end];
    (!coin.is_empty()).then_some(coin)
}

/// Number that follows `label`.
///
/// Spaces after the label are skipped, then one optional `=` or `:`. The number
/// may carry a sign, a decimal point, and an exponent. A label glued to a longer
/// word is not a hit, so `BID` does not read `BIDDING`.
///
/// Args:
///     msg: Text to scan.
///     label: Field label, matched by text, never by column.
///
/// Returns:
///     The first number that follows a real occurrence of `label`. None when the
///     label is missing or no number follows it.
pub(super) fn num_after(msg: &str, label: &str) -> Option<f64> {
    if label.is_empty() {
        return None;
    }
    let mut rest = msg;
    while let Some(at) = rest.find(label) {
        let after = &rest[at + label.len()..];
        if let Some(value) = number_after_label(after) {
            return Some(value);
        }
        rest = after;
    }
    None
}

/// One logged stop activation and the prices the core printed beside it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct StopFire {
    /// Event time, UTC milliseconds: the file's UTC day plus the TSV clock.
    pub(super) t_ms: i64,
    /// Label of the log file this line was read from.
    pub(super) core_label: String,
    /// Task id from `(<digits>)` after the `[<digits>]` counter.
    pub(super) task: Option<i64>,
    /// Name inside `(strategy <...>)`.
    pub(super) strategy: Option<String>,
    /// Parsed coin token, independent of any strategy text in the message.
    pub(super) coin: Option<String>,
    /// `BID =` or `BID:`.
    pub(super) bid: Option<f64>,
    /// `ASK:`.
    pub(super) ask: f64,
    /// `BidEMA:`, when the core printed it.
    pub(super) bid_ema: Option<f64>,
    /// `LastPrice =`.
    pub(super) last: f64,
    /// `BuyPrice =`.
    pub(super) buy: Option<f64>,
    /// Signed `AllowedDrop: BUY` percent.
    pub(super) allowed_drop_pct: Option<f64>,
    /// Logged spread, in percent.
    pub(super) spread_pct: Option<f64>,
    /// Trailing peak price, when present.
    pub(super) peak: Option<f64>,
    /// Floating trailing distance, in percent.
    pub(super) float_trailing_pct: Option<f64>,
    /// The message contains `Trailing`.
    pub(super) trailing: bool,
    /// `TakeProfit`, when the core printed it.
    pub(super) take_profit: Option<f64>,
    /// `StopLoss fixed:`, when the core printed it.
    pub(super) fixed: Option<f64>,
    /// Leading `HH:MM:SS` of the message, as seconds of that day.
    pub(super) msg_clock_s: Option<i64>,
}

/// Parse a stop-fire message.
///
/// The line must contain `StopLoss AutoActivated`, or both `AutoActivated` and
/// `Trailing`. Ask and last are required; a non-trailing stop also requires bid
/// and buy. Trailing lines do not carry those two readings.
///
/// Args:
///     t_ms: Event time, UTC milliseconds.
///     core_label: File label this line belongs to.
///     msg: Message field of the TSV line.
///
/// Returns:
///     The fire. None when the line is not an activation or a required price is missing.
pub(super) fn parse_stop_fire(t_ms: i64, core_label: &str, msg: &str) -> Option<StopFire> {
    if !is_stop_activation(msg) {
        return None;
    }
    let trailing = msg.contains("Trailing");
    let bid = num_after(msg, "BID");
    let buy = num_after(msg, "BuyPrice");
    if !trailing && (bid.is_none() || buy.is_none()) {
        return None;
    }
    Some(StopFire {
        t_ms,
        core_label: core_label.to_string(),
        task: task_of(msg),
        strategy: strategy_of(msg).map(str::to_string),
        coin: coin_of(msg).map(str::to_string),
        bid,
        ask: num_after(msg, "ASK")?,
        bid_ema: num_after(msg, "BidEMA:"),
        last: num_after(msg, "LastPrice =")?,
        buy,
        allowed_drop_pct: num_after(msg, "AllowedDrop: BUY"),
        spread_pct: num_after(msg, "spread:"),
        peak: num_after(msg, "PeakPrice"),
        float_trailing_pct: num_after(msg, "FloatTrailing:"),
        trailing,
        take_profit: num_after(msg, "TakeProfit"),
        fixed: num_after(msg, "StopLoss fixed:"),
        msg_clock_s: msg_clock_s(msg),
    })
}

/// One logged sell placement.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SellPlaced {
    /// Event time, UTC milliseconds.
    pub(super) t_ms: i64,
    /// Label of the log file this line was read from.
    pub(super) core_label: String,
    /// Task id from `(<digits>)` after the `[<digits>]` counter.
    pub(super) task: Option<i64>,
    /// Name inside `(strategy <...>)`.
    pub(super) strategy: Option<String>,
    /// Parsed coin token.
    pub(super) coin: Option<String>,
    /// Number after `SELL price:`.
    pub(super) sell: f64,
    /// Number after `SellPrice:`, when that label is present.
    pub(super) sell_price_field: Option<f64>,
    /// Signed percent inside the `(+N%)` or `(-N%)` that follows `SellPrice:`.
    pub(super) field_pct: Option<f64>,
    /// Strategy percent preceding `= price`, distinct from a SellAdjust field.
    pub(super) sell_pct: Option<f64>,
    /// Core explicitly used SellAdjust.
    pub(super) adjust: bool,
    /// Core explicitly used a predefined price.
    pub(super) predefined: bool,
}

/// Parse a sell-placement message.
///
/// The line must contain `SELL price:` and a number after that label.
///
/// Args:
///     t_ms: Event time, UTC milliseconds.
///     core_label: File label this line belongs to.
///     msg: Message field of the TSV line.
///
/// Returns:
///     The placement. None when the marker or its price is missing.
pub(super) fn parse_sell_placed(t_ms: i64, core_label: &str, msg: &str) -> Option<SellPlaced> {
    if !msg.contains("SELL price:") {
        return None;
    }
    let after = msg.split_once("SELL price:")?.1.trim_start();
    let first = leading_number(after)?;
    let percent_price = after.split_once('%').and_then(|(prefix, tail)| {
        (prefix.trim().parse::<f64>().ok() == Some(first))
            .then(|| tail.trim_start().strip_prefix('='))
            .flatten()
            .and_then(|tail| leading_number(tail.trim_start()))
    });
    Some(SellPlaced {
        t_ms,
        core_label: core_label.to_string(),
        task: task_of(msg),
        strategy: strategy_of(msg).map(str::to_string),
        coin: coin_of(msg).map(str::to_string),
        sell: percent_price.unwrap_or(first),
        sell_price_field: num_after(msg, "SellPrice:"),
        field_pct: pct_after_sell_price(msg),
        sell_pct: percent_price.map(|_| first),
        adjust: msg.contains("SellAdjust:"),
        predefined: msg.contains("Using predefined SELL price:"),
    })
}

/// Take branch inferred from the core's own placement, independently of the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TakeBranch {
    /// Logged strategy percent places the sell directly off the buy.
    SellPrice,
    /// Logged strategy percent does not reproduce the placed price off the buy.
    AskAdjusted,
    /// The core explicitly reports SellAdjust.
    Adjusted,
    /// Bare or predefined price supplies no branch evidence.
    Unknown,
}

/// Classify the logged branch using only its percent, buy price, side and tolerance.
/// SellAdjust wins explicitly; a bare/predefined price remains unknown. A percent
/// uses the production side-aware take formula for the diagnostic comparison.
pub(super) fn branch_of(
    placed: &SellPlaced,
    buy_price: f64,
    long: bool,
    tol_pct: f64,
) -> TakeBranch {
    if placed.adjust {
        return TakeBranch::Adjusted;
    }
    match placed.sell_pct {
        Some(pct) => {
            let level = crate::db::tuner::ticks::exit::level_off_buy(buy_price, pct, long);
            if within_pct(placed.sell, level, tol_pct) {
                TakeBranch::SellPrice
            } else {
                TakeBranch::AskAdjusted
            }
        }
        None => TakeBranch::Unknown,
    }
}

/// A stop fire or a sell placement, in event order once a file has been read.
#[derive(Debug, Clone)]
pub(super) enum CoreEvent {
    /// Stop activation.
    Stop(StopFire),
    /// Sell placement.
    Sell(SellPlaced),
}

/// Read stop fires and sell placements from `YYYY-MM-DD_<label>.log` files.
///
/// `*_app.log` and any other name are skipped. Days outside `from_day_ms..=to_day_ms`
/// are skipped. Lines are read as bytes and decoded lossily, so a byte that is not
/// UTF-8 does not stop the file. Only messages that contain `AutoActivated` or
/// `SELL price:` are parsed. Each label's events are sorted by event time.
///
/// Args:
///     logs_dir: Directory of rotated log files. This function only reads it.
///     from_day_ms: First UTC day start to keep, inclusive.
///     to_day_ms: Last UTC day start to keep, inclusive.
///
/// Returns:
///     Events keyed by file label and the number of files read. An error when the directory cannot be listed
///     or a listed file cannot be opened.
///
/// Raises:
///     `std::io::Error` from directory listing or file open.
pub(super) fn read_core_logs(
    logs_dir: &Path,
    from_day_ms: i64,
    to_day_ms: i64,
) -> std::io::Result<(BTreeMap<String, Vec<CoreEvent>>, usize)> {
    let mut out: BTreeMap<String, Vec<CoreEvent>> = BTreeMap::new();
    let mut files = 0;
    for entry in std::fs::read_dir(logs_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        let Some((day_ms, label)) = date_of_file(&file_name) else {
            continue;
        };
        if !(from_day_ms..=to_day_ms).contains(&day_ms) {
            continue;
        }
        let label = label.to_string();
        let file = std::fs::File::open(entry.path())?;
        files += 1;
        let mut reader = BufReader::new(file);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            if reader.read_until(b'\n', &mut buf)? == 0 {
                break;
            }
            if !buf
                .windows(b"AutoActivated".len())
                .any(|w| w == b"AutoActivated")
                && !buf
                    .windows(b"SELL price:".len())
                    .any(|w| w == b"SELL price:")
            {
                continue;
            }
            let line = String::from_utf8_lossy(&buf);
            let Some((ms_of_day, msg)) = split_tsv(&line) else {
                continue;
            };
            if !msg.contains("AutoActivated") && !msg.contains("SELL price:") {
                continue;
            }
            let t_ms = day_ms + ms_of_day;
            if let Some(stop) = parse_stop_fire(t_ms, &label, msg) {
                out.entry(label.clone())
                    .or_default()
                    .push(CoreEvent::Stop(stop));
            } else if let Some(sell) = parse_sell_placed(t_ms, &label, msg) {
                out.entry(label.clone())
                    .or_default()
                    .push(CoreEvent::Sell(sell));
            }
        }
    }
    for events in out.values_mut() {
        events.sort_by_key(event_ms);
    }
    Ok((out, files))
}

/// The deal identity the matcher compares with one core's events.
pub(super) struct DealKey<'a> {
    /// Report strategy. Compared only when the event also carries one.
    pub(super) strategy: Option<&'a str>,
    /// Report coin. Compared as a normalized coin token.
    pub(super) coin: &'a str,
    /// `orders_rep.taskid`, when the bench has one.
    pub(super) task: Option<i64>,
    /// Report buy time, UTC milliseconds.
    pub(super) buy_ms: i64,
    /// Report close time, UTC milliseconds.
    pub(super) close_ms: i64,
}

/// How one logged event was attached to a deal, or why it was not.
#[derive(Debug)]
pub(super) enum Match<'a, T> {
    /// Same task id inside the reuse window around the trade.
    Task(&'a T),
    /// Selected coin-and-time candidate (nearest stop or first sell).
    CoinTime(&'a T),
    /// Competing sell identities. The count is how many candidate placements.
    Ambiguous(usize),
    /// No candidate.
    None,
}

/// Attach a deal to one stop fire.
///
/// The task route runs when `key.task` is set: same task, event time in
/// `[buy_ms - 60s, close_ms + 60s]`. Several hits keep the one nearest `close_ms`.
/// The fallback compares parsed coin tokens and strategies in the same window,
/// keeping the fire nearest the close and rejecting conflicting task ids.
///
/// Args:
///     key: The deal.
///     events: One core label's events.
///
/// Returns:
///     The attachment.
pub(super) fn match_stop<'a>(key: &DealKey<'_>, events: &'a [CoreEvent]) -> Match<'a, StopFire> {
    if let Some(task) = key.task {
        let hits: Vec<&StopFire> = events
            .iter()
            .filter_map(as_stop)
            .filter(|fire| fire.task == Some(task) && in_task_window(fire.t_ms, key))
            .collect();
        if let Some(fire) = hits
            .iter()
            .copied()
            .min_by_key(|fire| fire.t_ms.abs_diff(key.close_ms))
        {
            return Match::Task(fire);
        }
    }
    let hit = events
        .iter()
        .filter_map(as_stop)
        .filter(|fire| {
            !matches!((key.task, fire.task), (Some(a), Some(b)) if a != b)
                && coin_and_strategy(key, fire.strategy.as_deref(), fire.coin.as_deref())
                && in_task_window(fire.t_ms, key)
        })
        .min_by_key(|fire| fire.t_ms.abs_diff(key.close_ms));
    hit.map_or(Match::None, Match::CoinTime)
}

/// Attach a deal to one sell placement.
///
/// The task route is the same window as [`match_stop`]. Several hits keep the
/// earliest non-adjust placement at or after `buy_ms - 5s`. The coin route uses
/// `[buy_ms - 5s, buy_ms + window_s]` and rejects competing tasks or strategies,
/// rather than treating repeated placements of the same order as ambiguous.
///
/// Args:
///     key: The deal.
///     events: One core label's events.
///     window_s: How long after the buy a coin-route sell may sit, in seconds.
///
/// Returns:
///     The attachment.
pub(super) fn match_sell<'a>(
    key: &DealKey<'_>,
    events: &'a [CoreEvent],
    window_s: i64,
) -> Match<'a, SellPlaced> {
    if let Some(task) = key.task {
        let hits: Vec<&SellPlaced> = events
            .iter()
            .filter_map(as_sell)
            .filter(|sell| {
                sell.task == Some(task)
                    && !sell.adjust
                    && sell.t_ms >= key.buy_ms.saturating_sub(5_000)
                    && in_task_window(sell.t_ms, key)
            })
            .collect();
        if competing_sells(&hits) {
            return Match::Ambiguous(hits.len());
        }
        if let Some(sell) = earliest_sell_after_buy(&hits, key.buy_ms) {
            return Match::Task(sell);
        }
    }
    let window_ms = coin_window_ms(window_s);
    let earliest = key.buy_ms.saturating_sub(5_000);
    let latest = key.buy_ms.saturating_add(window_ms);
    let hits: Vec<&SellPlaced> = events
        .iter()
        .filter_map(as_sell)
        .filter(|sell| {
            !matches!((key.task, sell.task), (Some(a), Some(b)) if a != b)
                && !sell.adjust
                && coin_and_strategy(key, sell.strategy.as_deref(), sell.coin.as_deref())
                && sell.t_ms >= earliest
                && sell.t_ms <= latest
        })
        .collect();
    if competing_sells(&hits) {
        Match::Ambiguous(hits.len())
    } else {
        earliest_sell_after_buy(&hits, key.buy_ms).map_or(Match::None, Match::CoinTime)
    }
}

/// File label of a core name, the same spelling `DatedWriter` uses.
///
/// Args:
///     name: Core name as the report stores it.
///
/// Returns:
///     [`crate::applog::sanitize_label`] of `name`.
pub(super) fn label_of_core(name: &str) -> String {
    crate::applog::sanitize_label(name)
}

/// `HH:MM:SS` or `HH:MM:SS.mmm` to milliseconds of that day.
fn ms_of_day(clock: &str) -> Option<i64> {
    let (hms, millis) = match clock.split_once('.') {
        Some((hms, frac)) => {
            if frac.len() != 3 || !frac.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            (hms, frac.parse::<i64>().ok()?)
        }
        None => (clock, 0),
    };
    let (hour, minute, second) = hms_parts(hms)?;
    Some(((hour * 60 + minute) * 60 + second) * 1000 + millis)
}

/// Two-digit `HH:MM:SS` within a real clock.
fn hms_parts(hms: &str) -> Option<(i64, i64, i64)> {
    let mut parts = hms.split(':');
    let hour_text = parts.next()?;
    let minute_text = parts.next()?;
    let second_text = parts.next()?;
    if parts.next().is_some()
        || hour_text.len() != 2
        || minute_text.len() != 2
        || second_text.len() != 2
    {
        return None;
    }
    let hour: i64 = hour_text.parse().ok()?;
    let minute: i64 = minute_text.parse().ok()?;
    let second: i64 = second_text.parse().ok()?;
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) || !(0..60).contains(&second) {
        return None;
    }
    Some((hour, minute, second))
}

/// Text after the first `[<digits>]` counter.
fn after_first_counter(msg: &str) -> Option<&str> {
    let mut rest = msg;
    loop {
        let open = rest.find('[')?;
        let tail = &rest[open + 1..];
        let close = tail.find(']')?;
        let inside = &tail[..close];
        if !inside.is_empty() && inside.bytes().all(|byte| byte.is_ascii_digit()) {
            return Some(&tail[close + 1..]);
        }
        rest = &tail[close + 1..];
    }
}

/// First `(<digits>)` in `text`.
fn first_parenthesized_i64(text: &str) -> Option<i64> {
    let mut rest = text;
    while let Some(open) = rest.find('(') {
        let tail = &rest[open + 1..];
        let close = tail.find(')')?;
        let body = &tail[..close];
        if !body.is_empty() && body.bytes().all(|byte| byte.is_ascii_digit()) {
            if let Ok(task) = body.parse() {
                return Some(task);
            }
        }
        rest = &tail[close + 1..];
    }
    None
}

/// A number after a label, rejecting a label that is only the prefix of a word.
fn number_after_label(after: &str) -> Option<f64> {
    if after
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
    {
        return None;
    }
    let trimmed = after.trim_start();
    let signed = match trimmed.chars().next() {
        Some('=' | ':') => trimmed[1..].trim_start(),
        _ => trimmed,
    };
    leading_number(signed)
}

/// Parse the number at the start of `text`.
fn leading_number(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let mut end = 0;
    if bytes[0] == b'+' || bytes[0] == b'-' {
        end = 1;
    }
    let mut saw_digit = false;
    let mut saw_dot = false;
    while end < bytes.len() {
        let byte = bytes[end];
        if byte.is_ascii_digit() {
            saw_digit = true;
            end += 1;
        } else if byte == b'.' && !saw_dot {
            saw_dot = true;
            end += 1;
        } else {
            break;
        }
    }
    if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        let mut exp = end + 1;
        if exp < bytes.len() && (bytes[exp] == b'+' || bytes[exp] == b'-') {
            exp += 1;
        }
        let exp_digits = exp;
        while exp < bytes.len() && bytes[exp].is_ascii_digit() {
            exp += 1;
        }
        if exp > exp_digits {
            end = exp;
        }
    }
    if !saw_digit {
        return None;
    }
    text[..end].parse().ok()
}

/// Leading `HH:MM:SS` of a message, as seconds of that day.
fn msg_clock_s(msg: &str) -> Option<i64> {
    let token = msg.split_whitespace().next()?;
    let hms = token.split_once('.').map(|(hms, _)| hms).unwrap_or(token);
    let (hour, minute, second) = hms_parts(hms)?;
    Some((hour * 60 + minute) * 60 + second)
}

/// `StopLoss AutoActivated`, or a trailing activation that says both words.
fn is_stop_activation(msg: &str) -> bool {
    msg.contains("StopLoss AutoActivated")
        || (msg.contains("AutoActivated") && msg.contains("Trailing"))
}

/// Signed percent inside the parenthesized percent that follows `SellPrice:`.
fn pct_after_sell_price(msg: &str) -> Option<f64> {
    let at = msg.find("SellPrice:")?;
    let mut rest = &msg[at + "SellPrice:".len()..];
    while let Some(open) = rest.find('(') {
        let tail = &rest[open + 1..];
        let close = tail.find(')')?;
        let inside = &tail[..close];
        if let Some(number) = inside.strip_suffix('%') {
            return number.trim().parse().ok();
        }
        rest = &tail[close + 1..];
    }
    None
}

/// `other` is within `tol_pct` percent of `sell`.
pub(super) fn within_pct(sell: f64, other: f64, tol_pct: f64) -> bool {
    (sell - other).abs() * 100.0 <= tol_pct * sell.abs()
}

/// Event time of either event kind.
fn event_ms(event: &CoreEvent) -> i64 {
    match event {
        CoreEvent::Stop(stop) => stop.t_ms,
        CoreEvent::Sell(sell) => sell.t_ms,
    }
}

/// Stop payload of an event.
fn as_stop(event: &CoreEvent) -> Option<&StopFire> {
    match event {
        CoreEvent::Stop(stop) => Some(stop),
        CoreEvent::Sell(_) => None,
    }
}

/// Sell payload of an event.
fn as_sell(event: &CoreEvent) -> Option<&SellPlaced> {
    match event {
        CoreEvent::Sell(sell) => Some(sell),
        CoreEvent::Stop(_) => None,
    }
}

/// Task-route window: one minute before the buy through one minute after the close.
fn in_task_window(t_ms: i64, key: &DealKey<'_>) -> bool {
    t_ms >= key.buy_ms.saturating_sub(60_000) && t_ms <= key.close_ms.saturating_add(60_000)
}

/// `window_s` as a non-negative millisecond span.
fn coin_window_ms(window_s: i64) -> i64 {
    window_s.max(0).saturating_mul(1000)
}

/// Equal normalized coin tokens, and equal strategy when both sides have one.
fn coin_and_strategy(key: &DealKey<'_>, strategy: Option<&str>, coin: Option<&str>) -> bool {
    coin.is_some_and(|coin| {
        crate::symbol::coin_match_key(coin)
            .eq_ignore_ascii_case(&crate::symbol::coin_match_key(key.coin))
    }) && strategy_agrees(key.strategy, strategy)
}

/// Strategies constrain the match only when both sides carry one.
fn strategy_agrees(deal: Option<&str>, event: Option<&str>) -> bool {
    match (deal, event) {
        (Some(deal), Some(event)) => deal == event,
        _ => true,
    }
}

/// Earliest non-adjust sell from five seconds before the buy onward.
fn earliest_sell_after_buy<'a>(hits: &[&'a SellPlaced], buy_ms: i64) -> Option<&'a SellPlaced> {
    hits.iter()
        .copied()
        .filter(|sell| !sell.adjust && sell.t_ms >= buy_ms.saturating_sub(5_000))
        .min_by_key(|sell| sell.t_ms)
}

/// Repeated placements are safe only when known task and strategy identities agree.
fn competing_sells(hits: &[&SellPlaced]) -> bool {
    let mut tasks = hits.iter().filter_map(|sell| sell.task);
    let task = tasks.next();
    let mut strategies = hits.iter().filter_map(|sell| sell.strategy.as_deref());
    let strategy = strategies.next();
    tasks.any(|other| Some(other) != task) || strategies.any(|other| Some(other) != strategy)
}
