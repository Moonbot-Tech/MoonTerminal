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
