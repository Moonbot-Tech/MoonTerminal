use super::*;
use moonproto::MoonTime;
use moonproto::state::TradeHistoryRow;

/// Independent tapes without connecting clients, for routing and revision regression coverage.
fn quote_source(code: u8) -> MarketDataSource {
    use std::collections::HashMap;
    let source = MarketDataSource::new(crate::market::MarketStore::shared(0.0));
    source.set_provider_map(&HashMap::from([(1, 1), (2, 1), (3, 3)]));
    source.set_provider_exchanges(&HashMap::from([
        (1, crate::feed::ExchangeId::new(code)),
        (3, crate::feed::ExchangeId::new(code)),
    ]));
    source.set_market_provider_overrides(&HashMap::from([
        (1, HashMap::from([("USDC".into(), 3)])),
        (2, HashMap::from([("USDC".into(), 3)])),
        (3, HashMap::from([("USDT".into(), 1)])),
    ]));
    source
}

/// Borrowing a cross-quote tape's execution rules would size an order using another core.
#[test]
fn order_rule_provider_ignores_cross_quote_tape_overrides() {
    let source = quote_source(4);
    let inner = source.inner.read().unwrap();
    assert_eq!(inner.market_provider(3, "BTCUSDT"), Some(1));
    assert_eq!(inner.order_rule_provider(3), Some(3));
    assert_eq!(inner.order_rule_provider(2), Some(1));
    assert_eq!(inner.order_rule_provider(99), None);
}

/// A USDT market on a USDC core must read the USDT tape, without moving its catalog.
#[test]
fn quote_routing_keeps_catalogs_and_unmatched_quotes_on_their_group() {
    let source = quote_source(4);
    assert_eq!(source.provider_of_market(3, "BTCUSDT"), Some(1));
    assert_eq!(source.provider_of_market(3, "ethusdt"), Some(1));
    assert_eq!(source.provider_of_market(3, "XALPHAUSDC"), Some(3));
    assert_eq!(source.provider_of_market(1, "BTCUSDT"), Some(1));
    assert_eq!(source.provider_of_market(3, "ETHBTC"), Some(3));
    assert_eq!(source.provider_of_market(2, "XALPHAUSDC"), Some(3));
    assert_eq!(source.provider_of(3), Some(3));
    assert_eq!(source.provider_of_market(99, "BTCUSDT"), None);
}

/// Applying concatenated-only rules would leave OKX and Gate markets on the wrong tape.
#[test]
fn quote_routing_obeys_exchange_market_spelling() {
    let okx = quote_source(15);
    assert_eq!(okx.provider_of_market(3, "BTC-USDT-SWAP"), Some(1));
    assert_eq!(okx.provider_of_market(3, "BTC-usdt"), Some(1));
    assert_eq!(okx.provider_of_market(3, "XALPHA-USDC-SWAP"), Some(3));
    let gate = quote_source(9);
    assert_eq!(gate.provider_of_market(3, "BTC_USDT"), Some(1));
    assert_eq!(gate.provider_of_market(3, "XALPHA_USDC"), Some(3));
}

/// Missing quote groups must fall back, while a newly elected BTC group must route ETHBTC too.
#[test]
fn quote_routing_is_not_specific_to_stablecoins() {
    use std::collections::HashMap;
    let source = quote_source(4);
    source.set_market_provider_overrides(&HashMap::new());
    assert_eq!(source.provider_of_market(3, "BTCUSDT"), Some(3));
    assert_eq!(source.provider_of_market(2, "ETHBTC"), Some(1));
    source.set_market_provider_overrides(&HashMap::from([(3, HashMap::from([("BTC".into(), 1)]))]));
    assert_eq!(source.provider_of_market(3, "ETHBTC"), Some(1));
}

/// Watching the catalog tape's counters would leave a cross-quote chart asleep on real prints.
#[test]
fn cross_quote_market_revisions_follow_the_routed_tape() {
    let source = quote_source(4);
    source.mark_dirty(
        1,
        &[crate::feed::MarketDirty::new(
            "BTCUSDT",
            crate::feed::MarketDirtyFlags::HISTORY,
        )],
    );
    let routed = source.market_revisions(3, "BTCUSDT").unwrap();
    assert_eq!((routed.provider, routed.history), (1, 1));
    let own = source.market_revisions(3, "XALPHAUSDC").unwrap();
    assert_eq!((own.provider, own.history), (3, 0));
}

/// The stored book is provider-keyed; resolving only the catalog core returns the wrong book.
#[test]
fn cross_quote_orderbook_reads_the_routed_store_view() {
    let source = quote_source(4);
    let store = source.inner.read().unwrap().store.clone();
    {
        let mut store = store.write().unwrap();
        for provider in [1, 3] {
            store.reset(provider, "BTCUSDT");
        }
        store.apply_book(1, "BTCUSDT", &crate::feed::OrderBook::default());
    }
    assert_eq!(
        source.with_orderbook_view(3, "BTCUSDT", |view| view.map(|(_, rev)| rev)),
        Some(1)
    );
    source.set_market_provider_overrides(&std::collections::HashMap::new());
    assert_eq!(
        source.with_orderbook_view(3, "BTCUSDT", |view| view.map(|(_, rev)| rev)),
        Some(0)
    );
}

/// Currency-specific tapes must retain the same coarse exchange address for replay caches.
#[test]
fn currency_providers_keep_the_coarse_replay_exchange_key() {
    let source = MarketDataSource::new(crate::market::MarketStore::shared(0.0));
    source.set_provider_map(&std::collections::HashMap::from([(1, 1), (2, 1), (3, 3)]));
    source.set_provider_exchanges(&std::collections::HashMap::from([
        (1, crate::feed::ExchangeId::new(1)),
        (3, crate::feed::ExchangeId::new(1)),
    ]));
    assert_eq!(source.provider_of(3), Some(3));
    assert_eq!(source.replay_address(3).unwrap().exchange_key, "1:00000000");
    assert_eq!(source.replay_address(2).unwrap().exchange_key, "1:00000000");
}

/// Synthetic retained print; negative quantities retain the wire sell-side encoding.
fn trade(ms: i64, index: usize) -> TradeHistoryRow {
    TradeHistoryRow {
        time: MoonTime::from_unix_millis(ms),
        price: 100.0 + (index % 100) as f32 * 0.01,
        qty: if index.is_multiple_of(2) { 1.0 } else { -1.0 },
    }
}

/// Frozen pinned copy_time_range loop over retained append order, including its reservation.
/// The protocol keeps its writer private, so this oracle models its immutable read view.
fn old_rows(
    retained: &[TradeHistoryRow],
    capacity: usize,
    from: i64,
    to: i64,
) -> Vec<TradeHistoryRow> {
    let mut rows = Vec::with_capacity(capacity);
    let start = retained
        .iter()
        .position(|row| row.unix_millis() >= from)
        .unwrap_or(retained.len());
    if from < to {
        for row in &retained[start..] {
            let time = row.unix_millis();
            if time >= from && time < to {
                rows.push(*row);
            }
        }
    }
    rows
}

/// Compare every wire field because equal timestamps alone would miss ordering or side changes.
fn row_values(rows: &[TradeHistoryRow]) -> Vec<(i64, u32, u32)> {
    rows.iter()
        .map(|row| (row.unix_millis(), row.price.to_bits(), row.qty.to_bits()))
        .collect()
}

/// Boundary inclusivity, wrapped retention and late resends must match the old thumbnail read.
#[test]
fn detects_trade_rows_match_frozen_window_read() {
    let mut retained = std::collections::VecDeque::with_capacity(8);
    for (index, ms) in [0, 100, 200, 300, 400, 500, 600, 700, 800, 450, 200, 900]
        .into_iter()
        .enumerate()
    {
        if retained.len() == 8 {
            retained.pop_front();
        }
        retained.push_back(trade(ms, index));
        for (from, to) in [(200, 800), (450, 900), (0, 1000), (900, 900), (950, 1000)] {
            let expected = old_rows(&retained.iter().copied().collect::<Vec<_>>(), 8, from, to);
            let (first, second) = retained.as_slices();
            let actual = detect_trade_rows(first.iter().chain(second.iter()), from, to);
            assert_eq!(
                row_values(&actual),
                row_values(&expected),
                "{index}: {from}..{to}"
            );
        }
    }
    let selected = detect_trade_rows(retained.iter(), 450, 800);
    assert_eq!(
        selected
            .iter()
            .map(|row| row.unix_millis())
            .collect::<Vec<_>>(),
        vec![500, 600, 700, 450]
    );
}

/// Time the old reservation/read model and production collection plus conversion/aggregation.
/// Provider cursor lookup, locks and kline-cache waits are deliberately outside this CPU probe.
#[test]
#[ignore = "synthetic retained-print collection and aggregation timing"]
fn detects_trade_ring_scale_probe() {
    let retained: Vec<_> = (0..100_000)
        .map(|index| trade(index as i64 * 864, index))
        .collect();
    for from in [0, 82_800_000] {
        let mut times = Vec::new();
        let mut values = Vec::new();
        for old in [true, false] {
            let start = std::time::Instant::now();
            for _ in 0..48 {
                let rows = if old {
                    old_rows(&retained, 131_072, from, 86_400_000)
                } else {
                    let start = retained
                        .iter()
                        .position(|row| row.unix_millis() >= from)
                        .unwrap_or(retained.len());
                    detect_trade_rows(retained[start..].iter(), from, 86_400_000)
                };
                let mut ticks = Vec::new();
                rows_to_ticks(&rows, &mut ticks);
                let mut candles = Vec::new();
                crate::market::candles::aggregate_trades(&ticks, 300_000, &mut candles);
                std::hint::black_box(&candles);
                values.push(candles);
            }
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        for index in 0..48 {
            assert_eq!(values[index], values[index + 48]);
        }
        eprintln!(
            "detects_ring_model from_ms={from} old_48_cpu_ms={:.6} new_48_background_cpu_ms={:.6}",
            times[0], times[1]
        );
    }
}
