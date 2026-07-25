//! Unread-news counters for the dock tab badge: how many unseen items each tag colour carries, and
//! the badge row that shows them.
//!
//! The buckets mirror the card's left rail: an item is counted in EVERY colour it carries, so a
//! news tagged `#hack` (red) and `#listing` (green) raises both counters. The bucket total can
//! therefore exceed the number of unread items — that is the point, the number answers "how much
//! unread on this topic", not "how many rows". Items whose tags are all neutral (or that carry no
//! tags) land in the neutral bucket instead.
//!
//! Hidden topics are skipped: the tag filter is persisted and shared with the chart's news marks,
//! so a topic the user switched off must not ring on the tab either. The coin filter and the search
//! box are NOT applied — they are transient view state of one open panel, not a statement about
//! what deserves attention.

use gpui::*;
use moon_ui::{MoonPalette, h_flex};

use super::{TAG_PALETTE, key_color};
use crate::design;
use moon_core::config::NewsTagSettings;
use moon_core::feed::NewsItem;
use moon_core::feed::news_marks::{tag_visible, usable_time_ms};

/// One bucket per palette colour plus the neutral one.
pub(super) const BUCKETS: usize = TAG_PALETTE.len() + 1;

/// The bucket for items with no coloured tag; the colour buckets are indexed by [`TAG_PALETTE`].
const NEUTRAL: usize = TAG_PALETTE.len();

/// Unread counts per bucket, indexed by [`TAG_PALETTE`] with [`NEUTRAL`] last.
pub(super) type Counts = [usize; BUCKETS];

/// Resolve a persisted palette colour key to its bucket index.
fn palette_index(key: &str) -> Option<usize> {
    TAG_PALETTE.iter().position(|k| *k == key)
}

/// What one pass over the feed yields: the per-colour counts, the deduplicated item total, and the
/// newest usable time in the feed.
#[derive(Default)]
pub(super) struct Scan {
    /// Unread per colour bucket. An item with two colours raises both.
    pub(super) counts: Counts,
    /// Unread items counted once each — what a merged badge shows. Not the sum of `counts`, which
    /// deliberately double-counts a multi-coloured item.
    pub(super) total: usize,
    /// Newest usable time across ALL items, read or unread and regardless of the tag filter: it is
    /// the value the panel marks read with, so a hidden topic must not hold it back.
    ///
    /// Times can come from different clocks — an item without a publication stamp falls back to a
    /// service or terminal one — so an item published slightly before a stampless item's receipt
    /// can land under the mark and count as read. The alternative, ignoring stampless items here,
    /// would leave a feed made only of them unmarkable forever.
    pub(super) newest_ms: i64,
}

/// Scan the feed once for everything the badge needs.
///
/// Item time comes from `news_marks::usable_time_ms` — the FEED's clock, when the news existed,
/// which is also what orders the panel. The chart draws its gems at delivery instead; both reject a
/// stamp from a skewed clock the same way.
pub(super) fn scan(
    items: &[NewsItem],
    watermark: i64,
    now_ms: i64,
    settings: &NewsTagSettings,
) -> Scan {
    let mut out = Scan::default();
    for item in items {
        let Some(ms) = usable_time_ms(item, now_ms) else {
            continue;
        };
        out.newest_ms = out.newest_ms.max(ms);
        if ms <= watermark || !tag_visible(item, settings) {
            continue;
        }
        out.total += 1;
        // Per item, each bucket counts at most once: two red tags on one news are still one red.
        // A hidden topic contributes nothing even when a sibling tag keeps the item visible —
        // switching a topic off means "do not count this", not "count it under another name".
        let mut hit = [false; BUCKETS];
        for key in item.tags.iter().map(|t| t.to_lowercase()) {
            if settings.is_hidden(&key) {
                continue;
            }
            if let Some(ix) = settings
                .color(&key)
                .and_then(palette_index)
                .filter(|ix| !hit[*ix])
            {
                hit[ix] = true;
                out.counts[ix] += 1;
            }
        }
        if !hit.iter().any(|h| *h) {
            out.counts[NEUTRAL] += 1;
        }
    }
    out
}

/// The badge row for a hidden tab: one small pill per non-empty bucket in palette order, coloured
/// like the tag it counts. Returns `None` when everything is read.
///
/// `merged` is the per-panel "do not split by colour" switch: `Some(total)` collapses the row into
/// one neutral pill.
pub(super) fn badges(
    counts: Counts,
    merged: Option<usize>,
    p: MoonPalette,
    cx: &App,
) -> Option<AnyElement> {
    if let Some(total) = merged {
        return (total > 0).then(|| pill(total, p.text_muted).into_any_element());
    }
    let mut row = h_flex().items_center().gap(design::ui_px(cx, 3.0));
    let mut any = false;
    for (ix, color) in TAG_PALETTE
        .iter()
        .enumerate()
        .map(|(ix, key)| (ix, key_color(key, p)))
        .chain(std::iter::once((NEUTRAL, None)))
        .filter(|(ix, _)| counts[*ix] > 0)
    {
        any = true;
        row = row.child(pill(counts[ix], color.unwrap_or(p.text_muted)));
    }
    any.then(|| row.into_any_element())
}

/// One counter pill in the tag's colour.
///
/// Rendered through the shared News badge helper, so a tab counter and the card chips that explain
/// it read as the same object; the component clamps a long count to `99+` itself.
fn pill(n: usize, color: u32) -> impl IntoElement {
    super::render::count_badge(n, color)
}

#[cfg(test)]
mod tests;
