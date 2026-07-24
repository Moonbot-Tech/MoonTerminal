//! News feed: parse the news-service JSON frames retained by moonproto into typed logical items.
//!
//! moonproto hands us decoded UTF-8 JSON strings (its `NewsState::items()`), one per wire frame,
//! preserving the core ring's multiplicity and same-`meta.id` revisions. This module turns that flat
//! frame list into the terminal's LOGICAL news list: one row per `meta.id`, with asynchronous
//! translations merged into the row created by the first frame. The transform is moonproto-free and
//! lives here (not in the UI) so it is unit-testable and shared across every panel/window.
//!
//! Schema note: the news JSON is service-owned and can grow, so we deserialize into `serde_json::Value`
//! and read only the fields the terminal uses, ignoring unknown fields (forward-compatible). Field
//! set confirmed against the MoonBot core: `meta.{id,timeMs,time,recvTime,sendTime,isOriginal,source,
//! author,coinsAuto,coinsSelf}`, `news.{en,ru,es}`, `tags.entity[*].text`.

use serde_json::Value;

/// One logical news item: the row shown to the user, built from one or more wire frames sharing a
/// `meta.id`. Moonproto-free.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewsItem {
    /// News identity (`meta.id`), opaque and stable; the reducer and cross-core dedup key on it.
    pub id: String,
    /// Publication time, Unix ms (`meta.timeMs`, or `meta.time` seconds ×1000). `0` if absent.
    pub time_ms: i64,
    /// News-service receive time, Unix ms (`meta.recvTime`), or `None`.
    pub recv_time_ms: Option<i64>,
    /// News-service send time, Unix ms (`meta.sendTime`), or `None`.
    pub send_time_ms: Option<i64>,
    /// Source code, e.g. `toa` / `nm` (`meta.source`).
    pub source: String,
    /// Optional author text (`meta.author`).
    pub author: Option<String>,
    /// De-duplicated ticker list, `meta.coinsAuto` then `meta.coinsSelf`, first-seen order.
    pub coins: Vec<String>,
    /// English body (`news.en`), fragments normalized and joined. The fallback for missing translations.
    pub en: String,
    /// Russian body (`news.ru`), possibly empty until a translation frame arrives.
    pub ru: String,
    /// Spanish body (`news.es`), possibly empty until a translation frame arrives.
    pub es: String,
    /// Tags attached to this item (`tags.entity[*].text`).
    pub tags: Vec<String>,
    /// Whether this row is still an original (`meta.isOriginal`); once a translation frame merges in,
    /// it becomes `false` and later original frames for the same id are ignored.
    pub is_original: bool,
}

/// A moonproto-free per-core news snapshot: the logical news items.
///
/// The service-wide tags CATALOG (`tags_json`, distinct from each item's own `tags`) is intentionally
/// not carried yet — it only feeds the tag filter/color surface, which is a later phase, so projecting
/// it now would be dead state that also churns `news_rev` on a tags-only relay.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewsSnapshot {
    /// Logical news items in first-seen (chronological) order; the UI sorts for display.
    pub items: Vec<NewsItem>,
}

impl NewsItem {
    /// Merge a later frame with the same `meta.id` into this original row: replace the translated
    /// texts, combined ticker list, and tags; keep identity, times, source, and author from the first
    /// accepted frame. Sets `is_original` from the incoming frame so an accepted translation locks out
    /// a late original (doc rule 4).
    fn merge_from(&mut self, other: NewsItem) {
        if !other.en.is_empty() {
            self.en = other.en;
        }
        if !other.ru.is_empty() {
            self.ru = other.ru;
        }
        if !other.es.is_empty() {
            self.es = other.es;
        }
        if !other.coins.is_empty() {
            self.coins = other.coins;
        }
        if !other.tags.is_empty() {
            self.tags = other.tags;
        }
        self.is_original = other.is_original;
    }
}

/// Build the logical news list from moonproto's flat frame strings (oldest to newest).
///
/// One row per first-seen `meta.id`. A later frame for an id whose row is still original merges its
/// translation/clarification; a later frame for an id whose row is already a translation is ignored.
/// Invalid frames (bad JSON, missing `meta`/`id`) are skipped. Order is first-seen.
pub fn reduce<'a>(frames: impl Iterator<Item = &'a str>) -> Vec<NewsItem> {
    // The ring is capped at 50 frames, so a linear id lookup keeps this in one Vec with no parallel
    // order/map bookkeeping and no measurable cost.
    let mut out: Vec<NewsItem> = Vec::new();
    for json in frames {
        let Some(item) = parse_frame(json) else {
            continue;
        };
        match out.iter_mut().find(|existing| existing.id == item.id) {
            None => out.push(item),
            // Only an original row accepts a merge; an already-accepted translation is kept.
            Some(existing) if existing.is_original => existing.merge_from(item),
            Some(_) => {}
        }
    }
    out
}

/// Parse one news-service frame JSON into a [`NewsItem`], or `None` if it is not a valid news document
/// (bad JSON, or a missing/empty `meta.id`).
pub fn parse_frame(json: &str) -> Option<NewsItem> {
    let v: Value = serde_json::from_str(json).ok()?;
    let meta = v.get("meta")?;
    let id = id_string(meta.get("id")?)?;
    let news = v.get("news");
    let time_ms = as_i64(meta.get("timeMs"))
        .or_else(|| as_i64(meta.get("time")).map(|s| s.saturating_mul(1000)))
        .unwrap_or(0);
    let mut coins = Vec::new();
    for c in string_array(meta.get("coinsAuto"))
        .into_iter()
        .chain(string_array(meta.get("coinsSelf")))
    {
        if !coins.contains(&c) {
            coins.push(c);
        }
    }
    let tags = v
        .get("tags")
        .and_then(|t| t.get("entity"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("text").and_then(Value::as_str))
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some(NewsItem {
        id,
        time_ms,
        recv_time_ms: as_i64(meta.get("recvTime")),
        send_time_ms: as_i64(meta.get("sendTime")),
        source: meta
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        author: meta
            .get("author")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        coins,
        en: join_fragments(news.and_then(|n| n.get("en"))),
        ru: join_fragments(news.and_then(|n| n.get("ru"))),
        es: join_fragments(news.and_then(|n| n.get("es"))),
        tags,
        is_original: meta
            .get("isOriginal")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    })
}

/// Read `meta.id`, which the service may encode as a string or a number, into a non-empty `String`.
fn id_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => (!s.is_empty()).then(|| s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Read an integer field that may arrive as an integer or a float.
fn as_i64(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

/// Collect a JSON array of strings, dropping non-strings and empties.
fn string_array(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Join a `news.<lang>` fragment array into one string: normalize escaped newlines to spaces, trim
/// each fragment, and join the non-empty ones with single spaces.
fn join_fragments(v: Option<&Value>) -> String {
    let Some(Value::Array(arr)) = v else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    for el in arr {
        if let Some(s) = el.as_str() {
            let cleaned = s.replace("\r\n", " ").replace(['\n', '\r'], " ");
            let trimmed = cleaned.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests;
