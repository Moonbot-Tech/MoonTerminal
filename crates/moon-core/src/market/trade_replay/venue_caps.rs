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
//! COIN-M), Bybit (spot, futures), Gate (spot, futures), BitGet (spot, futures), OKX (spot, swap)
//! and Hyperliquid (spot and perpetual share one shape).
//!
//! **HTX is the one brand left answering [`None`], and not for want of reading.** Its public spot
//! REST serves candles from `GET /market/history/kline`, which accepts `symbol`, `period` and
//! `size` and nothing else: there is no time range, no cursor and no anchor, so the only window it
//! can answer for is the most recent `size` minutes counted from NOW. Appending `from`/`to`
//! anyway returns HTTP 200 and a byte-identical body, which means a caller cannot even tell that
//! its window was discarded — the exact shape of failure this module's first rule exists to
//! forbid. A wrong answer here would not stay local either: [`super::worker`] merges fetched rows
//! into the kline cache the LIVE recorder shares, so recent bars filed as last Tuesday's history
//! would be read back by every other consumer of that table. [`None`] is the honest answer.
//!
//! # The directory, in one table
//!
//! Every column below was read off the vendor's own page or, where a vendor has withdrawn it,
//! MEASURED against the live endpoint. The market name is whatever the core reports, and it
//! already arrives in each venue's own spelling (`crate::symbol::Exchange`), so no route rewrites
//! one.
//!
//! | route | endpoint | gate key | 1m token | rows | order | market | error envelope |
//! |---|---|---|---|---|---|---|---|
//! | `BinanceSpot` | `/api/v3/klines` | `data-api.binance.vision` | `1m` | 1000 | asc | `BTCUSDT` | HTTP 4xx, `code -1121` |
//! | `BinanceUsdM` | `/fapi/v1/klines` | `fapi.binance.com` | `1m` | 1500 | asc | `BTCUSDT` | HTTP 4xx, `code -1121` |
//! | `BinanceCoinM` | `/dapi/v1/klines` | `dapi.binance.com` | `1m` | 1500 | asc | `BTCUSD_PERP` | HTTP 4xx, `code -1121` |
//! | `Bybit` | `/v5/market/kline` | `api.bybit.com` | `1` | 1000 | desc | `BTCUSDT` | HTTP 200, `retCode 10001` |
//! | `GateSpot` | `/api/v4/spot/candlesticks` | `api.gateio.ws` | `1m` | 1000 | asc | `BTC_USDT` | HTTP 400, `label INVALID_CURRENCY_PAIR` |
//! | `GateFutures` | `/api/v4/futures/usdt/candlesticks` | `api.gateio.ws` | `1m` | 2000 | asc | `BTC_USDT` | HTTP 400, `label CONTRACT_NOT_FOUND` |
//! | `BitgetSpot` | `/api/v2/spot/market/history-candles` | `api.bitget.com` | `1min` | 200 | asc | `BTCUSDT` | HTTP 400, `code 400172` |
//! | `BitgetFutures` | `/api/v2/mix/market/history-candles` | `api.bitget.com` | `1m` | 200 | asc | `BTCUSDT` | HTTP 400, `code 40034` |
//! | `OkxSpot` | `/api/v5/market/history-candles` | `www.okx.com` | `1m` | 300 | desc | `BTC-USDT` | HTTP 200, `code 51001` |
//! | `OkxSwap` | `/api/v5/market/history-candles` | `www.okx.com` | `1m` | 300 | desc | `BTC-USDT-SWAP` | HTTP 200, `code 51001` |
//! | `Hyperliquid` | POST `/info` `candleSnapshot` | `api.hyperliquid.xyz` | `1m` | 500 | asc | `BTC`, `@206`, `PURR/USDC` | HTTP 500, body `null` |
//!
//! The `order` column is DOCUMENTATION and nothing reads it: [`super::rest::fetch_klines`] sorts
//! every page ascending unconditionally, which also absorbs a vendor quietly changing direction.
//! It is recorded because a reader comparing this table against a vendor page needs it, not
//! because a branch depends on it.
//!
//! Two rows earn a second look. **BitGet spells the one-minute bar differently on its two
//! markets** — `1min` on spot and `1m` on futures — and the wrong one is an HTTP 400, so the token
//! belongs beside each fetch rather than in one shared constant. And **Hyperliquid cannot
//! distinguish an unknown coin from an outage**: both answer HTTP 500 with a body of literally
//! `null`, so its classifier calls every failure transient, which is the direction that caches
//! nothing.
//!
//! # The trade-route directory, in one table
//!
//! [`TradeRoute`] is the same idea one level down: which venues can answer a public "trades
//! between T1 and T2" question, for the trade-detail window's tick replay. Every row was read off
//! the vendor's own documentation, same discipline as the kline table above. Bybit, Hyperliquid
//! and HTX document no such endpoint reachable from this build, so [`trade_route`] answers
//! [`None`] for all three — the same honest degradation [`kline_route`] already gives HTX.
//!
//! | route | endpoint | gate key | rows/page | max query window | retention | cursor | order | unknown-symbol | doc |
//! |---|---|---|---|---|---|---|---|---|---|
//! | `BinanceSpotAggTrades` | `/api/v3/aggTrades` | `data-api.binance.vision` | 1000 | none documented | none documented | `fromId` | undocumented | HTTP 4xx `-1121` | developers.binance.com/docs/binance-spot-api-docs/rest-api/market-data-endpoints |
//! | `BinanceUsdMAggTrades` | `/fapi/v1/aggTrades` | `fapi.binance.com` | 1000 | **< 1 h** | **48 h** | `fromId` | undocumented | HTTP 4xx `-1121` | developers.binance.com/.../derivatives/usds-margined-futures/market-data/rest-api/Compressed-Aggregate-Trades-List |
//! | `BinanceCoinMAggTrades` | `/dapi/v1/aggTrades` | `dapi.binance.com` | 1000 | **< 1 h** | **48 h** | `fromId` | undocumented | HTTP 4xx `-1121` | developers.binance.com/.../derivatives/coin-margined-futures/market-data/rest-api/Compressed-Aggregate-Trades-List |
//! | `GateSpotTrades` | `/api/v4/spot/trades` | `api.gateio.ws` | 1000 | none (page cap `limit*(page-1) <= 100000`) | ~30 d | `from`/`to` in **SECONDS** + `page` | undocumented | label `INVALID_CURRENCY_PAIR` | gateio/gateapi-python docs/SpotApi.md |
//! | `GateFuturesTrades` | `/api/v4/futures/usdt/trades` | `api.gateio.ws` | **undocumented, default 100** | none | none documented | `from`/`to` in **SECONDS** + `offset` | undocumented | label `CONTRACT_NOT_FOUND` | gateio/gateapi-python docs/FuturesApi.md |
//! | `BitgetSpotFills` | `/api/v2/spot/market/fills-history` | `api.bitget.com` | 1000 | **7 d** | **90 d** | `idLessThan` | **desc** | envelope `code != "00000"` | bitget.com/api-doc/classic/spot/market/Get-Market-Trades |
//! | `BitgetMixFills` | `/api/v2/mix/market/fills-history` | `api.bitget.com` | 1000 | **7 d** | **90 d** | `idLessThan` (+`productType` required) | **desc** | envelope `code != "00000"` | bitget.com/api-doc/classic/contract/market/Get-Fills-History |
//! | `OkxHistoryTrades` | `/api/v5/market/history-trades` | `www.okx.com` | **100** | none (no `startTime`/`endTime` at all) | **3 months** | `type=2&after=<ms>` for the FIRST page, `type=1&after=<tradeId>` for every later one | **desc** | HTTP 200 + `code 51001` | okx.com/docs-v5/en/#order-book-trading-market-data-get-trades-history |
//!
//! Two venue facts that are silently wrong when mistaken: **Binance futures (both arms) retain
//! only 48 hours of aggTrades**, so the retention check must run before any request is spent; and
//! **Gate's `from`/`to` are in SECONDS**, while every other timestamp in this module is
//! milliseconds.
//!
//! **Three trade routes report a CONTRACT count where `Tick::qty` is documented as base-currency
//! quantity**, and this is deliberate rather than an oversight: `OkxHistoryTrades`
//! (SWAP instruments only — `sz` is base currency for SPOT), `BinanceCoinMAggTrades` (`q` on a
//! dapi row, with no `baseQty` alternative), and `GateFuturesTrades` (`size`, whose base amount
//! depends on the contract's `quanto_multiplier`). No code compensates for this: the chart's
//! volume bars stay shape-correct because the drawn scale is window-relative and a per-instrument
//! multiplier is a constant that cancels out of it, and a tick series' aggregated candles never
//! reach the shared kline cache where an absolute figure could be read as genuine history. See
//! each route's own parser for the fact restated in the vendor's own terms.

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
    /// `GET https://dapi.binance.com/dapi/v1/klines` — COIN-M futures (`QBinance`).
    BinanceCoinM,
    /// `GET https://api.bybit.com/v5/market/kline` — one host for every category.
    ///
    /// `category` is derived from the market's quote rather than stored here, because Bybit splits
    /// futures into `linear` and `inverse` while [`MarketKind`] does not.
    Bybit,
    /// `GET https://api.gateio.ws/api/v4/spot/candlesticks`.
    ///
    /// Its window is in SECONDS, and its row is a positional array whose cells are NOT in OHLC
    /// order — see the parser, which is where that is pinned.
    GateSpot,
    /// `GET https://api.gateio.ws/api/v4/futures/usdt/candlesticks` — USDT-margined perpetuals.
    ///
    /// A separate variant from [`Self::GateSpot`] rather than a parameter, because the two answer
    /// in different SHAPES: spot rows are positional arrays of strings, futures rows are objects
    /// with mixed string and number cells.
    GateFutures,
    /// `GET https://api.bitget.com/api/v2/spot/market/history-candles`.
    ///
    /// `history-candles`, never the plain `candles`: that one answers HTTP 200 with an EMPTY
    /// `data` array once the window is older than roughly a month, and an empty-but-successful
    /// answer is precisely what [`super::worker`] would then remember as this window's verdict.
    BitgetSpot,
    /// `GET https://api.bitget.com/api/v2/mix/market/history-candles` — USDT-margined perpetuals.
    ///
    /// Split from [`Self::BitgetSpot`] because BitGet spells the one-minute bar differently on the
    /// two markets, and because the futures call must also carry `productType`.
    BitgetFutures,
    /// `GET https://www.okx.com/api/v5/market/history-candles` — spot instruments.
    OkxSpot,
    /// `GET https://www.okx.com/api/v5/market/history-candles` — perpetual swaps.
    ///
    /// The same URL as [`Self::OkxSpot`] and deliberately still its own variant: on a SWAP row the
    /// volume cell OKX calls `vol` counts CONTRACTS, and the base-asset amount lives one cell
    /// further along. Storing that distinction in the route is what stops one parser from being
    /// a hundredfold wrong about half its callers.
    OkxSwap,
    /// `POST https://api.hyperliquid.xyz/info` — `candleSnapshot`, spot and perpetual alike.
    ///
    /// The only route here that is not a GET with a query string. That costs the abstraction
    /// nothing: a variant has always been a REQUEST SHAPE rather than a URL, and the shape is
    /// what the REST layer matches on. Spot and perpetual share one variant because they differ
    /// only in the market name, which the core already reports in Hyperliquid's own spelling.
    Hyperliquid,
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
            Self::GateSpot => "https://api.gateio.ws/api/v4/spot/candlesticks",
            Self::GateFutures => "https://api.gateio.ws/api/v4/futures/usdt/candlesticks",
            Self::BitgetSpot => "https://api.bitget.com/api/v2/spot/market/history-candles",
            Self::BitgetFutures => "https://api.bitget.com/api/v2/mix/market/history-candles",
            Self::OkxSpot | Self::OkxSwap => "https://www.okx.com/api/v5/market/history-candles",
            Self::Hyperliquid => "https://api.hyperliquid.xyz/info",
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
            // One host answers both Gate markets, so they share one budget, as Bybit's do.
            Self::GateSpot | Self::GateFutures => "api.gateio.ws",
            Self::BitgetSpot | Self::BitgetFutures => "api.bitget.com",
            // Spelt with the `www.` the endpoint actually lives on: a bare `okx.com` here would
            // read as a second host and hand one real IP budget two independent permits.
            Self::OkxSpot | Self::OkxSwap => "www.okx.com",
            Self::Hyperliquid => "api.hyperliquid.xyz",
        }
    }

    /// Return the largest number of rows one request of this route may ask for.
    ///
    /// The pager splits a window into requests of at most this many bars. A value larger than the
    /// vendor's own cap silently truncates the answer, which would read as a hole in the middle of
    /// the trade rather than as an error, so these are the DOCUMENTED caps and not round numbers.
    ///
    /// Where a vendor's own pages no longer state the cap, the number below is one that was
    /// MEASURED against the live endpoint, and the arm says so. That distinction matters most for
    /// OKX, which does not refuse an over-large `limit` — it silently serves fewer rows, so an
    /// optimistic number there is exactly the truncation this doc comment warns about.
    ///
    /// The widest window this module ever asks for is [`super::MAX_SPAN_MS`], seven days, or
    /// 10_080 one-minute bars. Against these caps that is 6 requests for Gate futures, 11 for Gate
    /// spot, 21 for Hyperliquid, 34 for OKX and 51 for BitGet. Only the last is close enough to
    /// [`super::worker::JOB_DEADLINE`] to reach it on a slow day, and the honest outcome there is
    /// a retryable failure rather than a short chart, so no per-venue span cap is imposed.
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
            // Gate documents 1000 points per spot request and 2000 per futures request.
            Self::GateSpot => 1_000,
            Self::GateFutures => 2_000,
            // MEASURED: 200 is accepted and 201 is refused with HTTP 400. BitGet has retired the
            // v2 pages that once documented this, and the 1000 those pages quoted belonged to the
            // plain `candles` endpoint, not to `history-candles`.
            Self::BitgetSpot | Self::BitgetFutures => 200,
            // MEASURED: 300 rows come back for any `limit` at or above 300, with no error. OKX
            // CLAMPS instead of refusing, so 300 is a ceiling that must not be guessed upward.
            Self::OkxSpot | Self::OkxSwap => 300,
            // Documented cap on one `candleSnapshot` response.
            Self::Hyperliquid => 500,
        }
    }
}

/// Route serving PUBLIC individual trades over a bounded past window.
///
/// A variant is a REQUEST SHAPE, exactly as [`KlineRoute`] is one level up: the host, the query
/// grammar, the pagination cursor and the response envelope differ per family, and [`super::rest`]
/// matches on this to build and parse the call. Coverage is narrower than [`KlineRoute`]'s on
/// purpose — Bybit and Hyperliquid document no public trade-history endpoint this build can use,
/// and HTX inherits [`kline_route`]'s own reason for answering [`None`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TradeRoute {
    /// `GET https://data-api.binance.vision/api/v3/aggTrades`.
    BinanceSpotAggTrades,
    /// `GET https://fapi.binance.com/fapi/v1/aggTrades` — retains only 48h, `< 1h` per request.
    BinanceUsdMAggTrades,
    /// `GET https://dapi.binance.com/dapi/v1/aggTrades` — retains only 48h, `< 1h` per request.
    BinanceCoinMAggTrades,
    /// `GET https://api.gateio.ws/api/v4/spot/trades` — `from`/`to` in SECONDS.
    GateSpotTrades,
    /// `GET https://api.gateio.ws/api/v4/futures/usdt/trades` — `from`/`to` in SECONDS, truncates
    /// silently at its page size with no error.
    GateFuturesTrades,
    /// `GET https://api.bitget.com/api/v2/spot/market/fills-history` — answers descending.
    BitgetSpotFills,
    /// `GET https://api.bitget.com/api/v2/mix/market/fills-history` — answers descending, needs
    /// `productType`.
    BitgetMixFills,
    /// `GET https://www.okx.com/api/v5/market/history-trades` — spot and swap alike, no
    /// `startTime`/`endTime` at all; paginates backward with `after` only.
    OkxHistoryTrades,
}

impl TradeRoute {
    /// Return the fully qualified request URL for this route.
    ///
    /// Returns:
    ///     Absolute HTTPS endpoint, without any query string.
    pub const fn url(self) -> &'static str {
        match self {
            Self::BinanceSpotAggTrades => "https://data-api.binance.vision/api/v3/aggTrades",
            Self::BinanceUsdMAggTrades => "https://fapi.binance.com/fapi/v1/aggTrades",
            Self::BinanceCoinMAggTrades => "https://dapi.binance.com/dapi/v1/aggTrades",
            Self::GateSpotTrades => "https://api.gateio.ws/api/v4/spot/trades",
            Self::GateFuturesTrades => "https://api.gateio.ws/api/v4/futures/usdt/trades",
            Self::BitgetSpotFills => "https://api.bitget.com/api/v2/spot/market/fills-history",
            Self::BitgetMixFills => "https://api.bitget.com/api/v2/mix/market/fills-history",
            Self::OkxHistoryTrades => "https://www.okx.com/api/v5/market/history-trades",
        }
    }

    /// Rate-limit key, and it is DERIVED, never re-typed: every arm returns the corresponding
    /// [`KlineRoute`]'s own [`KlineRoute::host`].
    ///
    /// [`super::gate::ReplayGate`] keys pacing and refusal history by this literal, so a second
    /// hand-typed copy of the same string is two production authorities that can drift on a later
    /// endpoint change and split ONE real IP budget into two independent permits. Delegation makes
    /// that drift unrepresentable rather than merely tested.
    ///
    /// Returns:
    ///     Bare host name, without scheme or path.
    pub const fn host(self) -> &'static str {
        match self {
            Self::BinanceSpotAggTrades => KlineRoute::BinanceSpot.host(),
            Self::BinanceUsdMAggTrades => KlineRoute::BinanceUsdM.host(),
            Self::BinanceCoinMAggTrades => KlineRoute::BinanceCoinM.host(),
            Self::GateSpotTrades => KlineRoute::GateSpot.host(),
            Self::GateFuturesTrades => KlineRoute::GateFutures.host(),
            Self::BitgetSpotFills => KlineRoute::BitgetSpot.host(),
            Self::BitgetMixFills => KlineRoute::BitgetFutures.host(),
            Self::OkxHistoryTrades => KlineRoute::OkxSpot.host(),
        }
    }

    /// Largest number of rows one request may ask for.
    ///
    /// Returns:
    ///     Maximum rows per request.
    pub const fn max_rows(self) -> usize {
        match self {
            Self::BinanceSpotAggTrades
            | Self::BinanceUsdMAggTrades
            | Self::BinanceCoinMAggTrades => 1_000,
            Self::GateSpotTrades => 1_000,
            // UNDOCUMENTED: this is the MEASURED default page size, not a vendor-stated cap.
            Self::GateFuturesTrades => 100,
            Self::BitgetSpotFills | Self::BitgetMixFills => 1_000,
            Self::OkxHistoryTrades => 100,
        }
    }

    /// Largest span one REQUEST may cover, or [`None`] when the vendor documents no cap.
    ///
    /// NEVER expressed as `i64::MAX`: [`None`] preserves the distinction between no documented
    /// cap and a finite maximum span.
    ///
    /// Returns:
    ///     Widest request window in milliseconds, or `None` when unbounded.
    pub const fn max_query_ms(self) -> Option<i64> {
        const HOUR_MS: i64 = 3_600_000;
        const DAY_MS: i64 = 24 * HOUR_MS;
        match self {
            Self::BinanceUsdMAggTrades | Self::BinanceCoinMAggTrades => Some(HOUR_MS),
            Self::BitgetSpotFills | Self::BitgetMixFills => Some(7 * DAY_MS),
            Self::BinanceSpotAggTrades
            | Self::GateSpotTrades
            | Self::GateFuturesTrades
            | Self::OkxHistoryTrades => None,
        }
    }

    /// How far back the vendor documents that this endpoint answers, or [`None`] when it
    /// documents no limit.
    ///
    /// Evaluated by [`super::worker::inside_retention`] BEFORE any request is spent, so an
    /// out-of-retention window costs zero.
    ///
    /// Returns:
    ///     Retention window in milliseconds, or `None` when unbounded.
    pub const fn retention_ms(self) -> Option<i64> {
        const HOUR_MS: i64 = 3_600_000;
        const DAY_MS: i64 = 24 * HOUR_MS;
        match self {
            Self::BinanceUsdMAggTrades | Self::BinanceCoinMAggTrades => Some(48 * HOUR_MS),
            Self::GateSpotTrades => Some(30 * DAY_MS),
            Self::BitgetSpotFills | Self::BitgetMixFills => Some(90 * DAY_MS),
            Self::OkxHistoryTrades => Some(90 * DAY_MS),
            Self::BinanceSpotAggTrades | Self::GateFuturesTrades => None,
        }
    }
}

/// Return the public-trade route this venue is served by, if this build knows one.
///
/// EVERY arm spelled out; no `_` catch-all, same discipline as [`kline_route`].
///
/// `(Binance, Quarterly)` maps to [`TradeRoute::BinanceCoinMAggTrades`], NOT to `None`:
/// `venue.rs:260-265` defines [`MarketKind::Quarterly`] as Binance COIN-M and `venue.rs:323-326`
/// maps the reachable `QBinance` code to it, so a blanket "Quarterly -> None" rule would make the
/// COIN-M route unreachable here exactly as it would in [`kline_route`].
///
/// [`None`] for Bybit, Hyperliquid, HTX — none document a public trade-history endpoint this
/// build can reach — and for the Gate / BitGet / OKX `Quarterly` arms, which name no product
/// those brands actually have, the same reason [`kline_route`] already gives them.
///
/// Args:
///     venue: Venue resolved from the core's reported platform ordinal.
///
/// Returns:
///     The route, or `None` when no verified public trade endpoint exists for it in this build.
pub const fn trade_route(venue: Venue) -> Option<TradeRoute> {
    match (venue.brand, venue.kind) {
        (Brand::Binance, MarketKind::Spot) => Some(TradeRoute::BinanceSpotAggTrades),
        (Brand::Binance, MarketKind::Futures) => Some(TradeRoute::BinanceUsdMAggTrades),
        (Brand::Binance, MarketKind::Quarterly) => Some(TradeRoute::BinanceCoinMAggTrades),
        (Brand::Gate, MarketKind::Spot) => Some(TradeRoute::GateSpotTrades),
        (Brand::Gate, MarketKind::Futures) => Some(TradeRoute::GateFuturesTrades),
        (Brand::BitGet, MarketKind::Spot) => Some(TradeRoute::BitgetSpotFills),
        (Brand::BitGet, MarketKind::Futures) => Some(TradeRoute::BitgetMixFills),
        (Brand::Okx, MarketKind::Spot) | (Brand::Okx, MarketKind::Futures) => {
            Some(TradeRoute::OkxHistoryTrades)
        }
        // Neither venue documents a public trade-history endpoint reachable from this build.
        (Brand::Bybit, _) => None,
        (Brand::Hyperliquid, _) => None,
        // HTX has no trade route either, for the same reason `kline_route` gives it none.
        (Brand::Htx, _) => None,
        // `venue` yields no quarterly market for these three brands today; spelled out rather
        // than folded into a catch-all for the same reason `kline_route` spells them out.
        (Brand::Gate, MarketKind::Quarterly)
        | (Brand::BitGet, MarketKind::Quarterly)
        | (Brand::Okx, MarketKind::Quarterly) => None,
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
        (Brand::Gate, MarketKind::Spot) => Some(KlineRoute::GateSpot),
        (Brand::Gate, MarketKind::Futures) => Some(KlineRoute::GateFutures),
        (Brand::BitGet, MarketKind::Spot) => Some(KlineRoute::BitgetSpot),
        (Brand::BitGet, MarketKind::Futures) => Some(KlineRoute::BitgetFutures),
        (Brand::Okx, MarketKind::Spot) => Some(KlineRoute::OkxSpot),
        (Brand::Okx, MarketKind::Futures) => Some(KlineRoute::OkxSwap),
        // Spot and perpetual are one SHAPE here, so one route answers both — but they are still
        // spelled out rather than matched with `_`, because `Venue` is publicly constructible and
        // a `_` would hand a quarterly Hyperliquid market a route for a product that does not
        // exist.
        (Brand::Hyperliquid, MarketKind::Spot) | (Brand::Hyperliquid, MarketKind::Futures) => {
            Some(KlineRoute::Hyperliquid)
        }
        // HTX's public spot REST cannot express an arbitrary past window at all — the module
        // header carries the evidence. `None` is a verified answer here, not an unread one.
        (Brand::Htx, _) => None,
        // `venue` yields no quarterly market for these four brands today. The arm is spelled out
        // rather than folded into a catch-all so that a future ordinal which DID yield one would
        // degrade honestly instead of being routed to a spot endpoint that cannot serve it.
        (Brand::Gate, MarketKind::Quarterly)
        | (Brand::BitGet, MarketKind::Quarterly)
        | (Brand::Okx, MarketKind::Quarterly)
        | (Brand::Hyperliquid, MarketKind::Quarterly) => None,
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

#[cfg(test)]
mod tests;
