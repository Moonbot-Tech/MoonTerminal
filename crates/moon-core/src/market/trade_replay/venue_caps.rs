//! What each venue's PUBLIC, UNAUTHENTICATED REST can serve for an ARBITRARY PAST window.
//!
//! The trade window replays one closed trade against the market around it, and the only source
//! that can answer for an arbitrary past window is the exchange itself: MoonProto keeps a bounded
//! live ring rather than history, and a core answers `request_coin_card` with ~500 recent bars and
//! no time range at all. So this module is the directory of what may be ASKED for, keyed off
//! [`crate::venue::Venue`] — never off an exchange name, for exactly the reason `venue.rs` states
//! in its own header: a name is a caption whose spelling belongs to the core build.
//!
//! Two rules govern every arm below, and both exist because the endpoints are public and
//! rate-limited per IP:
//!
//! - **An UNVERIFIED endpoint is [`None`], not an optimistic guess.** A capability table is
//!   precisely the place where "this probably works" must not be written down: a wrong arm spends
//!   the user's request budget on a 404 loop and can get their address limited. Every route named
//!   here was read off the vendor's own documentation.
//! - **A venue with no route degrades HONESTLY.** The window says which venue it cannot reach and
//!   offers no retry, because retrying cannot help. That is a supported outcome, not a failure —
//!   see [`super::TradeReplayEmpty::NoEndpoint`].
//!
//! Coverage is deliberately staged. This module ships the routes for Binance (spot, USD-M,
//! COIN-M) and Bybit (spot, futures); the remaining brands answer [`None`] until their vendor
//! pages have been read the same way.

use crate::venue::{Brand, MarketKind, Venue};

/// Route serving one-minute bars over a bounded past window.
///
/// A variant is a REQUEST SHAPE, not merely a URL: the host, the query grammar, the row limit and
/// the response envelope differ per family, and the REST layer matches on this to build and parse
/// the call. Two venues that share a shape share a variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KlineRoute {
    /// `GET https://data-api.binance.vision/api/v3/klines`.
    ///
    /// The public spot market-data mirror, already used by the valuation provider, so this build
    /// is a known-good client of it.
    BinanceSpot,
    /// `GET https://fapi.binance.com/fapi/v1/klines` — USD-M perpetual and delivery futures.
    BinanceUsdM,
    /// `GET https://dapi.binance.com/dapi/v1/klines` — COIN-M delivery contracts (`QBinance`).
    BinanceCoinM,
    /// `GET https://api.bybit.com/v5/market/kline` — one host for every category.
    ///
    /// `category` is derived from the market's quote rather than stored here, because Bybit splits
    /// futures into `linear` and `inverse` while [`MarketKind`] does not.
    Bybit,
}

impl KlineRoute {
    /// Return the fully qualified request URL for this route.
    ///
    /// Returns:
    ///     Absolute HTTPS endpoint, without any query string.
    pub const fn url(self) -> &'static str {
        match self {
            Self::BinanceSpot => "https://data-api.binance.vision/api/v3/klines",
            Self::BinanceUsdM => "https://fapi.binance.com/fapi/v1/klines",
            Self::BinanceCoinM => "https://dapi.binance.com/dapi/v1/klines",
            Self::Bybit => "https://api.bybit.com/v5/market/kline",
        }
    }

    /// Return the host whose IP budget this route spends.
    ///
    /// This is the RATE-LIMIT key, and it is the host rather than the venue because that is what
    /// actually enforces a budget: one Bybit host answers spot and futures alike and must not hand
    /// them a permit each, while Binance genuinely runs three hosts and deserves three.
    ///
    /// Returns:
    ///     Bare host name, without scheme or path.
    pub const fn host(self) -> &'static str {
        match self {
            Self::BinanceSpot => "data-api.binance.vision",
            Self::BinanceUsdM => "fapi.binance.com",
            Self::BinanceCoinM => "dapi.binance.com",
            Self::Bybit => "api.bybit.com",
        }
    }

    /// Return the largest number of rows one request of this route may ask for.
    ///
    /// The pager splits a window into requests of at most this many bars. A value larger than the
    /// vendor's own cap silently truncates the answer, which would read as a hole in the middle of
    /// the trade rather than as an error, so these are the DOCUMENTED caps and not round numbers.
    ///
    /// Returns:
    ///     Maximum rows per request.
    pub const fn max_rows(self) -> usize {
        match self {
            // Documented `limit` cap for the spot klines endpoint.
            Self::BinanceSpot => 1_000,
            // Both futures kline endpoints document a higher cap than spot.
            Self::BinanceUsdM | Self::BinanceCoinM => 1_500,
            // `limit` is documented as the range [1, 1000], default 200.
            Self::Bybit => 1_000,
        }
    }

    /// Whether this route answers newest-row-first.
    ///
    /// Bybit's kline envelope returns its `list` in DESCENDING open time, while Binance returns
    /// ascending. Composing a series from a descending answer without reversing it produces a
    /// chart whose bars walk backwards, which the eye reads as corrupt data rather than as a bug,
    /// so the direction is part of the route's contract instead of a parser detail.
    ///
    /// Returns:
    ///     `true` when rows arrive newest first.
    pub const fn newest_first(self) -> bool {
        match self {
            Self::BinanceSpot | Self::BinanceUsdM | Self::BinanceCoinM => false,
            Self::Bybit => true,
        }
    }
}

/// Return the one-minute-bar route this venue is served by, if this build knows one.
///
/// Args:
///     venue: Venue resolved from the core's reported platform ordinal.
///
/// Returns:
///     The route, or `None` when no verified public endpoint exists for it in this build.
pub const fn kline_route(venue: Venue) -> Option<KlineRoute> {
    match (venue.brand, venue.kind) {
        (Brand::Binance, MarketKind::Spot) => Some(KlineRoute::BinanceSpot),
        (Brand::Binance, MarketKind::Futures) => Some(KlineRoute::BinanceUsdM),
        (Brand::Binance, MarketKind::Quarterly) => Some(KlineRoute::BinanceCoinM),
        (Brand::Bybit, _) => Some(KlineRoute::Bybit),
        // Not yet verified against the vendor documentation. `None` is the honest answer, and the
        // window says so by name rather than failing a request nobody can satisfy.
        (Brand::Htx, _)
        | (Brand::Gate, _)
        | (Brand::BitGet, _)
        | (Brand::Hyperliquid, _)
        | (Brand::Okx, _) => None,
    }
}

/// Return the Bybit product category a market belongs to.
///
/// Bybit routes every category through one kline host but demands the right `category`, and it
/// splits futures into USDT/USDC-margined (`linear`) and coin-margined (`inverse`) where
/// [`MarketKind`] carries only one `Futures`. The quote asset is what actually decides it, and
/// `symbol::resolve_quote_on` already knows how to read a Bybit market name, so the split is
/// DERIVED here rather than guessed or configured.
///
/// **The test is the LITERAL settlement currency, deliberately not `symbol::is_usd_stable`.**
/// That helper asks a different question — "is this token worth about a dollar" — and its list
/// therefore contains bare `USD`, which is exactly how Bybit spells its COIN-MARGINED contracts
/// (`BTCUSD`). Routing on it sent every inverse market to `linear`, where Bybit answers
/// `retCode 10001` and the window told the user the market does not exist. Two questions that
/// happen to share a word are still two questions.
///
/// Args:
///     venue: Venue the market belongs to.
///     market: Exchange-native market name, as the core reports it.
///
/// Returns:
///     `spot`, `linear`, or `inverse`, or `None` when the venue is not Bybit.
pub fn bybit_category(venue: Venue, market: &str) -> Option<&'static str> {
    if venue.brand != Brand::Bybit {
        return None;
    }
    if venue.kind == MarketKind::Spot {
        return Some("spot");
    }
    let quote = crate::symbol::resolve_quote_on(market, crate::symbol::Exchange::Bybit);
    // Settled in the quote asset -> linear. Everything else, bare `USD` included, is settled in
    // the base and is a coin-margined contract.
    match quote.as_str() {
        "USDT" | "USDC" => Some("linear"),
        _ => Some("inverse"),
    }
}
