//! Delayed trade playback with connection identity and current quiet/mute authorization.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::Backend;
use moon_core::config::trade_sounds::{TradeSounds, exchange_key};
use moon_core::feed::ConnStatus;
use moon_core::feed::ExchangeId;
use moon_core::feed::trade_sound::{TradeEdge, TradeSound};
use moon_core::session::CoreId;

/// A captured edge remains tied to the same connection even after a same-venue reconnect.
pub(crate) struct PendingTradeSound {
    core: CoreId,
    exchange: ExchangeId,
    epoch: Arc<()>,
    event: TradeSound,
}

impl PendingTradeSound {
    /// A delayed sound needs current authorization, not merely the permission it had at arrival.
    fn authorized_name(
        &self,
        epoch: Option<&Arc<()>>,
        venue: Option<ExchangeId>,
        ready: bool,
        quiet: bool,
        cfg: &TradeSounds,
    ) -> Option<String> {
        if quiet
            || !ready
            || !Arc::ptr_eq(epoch?, &self.epoch)
            || venue? != self.exchange
            || self.exchange.code != self.event.platform
        {
            return None;
        }
        let name = match self.event.edge {
            TradeEdge::Open => &cfg.open,
            TradeEdge::Close => &cfg.close,
        };
        // An empty stem is the persisted mute. A name no file answers to is NOT filtered here:
        // the player plays its default for it and reports the name, which is how a deleted
        // sound file gets noticed at all.
        (!name.trim().is_empty()).then(|| name.clone())
    }
}

/// Bounded FIFO retains both open and close notices independently from ordinary sound traffic.
#[derive(Default)]
pub(crate) struct TradePlayback {
    pending: VecDeque<PendingTradeSound>,
}

impl TradePlayback {
    /// Reauthorize all waiting entries before offering the head; a busy player keeps it queued.
    fn pump(
        &mut self,
        authorize: impl Fn(&PendingTradeSound) -> Option<String>,
        play: impl FnOnce(Option<&str>) -> bool,
    ) {
        self.pending.retain(|edge| authorize(edge).is_some());
        let name = self.pending.front().and_then(authorize);
        if play(name.as_deref()) {
            self.pending.pop_front();
        }
    }

    /// Keep accepted edges in order; overflow cannot evict an earlier open or close.
    fn push(&mut self, pending: PendingTradeSound) {
        if self.pending.len() < 256 {
            self.pending.push_back(pending);
        } else {
            log::warn!("trade sound queue full; newest edge omitted");
        }
    }
}

impl Backend {
    /// Capture every authorized feed edge; detectors and price alerts cannot spend this FIFO.
    pub(crate) fn collect_trade_sounds(&mut self) {
        for (core, exchange, event) in self.session.take_trade_sounds() {
            let Some(epoch) = self.session.trade_sound_epoch(core).cloned() else {
                continue;
            };
            let pending = PendingTradeSound {
                core,
                exchange,
                epoch,
                event,
            };
            if self.trade_sound_name(&pending).is_some() {
                self.trade_playback.push(pending);
            }
        }
    }

    /// Revalidate every queued edge, including ones behind the head, so a temporary mute or
    /// disconnect spends the backlog immediately rather than letting it reappear on unmute.
    pub(crate) fn pump_sounds(&mut self) {
        let mut playback = std::mem::take(&mut self.trade_playback);
        playback.pump(
            |edge| self.trade_sound_name(edge),
            crate::media::sound::pump,
        );
        self.trade_playback = playback;
    }

    /// Resolve current settings only for the exact live connection and captured venue/platform.
    fn trade_sound_name(&self, pending: &PendingTradeSound) -> Option<String> {
        let cfg = self
            .layout
            .trade_sounds
            .get(&exchange_key(pending.exchange))
            .cloned()
            .unwrap_or_else(TradeSounds::default);
        pending.authorized_name(
            self.session.trade_sound_epoch(pending.core),
            self.session
                .core_venues()
                .get(&pending.core)
                .map(|venue| venue.id),
            self.session
                .store()
                .core(pending.core)
                .is_some_and(|data| matches!(data.status, ConnStatus::Ready)),
            self.quiet_sleeping,
            &cfg,
        )
    }
}

#[cfg(test)]
mod tests;
