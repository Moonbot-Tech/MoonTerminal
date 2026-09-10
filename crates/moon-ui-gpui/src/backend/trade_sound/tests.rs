//! Playback-lane fixtures exercise retention and authorization without starting OS audio.

use super::{PendingTradeSound, TradePlayback};
use moon_core::feed::{
    ExchangeId,
    trade_sound::{TradeEdge, TradeSound},
};
use std::sync::Arc;

/// Capture venue and order identity exactly as the session-to-backend handoff does.
fn pending(core: u64, edge: TradeEdge) -> PendingTradeSound {
    PendingTradeSound {
        core,
        exchange: ExchangeId::new(core as u8),
        epoch: Arc::new(()),
        event: TradeSound {
            uid: 42,
            market: "BTCUSDT".into(),
            is_short: false,
            platform: core as u8,
            edge,
        },
    }
}

/// Distinct fixture labels expose both the exchange route and the open/close edge in FIFO order.
fn label(edge: &PendingTradeSound) -> Option<String> {
    Some(format!("{}:{:?}", edge.exchange.code, edge.event.edge))
}

/// Losing a player turn must retain every edge, including two exchanges sharing a local UID.
#[test]
fn contention_retains_open_close_and_multiple_exchanges_until_idle_turns() {
    let mut queue = TradePlayback::default();
    queue.push(pending(1, TradeEdge::Open));
    queue.push(pending(1, TradeEdge::Close));
    queue.push(pending(2, TradeEdge::Open));
    queue.pump(label, |name| {
        assert_eq!(name, Some("1:Open"));
        false
    });
    queue.pump(label, |_| false);
    let mut heard = Vec::new();
    // No further arrivals: these are scheduler turns, not feed events.
    for _ in 0..3 {
        queue.pump(label, |name| {
            heard.push(name.unwrap().to_owned());
            true
        });
    }
    assert_eq!(heard, ["1:Open", "1:Close", "2:Open"]);
    assert!(queue.pending.is_empty());
}

/// Revalidation must visit the entire backlog while busy, so muted entries cannot return later.
#[test]
fn revoked_backlog_is_spent_even_when_the_head_cannot_play() {
    let mut queue = TradePlayback::default();
    queue.push(pending(1, TradeEdge::Open));
    queue.push(pending(2, TradeEdge::Close));
    queue.pump(|edge| (edge.core == 1).then(|| "open".into()), |_| false);
    assert_eq!(queue.pending.len(), 1);
    queue.pump(label, |name| {
        assert_eq!(name, Some("1:Open"));
        true
    });
    queue.pump(label, |name| {
        assert!(name.is_none());
        false
    });
}

/// Saturation retains accepted open/close sequence instead of silently evicting older entries.
#[test]
fn trade_backlog_is_bounded_and_preserves_accepted_head() {
    let mut queue = TradePlayback::default();
    queue.push(pending(1, TradeEdge::Open));
    queue.push(pending(1, TradeEdge::Close));
    for _ in 0..1000 {
        queue.push(pending(2, TradeEdge::Open));
    }
    assert_eq!(queue.pending.len(), 256);
    queue.pump(label, |name| {
        assert_eq!(name, Some("1:Open"));
        true
    });
    queue.pump(label, |name| {
        assert_eq!(name, Some("1:Close"));
        true
    });
}

/// A reconnect to the same venue must revoke the old edge; quiet/mute and route changes do too.
#[test]
fn delayed_authorization_checks_epoch_venue_status_quiet_and_current_stem() {
    use moon_core::config::trade_sounds::TradeSounds;
    let event = pending(1, TradeEdge::Open);
    let current = event.epoch.clone();
    let replacement = Arc::new(());
    let cfg = TradeSounds {
        open: "ding1".into(),
        close: "ding2".into(),
    };
    assert_eq!(
        event
            .authorized_name(Some(&current), Some(event.exchange), true, false, &cfg)
            .as_deref(),
        Some("ding1")
    );
    assert!(
        event
            .authorized_name(Some(&replacement), Some(event.exchange), true, false, &cfg)
            .is_none()
    );
    assert!(
        event
            .authorized_name(None, Some(event.exchange), true, false, &cfg)
            .is_none()
    );
    assert!(
        event
            .authorized_name(Some(&current), Some(ExchangeId::new(2)), true, false, &cfg)
            .is_none()
    );
    assert!(
        event
            .authorized_name(Some(&current), Some(event.exchange), false, false, &cfg)
            .is_none()
    );
    assert!(
        event
            .authorized_name(Some(&current), Some(event.exchange), true, true, &cfg)
            .is_none()
    );
    let muted = TradeSounds {
        open: String::new(),
        ..cfg
    };
    assert!(
        event
            .authorized_name(Some(&current), Some(event.exchange), true, false, &muted)
            .is_none()
    );
    let changed = TradeSounds {
        open: "pfiff".into(),
        ..muted
    };
    assert_eq!(
        event
            .authorized_name(Some(&current), Some(event.exchange), true, false, &changed)
            .as_deref(),
        Some("pfiff")
    );
}
