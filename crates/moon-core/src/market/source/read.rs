//! Source read plane for revisions, prices, ticker data, search, and chart-history draining.

use crate::data::OrderBookModel;
use crate::feed::SharedMoonClient;
use crate::market::source::{max_order_notional, MarketLabel, MarketLimits};
use crate::session::CoreId;

use super::{
    rows_to_ticks, ArbQuote, ArbVenue, CoinTag, DetectSnapshot, LatestPriceError,
    MarketContextReadout, MarketDataSource, MarketFiguresReadout, MarketRevisions,
    MarketTickerReadout, MarketWindowsReadout,
};
use crate::market::candles::ChartCandle;

/// Why a core's exchange could not be addressed for a trade replay.
///
/// Two DIFFERENT facts, and the window says a different thing for each: one asks the user to
/// reconnect a core, the other tells them this build does not know their exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayAddressError {
    /// The core has no live market-data provider, so nothing about its exchange is knowable.
    NotConnected,
    /// The core is connected, but its platform ordinal is newer than this build's directory.
    UnknownVenue,
}

/// How one core's exchange is addressed by the trade replay, resolved from one source snapshot.
///
/// See [`MarketDataSource::replay_address`], which is the only thing that builds it.
///
/// Deliberately not `Debug`: the cache handle owns a channel into a worker thread and has no
/// printable state, and deriving it here would force a `Debug` impl onto that type purely to
/// satisfy a value nobody logs.
#[derive(Clone)]
pub struct ReplayAddress {
    /// The venue the core is connected to, as the exchange directory names it.
    pub venue: crate::venue::Venue,
    /// Kline-cache address shared by every core on this exchange.
    pub exchange_key: String,
    /// Handle to the ONE open kline cache, or `None` before the terminal supplied its path.
    ///
    /// Handed over rather than reopened: the cache owns a SQLite connection and a worker thread,
    /// and a second instance on the same file would mean a second of each.
    pub cache: Option<crate::market::kline_cache::KlineCache>,
}

/// What a market's position actually is: size signed by DIRECTION, with the entry and liquidation
/// prices of the leg that size came from.
///
/// Two facts the net figure alone gets wrong, and both are already handled this way in
/// `feed::assets` — the Assets panel lost short positions to each of them once:
///
/// 1. **Hedge mode keeps the legs separately.** A core holding only a short reports `pos_size == 0`
///    with `short_pos_size` set, and reading the net would call that market flat.
/// 2. **Direction is not always in the sign.** A core can report a POSITIVE NET size with
///    `pos_dir == Sell`, and taking the sign alone would paint that short as a long.
///
/// The second rule applies to the NET branch only, and that is not a shortcut: `OrderType::Sell` is
/// byte 0 and is also `OrderType::default()` — what a core writes when it states no direction at
/// all. A leg branch already knows its direction from the leg it read, so consulting `pos_dir`
/// there would turn every long-only hedge position into a short the moment that field was absent.
/// `feed::assets` draws the line in the same place.
pub(super) fn position_of(
    pos: &moonproto::state::MarketBalancePosition,
) -> (Option<f64>, f64, f64) {
    let (size, price, liq) = if pos.pos_size != 0.0 {
        (pos.pos_size, pos.pos_price, pos.liq_price)
    } else if pos.short_pos_size.abs() > pos.long_pos_size.abs() {
        (
            -pos.short_pos_size.abs(),
            pos.short_pos_price,
            pos.short_liq_price,
        )
    } else if pos.long_pos_size != 0.0 {
        (
            pos.long_pos_size.abs(),
            pos.long_pos_price,
            pos.long_liq_price,
        )
    } else {
        (0.0, 0.0, 0.0)
    };
    if !(size.is_finite() && size != 0.0) {
        return (None, 0.0, 0.0);
    }
    // Only the NET figure can be positive on a short; a leg's sign was decided above.
    let net_short = pos.pos_size != 0.0 && pos.pos_dir == moonproto::OrderType::Sell;
    let signed = match net_short {
        true => -size.abs(),
        false => size,
    };
    (Some(signed), price, liq)
}

/// Deployer names out of one snapshot's `AuthCheck` response.
pub(super) fn dex_names_of(snapshot: &moonproto::MoonClientSnapshot) -> Vec<String> {
    snapshot
        .auth_info()
        .map(|info| info.known_dexes.iter().map(|d| d.name.clone()).collect())
        .unwrap_or_default()
}

/// The protocol's platform code for a raw wire byte.
///
/// `ArbPlatformCode::from_byte` is not public outside the protocol's diagnostics build, and the
/// named constants cover only what THAT build knew about — which is exactly the venue this terminal
/// then cannot read (the reference terminal shows an `OkxF` column whose code appears in no
/// constant). `hyper_deployer` is public and is a plain `50 + index` with wrapping arithmetic, so
/// it reaches every byte.
///
/// Ugly on purpose, and deliberately the ONLY place the numbering is bridged: see
/// docs-internal/FORK_BUGS.md, where a public byte constructor is asked for.
pub(super) fn platform_code(byte: u8) -> moonproto::ArbPlatformCode {
    moonproto::ArbPlatformCode::hyper_deployer(byte.wrapping_sub(ArbVenue::DEPLOYER_BASE))
}

/// A figure the venue states, or nothing.
///
/// Zero is how every unset double arrives from the wire — an absent bid, a spot market's mark, a
/// coin whose 24-hour volume has not been sent yet — and a caption that printed it would state a
/// fact the exchange never gave. Non-finite is the same case through a different door.
pub(super) fn positive(v: f64) -> Option<f64> {
    (v.is_finite() && v > 0.0).then_some(v)
}

/// Return a short exchange-type label from `server_info.exchange_type_mask`.
///
/// The core is i18n-agnostic, so spot and futures labels are plain Russian strings that the UI may
/// relocalize. The mask represents connection capabilities; a single connection usually sets one
/// trading bit.
fn exchange_kind_label(info: &moonproto::ServerInfo) -> String {
    use moonproto::ExchangeTypeMask as M;
    let spot = info.supports(M::SPOT);
    let fut = info.supports(M::FUTURES);
    let dex = info.supports(M::DEX);
    match (dex, fut, spot) {
        (true, _, _) => "DEX".to_string(),
        (false, true, false) => "Фьючи".to_string(),
        (false, false, true) => "Спот".to_string(),
        (false, true, true) => "Спот/Фьючи".to_string(),
        _ => String::new(),
    }
}

impl MarketDataSource {
    /// Cheap hot-path revision for a consumer core. This reads one monotonic
    /// MoonProto snapshot number and does not clone the snapshot or drain rings.
    pub fn snapshot_revision(&self, core: CoreId) -> Option<(CoreId, u64)> {
        let (provider, client) = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            let client = inner.clients.get(&provider)?.get()?;
            (provider, client)
        };
        Some((provider, client.snapshot_revision().unwrap_or(0)))
    }

    /// Cheap per-market wake revision for a consumer core.
    ///
    /// This is terminal-owned causality, not a MoonProto storage policy:
    /// feed threads mark the markets touched by domain events, and visible
    /// charts compare this one number before pulling retained rows or books.
    pub fn market_revisions(&self, core: CoreId, market: &str) -> Option<MarketRevisions> {
        let inner = self.inner.read().expect("market source poisoned");
        let provider = inner.core_provider.get(&core).copied()?;
        let generation = inner
            .provider_generations
            .get(&provider)
            .copied()
            .unwrap_or(0);
        let counters = inner
            .market_revisions
            .get(&provider)
            .and_then(|markets| markets.get(market))
            .copied()
            .unwrap_or_default();
        Some(MarketRevisions {
            provider,
            generation,
            history: counters.history,
            book: counters.book,
            meta: counters.meta,
            archive: counters.archive,
        })
    }

    pub fn latest_price(&self, core: CoreId, market: &str) -> Result<f32, LatestPriceError> {
        let (provider, client) = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner
                .core_provider
                .get(&core)
                .copied()
                .ok_or(LatestPriceError::NoProvider)?;
            let client = inner
                .clients
                .get(&provider)
                .and_then(SharedMoonClient::get)
                .ok_or(LatestPriceError::NoClient)?;
            (provider, client)
        };
        let _ = provider;
        let snapshot = client
            .snapshot_versioned()
            .ok_or(LatestPriceError::NoSnapshot)?;
        let readers = snapshot
            .market_history_readers(market)
            .ok_or(LatestPriceError::NoHistoryReaders)?;

        let mut trades = Vec::new();
        if let Some(reader) = readers.futures_trades.or(readers.spot_trades) {
            reader.copy_last(1, &mut trades);
            if let Some(row) = trades.last() {
                if row.price.is_finite() && row.price > 0.0 {
                    return Ok(row.price);
                }
            }
        }

        let mut last_prices = Vec::new();
        if let Some(reader) = readers.last_prices {
            reader.copy_last(1, &mut last_prices);
            if let Some(row) = last_prices.last() {
                let price = row.price();
                if price.is_finite() && price > 0.0 {
                    return Ok(price);
                }
            }
        }

        let price = snapshot
            .markets()
            .price(market)
            .map(|p| p.p_last as f32)
            .filter(|p| p.is_finite() && *p > 0.0)
            .ok_or(LatestPriceError::NoPrice)?;
        Ok(price)
    }

    /// Return the USD rate for `currency`.
    ///
    /// A USD stablecoin maps to 1; otherwise this uses `p_last` for the coin's USDT market,
    /// whatever this exchange calls it. `None` means the provider, snapshot, market, or rate is
    /// unavailable. This uses the same linear model as `feed::assets`, without contract multipliers.
    pub fn currency_usd_rate(&self, core: CoreId, currency: &str) -> Option<f64> {
        if currency.is_empty() {
            return None;
        }
        if crate::symbol::is_usd_stable(currency) {
            return Some(1.0);
        }
        // Client and naming family come from ONE guard: read separately, a provider election
        // landing between them would price against one exchange's catalog while spelling the
        // market for another's.
        let (client, exchange) = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            let client = inner
                .clients
                .get(&provider)
                .and_then(SharedMoonClient::get)?;
            (client, inner.exchange_of_provider(provider))
        };
        let snapshot = client.snapshot_versioned()?;
        // How the coin/USDT market is spelled depends on the exchange; the shared helper asks the
        // naming module instead of concatenating, which only ever produced Binance-style names.
        crate::feed::assets::price_of_pair(
            snapshot.markets(),
            &currency.to_ascii_uppercase(),
            "USDT",
            exchange,
        )
    }

    /// Everything a trade replay needs to address one core's exchange, resolved in ONE lock.
    ///
    /// This is the single bridge from a report row's `core_uid` to the public REST world, and it
    /// exists as one call rather than three because all three answers must come from the SAME
    /// snapshot: a provider election that moved between two reads would pair one core's venue with
    /// another's cache key and quietly file rows under the wrong exchange.
    ///
    /// It also carries the ONE honest limitation of the whole feature. Neither the venue nor the
    /// cache key is durable — the platform ordinal is reported by a LIVE core and is never written
    /// to `servers.enc` — so a trade whose core is offline, disabled, or since removed resolves to
    /// `None` here and the window says so by name. That is the same boundary the existing Report
    /// coin click already stops at; making it durable means widening the report replica's schema,
    /// which is a different change with a different blast radius.
    ///
    /// Args:
    ///     core: Core that recorded the report row.
    ///
    /// Returns:
    ///     The venue, the shared kline-cache address, and a handle to the cache itself, or `None`
    ///     when this core has no live market-data provider.
    pub fn replay_address(&self, core: CoreId) -> Result<ReplayAddress, ReplayAddressError> {
        let inner = self.inner.read().expect("market source poisoned");
        let provider = inner
            .core_provider
            .get(&core)
            .copied()
            .ok_or(ReplayAddressError::NotConnected)?;
        let exchange = inner
            .provider_exchange
            .get(&provider)
            .copied()
            .ok_or(ReplayAddressError::NotConnected)?;
        // `venue` answers `None` for an ordinal this build does not know rather than guessing a
        // neighbour's. That is a DIFFERENT fact from a core being offline — the core here is
        // connected and answering — so it is carried as its own error rather than collapsed into
        // one "cannot resolve", which would tell the user to reconnect something already
        // connected.
        let venue = crate::venue::venue(exchange.code).ok_or(ReplayAddressError::UnknownVenue)?;
        Ok(ReplayAddress {
            venue,
            // The exact spelling the live path and the recorder already file rows under; a
            // divergence here would silently split the cache in two.
            exchange_key: format!("{}:{:08x}", exchange.code, exchange.dex),
            cache: inner.kline_cache.clone(),
        })
    }

    /// The naming family of the exchange `core` reads market data from.
    ///
    /// It follows the market-data provider, because that is whose catalog the names come from.
    /// Before identity arrives the family is `Unknown`, which recognizes the name's shape.
    pub fn exchange_of(&self, core: CoreId) -> crate::symbol::Exchange {
        let inner = self.inner.read().expect("market source poisoned");
        let provider = inner.core_provider.get(&core).copied().unwrap_or(core);
        inner.exchange_of_provider(provider)
    }

    /// How a market is NAMED to the user: its coin token and quote currency.
    ///
    /// THE source of truth for every surface that shows a ticker — the search popup, the chart
    /// label, tab titles, the header list, detect cards, Alerts. The catalog answers first
    /// (`market_currency` / `base_currency`, the fields the core itself uses and matches its coin
    /// lists against), and the per-exchange name rules answer only for a market the catalog does
    /// not hold. Both live behind this one call so a caller cannot pick the weaker one by
    /// accident: that is how the same coin came to read three different ways in three panels.
    ///
    /// Resolve this where a market is ASSIGNED — a pane, a row, a search hit — and keep the
    /// result. It takes the source lock and reads a snapshot, so it does not belong in a
    /// per-frame render path.
    pub fn market_label(&self, core: CoreId, market: &str) -> MarketLabel {
        let mut labels = self.market_labels(core, std::slice::from_ref(&market));
        debug_assert_eq!(labels.len(), 1, "market_labels is parallel to its input");
        labels.pop().unwrap_or_default()
    }

    /// [`Self::market_label`] for many markets of ONE core, resolved under a single lock and a
    /// single snapshot.
    ///
    /// A search popup asks about dozens of markets per keystroke; asking one at a time would take
    /// the source lock once per row. The result is parallel to `markets`.
    pub fn market_labels(&self, core: CoreId, markets: &[&str]) -> Vec<MarketLabel> {
        let (client, exchange) = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied().unwrap_or(core);
            let client = inner.clients.get(&provider).and_then(SharedMoonClient::get);
            (client, inner.exchange_of_provider(provider))
        };
        let snapshot = client.and_then(|client| client.snapshot_versioned());
        let catalog = snapshot.as_ref().map(|snapshot| snapshot.markets());
        markets
            .iter()
            .map(|market| {
                catalog
                    .as_ref()
                    .and_then(|catalog| catalog.get(market))
                    .and_then(|handle| {
                        handle.with(|m| {
                            let coin = m.market_currency.trim();
                            (!coin.is_empty()).then(|| {
                                let quote = m.base_currency.trim().to_ascii_uppercase();
                                // What the NAME alone says, read ONCE: both fields below fall back
                                // to it, and this runs per market of a search result.
                                let from_name = MarketLabel::from_name(market, exchange);
                                MarketLabel {
                                    coin: coin.to_string(),
                                    // The core's own cross-exchange identity, carried whole. See
                                    // `MarketLabel::canonic` for why it is not derived here.
                                    canonic: m.market_currency_canonic.trim().to_string(),
                                    // A COIN-M contract reports no base currency at all, so its
                                    // quote comes from the name (`BTCUSD_PERP` → `USD`); without
                                    // this the pair collapses to a bare coin.
                                    quote: match quote.is_empty() {
                                        true => from_name.quote,
                                        false => quote,
                                    },
                                    // The expiry the NAME carries, which the token often does not:
                                    // Bybit's dated contract is the market `BTCUSDT-25SEP26` whose
                                    // token is a flat `BTCUSDT25SEP`, and reading only the token
                                    // made every one of its ten expiries look like a perpetual.
                                    // COIN-M spells it inside the token (`BTC_0925`) and answers
                                    // from there, so this fills a gap rather than competing.
                                    contract: from_name.contract,
                                }
                            })
                        })
                    })
                    // A market the catalog does not hold — delisted under an open order, or a
                    // core that has not sent its list yet.
                    .unwrap_or_else(|| MarketLabel::from_name(market, exchange))
            })
            .collect()
    }

    /// Return the USD rate for the quote currency of `market`.
    ///
    /// This converts `quantity * price` notional into USD. A USDT quote maps to 1, while a BTC quote
    /// uses the BTC/USDT rate. `None` means the rate is unknown.
    pub fn quote_usd_rate(&self, core: CoreId, market: &str) -> Option<f64> {
        // Read by SHAPE, without the exchange: this sits on the chart's per-frame order-label
        // rebuild, and taking the market-source lock to learn the family would cost one there for
        // every label — while the shape reading lands on the same quote for every name a core
        // lists (asserted over the real-name fixture in `tests/market_naming.rs`).
        let quote = crate::symbol::resolve_quote(market);
        if quote.is_empty() {
            // HL/HIP-3 DEX perpetuals use names such as `xyz:BIRD`, consisting of a DEX prefix and
            // coin. Their USDC quote is absent from the name, so the suffix parser cannot find it.
            // These markets are nevertheless quoted in USDC, a USD stablecoin with a rate near 1.
            // Without this fallback, `quote_usd` was None and the size label fell back from a USD
            // notional such as `$50` to a coin quantity such as `11.8`.
            return Some(1.0);
        }
        self.currency_usd_rate(core, &quote)
    }

    /// Return the market-data provider core for a consumer core.
    ///
    /// This is the exchange deduplication key: cores on the same exchange share a provider. The
    /// screener groups cores by this value to avoid duplicate coins.
    pub fn provider_of(&self, core: CoreId) -> Option<CoreId> {
        self.inner
            .read()
            .expect("market source poisoned")
            .core_provider
            .get(&core)
            .copied()
    }

    /// Return the live MoonProto client for the specific core, not its market-data provider.
    ///
    /// The screener uses this for account fields that belong to the individual core.
    pub(crate) fn core_client(
        &self,
        core: CoreId,
    ) -> Option<std::sync::Arc<moonproto::MoonClient>> {
        let inner = self.inner.read().expect("market source poisoned");
        inner.clients.get(&core).and_then(SharedMoonClient::get)
    }

    /// Return the market price step from MoonProto's `chart_price_step`.
    ///
    /// It is the market's own tick, used where a price difference has to be judged against what the
    /// exchange can actually express — the sells-to-rectangle band, for one. `None` means the
    /// provider, snapshot, or market is unavailable, or the step is non-positive; callers then fall
    /// back to their own rule rather than inventing a step.
    pub fn price_step(&self, core: CoreId, market: &str) -> Option<f64> {
        let client = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            inner
                .clients
                .get(&provider)
                .and_then(SharedMoonClient::get)?
        };
        let snapshot = client.snapshot_versioned()?;
        let step = snapshot.markets().price(market)?.chart_price_step;
        (step.is_finite() && step > 0.0).then_some(step)
    }

    /// Return the last price and signed 1-hour and 24-hour percentage deltas for a market.
    ///
    /// MoonProto's `MarketDeltaState` defines `coin_1h_delta` and `coin_24h_delta` as deviations
    /// from retained averages, matching MoonBot's Coin1hDelta semantics. This feeds the header
    /// ticker; the Screener uses unsigned retained-range deltas instead. `None` means the provider,
    /// snapshot, or market is unavailable.
    pub fn market_ticker(&self, core: CoreId, market: &str) -> Option<MarketTickerReadout> {
        let client = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            inner
                .clients
                .get(&provider)
                .and_then(SharedMoonClient::get)?
        };
        let snapshot = client.snapshot_versioned()?;
        let last = snapshot
            .markets()
            .price(market)
            .map(|p| p.p_last)
            .filter(|p| p.is_finite() && *p > 0.0)?;
        let delta = snapshot.markets().delta_state(market).unwrap_or_default();
        Some(MarketTickerReadout {
            last,
            delta_1h_pct: delta.coin_1h_delta,
            delta_24h_pct: delta.coin_24h_delta,
        })
    }

    /// Return the market-wide context for a chart caption: background deltas and funding.
    ///
    /// The background deltas are the CORE's own, not a per-market figure: `global_deltas` is one
    /// record per provider, so every pane on that core reads the same exchange and BTC movement.
    /// Funding is per market and is reported only where the venue charges it — a spot market
    /// returns `None` for both halves rather than a confident zero, which would read as "funding
    /// is free here".
    ///
    /// Args:
    ///     core: Consumer core whose provider is asked.
    ///     market: Data-key market name.
    ///
    /// Returns:
    ///     The context, or `None` when the provider, snapshot, or market is unavailable.
    pub fn market_context(&self, core: CoreId, market: &str) -> Option<MarketContextReadout> {
        let client = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            inner
                .clients
                .get(&provider)
                .and_then(SharedMoonClient::get)?
        };
        let snapshot = client.snapshot_versioned()?;
        let markets = snapshot.markets();
        let globals = markets.global_deltas();
        let price = markets.price(market);
        // A rate of exactly zero is a real answer on a futures market between fundings, so the
        // absence test is the TIME the core reports, not the rate.
        let funding_at_ms = price
            .map(|p| p.funding_time())
            .map(|t| t.unix_millis())
            .filter(|ms| *ms > 0);
        Some(MarketContextReadout {
            exchange_1h_pct: globals.exchange_1h_delta,
            exchange_24h_pct: globals.exchange_24h_delta,
            btc_1h_pct: globals.btc_1h_delta,
            btc_24h_pct: globals.btc_24h_delta,
            btc_72h_pct: globals.btc_72h_delta,
            funding_pct: funding_at_ms
                .and(price)
                .map(|p| p.funding_rate * 100.0)
                .filter(|v| v.is_finite()),
            funding_at_ms,
        })
    }

    /// Return the per-market figures a chart caption can print, beyond price and context.
    ///
    /// TWO snapshots, because the value spans two subjects. The market half — quotes, the venue's
    /// caps, its tags — comes from the deduplicated PROVIDER, since `BTCUSDT@Binance` quotes the
    /// same for every core on that exchange. The position half comes from the CONSUMER core, since
    /// what is open is an account fact and differs between two cores watching one market. A caller
    /// that read them separately could pair one core's position with another's price; here it
    /// cannot.
    ///
    /// Everything is `Option` and absence is a real answer: a spot market has no mark price, an
    /// unlevered account no leverage, and a caption prints nothing rather than a confident zero.
    ///
    /// Args:
    ///     core: Consumer core the pane belongs to.
    ///     market: Data-key market name.
    ///
    /// Returns:
    ///     The figures, or `None` when neither half has a snapshot yet.
    pub fn market_figures(&self, core: CoreId, market: &str) -> Option<MarketFiguresReadout> {
        let mut out = MarketFiguresReadout::default();
        let exchange = self.exchange_of(core);
        let provider_snapshot = self
            .provider_of(core)
            .and_then(|provider| self.core_client(provider))
            .and_then(|client| client.snapshot_versioned());
        let mut any = false;
        if let Some(snapshot) = provider_snapshot.as_ref() {
            let markets = snapshot.markets();
            if let Some(handle) = markets.get(market) {
                any = true;
                out.tags = CoinTag::from_bits(markets.tags(market).bits());
                handle.with(|m| {
                    out.bid = positive(m.price.bid);
                    out.ask = positive(m.price.ask);
                    // `mark_price_found` is the venue's own answer to "does this market have one",
                    // and it is the only one that separates a spot market from a futures market
                    // whose first mark has not arrived.
                    out.mark = m
                        .price
                        .mark_price_found
                        .then(|| m.price.mark_price)
                        .and_then(positive);
                    out.price_step = positive(m.price.chart_price_step);
                    out.vol_24h = positive(m.volume);
                    out.max_leverage = (m.max_leverage > 0).then_some(m.max_leverage);
                    out.max_order = max_order_notional(
                        market,
                        exchange,
                        m.max_notional(),
                        m.max_qty(),
                        m.price.ask,
                        m.contract_size(),
                    );
                });
            }
        }
        let core_snapshot = self
            .core_client(core)
            .and_then(|client| client.snapshot_versioned());
        if let Some(snapshot) = core_snapshot.as_ref() {
            if let Some(handle) = snapshot.markets().get(market) {
                any = true;
                let pos = handle.balance_position();
                // A flat market reports a zero size, and that is NOT the same as "no position
                // arrived": the liquidation price is only meaningful while something is open, so it
                // is withheld together with it rather than printed as a zero.
                //
                // The entry price this helper also resolves has no reader: the core sets it only
                // behind a flag of its own and left it at zero in every one of 56 060 diagnostic
                // samples across 21 cores, so the caption that stated it was retired. The helper
                // keeps returning it — it is what makes the withholding rule testable in one place.
                let (size, _entry, liq) = position_of(&pos);
                out.pos_size = size;
                out.liq_price = size.and(positive(liq));
                out.leverage_x = (pos.leverage_x > 0).then_some(pos.leverage_x);
                out.isolated = pos
                    .position_type
                    .is_known()
                    .then(|| pos.position_type.is_isolated());
                // Reported whenever it is a number at all; whether a ZERO is worth printing is the
                // caption's call, not this readout's. It is not the "traded to break even" it looks
                // like: the core leaves this counter at zero on part of its venues.
                out.session_pnl = pos.total_profit().is_finite().then(|| pos.total_profit());
                out.coin_balance = pos.asset_balance.is_finite().then_some(pos.asset_balance);
            }
        }
        any.then_some(out)
    }

    /// Return the Hyperliquid deployer names one core knows, indexed by deployer index.
    ///
    /// The same list the arbitrage quotes are named from, read WITHOUT a market — the settings
    /// window has a roster but no coin, and a window that numbered the deployers while the chart
    /// beside it named them would be two answers to one question.
    ///
    /// Args:
    ///     core: Core whose `AuthCheck` response is read. Its OWN, not its market provider's: the
    ///         deployer list is part of a core's identity, and a Binance provider knows none.
    ///
    /// Returns:
    ///     Names by index, empty when the core sent no list.
    pub fn arb_dex_names(&self, core: CoreId) -> Vec<String> {
        let own = self
            .core_client(core)
            .and_then(|c| c.snapshot_versioned())
            .map(|snapshot| dex_names_of(&snapshot))
            .unwrap_or_default();
        if !own.is_empty() {
            return own;
        }
        // The list is Hyperliquid's, and a core connected elsewhere sends none — but the arbitrage
        // slots it reports still carry deployer codes, and a terminal watching several cores
        // usually has a Hyperliquid one among them. Any core's list names the same deployers,
        // because the index comes from the same protocol.
        self.any_dex_names()
    }

    /// The first non-empty deployer list any connected core knows, by ASCENDING core id.
    ///
    /// The order is the point: cores live in a `HashMap`, and taking whichever one iteration
    /// happened to reach first would let the settings window and the chart behind it spell the same
    /// deployer differently — and differently again on the next open.
    pub fn any_dex_names(&self) -> Vec<String> {
        let clients: Vec<_> = {
            let inner = self.inner.read().expect("market source poisoned");
            let mut cores: Vec<CoreId> = inner.clients.keys().copied().collect();
            cores.sort_unstable();
            cores
                .into_iter()
                .filter_map(|core| inner.clients.get(&core).and_then(SharedMoonClient::get))
                .collect()
        };
        clients
            .into_iter()
            .filter_map(|client| client.snapshot_versioned())
            .map(|snapshot| dex_names_of(&snapshot))
            .find(|names| !names.is_empty())
            .unwrap_or_default()
    }

    /// Return every arbitrage price known for the coin on this market, newest first by venue order.
    ///
    /// The prices are the COIN's, not the core's: Moonbot's server sends one arbitrage stream and a
    /// core merely files it, so a terminal with arbitrage configured on a single core prints the
    /// column on every chart it has. The spread is restated against the CHARTED market's own price,
    /// the chart's own venue is left out of its own column, and a quote older than the book's
    /// staleness bound is dropped rather than compared against a live price it no longer belongs
    /// with. All of it lives in [`super::arb`], which also explains why coverage is the union of the
    /// donors' market universes.
    ///
    /// Args:
    ///     core: Consumer core whose pane is being captioned.
    ///     market: Data-key market name on that core.
    ///
    /// Returns:
    ///     The quotes. An empty vector means nothing is known about this coin — no core has
    ///     arbitrage configured, none of them trades it, or every quote for it has gone stale.
    pub fn market_arb(&self, core: CoreId, market: &str) -> Option<Vec<ArbQuote>> {
        self.arb_quotes(core, market)
    }

    /// Return the retained-history MOVEMENT for every window a caption may ask for.
    ///
    /// Separate from [`Self::market_figures`] because it costs more: the derived snapshot walks the
    /// retained trade buckets and the 5-minute candle ring, while the figures above are field reads
    /// off a market object. Splitting them lets a chart that prints only a spread pay for only a
    /// spread.
    ///
    /// The delta is the COMBINED range magnitude — the same figure the Screener's columns show, so
    /// a coin cannot read as moving 3% on the chart and 5% in the table. Traded AMOUNTS are not
    /// here; see [`Self::market_volume_span`] and [`WindowFigures`] for why they all come from one
    /// place instead.
    ///
    /// Args:
    ///     core: Consumer core whose provider owns the history.
    ///     market: Data-key market name.
    ///
    /// Returns:
    ///     The windows, or `None` when the provider, its client, or its snapshot is unavailable.
    pub fn market_windows(&self, core: CoreId, market: &str) -> Option<MarketWindowsReadout> {
        let provider = self.provider_of(core)?;
        let snapshot = self.core_client(provider)?.snapshot_versioned()?;
        let derived = snapshot.market_history_derived_snapshot_now(market)?;
        let deltas = derived.deltas;
        let trades = derived.trade_volumes;
        let mut out = MarketWindowsReadout::default();
        // Ordered exactly like `LabelWindow::ALL`, which is what the caption indexes by.
        //
        // Three minutes has no delta of its own in the derived snapshot — the core's own windows
        // skip it — so it is the rolling trade buckets' own range, which is the same figure by
        // another route: highest over lowest, as a percentage.
        //
        // Floored at the MINUTE's, because the minute is inside it: a three-minute range cannot be
        // narrower than the one-minute range it contains, and the derived minute is a combination
        // of three sources while this is a single one. Without the floor a coin that moved on a
        // source the trade buckets do not see printed `Δ3м` blank beside a live `Δ1м`.
        let rows: [f64; crate::config::LABEL_WINDOW_COUNT] = [
            deltas.one_minute,
            trades
                .three_minutes
                .price_delta_percent()
                .max(deltas.one_minute),
            deltas.five_minutes,
            deltas.fifteen_minutes,
            deltas.thirty_minutes,
            deltas.one_hour,
            deltas.two_hours,
            deltas.three_hours,
            deltas.twenty_four_hours,
            deltas.seventy_two_hours,
        ];
        for (slot, delta) in out.windows.iter_mut().zip(rows) {
            slot.delta_pct = positive(delta);
        }
        Some(out)
    }

    /// Return one market's exchange-imposed trading limits for a consumer core.
    ///
    /// Shaped after [`MarketDataSource::price_step`]: resolve the consumer core's provider, take
    /// its client, then read a SINGLE market out of the snapshot. This is the cheap counterpart to
    /// `screener_rows`, which builds a row for every market on the exchange and does retained
    /// history work per row — far too heavy for a control that renders every frame.
    ///
    /// `max_order` goes through the shared [`max_order_notional`] rule rather than repeating it,
    /// so the Screener's `Max.Order` column and the trading toolbar cannot disagree about one
    /// coin's cap.
    ///
    /// Args:
    ///     core: Consumer core whose provider owns the market data.
    ///     market: Canonical market name from `MarketHandle::name()`.
    ///
    /// Returns:
    ///     The market's limits, or `None` when the provider, its client, the snapshot, or the
    ///     market itself has not arrived yet. That is NOT the same as the exchange stating no
    ///     limit, which is carried inside the value — see [`MarketLimits`].
    pub fn market_limits(&self, core: CoreId, market: &str) -> Option<MarketLimits> {
        let client = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            inner
                .clients
                .get(&provider)
                .and_then(SharedMoonClient::get)?
        };
        let snapshot = client.snapshot_versioned()?;
        let handle = snapshot.markets().get(market)?;
        let exchange = self.exchange_of(core);
        Some(handle.with(|m| MarketLimits {
            max_order: max_order_notional(
                market,
                exchange,
                m.max_notional(),
                m.max_qty(),
                m.price.ask,
                m.contract_size(),
            ),
            max_leverage: m.max_leverage,
        }))
    }

    /// Build a frozen snapshot for a detection card.
    ///
    /// The snapshot contains the latest `bars` 5-minute OHLC candles, ordered oldest to newest,
    /// plus the exchange name and type. History combines the provider's retained 5-minute snapshot,
    /// the local kline cache populated for every market by the background recorder, and the live
    /// trade-ring tail. None of these sources calls the exchange API here. Data belongs to the
    /// exchange's deduplicated provider and is shared by cores on the same exchange and market.
    /// Build this once when the detection occurs and retain it in the card. Missing provider,
    /// client, or snapshot yields the empty default. Missing history leaves `bars` and `line` empty
    /// while exchange metadata can remain populated.
    pub fn detect_snapshot(&self, core: CoreId, market: &str, bars: usize) -> DetectSnapshot {
        let t0 = std::time::Instant::now();
        let (client, kline_cache, exchange_key) = {
            let inner = self.inner.read().expect("market source poisoned");
            let Some(provider) = inner.core_provider.get(&core).copied() else {
                return DetectSnapshot::default();
            };
            let Some(client) = inner.clients.get(&provider).and_then(SharedMoonClient::get) else {
                return DetectSnapshot::default();
            };
            // Use the provider's stable exchange key for the kline cache, as in
            // read_chart_history_into.
            let exchange_key = inner
                .provider_exchange
                .get(&provider)
                .map(|e| format!("{}:{:08x}", e.code, e.dex));
            (client, inner.kline_cache.clone(), exchange_key)
        };
        let Some(snapshot) = client.snapshot_versioned() else {
            return DetectSnapshot::default();
        };
        let mut out = DetectSnapshot::default();
        // Read the venue and the connection type from the identity BaseCheck supplied. The venue
        // comes from the platform CODE, as everywhere else; the reported name rides along only for
        // an ordinal this build cannot name, and the type mask is a separate axis — it states which
        // wallets the connection can trade, which is not the same question as which venue it is.
        let info = snapshot.server_info();
        out.venue = info
            .exchange_code
            .map(|code| {
                crate::venue::CoreVenue::identify(
                    code.stable_id(),
                    info.dex_name.as_deref().unwrap_or_default(),
                    info.exchange_name.as_deref(),
                )
            })
            .filter(crate::venue::CoreVenue::is_nameable);
        out.exchange_kind = exchange_kind_label(info);

        let tf_ms: i64 = 300_000; // 5 minutes
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        // Candle mode takes its latest `bars` buckets, roughly two hours, from a 24-hour window;
        // line mode uses every close in that window. `line_cap` is the maximum number of 5-minute
        // buckets in 24 hours: 288.
        let from_ms = now_ms - 24 * 3_600_000;
        let line_cap = (24 * 3_600_000 / tf_ms) as usize;

        // Build 5-minute candle buckets keyed by `t_open`, floored to the timeframe so sources
        // align. Base 1 is the core's retained 5-minute startup snapshot, which supplies depth;
        // base 2 is the local kline cache with real OHLC and overrides the snapshot; the trade ring
        // is the live tail and overrides recent buckets. Insertion order defines freshness priority:
        // snapshot < cache < ring. The snapshot used to be only an empty-data fallback, so the ring
        // always supplied a few bars and caused the snapshot's depth to be ignored.
        let mut buckets: std::collections::BTreeMap<i64, (f32, f32, f32, f32)> =
            std::collections::BTreeMap::new();
        let bucket_key = |t_ms: i64| (t_ms.max(0) / tf_ms) * tf_ms;
        let mut snap5_n = 0usize;
        let mut cache_n = 0usize;

        if let Some(readers) = snapshot.market_history_readers(market) {
            // Base 1 is the core's 5-minute snapshot. It carries only high and low, represented as
            // open == high and close == low, so the body spans the range. Rows are stamped at the
            // end of the period; shift them back one timeframe to align their open with the cache.
            // As in the chart, normalize_ohlc and orient_range_rows orient these range-only candles
            // by the average-price trend. Otherwise body-only rendering would color the entire
            // history in one direction because close < open.
            if let Some(candles) = readers.candles_5m {
                let mut snap: Vec<ChartCandle> = Vec::new();
                candles.with_last(line_cap, |view| {
                    view.for_each(|c| {
                        let (o, h, l, cl) = crate::market::candles::normalize_ohlc(
                            c.open(),
                            c.high(),
                            c.low(),
                            c.close(),
                        );
                        if h.is_finite() && l.is_finite() && h > 0.0 {
                            snap.push(ChartCandle {
                                t_open_ms: (c.time().unix_millis() - tf_ms) as f64,
                                open: o,
                                high: h,
                                low: l,
                                close: cl,
                                volume: 0.0,
                            });
                        }
                    });
                });
                crate::market::candles::orient_range_rows(&mut snap);
                for c in &snap {
                    buckets.insert(
                        bucket_key(c.t_open_ms as i64),
                        (c.open, c.high, c.low, c.close),
                    );
                }
                snap5_n = buckets.len();
            }
            // Base 2 is the kline cache: real OHLC from prior minutes and sessions at kind_min 5.
            if let (Some(cache), Some(ex)) = (kline_cache.as_ref(), exchange_key.as_ref()) {
                for c in cache
                    .read_range(ex, market, 5, from_ms, now_ms)
                    .unwrap_or_default()
                {
                    if c.high.is_finite() && c.low.is_finite() && c.high > 0.0 {
                        buckets.insert(
                            bucket_key(c.t_open_ms as i64),
                            (c.open, c.high, c.low, c.close),
                        );
                    }
                }
            }
            cache_n = buckets.len();
            // Aggregate the provider trade-ring tail into 5-minute candles that override recent
            // buckets.
            if let Some(reader) = readers.futures_trades.or(readers.spot_trades) {
                let from_t = moonproto::MoonTime::from_unix_millis(from_ms);
                let to_t = moonproto::MoonTime::from_unix_millis(now_ms);
                let mut rows = Vec::new();
                reader.copy_time_range(from_t, to_t, reader.capacity(), &mut rows);
                let mut ticks = Vec::new();
                rows_to_ticks(&rows, &mut ticks);
                let mut candles = Vec::new();
                crate::market::candles::aggregate_trades(&ticks, tf_ms, &mut candles);
                for c in &candles {
                    if c.high.is_finite() && c.low.is_finite() && c.high > 0.0 {
                        buckets.insert(
                            bucket_key(c.t_open_ms as i64),
                            (c.open, c.high, c.low, c.close),
                        );
                    }
                }
            }
        }

        // With the detect channel on, report actual bucket structure: candle count, total span, maximum
        // time gap, and price range. Index-based rendering hides time gaps, but they reveal sparse
        // data; a price outlier explains flattening. This avoids guessing about discontinuities.
        if crate::detect_diag::enabled() {
            let keys: Vec<i64> = buckets.keys().copied().collect();
            let max_gap = keys.windows(2).map(|w| w[1] - w[0]).max().unwrap_or(0);
            let (mut pmin, mut pmax) = (f32::INFINITY, f32::NEG_INFINITY);
            for &(_, bh, bl, _) in buckets.values() {
                pmax = pmax.max(bh);
                pmin = pmin.min(bl);
            }
            let span_ms = keys.last().copied().unwrap_or(0) - keys.first().copied().unwrap_or(0);
            log::info!(
                "detect_thumb {market}: n={} (snap5={} cache+={} ex_key={}) span_ms={} \
                 max_gap_ms={} (tf={}) price=[{:.6},{:.6}] data={:.1}мкс",
                keys.len(),
                snap5_n,
                cache_n.saturating_sub(snap5_n),
                exchange_key.as_deref().unwrap_or("<none>"),
                span_ms,
                max_gap,
                tf_ms,
                pmin,
                pmax,
                t0.elapsed().as_nanos() as f64 / 1000.0
            );
        }

        // Line mode uses every close in the 24-hour window, ordered oldest to newest.
        out.line = buckets.values().map(|&(_, _, _, c)| c).collect();
        // Candle mode uses the latest `bars` buckets, roughly two hours, oldest to newest.
        let start = buckets.len().saturating_sub(bars);
        out.bars = buckets.values().skip(start).copied().collect();
        // Derive actual 1-hour and 24-hour price changes from our buckets by comparing now with the
        // earlier price, so the deltas match the line's movement. MoonProto's coin_*_delta measures
        // deviation from the period average instead and remains the header ticker metric.
        if let Some((_, &(_, _, _, last))) = buckets.iter().next_back() {
            let close_at = |ago_ms: i64| -> Option<f32> {
                let target = now_ms - ago_ms;
                buckets
                    .range(..=target)
                    .next_back()
                    .map(|(_, &(_, _, _, c))| c)
                    .or_else(|| buckets.values().next().map(|&(_, _, _, c)| c))
            };
            if let Some(c1) = close_at(3_600_000).filter(|v| *v > 0.0) {
                out.delta_1h = (last - c1) / c1 * 100.0;
            }
            if let Some(c24) = close_at(24 * 3_600_000).filter(|v| *v > 0.0) {
                out.delta_24h = (last - c24) / c24 * 100.0;
            }
        }
        out
    }

    /// Write one core's catalog spellings for the coins the naming channel follows.
    ///
    /// Reads the core's OWN snapshot rather than its provider's: the question is how THIS core
    /// spells a coin, and under deduplication two cores of one exchange share a provider while
    /// their catalogs are what the terminal reads names from.
    ///
    /// Costs one atomic load when the channel is off and a set lookup once the core has been
    /// written, so it can sit on the reconciliation tick without scanning a market universe
    /// several times a second — see [`crate::coin_naming`].
    ///
    /// Args:
    ///     core: Core whose catalog is being read.
    ///     core_name: Server name as the user sees it.
    ///     venue: Exchange caption, for reading the table without resolving core ids.
    pub fn dump_coin_naming(&self, core: CoreId, core_name: &str, venue: &str) {
        let Some(queries) = crate::coin_naming::queries_for(core) else {
            return;
        };
        // No snapshot is not "this core does not have the coin": the catalog arrives after the
        // connection, so the sweep has to come back for it.
        let Some(snapshot) = self
            .core_client(core)
            .and_then(|client| client.snapshot_versioned())
        else {
            return;
        };
        // Copied out of the market lock before anything is written: the channel appends to a file,
        // and doing that with a market held would put disk I/O inside a lock the feed threads take.
        let mut rows: Vec<crate::coin_naming::CatalogNaming> = Vec::new();
        for query in &queries {
            for handle in snapshot
                .markets()
                .search(query, crate::coin_naming::MARKETS_PER_CORE)
            {
                let key = handle.name().to_string();
                if rows.iter().any(|row| row.key == key) {
                    continue;
                }
                rows.push(handle.with(|m| crate::coin_naming::CatalogNaming {
                    key: key.clone(),
                    // A DIFFERENT field from the key above, which is `bn_market_name`. On a
                    // multiplier coin these two are exactly where the disagreement can show, so
                    // both are printed.
                    name: m.market_name.clone(),
                    classic: m.market_name_mb_classic.clone(),
                    currency: m.market_currency.clone(),
                    canonic: m.market_currency_canonic.clone(),
                    long: m.market_currency_long.clone(),
                    base: m.base_currency.clone(),
                    leading1000: m.leading1000.clone(),
                    k1000: m.k1000,
                }));
            }
        }
        crate::coin_naming::record_core(core, core_name, venue, &rows, true);
    }

    /// Search one core's markets and label every hit, for a caller that filters by identity.
    ///
    /// The pairing is the shape [`super::pick_market_for_identity`] takes, and it exists as one
    /// function because three callers build it — the arbitrage book, the comparison tab and the
    /// arbitrage row's click — and a caller that zipped the two lists in a different order would
    /// label markets with each other's names.
    ///
    /// Args:
    ///     core: Core whose universe is searched.
    ///     query: Text to search for; the coin's identity, for the callers here.
    ///     limit: Most markets to take from this core.
    ///
    /// Returns:
    ///     `(market name, label)` pairs, parallel and in the search's own ranking.
    pub fn labelled_search(
        &self,
        core: CoreId,
        query: &str,
        limit: usize,
    ) -> Vec<(String, MarketLabel)> {
        let names = self.search_markets(core, query, limit);
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        names
            .iter()
            .cloned()
            .zip(self.market_labels(core, &refs))
            .collect()
    }

    /// Search the provider's market universe for a terminal coin-search box.
    ///
    /// Returns canonical market names (e.g. `"BTCUSDT"`) ranked by MoonProto's
    /// built-in search (exact → prefix → contains). Empty when the core has no
    /// provider/client/snapshot yet or the query is blank. The terminal pairs
    /// each name with the core's server name for the `"BTC - Bybit1"` display.
    pub fn search_markets(&self, core: CoreId, query: &str, limit: usize) -> Vec<String> {
        let client = {
            let inner = self.inner.read().expect("market source poisoned");
            let Some(provider) = inner.core_provider.get(&core).copied() else {
                return Vec::new();
            };
            match inner.clients.get(&provider).and_then(SharedMoonClient::get) {
                Some(client) => client,
                None => return Vec::new(),
            }
        };
        let Some(snapshot) = client.snapshot_versioned() else {
            return Vec::new();
        };
        snapshot
            .markets()
            .search(query, limit)
            .into_iter()
            .map(|handle| handle.name().to_string())
            .collect()
    }

    pub fn with_orderbook_view<R>(
        &self,
        core: CoreId,
        market: &str,
        f: impl FnOnce(Option<(&OrderBookModel, u64)>) -> R,
    ) -> R {
        let (provider, store) = {
            let inner = self.inner.read().expect("market source poisoned");
            (inner.core_provider.get(&core).copied(), inner.store.clone())
        };
        let store = store.read().expect("market store poisoned");
        f(provider
            .and_then(|p| store.view(p, market))
            .map(|view| (&view.book, view.book_rev)))
    }
}
