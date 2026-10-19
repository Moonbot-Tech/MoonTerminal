//! The post-delete recount reads each market once, converges under live closes and shows progress.

use super::*;
use crate::market::trade_replay::trade_cache::{TrimOutcome, TrimReport};
use crate::station_api::TapePurge;

/// The default recording shape the recount judges trades by.
fn shape() -> catchup::Shape {
    catchup::Shape {
        margin_ms: crate::market::trade_replay::margin_ms(),
        long_ms: crate::market::trade_replay::long_position_ms(),
        tail_ms: crate::market::trade_replay::margin_ms(),
    }
}

/// A synthetic trade `n` on `market`, its window one minute long and `n` six-hour steps back.
fn trade_on(market: &str, n: u64) -> CatchupTrade {
    let mut trade = row();
    trade.id.rec_id = n.try_into().unwrap();
    trade.market = market.into();
    trade.close_ms = NOW - DAY - n as i64 * 6 * 3_600_000;
    trade.open_ms = trade.close_ms - 60_000;
    trade
}

/// File the whole needed window of `trade`, so it counts as complete.
fn file(cache: &TradeCache, trade: &CatchupTrade) {
    let (from, to) = catchup::needed(trade, &shape()).hull().unwrap();
    cache.insert(
        &trade.exchange,
        &trade.market,
        from,
        to,
        vec![],
        crate::market::trade_replay::tick_tiles::TileSource::Core,
    );
}

/// A state sitting at the end of a trim that freed nothing, about to enter the recount.
fn after_trim(trades: &[CatchupTrade]) -> RecorderCatchup {
    let mut state = RecorderCatchup::default();
    state.protect(trades.to_vec());
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Ok(TrimOutcome {
        report: TrimReport::default(),
        compacted: Ok(()),
        bytes_before: 0,
        bytes_after: 0,
    }))
    .unwrap();
    state.retention.phase = retention::Phase::Trim {
        rx,
        progress: Arc::default(),
    };
    state
}

/// One tick of the retention machine at the fixture clock.
fn tick(state: &mut RecorderCatchup, cache: &TradeCache) -> bool {
    state.retention_tick(cache, NOW, &HashMap::new(), &shape())
}

/// The old per-trade rule: each trade judged from its own full read of its market.
fn per_trade_complete(cache: &TradeCache, trades: &[CatchupTrade]) -> HashSet<TradeId> {
    trades
        .iter()
        .filter(|t| {
            let spans = cache
                .held_spans(&t.exchange, &t.market, i64::MIN, i64::MAX)
                .unwrap();
            Coverage::from_spans(spans).covers(&catchup::needed(t, &shape()))
        })
        .map(|t| t.id)
        .collect()
}

/// Reset the read counter and the settings the recount consults.
fn fixture() -> (
    std::sync::MutexGuard<'static, ()>,
    Restore,
    RetentionSettings,
    TradeCache,
) {
    let lock = SWITCH_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let restore = Restore::new();
    let settings = RetentionSettings::new();
    catchup::set_keep(0, true);
    catchup::take_purge_wanted();
    SPAN_READS.with(|reads| reads.set(0));
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    (lock, restore, settings, cache)
}

/// Many trades on one market cost one span read for that market, not one per trade.
#[test]
fn recount_reads_each_market_once() {
    let (_lock, _restore, _settings, cache) = fixture();
    let trades: Vec<_> = (1..=60)
        .map(|n| trade_on("SYNAUSDT", n))
        .chain((61..=63).map(|n| trade_on("SYNBUSDT", n)))
        .collect();
    let mut state = after_trim(&trades);
    settle(&mut state, &cache, NOW);
    assert_eq!(SPAN_READS.with(std::cell::Cell::get), 2);
    assert!(matches!(
        catchup::status().purge,
        Some(TapePurge::Done { .. })
    ));
}

/// A live close mid-recount re-reads only its market and the phase still reaches Done.
#[test]
fn protect_during_recount_keeps_progress_and_converges() {
    let (_lock, _restore, _settings, cache) = fixture();
    let a = trade_on("SYNAUSDT", 1);
    let b = trade_on("SYNBUSDT", 2);
    let c = trade_on("SYNCUSDT", 3);
    for trade in [&a, &b, &c] {
        file(&cache, trade);
    }
    let mut state = RecorderCatchup::default();
    state.protect(vec![a.clone(), b.clone(), c.clone()]);
    // The first reads are held open by the test, so the live closes below land mid-recount.
    let mut senders = Vec::new();
    let mut recount = retention::Recount::default();
    for trade in [&a, &b, &c] {
        let (tx, rx) = std::sync::mpsc::channel();
        senders.push((trade.clone(), tx));
        recount.reads.insert(market(trade), rx);
    }
    state.retention.phase = retention::Phase::Recount {
        recount,
        failure: None,
        freed: 0,
    };
    assert!(tick(&mut state, &cache));
    let late_a = trade_on("SYNAUSDT", 4);
    let late_d = trade_on("SYNDUSDT", 5);
    file(&cache, &late_a);
    file(&cache, &late_d);
    state.protect(vec![late_a.clone(), late_d.clone()]);
    assert!(tick(&mut state, &cache));
    // Closes keep landing on the market whose read is still open: none of them discards it.
    for _ in 0..5 {
        state.protect(vec![late_a.clone()]);
        assert!(tick(&mut state, &cache));
    }
    assert_eq!(SPAN_READS.with(std::cell::Cell::get), 1);
    for (trade, tx) in &senders {
        tx.send(Ok(cache
            .held_spans(&trade.exchange, &trade.market, i64::MIN, i64::MAX)
            .unwrap()))
            .unwrap();
    }
    // Closes landing after market A's one re-read never reopen it, so the phase ends.
    for _ in 0..5 {
        state.protect(vec![late_a.clone()]);
        tick(&mut state, &cache);
    }
    settle(&mut state, &cache, NOW);
    // Market D once, and market A once more after its open read answered.
    assert_eq!(SPAN_READS.with(std::cell::Cell::get), 2);
    assert!(matches!(
        catchup::status().purge,
        Some(TapePurge::Done { .. })
    ));
    let complete: HashSet<_> = [&a, &b, &c, &late_a, &late_d].map(|t| t.id).into();
    assert_eq!(state.complete, complete);
    assert!(state.pending.is_empty());
}

/// Judging once per market settles exactly the trades the per-trade reads settled.
#[test]
fn recount_matches_the_per_trade_rule() {
    let (_lock, _restore, _settings, cache) = fixture();
    let mut trades = Vec::new();
    for (m, market) in ["SYNAUSDT", "SYNBUSDT", "SYNCUSDT", "SYNDUSDT"]
        .into_iter()
        .enumerate()
    {
        for i in 0..6 {
            let trade = trade_on(market, (m * 10 + i) as u64 + 1);
            // Partial coverage: every other trade of the first three markets is filed, and the
            // fourth market holds only half of one trade's window.
            if m < 3 && i % 2 == 0 {
                file(&cache, &trade);
            } else if m == 3 && i == 0 {
                let (from, to) = catchup::needed(&trade, &shape()).hull().unwrap();
                cache.insert(
                    &trade.exchange,
                    &trade.market,
                    from,
                    (from + to) / 2,
                    vec![],
                    crate::market::trade_replay::tick_tiles::TileSource::Core,
                );
            }
            trades.push(trade);
        }
    }
    let expected = per_trade_complete(&cache, &trades);
    assert_eq!(expected.len(), 9);
    let mut state = after_trim(&trades);
    settle(&mut state, &cache, NOW);
    assert_eq!(state.complete, expected);
    assert_eq!(state.tape_complete, expected);
    let pending: HashSet<_> = state.pending.keys().copied().collect();
    let all: HashSet<_> = trades.iter().map(|t| t.id).collect();
    assert_eq!(pending, &all - &expected);
}

/// Each answered market moves the purge bar instead of leaving it at zero.
#[test]
fn recount_publishes_markets_judged_of_markets_total() {
    let (_lock, _restore, _settings, cache) = fixture();
    let trades: Vec<_> = ["SYNAUSDT", "SYNBUSDT", "SYNCUSDT"]
        .into_iter()
        .enumerate()
        .map(|(n, m)| trade_on(m, n as u64 + 1))
        .collect();
    let mut state = RecorderCatchup::default();
    state.protect(trades.clone());
    let mut senders = Vec::new();
    let mut recount = retention::Recount::default();
    for trade in &trades {
        let (tx, rx) = std::sync::mpsc::channel();
        senders.push(tx);
        recount.reads.insert(market(trade), rx);
    }
    state.retention.phase = retention::Phase::Recount {
        recount,
        failure: None,
        freed: 0,
    };
    for (n, tx) in senders.iter().enumerate() {
        tx.send(Ok(vec![])).unwrap();
        let open = tick(&mut state, &cache);
        let status = catchup::status().purge;
        if n + 1 < trades.len() {
            assert!(open);
            assert!(matches!(
                status,
                Some(TapePurge::Running { done, total: 3 }) if done == n as u64 + 1
            ));
        } else {
            assert!(!open);
            assert!(matches!(status, Some(TapePurge::Done { .. })));
        }
    }
}

/// Test-only timing: a 40k-trade, 400-market recount against the old one-read-per-trade cost.
#[test]
#[ignore = "timing report, run by hand"]
fn recount_timing_40k_trades_400_markets() {
    let (_lock, _restore, _settings, cache) = fixture();
    let trades: Vec<_> = (0..40_000u64)
        .map(|n| {
            let mut trade = trade_on(&format!("SYN{}USDT", n % 400), n / 400 + 1);
            trade.id.rec_id = (n + 1).try_into().unwrap();
            trade
        })
        .collect();
    for trade in trades.iter().step_by(3) {
        file(&cache, trade);
    }
    let started = std::time::Instant::now();
    let old = per_trade_complete(&cache, &trades);
    let old_elapsed = started.elapsed();
    let mut state = after_trim(&trades);
    let started = std::time::Instant::now();
    let deadline = started + std::time::Duration::from_secs(600);
    while tick(&mut state, &cache) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    let new_elapsed = started.elapsed();
    assert_eq!(state.complete, old);
    println!(
        "recount 40k trades / 400 markets: per-trade reads {old_elapsed:?}, per-market {new_elapsed:?}"
    );
}
