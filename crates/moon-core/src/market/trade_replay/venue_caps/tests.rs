use super::*;

/// `venue_caps.rs:kline_route` removing a supported arm or adding a quarterly fallback either
/// hides a replay a user can fetch or sends an unsupported market to the wrong public endpoint.
#[test]
fn kline_route_is_exact_for_reachable_and_synthetic_venue_pairs() {
    let expected = [
        (
            Brand::Binance,
            MarketKind::Spot,
            Some(KlineRoute::BinanceSpot),
        ),
        (
            Brand::Binance,
            MarketKind::Futures,
            Some(KlineRoute::BinanceUsdM),
        ),
        (
            Brand::Binance,
            MarketKind::Quarterly,
            Some(KlineRoute::BinanceCoinM),
        ),
        (Brand::Bybit, MarketKind::Spot, Some(KlineRoute::Bybit)),
        (Brand::Bybit, MarketKind::Futures, Some(KlineRoute::Bybit)),
        (Brand::Bybit, MarketKind::Quarterly, Some(KlineRoute::Bybit)),
        (Brand::Gate, MarketKind::Spot, Some(KlineRoute::GateSpot)),
        (
            Brand::Gate,
            MarketKind::Futures,
            Some(KlineRoute::GateFutures),
        ),
        (
            Brand::BitGet,
            MarketKind::Spot,
            Some(KlineRoute::BitgetSpot),
        ),
        (
            Brand::BitGet,
            MarketKind::Futures,
            Some(KlineRoute::BitgetFutures),
        ),
        (Brand::Okx, MarketKind::Spot, Some(KlineRoute::OkxSpot)),
        (Brand::Okx, MarketKind::Futures, Some(KlineRoute::OkxSwap)),
        (
            Brand::Hyperliquid,
            MarketKind::Spot,
            Some(KlineRoute::Hyperliquid),
        ),
        (
            Brand::Hyperliquid,
            MarketKind::Futures,
            Some(KlineRoute::Hyperliquid),
        ),
    ];
    for (brand, kind, route) in expected {
        assert_eq!(kline_route(Venue { brand, kind }), route);
    }
    for brand in [
        Brand::Htx,
        Brand::Gate,
        Brand::BitGet,
        Brand::Okx,
        Brand::Hyperliquid,
    ] {
        assert_eq!(
            kline_route(Venue {
                brand,
                kind: MarketKind::Quarterly
            }),
            None
        );
    }
    for kind in [MarketKind::Spot, MarketKind::Futures, MarketKind::Quarterly] {
        assert_eq!(
            kline_route(Venue {
                brand: Brand::Htx,
                kind
            }),
            None
        );
    }
    for code in 0..=20 {
        if let Some(venue) = crate::venue::venue(code) {
            assert!(kline_route(venue).is_some() || venue.brand == Brand::Htx);
        }
    }
}

/// The band's value rule: base currency on every route but the three contract ones, and on
/// those the core's own contract terms decide — inverse by the empty quote, linear otherwise,
/// unknown while the core has not described the market.
#[test]
fn tick_value_follows_the_route_and_the_cores_contract_terms() {
    let v = |brand: Brand, kind: MarketKind| Venue { brand, kind };
    assert_eq!(
        tick_value(v(Brand::Binance, MarketKind::Spot), Some(("", 100.0))),
        TickValue::Base
    );
    assert_eq!(
        tick_value(v(Brand::Binance, MarketKind::Futures), None),
        TickValue::Base
    );
    assert_eq!(
        tick_value(v(Brand::Binance, MarketKind::Quarterly), Some(("", 100.0))),
        TickValue::InverseContracts {
            usd_per_contract: 100.0
        }
    );
    assert_eq!(
        tick_value(
            v(Brand::Gate, MarketKind::Futures),
            Some(("USDT", 10_000.0))
        ),
        TickValue::LinearContracts {
            coins_per_contract: 10_000.0
        }
    );
    assert_eq!(
        tick_value(v(Brand::Okx, MarketKind::Futures), None),
        TickValue::Unknown
    );
    assert_eq!(
        tick_value(v(Brand::Okx, MarketKind::Futures), Some(("USDT", 0.0))),
        TickValue::Unknown
    );
    // An empty quote beside a size of one is linear, as `market_quantity_unit` reads it.
    assert_eq!(
        tick_value(v(Brand::Gate, MarketKind::Futures), Some(("", 1.0))),
        TickValue::LinearContracts {
            coins_per_contract: 1.0
        }
    );
}

/// `trade_route` is not `kline_route` under another name.
///
/// Bybit and Hyperliquid still have a kline route and must not gain a trade route: this build
/// has no public trade-history endpoint for them, and a guessed one spends the IP budget on a
/// 404 loop. Binance quarterly is COIN-M aggTrades, not "no route" — `MarketKind::Quarterly` is
/// how `venue` spells QBinance. OKX spot and futures share one trade history endpoint while
/// their kline routes stay split, because a swap's candle volume is contracts and its trades
/// are not.
#[test]
fn trade_route_refuses_venues_that_still_have_klines() {
    let expected = [
        (
            Brand::Binance,
            MarketKind::Spot,
            Some(TradeRoute::BinanceSpotAggTrades),
        ),
        (
            Brand::Binance,
            MarketKind::Futures,
            Some(TradeRoute::BinanceUsdMAggTrades),
        ),
        (
            Brand::Binance,
            MarketKind::Quarterly,
            Some(TradeRoute::BinanceCoinMAggTrades),
        ),
        (
            Brand::Gate,
            MarketKind::Spot,
            Some(TradeRoute::GateSpotTrades),
        ),
        (
            Brand::Gate,
            MarketKind::Futures,
            Some(TradeRoute::GateFuturesTrades),
        ),
        (
            Brand::BitGet,
            MarketKind::Spot,
            Some(TradeRoute::BitgetSpotFills),
        ),
        (
            Brand::BitGet,
            MarketKind::Futures,
            Some(TradeRoute::BitgetMixFills),
        ),
        (
            Brand::Okx,
            MarketKind::Spot,
            Some(TradeRoute::OkxHistoryTrades),
        ),
        (
            Brand::Okx,
            MarketKind::Futures,
            Some(TradeRoute::OkxHistoryTrades),
        ),
    ];
    for (brand, kind, route) in expected {
        let venue = Venue { brand, kind };
        assert_eq!(trade_route(venue), route);
    }
    for brand in [Brand::Bybit, Brand::Hyperliquid, Brand::Htx] {
        for kind in [MarketKind::Spot, MarketKind::Futures, MarketKind::Quarterly] {
            let venue = Venue { brand, kind };
            assert_eq!(trade_route(venue), None, "{brand:?} {kind:?}");
        }
    }
    for brand in [Brand::Gate, Brand::BitGet, Brand::Okx] {
        assert_eq!(
            trade_route(Venue {
                brand,
                kind: MarketKind::Quarterly
            }),
            None,
            "{brand:?} quarterly"
        );
    }

    let bybit = Venue {
        brand: Brand::Bybit,
        kind: MarketKind::Futures,
    };
    assert_eq!(kline_route(bybit), Some(KlineRoute::Bybit));
    assert_eq!(trade_route(bybit), None);
    let hyperliquid = Venue {
        brand: Brand::Hyperliquid,
        kind: MarketKind::Spot,
    };
    assert_eq!(kline_route(hyperliquid), Some(KlineRoute::Hyperliquid));
    assert_eq!(trade_route(hyperliquid), None);

    let okx_spot = Venue {
        brand: Brand::Okx,
        kind: MarketKind::Spot,
    };
    let okx_futures = Venue {
        brand: Brand::Okx,
        kind: MarketKind::Futures,
    };
    assert_ne!(kline_route(okx_spot), kline_route(okx_futures));
    assert_eq!(trade_route(okx_spot), trade_route(okx_futures));
}

/// Retention and the per-request window are different questions.
///
/// Binance futures keeps 48 hours of aggTrades but accepts less than an hour per request, so
/// copying the query cap into retention refuses a trade that closed yesterday. Gate spot
/// documents about 30 days and no query cap; Gate futures documents neither, so copying the
/// spot retention there drops a futures tape the endpoint would still serve. Bitget caps one
/// request at 7 days and retains 90. OKX retains 90 days and has no `startTime` at all.
#[test]
fn retention_is_not_the_query_window() {
    const HOUR_MS: i64 = 60 * 60 * 1_000;
    const DAY_MS: i64 = 24 * HOUR_MS;
    let capped = [
        (
            TradeRoute::BinanceUsdMAggTrades,
            48 * HOUR_MS,
            Some(HOUR_MS),
        ),
        (
            TradeRoute::BinanceCoinMAggTrades,
            48 * HOUR_MS,
            Some(HOUR_MS),
        ),
        (TradeRoute::GateSpotTrades, 30 * DAY_MS, None),
        (TradeRoute::BitgetSpotFills, 90 * DAY_MS, Some(7 * DAY_MS)),
        (TradeRoute::BitgetMixFills, 90 * DAY_MS, Some(7 * DAY_MS)),
        (TradeRoute::OkxHistoryTrades, 90 * DAY_MS, None),
    ];
    for (route, retention, query) in capped {
        assert_eq!(route.retention_ms(), Some(retention), "{route:?} retention");
        assert_eq!(route.max_query_ms(), query, "{route:?} query");
        assert_ne!(
            route.retention_ms(),
            route.max_query_ms(),
            "{route:?} must not reuse one number for both caps"
        );
    }
    for route in [
        TradeRoute::BinanceSpotAggTrades,
        TradeRoute::GateFuturesTrades,
    ] {
        assert_eq!(route.retention_ms(), None, "{route:?}");
        assert_eq!(route.max_query_ms(), None, "{route:?}");
    }
    assert_ne!(
        TradeRoute::GateFuturesTrades.retention_ms(),
        TradeRoute::GateSpotTrades.retention_ms()
    );
}

/// Bybit's `category` is the settlement currency, not "is this a USD stablecoin".
///
/// `is_usd_stable` lists bare `USD`, and that is how Bybit spells a coin-margined contract
/// (`BTCUSD`). Routing on it sends the inverse market to `linear`, where Bybit answers
/// `retCode 10001`. Spot is `spot` before the quote is read, including a name that would
/// otherwise look inverse. `BTCPERP` is deliberately not asserted: issue #732.
#[test]
fn bybit_category_splits_usdt_from_bare_usd() {
    let bybit = |kind| Venue {
        brand: Brand::Bybit,
        kind,
    };
    assert_eq!(
        bybit_category(
            Venue {
                brand: Brand::Binance,
                kind: MarketKind::Futures,
            },
            "BTCUSDT"
        ),
        None
    );
    assert_eq!(
        bybit_category(bybit(MarketKind::Spot), "BTCUSD"),
        Some("spot")
    );
    assert_eq!(
        bybit_category(bybit(MarketKind::Futures), "BTCUSDT"),
        Some("linear")
    );
    assert_eq!(
        bybit_category(bybit(MarketKind::Futures), "BTCUSDC"),
        Some("linear")
    );
    assert_eq!(
        bybit_category(bybit(MarketKind::Futures), "btcusdt"),
        Some("linear")
    );
    assert_eq!(
        bybit_category(bybit(MarketKind::Futures), "BTCUSDT-07AUG26"),
        Some("linear")
    );
    assert!(crate::symbol::is_usd_stable("USD"));
    assert_eq!(
        bybit_category(bybit(MarketKind::Futures), "BTCUSD"),
        Some("inverse")
    );
    assert_eq!(
        bybit_category(bybit(MarketKind::Quarterly), "BTCUSD"),
        Some("inverse")
    );
}

/// The kline cap and the trade cap on one venue are not the same number.
///
/// BitGet history-candles refuses 201 and the retired docs quoted 1000 for a different
/// endpoint; its fills endpoint does take 1000. OKX clamps klines at 300 and history-trades
/// at 100. Gate futures candles take 2000 and its trades take 1000. Binance futures candles
/// take 1500 and aggTrades take 1000. Using either number for the other endpoint truncates
/// a page or gets the page refused.
#[test]
fn row_caps_differ_between_kline_and_trade() {
    assert_eq!(KlineRoute::BitgetSpot.max_rows(), 200);
    assert_eq!(KlineRoute::BitgetFutures.max_rows(), 200);
    assert_eq!(TradeRoute::BitgetSpotFills.max_rows(), 1_000);
    assert_eq!(TradeRoute::BitgetMixFills.max_rows(), 1_000);

    assert_eq!(KlineRoute::OkxSpot.max_rows(), 300);
    assert_eq!(KlineRoute::OkxSwap.max_rows(), 300);
    assert_eq!(TradeRoute::OkxHistoryTrades.max_rows(), 100);

    assert_eq!(KlineRoute::GateSpot.max_rows(), 1_000);
    assert_eq!(KlineRoute::GateFutures.max_rows(), 2_000);
    assert_eq!(TradeRoute::GateSpotTrades.max_rows(), 1_000);
    assert_eq!(TradeRoute::GateFuturesTrades.max_rows(), 1_000);

    assert_eq!(KlineRoute::BinanceSpot.max_rows(), 1_000);
    assert_eq!(KlineRoute::BinanceUsdM.max_rows(), 1_500);
    assert_eq!(KlineRoute::BinanceCoinM.max_rows(), 1_500);
    assert_eq!(TradeRoute::BinanceSpotAggTrades.max_rows(), 1_000);
    assert_eq!(TradeRoute::BinanceUsdMAggTrades.max_rows(), 1_000);
    assert_eq!(TradeRoute::BinanceCoinMAggTrades.max_rows(), 1_000);

    assert_eq!(KlineRoute::Bybit.max_rows(), 1_000);
    assert_eq!(KlineRoute::Hyperliquid.max_rows(), 500);

    assert_ne!(
        KlineRoute::BitgetFutures.max_rows(),
        TradeRoute::BitgetMixFills.max_rows()
    );
    assert_ne!(
        KlineRoute::OkxSwap.max_rows(),
        TradeRoute::OkxHistoryTrades.max_rows()
    );
    assert_ne!(
        KlineRoute::GateFutures.max_rows(),
        TradeRoute::GateFuturesTrades.max_rows()
    );
    assert_ne!(
        KlineRoute::BinanceUsdM.max_rows(),
        TradeRoute::BinanceUsdMAggTrades.max_rows()
    );
}

/// Two trade routes need a slower floor than the gate's default, and they are not the same
/// floor. Binance futures aggTrades weigh 20, so 670 ms stays at 1 800 weight a minute; Gate
/// futures repeated a page when the next request came sooner than 350 ms. Every other trade
/// route stays on the default. COIN-M shares the USD-M floor: same weight, same budget.
#[test]
fn page_floors_keep_gate_futures_off_the_binance_pace() {
    use std::time::Duration;

    let binance = TradeRoute::BinanceUsdMAggTrades.page_interval();
    let gate = TradeRoute::GateFuturesTrades.page_interval();
    assert_eq!(TradeRoute::BinanceCoinMAggTrades.page_interval(), binance);
    assert_eq!(gate, Duration::from_millis(350));
    assert!(gate > super::super::gate::MIN_INTERVAL);
    assert!(gate < binance);
    for route in [
        TradeRoute::BinanceSpotAggTrades,
        TradeRoute::GateSpotTrades,
        TradeRoute::BitgetSpotFills,
        TradeRoute::BitgetMixFills,
        TradeRoute::OkxHistoryTrades,
    ] {
        assert_eq!(
            route.page_interval(),
            super::super::gate::MIN_INTERVAL,
            "{route:?}"
        );
    }
}

/// Weight is a Binance ledger. Futures aggTrades cost 20 on both margins, spot aggTrades cost
/// 4, a futures kline page of 1 500 rows costs 10 and a spot kline costs 2. Every other route
/// weighs nothing; charging Gate or OKX the futures 20 would pause a host that has no such
/// budget.
#[test]
fn non_binance_pages_weigh_nothing() {
    assert_eq!(TradeRoute::BinanceUsdMAggTrades.request_weight(), 20);
    assert_eq!(TradeRoute::BinanceCoinMAggTrades.request_weight(), 20);
    assert_eq!(TradeRoute::BinanceSpotAggTrades.request_weight(), 4);
    for route in [
        TradeRoute::GateSpotTrades,
        TradeRoute::GateFuturesTrades,
        TradeRoute::BitgetSpotFills,
        TradeRoute::BitgetMixFills,
        TradeRoute::OkxHistoryTrades,
    ] {
        assert_eq!(route.request_weight(), 0, "{route:?}");
    }
    assert_eq!(KlineRoute::BinanceUsdM.request_weight(), 10);
    assert_eq!(KlineRoute::BinanceCoinM.request_weight(), 10);
    assert_eq!(KlineRoute::BinanceSpot.request_weight(), 2);
    for route in [
        KlineRoute::Bybit,
        KlineRoute::GateSpot,
        KlineRoute::GateFutures,
        KlineRoute::BitgetSpot,
        KlineRoute::BitgetFutures,
        KlineRoute::OkxSpot,
        KlineRoute::OkxSwap,
        KlineRoute::Hyperliquid,
    ] {
        assert_eq!(route.request_weight(), 0, "{route:?}");
    }
}
