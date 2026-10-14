//! Regression coverage for unavailable versus authoritative empty path-prefix replies.

use super::*;
use crate::market::MarketStore;
use crate::market::kline_cache::{KlineCache, Op, PrefixRows};

/// An exchange address can resolve or change without provider election; retaining the old
/// ranges would show prices belonging to a different exchange in the current candle.
#[test]
fn open_candle_path_exchange_change_drops_old_ranges() {
    let source = MarketDataSource::new(MarketStore::shared(0.0));
    let done = Arc::new(AtomicU64::new(0));
    let mut state = OpenPathHistory::default();
    assert!(state.sync(
        &source,
        (7, "SYNTH"),
        86_400_000,
        86_460_001,
        false,
        Arc::clone(&done)
    ));
    state.rows.minutes = Some(vec![ChartCandle {
        t_open_ms: 86_400_000.0,
        low: 10.0,
        high: 12.0,
        ..Default::default()
    }]);
    source
        .inner
        .write()
        .unwrap()
        .provider_exchange
        .insert(7, crate::feed::ExchangeId::new(1));
    assert!(state.sync(&source, (7, "SYNTH"), 86_400_000, 86_460_002, false, done));
    assert!(state.rows.minutes.is_none());
    assert!(state.rows.five.is_empty());
}

/// A successful empty reply must settle the minute, while a pending reply must remain unknown.
/// Treating pending as empty used to make a temporary cache stall permanently blank history.
#[test]
fn open_candle_path_pending_and_empty_are_distinct() {
    let source = MarketDataSource::new(MarketStore::shared(0.0));
    let done = Arc::new(AtomicU64::new(0));
    let (cache, queue) = KlineCache::stalled_for_tests();
    let pending = cache
        .request_prefix(PrefixReadRequest {
            exchange: "synthetic".into(),
            market: "SYNTH".into(),
            native_kind: 1,
            from_ms: 86_400_000,
            now_ms: 86_460_000,
            want_5m: true,
            want_1d: false,
            done: Arc::clone(&done),
        })
        .unwrap();
    let mut state = OpenPathHistory {
        key: Some(PathReadKey {
            provider: 7,
            exchange: None,
            market: "SYNTH".into(),
            tf_ms: 86_400_000,
            open_ms: 86_400_000,
        }),
        pending: Some(pending),
        requested_minute: Some(1441),
        ..Default::default()
    };
    assert!(!state.sync(
        &source,
        (7, "SYNTH"),
        86_400_000,
        86_460_001,
        false,
        Arc::clone(&done)
    ));
    assert!(state.rows.minutes.is_none());
    let Op::ReadPrefix { reply, .. } = queue.recv().unwrap() else {
        panic!("expected a prefix request")
    };
    reply
        .send(PrefixRows {
            native: vec![],
            finer: vec![],
            m5: vec![],
            d1: vec![],
        })
        .unwrap();
    assert!(state.sync(
        &source,
        (7, "SYNTH"),
        86_400_000,
        86_460_001,
        false,
        Arc::clone(&done)
    ));
    assert_eq!(state.rows.minutes, Some(vec![]));
    assert_eq!(state.frontier, Some(86_460_000));
    assert!(!state.sync(&source, (7, "SYNTH"), 86_400_000, 86_460_002, false, done));
    assert!(
        queue.try_recv().is_err(),
        "an empty result must not queue a per-frame retry"
    );
}

/// Losing a reply must not advance the frontier or turn unknown history into authoritative empty.
#[test]
fn open_candle_path_lost_reply_preserves_retry_coverage() {
    let source = MarketDataSource::new(MarketStore::shared(0.0));
    let done = Arc::new(AtomicU64::new(0));
    let (cache, queue) = KlineCache::stalled_for_tests();
    let pending = cache
        .request_prefix(PrefixReadRequest {
            exchange: "synthetic".into(),
            market: "SYNTH".into(),
            native_kind: 1,
            from_ms: 86_400_000,
            now_ms: 86_460_000,
            want_5m: true,
            want_1d: false,
            done: Arc::clone(&done),
        })
        .unwrap();
    drop(queue.recv().unwrap());
    let mut state = OpenPathHistory {
        key: Some(PathReadKey {
            provider: 7,
            exchange: None,
            market: "SYNTH".into(),
            tf_ms: 86_400_000,
            open_ms: 86_400_000,
        }),
        pending: Some(pending),
        requested_minute: Some(1441),
        ..Default::default()
    };
    assert!(!state.sync(&source, (7, "SYNTH"), 86_400_000, 86_460_001, false, done));
    assert!(state.rows.minutes.is_none());
    assert_eq!(state.frontier, None);
}
