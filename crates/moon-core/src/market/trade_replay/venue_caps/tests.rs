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
