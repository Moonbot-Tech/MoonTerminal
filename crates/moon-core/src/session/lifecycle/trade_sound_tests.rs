//! Real FeedTx-to-session drain tests with inert channels and no production client.

use super::{CoreSession, CoreStore, SessionManager};
use crate::feed::{
    ConnStatus, ExchangeId, FeedMsg, FeedTx,
    trade_sound::{TradeEdge, TradeSound},
};
use crate::market::{MarketDataMode, MarketDataSource, MarketStore};
use std::collections::HashMap;
use std::sync::Arc;

/// Construct only the account and routing state needed by the production drain.
fn fixture() -> (SessionManager, FeedTx) {
    let market = MarketStore::shared(0.0);
    let (tx, handle) = crate::feed::trade_sound_test_feed();
    let mut store = CoreStore::default();
    store.ensure(1);
    let session = SessionManager {
        trade_sounds: Vec::new(),
        trade_sound_epochs: HashMap::new(),
        sessions: vec![CoreSession {
            id: 1,
            name: "fixture".into(),
            group: String::new(),
            conn_sig: 0,
            handle,
        }],
        config_order: vec![1],
        feed_wake: None,
        store,
        market_source: MarketDataSource::new(market.clone()),
        market,
        mode: MarketDataMode::default(),
        core_venue: HashMap::new(),
        core_base: HashMap::new(),
        core_provider: HashMap::new(),
        providers: HashMap::new(),
        wanted: HashMap::new(),
        pending_drop: HashMap::new(),
        wanted_orderbook: HashMap::new(),
        pending_ob_drop: HashMap::new(),
        last_cmd: HashMap::new(),
        core_updates: Default::default(),
    };
    (session, tx)
}

/// Same venue identity is intentional: a reconnect cannot be detected by venue comparison alone.
fn connect(tx: &FeedTx) {
    tx.send(FeedMsg::Identity {
        id: ExchangeId::new(1),
        dex: String::new(),
        reported: String::new(),
    })
    .unwrap();
    tx.send(FeedMsg::Status(ConnStatus::Ready)).unwrap();
}

/// Feed-owned edges carry the platform independently from the session venue.
fn edge(kind: TradeEdge) -> TradeSound {
    TradeSound {
        uid: 8,
        market: "BTCUSDT".into(),
        is_short: false,
        platform: 1,
        edge: kind,
    }
}

/// The real drain must retain both captured edges, then revoke their token on same-venue reconnect.
#[test]
fn feed_drain_preserves_pair_and_invalidates_delayed_playback_on_reconnect() {
    let (mut session, tx) = fixture();
    connect(&tx);
    tx.send(FeedMsg::TradeSounds(vec![
        edge(TradeEdge::Open),
        edge(TradeEdge::Close),
    ]))
    .unwrap();
    assert!(session.drain().any);
    let pending_epoch = session.trade_sound_epoch(1).unwrap().clone();
    let sounds = session.take_trade_sounds();
    assert_eq!(
        sounds
            .iter()
            .map(|(core, venue, event)| (*core, venue.code, event.edge))
            .collect::<Vec<_>>(),
        vec![(1, 1, TradeEdge::Open), (1, 1, TradeEdge::Close)]
    );
    assert!(session.take_trade_sounds().is_empty());
    tx.send(FeedMsg::Status(ConnStatus::Disconnected)).unwrap();
    connect(&tx);
    session.drain();
    assert!(!Arc::ptr_eq(
        &pending_epoch,
        session.trade_sound_epoch(1).unwrap()
    ));
    session.clear_core_coordination(1);
    assert!(session.trade_sound_epoch(1).is_none());
}

/// Connection boundaries inside one channel drain must discard only old-generation notices.
#[test]
fn mixed_status_trade_drain_keeps_only_new_connection_edges() {
    let (mut session, tx) = fixture();
    connect(&tx);
    tx.send(FeedMsg::TradeSounds(vec![edge(TradeEdge::Open)]))
        .unwrap();
    tx.send(FeedMsg::Status(ConnStatus::Disconnected)).unwrap();
    connect(&tx);
    tx.send(FeedMsg::TradeSounds(vec![edge(TradeEdge::Close)]))
        .unwrap();
    session.drain();
    let sounds = session.take_trade_sounds();
    assert_eq!(sounds.len(), 1);
    assert_eq!(sounds[0].2.edge, TradeEdge::Close);
}
