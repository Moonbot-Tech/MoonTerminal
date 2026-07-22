//! Log rebuild signature, aggregation of live core logs, line classification, and rendering of
//! one row with severity colors plus coin and search-match highlighting.
//!
//! [`super::LogPanel`] owns filtering, the virtual list, and follow-tail scrolling. Classification
//! ([`classify`]) and coin detection ([`find_coin`]) run once while `apply_filter` builds
//! [`LineView`] values instead of every frame: parsing is expensive, while the virtual list calls
//! the row renderer for every visible line on each frame.

use super::*;
use std::collections::HashSet;
use std::ops::Range;

/// Line severity, which determines the row's base text color.
///
/// Core logs have no source level and [`LogLine::core`] assigns `Info`, so their `Error` and
/// `Warn` severities are inferred from message text.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Sev {
    Error,
    Warn,
    Info,
    /// Muted `Debug` and `Trace` output.
    Dim,
    /// Noise from the GPUI fork, including bursts of `window not found` errors while windows close.
    ///
    /// These are muted so they do not obscure real errors and are excluded from errors-only mode.
    Noise,
}

/// Semantic line category, independent of severity, used for the compact category badge.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Cat {
    None,
    /// Order failure or rejection, insufficient funds, and similar messages.
    Reject,
    /// Connection loss, reconnection, or timeout.
    Conn,
}

/// Severity and category produced together from a single lowercase conversion.
#[derive(Clone, Copy)]
pub(super) struct Class {
    pub(super) sev: Sev,
    pub(super) cat: Cat,
}

/// Precomputed render-ready line containing time, source, classification, flattened text, and an
/// optional coin-token byte range into `flat`.
#[derive(Clone)]
pub(super) struct LineView {
    /// `HH:MM:SS.mmm` suffix of `ts`.
    pub(super) time: String,
    pub(super) target: String,
    pub(super) sev: Sev,
    pub(super) cat: Cat,
    /// Message with line breaks flattened for single-line rendering.
    pub(super) flat: String,
    /// Detected coin token: its byte range in `flat` for highlighting and its base for
    /// click-filtering.
    ///
    /// For example, token `USDT-SPK` yields base `SPK`.
    pub(super) coin: Option<(Range<usize>, String)>,
}

impl LineView {
    /// Build a render-ready line from an existing classification.
    ///
    /// Coin detection happens here so errors-only filtering can reject a line before scanning it.
    /// `cl` comes from [`classify`], which the caller runs before that filter to avoid classifying
    /// twice. `known` contains coin bases collected from the full buffer, allowing bare tickers
    /// such as `SPK` to be highlighted.
    pub(super) fn from_parts(line: &LogLine, cl: Class, known: &HashSet<String>) -> Self {
        let time = line
            .ts
            .rsplit(' ')
            .next()
            .unwrap_or(line.ts.as_str())
            .to_string();
        // Scan the flattened text because folding can change byte offsets.
        let flat = crate::display_text::flatten_lines(&line.msg);
        let coin = find_coin(&flat, known);
        Self {
            time,
            target: line.target.clone(),
            sev: cl.sev,
            cat: cl.cat,
            flat,
            coin,
        }
    }
}

/// Return whether a severity passes errors-only mode; GPUI noise is deliberately excluded.
pub(super) fn is_error(sev: Sev) -> bool {
    matches!(sev, Sev::Error | Sev::Warn)
}

const ERROR_KW: [&str; 8] = [
    "error",
    "panic",
    "exception",
    "critical",
    "fail",
    "reject",
    "ошиб",
    "отклон",
];
const WARN_KW: [&str; 5] = ["warn", "timeout", "таймаут", "retry", "предупре"];
const REJECT_KW: [&str; 6] = [
    "reject",
    "отклон",
    "denied",
    "insufficient",
    "недостаточно",
    "rejected",
];
const CONN_KW: [&str; 8] = [
    "disconnect",
    "reconnect",
    "connection",
    "разрыв",
    "timeout",
    "таймаут",
    "соедин",
    "подключ",
];

/// Classify severity and category using one lowercase conversion.
///
/// Explicit non-`Info` levels (`Error`, `Warn`, `Debug`, and `Trace`) take precedence over message
/// text, while recognized GPUI noise overrides everything. Every `Info` line is inferred from text;
/// this includes application/local entries and core lines, which use `Info` as a placeholder
/// because their source has no level.
pub(super) fn classify(line: &LogLine) -> Class {
    let lower = line.msg.to_lowercase();
    if lower.contains("window not found") || lower.contains("недопустимый дескриптор окна")
    {
        return Class {
            sev: Sev::Noise,
            cat: Cat::None,
        };
    }
    let cat = categorize(&lower);
    let sev = match line.level {
        log::Level::Error => Sev::Error,
        log::Level::Warn => Sev::Warn,
        log::Level::Debug | log::Level::Trace => Sev::Dim,
        log::Level::Info => {
            if ERROR_KW.iter().any(|k| lower.contains(k)) {
                Sev::Error
            } else if WARN_KW.iter().any(|k| lower.contains(k)) {
                Sev::Warn
            } else {
                Sev::Info
            }
        }
    };
    Class { sev, cat }
}

/// Categorize already-lowercased text, preferring `Reject` over `Conn` for rejection timeouts.
fn categorize(lower: &str) -> Cat {
    if REJECT_KW.iter().any(|k| lower.contains(k)) {
        Cat::Reject
    } else if CONN_KW.iter().any(|k| lower.contains(k)) {
        Cat::Conn
    } else {
        Cat::None
    }
}

const QUOTES: [&str; 6] = ["USDT", "USDC", "BUSD", "PERP", "USDe", "USD"];

fn is_tick(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-' || b == b'_'
}

/// Extract the base coin from a market-shaped token:
///
/// - spot or perpetual with a quote suffix: `SPKUSDT`, `SPK-USDT`, or `SPK_USDT` becomes `SPK`;
/// - quote prefix: `USDT-SPK` or `USDT_SPK` becomes `SPK`;
/// - dated delivery contract: `BTC_0626` or `ETH-240628` becomes `BTC` or `ETH`.
///
/// Return `None` for other shapes. At least one uppercase letter is required so a date such as
/// `2026-06-30` is not mistaken for a ticker.
fn market_base(w: &str) -> Option<String> {
    if w.len() < 4 || !w.bytes().any(|b| b.is_ascii_uppercase()) {
        return None;
    }
    for q in QUOTES {
        // Quote suffix: <BASE><QUOTE>, <BASE>-<QUOTE>, or <BASE>_<QUOTE>.
        if w.len() > q.len() && w.ends_with(q) {
            let base = w[..w.len() - q.len()].trim_end_matches(['-', '_']);
            if base.len() >= 3 && base.bytes().any(|b| b.is_ascii_uppercase()) {
                return Some(base.to_string());
            }
        }
        // Quote prefix: <QUOTE>-<BASE> or <QUOTE>_<BASE>.
        if w.len() > q.len() + 1 && w.starts_with(q) {
            let after = &w[q.len()..];
            if let Some(base) = after.strip_prefix('-').or_else(|| after.strip_prefix('_')) {
                if base.len() >= 3 && base.bytes().any(|b| b.is_ascii_uppercase()) {
                    return Some(base.to_string());
                }
            }
        }
    }
    // Delivery or quarterly contract: base prefix, separator, then a numeric suffix.
    if let Some(pos) = w.rfind(['_', '-']) {
        let (base, tail) = (&w[..pos], &w[pos + 1..]);
        let base_ok = base.len() >= 3 && base.bytes().any(|b| b.is_ascii_uppercase());
        let tail_ok = (2..=8).contains(&tail.len()) && tail.bytes().all(|b| b.is_ascii_digit());
        if base_ok && tail_ok {
            return Some(base.to_string());
        }
    }
    None
}

/// Collect coin bases that appear in market-shaped tokens anywhere in the buffer.
///
/// This allows later bare mentions to be highlighted because `SPK` alone is structurally
/// indistinguishable from words such as `BUY` or `API`.
pub(super) fn collect_coin_bases(lines: &[LogLine]) -> HashSet<String> {
    let mut set = HashSet::new();
    for l in lines {
        let bytes = l.msg.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if !is_tick(bytes[i]) {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && is_tick(bytes[i]) {
                i += 1;
            }
            if let Some(base) = market_base(&l.msg[start..i]) {
                set.insert(base);
            }
        }
    }
    set
}

/// Find the first coin in a message with a purpose-built scanner rather than a regex.
///
/// A market-shaped token takes its full byte range for highlighting and yields a base for the
/// click filter. A bare ticker is accepted only when present in `known`. Return the byte range and
/// base, or `None` when no coin is found.
pub(super) fn find_coin(msg: &str, known: &HashSet<String>) -> Option<(Range<usize>, String)> {
    let bytes = msg.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !is_tick(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_tick(bytes[i]) {
            i += 1;
        }
        let word = &msg[start..i];
        if let Some(base) = market_base(word) {
            return Some((start..i, base));
        }
        if word.len() >= 3 && known.contains(word) {
            return Some((start..i, word.to_string()));
        }
    }
    None
}

/// Build the log-rebuild signature from the application ring revision and an ordered fold of
/// `log_rev` for cores in scope.
///
/// A new local or scoped-core line changes the signature; an unchanged signature needs no rebuild.
pub(super) fn log_sig(b: &Backend, group: &str) -> u64 {
    let store = b.session.store();
    let scoped = !group.is_empty();
    let cores: u64 = b
        .session
        .sessions()
        .iter()
        .filter(|s| !scoped || s.group == group)
        .filter_map(|s| store.core(s.id))
        .fold(0u64, |a, c| a.wrapping_mul(31).wrapping_add(c.log_rev));
    applog::revision().wrapping_add(cores)
}

/// Merge live logs from all scoped core sources in chronological order.
///
/// The fixed timestamp format makes lexicographic `ts` order chronological. Retain only the newest
/// [`VIEW_LIMIT`] merged lines.
pub(super) fn aggregate(store: &CoreStore, sources: &[LogSourceItem]) -> Vec<LogLine> {
    let mut merged: Vec<LogLine> = Vec::new();
    for item in sources {
        if let LogSource::Core(id) = item.source {
            if let Some(c) = store.core(id) {
                for mut l in c.log_snapshot(AGG_PER_CORE) {
                    l.target = item.display.clone();
                    merged.push(l);
                }
            }
        }
    }
    merged.sort_by(|a, b| a.ts.cmp(&b.ts));
    if merged.len() > VIEW_LIMIT {
        let drop = merged.len() - VIEW_LIMIT;
        merged.drain(0..drop);
    }
    merged
}

/// Return the row's base text color for its severity.
fn sev_color(sev: Sev, p: MoonPalette) -> u32 {
    match sev {
        Sev::Error => p.red,
        Sev::Warn => p.amber,
        Sev::Info => p.text_soft,
        Sev::Dim | Sev::Noise => p.text_muted,
    }
}

/// Return the severity badge for `Error` or `Warn`.
fn badge(sev: Sev, p: MoonPalette) -> Option<(&'static str, u32)> {
    match sev {
        Sev::Error => Some(("ERR", p.red)),
        Sev::Warn => Some(("WARN", p.amber)),
        _ => None,
    }
}

/// Return the rejection or connection category badge.
fn cat_badge(cat: Cat, p: MoonPalette) -> Option<(&'static str, u32)> {
    match cat {
        Cat::Reject => Some(("REJ", p.orange)),
        Cat::Conn => Some(("NET", p.yellow)),
        Cat::None => None,
    }
}

/// Message segment kind used for styling; a search match takes precedence over a coin.
#[derive(Clone, Copy, PartialEq)]
enum Seg {
    Plain,
    Coin,
    Match,
}

fn seg_at(idx: usize, coin: &Option<Range<usize>>, matches: &[Range<usize>]) -> Seg {
    if matches.iter().any(|r| r.contains(&idx)) {
        Seg::Match
    } else if coin.as_ref().is_some_and(|r| r.contains(&idx)) {
        Seg::Coin
    } else {
        Seg::Plain
    }
}

/// Split a message into styled spans.
///
/// Plain text uses the severity color, a clickable coin is blue, and search matches use bold
/// accent text. An empty query with no coin takes the single-span fast path.
fn message_spans(
    flat: &str,
    base: u32,
    coin: &Option<Range<usize>>,
    query: &str,
    // Panel, coin base, and row source used by left-click filtering and right-click chart opening.
    coin_click: Option<(WeakEntity<LogPanel>, SharedString, SharedString)>,
    p: MoonPalette,
) -> Vec<AnyElement> {
    let mut matches: Vec<Range<usize>> = Vec::new();
    if !query.is_empty() {
        let lower = flat.to_lowercase();
        let mut from = 0;
        while let Some(pos) = lower[from..].find(query) {
            let s = from + pos;
            matches.push(s..s + query.len());
            from = s + query.len();
        }
    }
    if coin.is_none() && matches.is_empty() {
        return vec![
            div()
                .flex_none()
                .text_color(rgb(base))
                .child(flat.to_string())
                .into_any_element(),
        ];
    }
    let span = |text: &str, seg: Seg| -> AnyElement {
        match seg {
            Seg::Plain => div()
                .flex_none()
                .text_color(rgb(base))
                .child(text.to_string())
                .into_any_element(),
            Seg::Match => div()
                .flex_none()
                .font_bold()
                .text_color(rgb(p.accent))
                .child(text.to_string())
                .into_any_element(),
            Seg::Coin => {
                let mut d = div()
                    .flex_none()
                    .font_bold()
                    .text_color(rgb(p.blue))
                    .child(text.to_string());
                if let Some((weak, ticker, target)) = coin_click.clone() {
                    // Left-click filters the panel to this coin base.
                    d = d.cursor_pointer().on_mouse_down(MouseButton::Left, {
                        let weak = weak.clone();
                        let ticker = ticker.clone();
                        move |_ev, _w, app| {
                            if let Some(e) = weak.upgrade() {
                                let ticker = ticker.to_string();
                                e.update(app, |t, cx| t.set_coin_filter(Some(ticker), cx));
                            }
                        }
                    });
                    // Right-click resolves the coin against the row source and opens it on Main.
                    // Stop propagation so the row-level clipboard handler does not also run.
                    d = d.on_mouse_down(MouseButton::Right, move |_ev, _w, app| {
                        if let Some(e) = weak.upgrade() {
                            let (base, target) = (ticker.to_string(), target.to_string());
                            e.update(app, |t, cx| t.open_coin_chart(base, target, cx));
                        }
                        app.stop_propagation();
                    });
                }
                d.into_any_element()
            }
        }
    };
    let mut out: Vec<AnyElement> = Vec::new();
    let mut cur: Option<Seg> = None;
    let mut buf = String::new();
    for (idx, ch) in flat.char_indices() {
        let seg = seg_at(idx, coin, &matches);
        if Some(seg) != cur {
            if let Some(prev) = cur {
                out.push(span(&buf, prev));
                buf.clear();
            }
            cur = Some(seg);
        }
        buf.push(ch);
    }
    if let Some(prev) = cur {
        if !buf.is_empty() {
            out.push(span(&buf, prev));
        }
    }
    out
}

/// Render one log row as time, optional severity and category badges, source, and message.
///
/// `query` is already trimmed and lowercased for match highlighting. Right-clicking the row outside
/// a coin copies it to the clipboard; clicking a coin filters to its base, while right-clicking it
/// opens the resolved market on Main.
pub(super) fn log_row(
    v: &LineView,
    query: &str,
    weak: &WeakEntity<LogPanel>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let base = sev_color(v.sev, p);
    let mut row = h_flex()
        .w_full()
        .gap_1()
        .items_baseline()
        .text_size(crate::design::t_body(cx))
        .px_1();
    row = row.child(
        div()
            .flex_none()
            .text_color(rgb(p.text_muted))
            .child(v.time.clone()),
    );
    if let Some((tag, col)) = badge(v.sev, p) {
        row = row.child(
            div()
                .flex_none()
                .font_bold()
                .text_color(rgb(col))
                .child(tag),
        );
    }
    if let Some((tag, col)) = cat_badge(v.cat, p) {
        row = row.child(
            div()
                .flex_none()
                .font_bold()
                .text_color(rgb(col))
                .child(tag),
        );
    }
    if !v.target.is_empty() {
        row = row.child(
            div()
                .flex_none()
                .text_color(rgb(p.text_soft))
                .child(v.target.clone()),
        );
    }
    // Left-click filters by the coin base (`USDT-SPK` becomes `SPK`); right-click resolves the
    // market from the panel source and row target before opening its chart.
    let coin_range = v.coin.as_ref().map(|(r, _)| r.clone());
    let coin_click = v.coin.as_ref().map(|(_, base)| {
        (
            weak.clone(),
            SharedString::from(base.clone()),
            SharedString::from(v.target.clone()),
        )
    });
    // Build the space-separated time, optional source, and text copied on row right-click.
    let copy = if v.target.is_empty() {
        format!("{} {}", v.time, v.flat)
    } else {
        format!("{} {} {}", v.time, v.target, v.flat)
    };
    row.child(
        h_flex()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .children(message_spans(
                &v.flat,
                base,
                &coin_range,
                query,
                coin_click,
                p,
            )),
    )
    .on_mouse_down(MouseButton::Right, move |_ev, _w, app| {
        app.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
    })
    .into_any_element()
}

#[cfg(test)]
/// Tests for render-ready log-line construction.
mod tests;
