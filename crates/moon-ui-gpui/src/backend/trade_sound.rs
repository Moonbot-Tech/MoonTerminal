//! Delayed trade playback with connection identity and current quiet/mute authorization.

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

/// The newest trade edge waiting behind the clip on the device, in a slot of its own so that
/// ordinary sound traffic — a newer detect above all — can never displace it.
///
/// One slot, latest-wins, not a FIFO: the trades are on screen, and replaying every open and close
/// of a burst kept the terminal signalling events that were over (issue #1447). A newer edge
/// replaces a waiting one; the clip already playing is never cut.
#[derive(Default)]
pub(crate) struct TradePlayback {
    pending: Option<PendingTradeSound>,
}

impl TradePlayback {
    /// Reauthorize the waiting edge before offering it; a busy player keeps it waiting, and an
    /// edge no longer authorized is spent, so it cannot reappear on unmute.
    fn pump(
        &mut self,
        authorize: impl Fn(&PendingTradeSound) -> Option<String>,
        play: impl FnOnce(Option<&str>) -> bool,
    ) {
        let name = self.pending.as_ref().and_then(&authorize);
        if name.is_none() {
            self.pending = None;
        }
        if play(name.as_deref()) {
            self.pending = None;
        }
    }

    /// Make `pending` the waiting edge, replacing one that has not played yet.
    fn push(&mut self, pending: PendingTradeSound) {
        self.pending = Some(pending);
    }
}

impl Backend {
    /// Capture every authorized feed edge; detectors and price alerts cannot spend this slot.
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

    /// Revalidate the waiting edge on every turn, so a temporary mute or disconnect spends it
    /// immediately rather than letting it reappear on unmute.
    pub(crate) fn pump_sounds(&mut self) {
        let mut playback = std::mem::take(&mut self.trade_playback);
        playback.pump(
            |edge| self.trade_sound_name(edge),
            |trade| crate::media::sound::pump(trade, self.layout.trade_sound_volume_percent()),
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
