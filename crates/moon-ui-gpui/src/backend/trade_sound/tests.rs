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

/// Distinct fixture labels expose both the exchange route and the open/close edge.
fn label(edge: &PendingTradeSound) -> Option<String> {
    Some(format!("{}:{:?}", edge.exchange.code, edge.event.edge))
}

/// Losing a player turn must keep the waiting edge, and the slot holds the newest of a burst.
///
/// Rewritten for #1447: it pinned a FIFO that replayed every edge of a burst in order. The slot
/// now keeps only the newest edge, across exchanges, and still survives lost turns.
#[test]
fn contention_keeps_the_newest_edge_until_an_idle_turn() {
    let mut queue = TradePlayback::default();
    queue.push(pending(1, TradeEdge::Open));
    queue.push(pending(1, TradeEdge::Close));
    queue.push(pending(2, TradeEdge::Open));
    queue.pump(label, |name| {
        assert_eq!(name, Some("2:Open"));
        false
    });
    queue.pump(label, |_| false);
    let mut heard = Vec::new();
    // No further arrivals: these are scheduler turns, not feed events.
    for _ in 0..3 {
        queue.pump(label, |name| {
            if let Some(name) = name {
                heard.push(name.to_owned());
            }
            name.is_some()
        });
    }
    assert_eq!(heard, ["2:Open"]);
    assert!(queue.pending.is_none());
}

/// Revalidation runs while the player is busy, so a muted edge cannot return later.
///
/// Rewritten for #1447: with one slot there is no backlog behind a head, so the busy turn itself
/// has to spend the revoked edge.
#[test]
fn revoked_edge_is_spent_even_when_the_player_is_busy() {
    let mut queue = TradePlayback::default();
    queue.push(pending(1, TradeEdge::Open));
    queue.pump(|edge| (edge.core == 1).then(|| "open".into()), |_| false);
    assert!(
        queue.pending.is_some(),
        "an authorized edge outlives a lost turn"
    );
    queue.push(pending(2, TradeEdge::Close));
    queue.pump(|edge| (edge.core == 1).then(|| "open".into()), |_| false);
    assert!(
        queue.pending.is_none(),
        "the revoked edge is spent while busy"
    );
    queue.pump(label, |name| {
        assert!(name.is_none());
        false
    });
}

/// A burst of edges waits as its newest one only, whatever came before it.
///
/// Rewritten for #1447: it pinned a 256-edge FIFO whose overflow dropped the NEWEST edge.
///
/// Plausible breakage: a FIFO again — the burst's opens and closes replay one by one long after
/// the trades are on screen.
#[test]
fn a_trade_burst_waits_as_its_newest_edge_only() {
    let mut queue = TradePlayback::default();
    queue.push(pending(1, TradeEdge::Open));
    for _ in 0..1000 {
        queue.push(pending(2, TradeEdge::Open));
    }
    queue.push(pending(1, TradeEdge::Close));
    queue.pump(label, |name| {
        assert_eq!(name, Some("1:Close"));
        true
    });
    queue.pump(label, |name| {
        assert!(name.is_none(), "nothing of the burst replays");
        false
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
    // A name no file answers to still travels to the player: it plays the default for it and
    // reports the name. Muting it here would hide a deleted file behind silence.
    let gone = TradeSounds {
        open: "deleted_file".into(),
        ..changed
    };
    assert_eq!(
        event
            .authorized_name(Some(&current), Some(event.exchange), true, false, &gone)
            .as_deref(),
        Some("deleted_file")
    );
}
