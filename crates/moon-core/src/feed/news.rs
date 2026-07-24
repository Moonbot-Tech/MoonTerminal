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
    /// Terminal receive time, Unix ms — stamped by the store on first sight of this `meta.id` (the
    /// wire carries no such stamp). `None` until the store fills it; the reducer leaves it unset.
    pub recv_terminal_ms: Option<i64>,
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
    /// Tags attached to this item (`tags.entity[*].text`), without a leading `#`, deduped case-
    /// insensitively. Colours are assigned locally (`NewsTagSettings`), not taken from the wire.
    pub tags: Vec<String>,
    /// Whether this row is still an original (`meta.isOriginal`); once a translation frame merges in,
    /// it becomes `false` and later original frames for the same id are ignored.
    pub is_original: bool,
}

/// A moonproto-free per-core news snapshot: the logical news items plus the service tag catalog.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewsSnapshot {
    /// Logical news items in first-seen (chronological) order; the UI sorts for display.
    pub items: Vec<NewsItem>,
    /// The service-wide tag vocabulary (`tags_json`), tag labels without a leading `#`, deduped in
    /// catalog order. Distinct from each item's own `tags`: entity tags are sparse (only some news
    /// carry them), so the catalog is what fills the Tags filter with every known topic.
    pub catalog: Vec<String>,
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
            // Labels without a leading '#', deduped by case-folded key so a frame that repeats an
            // entity does not double the chip and rail; keep the first occurrence.
            let mut out: Vec<String> = Vec::new();
            for e in arr {
                let Some(text) = e.get("text").and_then(Value::as_str).map(strip_hash) else {
                    continue;
                };
                if text.is_empty() {
                    continue;
                }
                // Dedup by the same case-folded key the UI uses (`to_lowercase`), so an item can't
                // carry two rail/chip entries for one logical tag.
                let key = text.to_lowercase();
                if out.iter().any(|t| t.to_lowercase() == key) {
                    continue;
                }
                out.push(text);
            }
            out
        })
        .unwrap_or_default();
    Some(NewsItem {
        id,
        time_ms,
        recv_time_ms: as_i64(meta.get("recvTime")),
        send_time_ms: as_i64(meta.get("sendTime")),
        // Filled by the store from `news_seen_at`, not from the wire.
        recv_terminal_ms: None,
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

/// Parse the service tag catalog JSON (`{"count":N,"tags":[{"name":"#listing",...},...]}`) into the
/// tag labels, each without its leading `#`, deduped case-insensitively in catalog order. Returns an
/// empty vec for `None`/invalid input. The catalog is the full vocabulary that fills the Tags filter.
pub fn parse_catalog(json: Option<&str>) -> Vec<String> {
    let Some(json) = json else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    if let Some(arr) = v.get("tags").and_then(Value::as_array) {
        for e in arr {
            let Some(name) = e.get("name").and_then(Value::as_str) else {
                continue;
            };
            let name = strip_hash(name);
            if !name.is_empty() && !out.iter().any(|x| x.eq_ignore_ascii_case(&name)) {
                out.push(name);
            }
        }
    }
    out
}

/// Drop leading `#`(es) and surrounding whitespace from a tag label. The service delivers tags as
/// `#ElonMusk` / `#listing`; the terminal stores the bare label and re-adds the `#` for display. The
/// re-trim after stripping keeps a degenerate `"# Elon"` from carrying a leading space into its key.
fn strip_hash(s: &str) -> String {
    s.trim().trim_start_matches('#').trim().to_string()
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
