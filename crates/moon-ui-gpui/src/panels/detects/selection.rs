//! Select the retained card queue before resolving labels or reading any market history.

use std::collections::{HashMap, HashSet, VecDeque};

use moon_core::feed::DetectRow;
use moon_core::session::CoreId;

use super::rules::{detect_card_ttl_ms, detect_expired};
use super::{DetectItem, MAX_DETECT_BTNS};

#[cfg(test)]
mod tests;

/// One core's rows newer than `last`, cut to the NEWEST row of each market, newest first.
///
/// The first half of "what the feed takes from a pass", shared with Main's detect switch for the
/// reason [`feed_admits`] is: the cut runs BEFORE eligibility, so a market whose newest row the feed
/// rejects shows nothing — and must open nothing — even when an older row of it would have passed.
///
/// Args:
///     detects: The core's detect ring, oldest first.
///     last: Highest sequence number an earlier pass already took.
///
/// Returns:
///     The surviving rows, newest first; the first one carries the pass's new cursor.
pub(crate) fn newest_per_market<'a>(
    detects: impl DoubleEndedIterator<Item = &'a DetectRow>,
    last: u64,
) -> Vec<&'a DetectRow> {
    let mut fresh: Vec<&DetectRow> = Vec::new();
    let mut newest_of_market: HashSet<&str> = HashSet::new();
    for det in detects.rev() {
        if det.seq <= last {
            break;
        }
        // Deduplicate before eligibility, exactly as the old ingest did.
        if newest_of_market.insert(det.market.as_str()) {
            fresh.push(det);
        }
    }
    fresh
}

/// Whether the Detects feed takes one core row in, and for how long its card would stay.
///
/// The ONE statement of "this detect is in the feed", shared by the panel's ingest and by Main's
/// detect switch (`ChartTabs::ingest`), which opens the charts of the rows the feed takes in.
/// Two copies would drift, and the drift would be a chart opening for a card nobody saw.
///
/// Three gates, in the feed's own order. The sound gate stays separate from the routing gate: a
/// chart-routed detect is an ordinary detect and still needs `SoundAlert` or an alert firing.
///
/// Args:
///     det: The core's detect row.
///     show_add_to_chart: Whether this group's feed shows chart-routed detects.
///     figure_duration_s: Resolved Alerts panel duration, for a strategy-less figure alert.
///     now_ms: Wall-clock time of this pass, in Unix milliseconds.
///
/// Returns:
///     The card's lifetime in milliseconds, or `None` when the feed does not show the row.
pub(crate) fn feed_admits(
    det: &DetectRow,
    show_add_to_chart: bool,
    figure_duration_s: u32,
    now_ms: f64,
) -> Option<f64> {
    if !det.sound_alert && !det.is_alert {
        return None;
    }
    if det.add_to_chart > 0 && !show_add_to_chart {
        return None;
    }
    let ttl = detect_card_ttl_ms(det.keep_alert_secs, det.is_alert, figure_duration_s);
    (!detect_expired(now_ms, det.time_ms, ttl)).then_some(ttl)
}

/// A qualifying detection borrowed from the backend for one ingest pass.
#[derive(Clone, Copy)]
pub(super) struct Incoming<'a> {
    pub core: CoreId,
    pub name: &'a str,
    pub color: [u8; 3],
    pub row: &'a DetectRow,
    pub ttl_ms: f64,
}

/// One surviving queue position: its old card, its replacement, or both for an in-place refresh.
pub(super) struct Selected<'a> {
    pub held: Option<usize>,
    pub incoming: Option<Incoming<'a>>,
    born_ms: f64,
    order: usize,
}

/// Apply the historical replace-in-place and stable replay order, then retain the same 48 seats.
/// Keys borrow the old queue and backend rows; only survivors become cards or snapshot requests.
pub(super) fn select<'a>(
    held: &VecDeque<DetectItem>,
    incoming: impl IntoIterator<Item = Incoming<'a>>,
    replayed: bool,
) -> Vec<Selected<'a>> {
    let mut slots: Vec<_> = held
        .iter()
        .enumerate()
        .map(|(index, item)| Selected {
            held: Some(index),
            incoming: None,
            born_ms: item.born_ms,
            order: index,
        })
        .collect();
    let by_market: HashMap<_, _> = held
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            item.core()
                .map(|core| ((core, item.market.as_str()), index))
        })
        .collect();
    for next in incoming {
        let key = (next.core, next.row.market.as_str());
        if let Some(&index) = by_market.get(&key) {
            let slot = &mut slots[index];
            if slot.born_ms == next.row.time_ms {
                continue;
            }
            slot.born_ms = next.row.time_ms;
            slot.incoming = Some(next);
        } else {
            // Ingest already selected one newest row per (core, market). Only retained seats
            // can be refreshed, so the index never needs the thousands of transient newcomers.
            let order = slots.len();
            slots.push(Selected {
                held: None,
                incoming: Some(next),
                born_ms: next.row.time_ms,
                order,
            });
        }
    }
    let cut = slots.len().saturating_sub(MAX_DETECT_BTNS);
    if replayed && cut > 0 {
        slots.select_nth_unstable_by(cut, replay_order);
    }
    slots.drain(..cut);
    if replayed {
        slots.sort_by(replay_order);
    }
    slots
}

/// Match stable birth sorting by using the original queue position to resolve equal timestamps.
fn replay_order(a: &Selected<'_>, b: &Selected<'_>) -> std::cmp::Ordering {
    a.born_ms.total_cmp(&b.born_ms).then(a.order.cmp(&b.order))
}
