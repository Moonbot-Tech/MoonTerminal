//! Select the retained card queue before resolving labels or reading any market history.

use std::collections::{HashMap, VecDeque};

use moon_core::feed::DetectRow;
use moon_core::session::CoreId;

use super::{DetectItem, MAX_DETECT_BTNS};

#[cfg(test)]
mod tests;

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
