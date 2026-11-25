//! A pane's entry-price notice (MoonBot 7.73 N2): which notice it draws now, the signature that
//! wakes the order sync when the picture steps, and the geometry it adds to the order layer.
//!
//! What the notice LOOKS like is decided once, backend-free, in `moon_chart::entry_range`; this is
//! only the glue between the session store and the order sync. A pane reads its OWN core's store
//! and its OWN market's entry in it, so a notice reaches exactly the charts of that core and
//! market — the same market on another core reads another store and never lights up.

use std::collections::HashMap;
use std::time::Instant;

use moon_chart::entry_range::{self, EntryRangePlan};
use moon_chart::figures::{FigureLabel, LabelValue};
use moon_chart::layers::{LineInstance, ZoneInstance};
use moon_core::config::ChartTheme;
use moon_core::feed::EntryPriceNotice;
use moon_core::figures::LabelPlace;

/// Absolute time far enough out that the caption's span covers any plot, so its left end is
/// clipped to the plot's own left edge — where the reference prints the requested price.
const CAPTION_SPAN_MS: f64 = 1e15;

/// The notice one pane draws at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::chartdx) struct PaneEntryRange {
    /// The store's number for the notice; a repeat of the same market is a new one.
    pub(in crate::chartdx) seq: u64,
    pub(in crate::chartdx) plan: EntryRangePlan,
    /// When this picture stops being current: the next fade step, or the expiry.
    pub(in crate::chartdx) due: Instant,
}

/// What a pane showing `market` draws from its core's notices at `now`, or `None`.
///
/// Args:
///     notices: The pane's own core's notices, by market (`CoreData::entry_price_notices`).
///     market: The pane's market.
///     now: The moment to draw.
///
/// Returns:
///     The notice and its current picture; `None` when the market has none or it expired.
pub(in crate::chartdx) fn pane_entry_range(
    notices: &HashMap<String, EntryPriceNotice>,
    market: &str,
    now: Instant,
) -> Option<PaneEntryRange> {
    let notice = notices.get(market)?;
    let c = &notice.correction;
    let elapsed = now.saturating_duration_since(notice.received_at);
    let plan = entry_range::plan(c.requested_price, c.min_price, c.max_price, elapsed)?;
    Some(PaneEntryRange {
        seq: notice.seq,
        plan,
        due: notice.received_at + plan.next_change,
    })
}

/// The pane's notice as a signature: `0` for none, otherwise one value per notice and fade step.
///
/// Two moments inside one step sign the same, so a wake that finds the step unchanged rebuilds
/// nothing; the next step, a new notice and the expiry each sign differently.
pub(in crate::chartdx) fn entry_range_sig(range: Option<&PaneEntryRange>) -> u64 {
    range.map_or(0, |r| (r.seq << 6) | (u64::from(r.plan.step) + 1))
}

/// Append a pane's notice to its order-layer buffers: the band, the line and the price caption.
///
/// The colours are the chart theme's own sign pair — the positive one for the range the exchange
/// allows, the negative one for the price it refused — so a theme or a user's colour choice
/// carries over, and no colour is spelled here.
///
/// Args:
///     range: The pane's notice.
///     theme: The chart theme.
///     visible: The pane's visible price window as `(low, high)`, where an open band side ends.
///     scale: Device pixels per logical pixel.
///     zones: The zone buffer, baked into the base cache.
///     hlines: The full-width line buffer.
///     labels: The pane's figure readouts, which the text pass formats with the axis's precision.
pub(in crate::chartdx) fn append_entry_range(
    range: &PaneEntryRange,
    theme: &ChartTheme,
    visible: (f64, f64),
    scale: f32,
    zones: &mut Vec<ZoneInstance>,
    hlines: &mut Vec<LineInstance>,
    labels: &mut Vec<FigureLabel>,
) {
    let (band, line) = entry_range::instances(
        &range.plan,
        visible,
        theme.label_positive,
        theme.label_negative,
        scale,
    );
    zones.extend(band);
    hlines.push(line);
    labels.push(FigureLabel {
        t_rel: 0.0,
        price: range.plan.requested as f32,
        place: LabelPlace::LineSpan {
            t0_ms: -CAPTION_SPAN_MS,
            t1_ms: CAPTION_SPAN_MS,
        },
        text: LabelValue::Price(range.plan.requested),
        // Not a resting figure caption: the "line labels" switch must not hide a notice.
        permanent: false,
        // Fades with the band and the line, by mixing toward the background it is printed on.
        color: entry_range::caption_rgb(&range.plan, theme.label_negative, theme.bg),
    });
}

#[cfg(test)]
mod tests;
