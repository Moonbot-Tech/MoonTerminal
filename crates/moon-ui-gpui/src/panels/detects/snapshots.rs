//! Background snapshot preparation and generation-checked publication for retained cards.

use std::collections::VecDeque;

use gpui::Context;
use moon_core::market::DetectSnapshot;

use super::{DETECT_THUMB_BARS, DetectItem, DetectsPanel};

#[cfg(test)]
mod tests;

impl DetectsPanel {
    /// Queue history reads only for surviving cards; replacing or pruning a card cancels its task.
    pub(super) fn start_snapshots(&mut self, cx: &mut Context<Self>) {
        let source = self.backend.read(cx).session.market_source().clone();
        for item in &mut self.items {
            if !item.snapshot_pending {
                continue;
            }
            item.snapshot_pending = false;
            self.snapshot_generation = self.snapshot_generation.wrapping_add(1);
            let generation = self.snapshot_generation;
            item.snapshot_generation = generation;
            let Some(core) = item.snapshot_core else {
                continue;
            };
            let request = source.prepare_detect_snapshot(core, &item.market, DETECT_THUMB_BARS);
            let executor = cx.background_executor().clone();
            item.snapshot_task = Some(cx.spawn(async move |this, cx| {
                let snapshot = executor.spawn(async move { request.build() }).await;
                let _ = this.update(cx, |this, cx| {
                    if publish(&mut this.items, generation, snapshot) {
                        cx.notify();
                    }
                });
            }));
        }
    }
}

/// Replace only the still-live generation; the same market may since have acquired a new card.
pub(super) fn publish(
    items: &mut VecDeque<DetectItem>,
    generation: u64,
    snapshot: DetectSnapshot,
) -> bool {
    let Some(item) = items
        .iter_mut()
        .find(|item| item.snapshot_generation == generation)
    else {
        return false;
    };
    item.bars = snapshot.bars;
    item.line = snapshot.line;
    item.delta_24h = snapshot.delta_24h;
    item.delta_1h = snapshot.delta_1h;
    item.venue = snapshot.venue;
    item.exchange_kind = snapshot.exchange_kind;
    true
}
