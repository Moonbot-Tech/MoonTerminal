//! Source read plane for revisions, prices, ticker data, search, and chart-history draining.

use crate::data::OrderBookModel;
use crate::feed::SharedMoonClient;
use crate::market::source::{MarketLabel, MarketLimits, max_order_notional};
use crate::session::CoreId;

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use moonproto::DeepHistoryKind;

/// Convert a retained CoinCard row into a chart candle, normalizing shuffled wire fields.
fn deep_row_candle(r: &moonproto::DeepPrice) -> crate::market::candles::ChartCandle {
    let (open, high, low, close) =
        crate::market::candles::normalize_ohlc(r.open(), r.high(), r.low(), r.close());
    crate::market::candles::ChartCandle {
        t_open_ms: r.unix_millis() as f64,
        open,
        high,
        low,
        close,
        volume: r.volume(),
    }
}

use super::{
    ArbQuote, ArbVenue, CandleReadParams, ChartHistoryBuffers, ChartHistoryCursor,
    ChartHistoryRead, CoinTag,
    DetectSnapshot, LatestPriceError, MarketContextReadout, MarketDataSource,
    MarketFiguresReadout, MarketRevisions, MarketTickerReadout, MarketWindowsReadout,
    drain_price_line, moon_time_from_rel_ms, price_rows_to_points, rows_to_ticks,
    trade_price_range,
};
use crate::market::candles::ChartCandle;
use moonproto::state::TradeVolumeTotals;

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
pub(super) fn position_of(pos: &moonproto::state::MarketBalancePosition) -> (Option<f64>, f64, f64) {
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
fn dex_names_of(snapshot: &moonproto::MoonClientSnapshot) -> Vec<String> {
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
fn platform_code(byte: u8) -> moonproto::ArbPlatformCode {
    moonproto::ArbPlatformCode::hyper_deployer(byte.wrapping_sub(ArbVenue::DEPLOYER_BASE))
}

/// A figure the venue states, or nothing.
///
/// Zero is how every unset double arrives from the wire — an absent bid, a spot market's mark, a
/// coin whose 24-hour volume has not been sent yet — and a caption that printed it would state a
/// fact the exchange never gave. Non-finite is the same case through a different door.
fn positive(v: f64) -> Option<f64> {
    (v.is_finite() && v > 0.0).then_some(v)
}

/// Lower bound of every history-retry delay in this file, in seconds.
///
/// Deliberately the same 30 seconds the effective-kind deep request starts from, so the two
/// request paths cannot beat each other into the core's API-limit auto-stop.
const HISTORY_RETRY_MIN_S: u32 = 30;
/// Upper bound of every history-retry delay in this file, in seconds.
const HISTORY_RETRY_MAX_S: u32 = 600;
/// Minimum gap between two attempts at a cache read that timed out, in milliseconds.
///
/// The read itself gives up after 250 ms, and this block runs on the frame path, so an unthrottled
/// retry would ask a worker that is already busy again on the very next frame.
const CACHE_RETRY_MS: u64 = 500;
/// Period of the core's automatic 5-minute snapshot ring, in milliseconds.
///
/// Named because it is used as a TIMESTAMP SHIFT rather than as a bucket width: rows in that ring
/// are stamped at the end of their period, so an open is one of these behind its stamp.
const SNAP5_TF_MS: i64 = 300_000;
/// Claims allowed per key before the backfill gives up for this client slot.
///
/// A budget is required, not merely tidy: the completion guard reads coarse depth out of the
/// snapshot and the cache, and a market that has no coarse depth to find — a young listing, or one
/// the core answers with an empty vector — can never satisfy it. Unbudgeted, such a key would keep
/// spending exchange weight at the 600-second cap forever. Five claims land at roughly 0, 30, 90,
/// 210 and 450 seconds, so the budget covers about seven and a half minutes of outage and the cap
/// is never actually reached on this path. That is sized against what a reconnect does rather than
/// against the outage alone: a dropped or replaced client slot clears the claims outright, so the
/// failure this exists for — a disconnected or momentarily busy core — gets a fresh budget exactly
/// when its cause clears.
const NATIVE_BACKFILL_MAX_ATTEMPTS: u32 = 5;

/// Whether a native backfill may be claimed now for a key in this state.
///
/// An absent entry has never been tried; a present one is due again once its own backoff elapses,
/// and never once it has spent its claim budget.
///
/// Success is deliberately NOT decided here: `request_coin_card` returns on QUEUEING, so no answer
/// at this call site can mean the backfill applied. The caller's `!have_native && !cache_covers`
/// guard is what observes the applied response and stops the requests for good; the budget is the
/// backstop for the markets that guard can never be satisfied for.
///
/// `now` is a parameter rather than an `Instant::now()` inside, so the decision is testable without
/// sleeping, and the comparison is saturating because the two instants come from separate clock
/// readings and a caller may legitimately hand back an earlier one.
fn native_backfill_due(state: Option<&NativeBackfillAttempt>, now: Instant) -> bool {
    match state {
        None => true,
        Some(a) => {
            a.attempts < NATIVE_BACKFILL_MAX_ATTEMPTS
                && now.saturating_duration_since(a.last_attempt)
                    >= Duration::from_secs(a.delay_s.max(HISTORY_RETRY_MIN_S) as u64)
        }
    }
}

/// One claimed native-backfill attempt; see [`NativeBackfillGate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NativeBackfillAttempt {
    /// When the claim was taken, which is when the request was about to be queued.
    last_attempt: Instant,
    /// Seconds that must elapse before the key may be claimed again.
    delay_s: u32,
    /// Claims taken so far for this key, against [`NATIVE_BACKFILL_MAX_ATTEMPTS`].
    attempts: u32,
}

/// Who may currently ask the core for a coarse-timeframe native backfill.
///
/// Shaped after [`super::archive::ArchiveGate`], which guards the analogous one-per-client archive
/// request: the state is private to the gate and the lifecycle points call methods on it rather
/// than reaching into a map and repeating the poison handling at each site.
///
/// A claim is taken BEFORE the request is queued, never after it lands. It cannot be the latter:
/// `request_coin_card` returns as soon as the request is queued, and whether it applied arrives
/// later as `CoinCardCandles::Updated` or `UpdateFailed`. So this gate only says "do not ask again
/// yet"; what says "stop asking" is the caller's own depth guard, which reads the applied response
/// out of the snapshot and the cache, backstopped by the claim budget for the markets that guard
/// can never be satisfied for.
///
/// The gate is PROCESS-GLOBAL rather than per panel — unlike [`super::ChartHistoryCursor`]'s
/// `deep_retry_delay_s` — because the request deliberately changes the core's shared timeframe
/// slot. A per-panel clock would multiply the attempt rate by the number of panels showing the
/// coin, which is exactly what the core's API-limit auto-stop punishes.
#[derive(Default)]
pub(super) struct NativeBackfillGate {
    claims: Mutex<HashMap<(CoreId, String, u32), NativeBackfillAttempt>>,
}

impl NativeBackfillGate {
    /// Take the send permit for `key`, or `None` when it is not due.
    ///
    /// Returns the backoff the gate will enforce before allowing a retry, so a failed-send
    /// diagnostic can name it. Claiming under the lock is what keeps N panels of one coin from all
    /// sending on the same frame: recording only the OUTCOME would leave every panel seeing an
    /// absent entry at once.
    fn claim(&self, key: (CoreId, String, u32), now: Instant) -> Option<u32> {
        let mut claims = self.claims.lock().expect("native backfill gate poisoned");
        let prev = claims.get(&key).copied();
        if !native_backfill_due(prev.as_ref(), now) {
            return None;
        }
        let delay_s = history_retry_next_delay_s(prev.map(|a| a.delay_s));
        claims.insert(
            key,
            NativeBackfillAttempt {
                last_attempt: now,
                delay_s,
                attempts: prev.map_or(1, |a| a.attempts + 1),
            },
        );
        Some(delay_s)
    }

    /// Drop every claim belonging to one provider, restoring its full budget.
    ///
    /// Called wherever the archive claims are forgotten: a replacement client slot has empty
    /// retained rings, so an old slot's backoff would only delay the first request the new one
    /// genuinely needs.
    pub(super) fn forget_provider(&self, provider: CoreId) {
        self.claims
            .lock()
            .expect("native backfill gate poisoned")
            .retain(|(p, _, _), _| *p != provider);
    }

    /// Drop the claims of every provider outside `keep`.
    pub(super) fn retain_providers(&self, keep: &HashSet<CoreId>) {
        self.claims
            .lock()
            .expect("native backfill gate poisoned")
            .retain(|(p, _, _), _| keep.contains(p));
    }

    /// Drop every claim.
    pub(super) fn clear(&self) {
        self.claims
            .lock()
            .expect("native backfill gate poisoned")
            .clear();
    }
}

/// Next history-retry delay in seconds: 30 doubling to a 600 cap, floored on a fresh start.
///
/// ONE backoff shape for both history request paths in this file. The effective-kind deep request
/// and the coarse native backfill sit on different state — one per panel, one process-global — but
/// they answer to the same thing: the core spends exchange request weight on our behalf and stops
/// itself when it runs out. Two copies of this arithmetic drifted apart once already, the deep one
/// carrying bare literals where the named bounds belong.
///
/// The doubling SATURATES. Nothing in this file can currently reach a delay near `u32::MAX` — the
/// cap below is applied to every value that comes back out — but this is a total function now
/// serving two independent callers, and a plain `* 2` makes it panic in a debug build for an input
/// its own signature accepts. Saturating costs nothing here, because the cap discards the excess
/// either way.
fn history_retry_next_delay_s(prev: Option<u32>) -> u32 {
    match prev {
        None => HISTORY_RETRY_MIN_S,
        Some(d) => d
            .max(HISTORY_RETRY_MIN_S)
            .saturating_mul(2)
            .min(HISTORY_RETRY_MAX_S),
    }
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

/// Map a CoinCard history timeframe in minutes to its MoonProto wire kind.
fn deep_history_kind(tf_min: u32) -> DeepHistoryKind {
    match tf_min {
        1 => DeepHistoryKind::Min1,
        30 => DeepHistoryKind::Min30,
        60 => DeepHistoryKind::Hour1,
        240 => DeepHistoryKind::Hour4,
        1440 => DeepHistoryKind::Day1,
        _ => DeepHistoryKind::Min5,
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
                                MarketLabel {
                                    coin: coin.to_string(),
                                    // A COIN-M contract reports no base currency at all, so its
                                    // quote comes from the name (`BTCUSD_PERP` → `USD`); without
                                    // this the pair collapses to a bare coin.
                                    quote: if quote.is_empty() {
                                        MarketLabel::from_name(market, exchange).quote
                                    } else {
                                        quote
                                    },
                                    contract: None,
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
                    out.mark = m.price.mark_price_found.then(|| m.price.mark_price).and_then(positive);
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
                // arrived": the entry and liquidation prices are only meaningful while something is
                // open, so they are withheld together with it rather than printed as zeros.
                let (size, price, liq) = position_of(&pos);
                out.pos_size = size;
                out.pos_price = size.and(positive(price));
                out.liq_price = size.and(positive(liq));
                out.leverage_x = (pos.leverage_x > 0).then_some(pos.leverage_x);
                out.isolated = pos
                    .position_type
                    .is_known()
                    .then(|| pos.position_type.is_isolated());
                // Session profit is a SUM and is legitimately zero on a coin traded to break even,
                // so it is reported whenever it is a number at all.
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

    /// Return every arbitrage price the core holds for one market, newest first by venue order.
    ///
    /// The core keeps a ring of ten points per venue plus a "now" entry, and both carry the OTHER
    /// venue's price; only the ring carries what THIS market cost at the same moment. The spread is
    /// therefore computed from the ring's latest point, and a venue whose ring is still empty is
    /// skipped rather than compared against a live price it was never quoted with.
    ///
    /// Venues the core is not watching (`enabled == false`) never appear: the switch is the core's
    /// own, and a row for a venue that reports nothing would be a permanently empty line.
    ///
    /// Args:
    ///     core: Consumer core whose provider owns the market data.
    ///     market: Data-key market name.
    ///
    /// Returns:
    ///     The quotes, or `None` when the provider, its client, its snapshot or the market is
    ///     unavailable. An empty vector means the market has no arbitrage venues at all.
    pub fn market_arb(&self, core: CoreId, market: &str) -> Option<Vec<ArbQuote>> {
        let provider = self.provider_of(core)?;
        let snapshot = self.core_client(provider)?.snapshot_versioned()?;
        let handle = snapshot.markets().get(market)?;
        let mut out = Vec::new();
        // WHICH venues to ask about comes from the core itself: `client_settings.arb_config` is the
        // very checkbox list the reference terminal shows, so a venue this build has never heard of
        // — a newer exchange, a deployer nobody hard-coded — is read as soon as the core watches
        // it. A table of our own could only ever list what was known when it was written.
        //
        // Asked venue by venue because that is the only door the protocol opens: the slot map is
        // private and `arb_slot` copies one entry at a time (see docs-internal/FORK_BUGS.md). The
        // mask test itself is an array read, so scanning the whole byte range costs nothing; only a
        // WATCHED venue pays for a lock, and the whole walk is behind the caller's throttle.
        let wanted = snapshot
            .settings()
            .client_settings
            .as_ref()
            .map(|s| &s.arb_config);
        // Deployer NAMES. `AuthCheck` is a mandatory start-up step, so a Hyperliquid core carries
        // this list; a core connected elsewhere sends none and borrows one from a core that does —
        // the index is the protocol's, not the core's.
        //
        // The index is the arbitrage code minus the deployer base, which is the same shape
        // `HyperDexIndex` uses into this very list. NOT VERIFIED against a live core: if the two
        // turn out to be off by one — `known_dexes[0]` is the unnamed default validator — this is
        // the one line to shift.
        let dex_names = self.arb_dex_names(provider);
        for byte in 0u8..=255 {
            let code = platform_code(byte);
            // No settings yet — the core has not sent them, or this is a build that does not — so
            // fall back to what this build can name. Without this an arbitrage column would stay
            // empty until the settings arrive, which looks like the feature is broken.
            let asked = match wanted {
                Some(cfg) => cfg.is_wanted(code),
                None => ArbVenue::from_code(byte).is_known_or_scanned_deployer(),
            };
            if !asked {
                continue;
            }
            let Some(slot) = handle.arb_slot(code).filter(|s| s.enabled) else {
                continue;
            };
            let venue = ArbVenue::from_code(byte);
            let point = slot.latest_point();
            let (price, my_price) = (f64::from(point.price), f64::from(point.my_price));
            if !(price.is_finite() && price > 0.0 && my_price.is_finite() && my_price > 0.0) {
                continue;
            }
            out.push(ArbQuote {
                venue,
                dex_name: venue
                    .deployer_index()
                    .and_then(|index| dex_names.get(usize::from(index)))
                    .cloned()
                    .unwrap_or_default(),
                price,
                my_price,
                spread_pct: (price - my_price) / my_price * 100.0,
                deposit_blocked: slot.isolated_flags.deposit_blocked(),
                withdraw_blocked: slot.isolated_flags.withdraw_blocked(),
            });
        }
        Some(out)
    }

    /// Return the retained-history figures for every window a caption may ask for.
    ///
    /// Separate from [`Self::market_figures`] because it costs more: the derived snapshot walks the
    /// retained trade buckets and the 5-minute candle ring, while the figures above are field reads
    /// off a market object. Splitting them lets a chart that prints only a spread pay for only a
    /// spread.
    ///
    /// The delta is the COMBINED range magnitude — the same figure the Screener's columns show, so
    /// a coin cannot read as moving 3% on the chart and 5% in the table. Volume comes from the
    /// retained trade buckets for the windows they cover (five minutes) and from candles beyond
    /// that; the buy share exists only where the trades do, because candles carry no split.
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
        let candles = derived.candle_volumes;
        let mut out = MarketWindowsReadout::default();
        // Ordered exactly like `LabelWindow::ALL`, which is what the caption indexes by.
        let rows: [(f64, f64, Option<TradeVolumeTotals>); crate::config::LABEL_WINDOW_COUNT] = [
            (deltas.one_minute, 0.0, Some(trades.one_minute)),
            (deltas.five_minutes, candles.five_minutes, Some(trades.five_minutes)),
            (deltas.fifteen_minutes, candles.fifteen_minutes, None),
            (deltas.thirty_minutes, candles.thirty_minutes, None),
            (deltas.one_hour, candles.one_hour, None),
            (deltas.three_hours, candles.three_hours, None),
            (deltas.twenty_four_hours, candles.twenty_four_hours, None),
            (deltas.seventy_two_hours, candles.seventy_two_hours, None),
        ];
        for (slot, (delta, candle_volume, trade_totals)) in out.windows.iter_mut().zip(rows) {
            slot.delta_pct = positive(delta);
            // Trades win where they exist: they are the live tail, while a candle window shorter
            // than one 5-minute bar cannot be built at all.
            let traded = trade_totals.map(|t| t.total_value()).and_then(positive);
            slot.volume_quote = traded.or_else(|| positive(candle_volume));
            slot.buy_share_pct = trade_totals.and_then(|t| {
                let total = t.total_value();
                (total > 0.0).then(|| t.buy_value / total * 100.0)
            });
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

    #[allow(clippy::too_many_arguments)]
    pub fn read_chart_history_into(
        &self,
        core: CoreId,
        market: &str,
        epoch_ms: f64,
        from_rel_ms: f32,
        to_rel_ms: f32,
        force_reset: bool,
        scan_price: bool,
        candle_params: Option<&CandleReadParams>,
        cursor: &mut ChartHistoryCursor,
        out: &mut ChartHistoryBuffers,
    ) -> Option<ChartHistoryRead> {
        out.clear();
        // Fixture bench FIRST, and only then the live path. A bench process has no core and no
        // MoonProto client, so every guard below would decline and the chart would stay empty;
        // this branch cannot fire in a normal run because `fixture::active` is `None` unless the
        // process was started with `--fixture`, and even then only for the one market the bench
        // carries. The candle cache is the application's own — the bench copy IS the data root,
        // so opening a second one would mean a second connection and a second worker thread on
        // the very same file.
        if let Some(fixture) = crate::fixture::active() {
            if fixture.covers(market) {
                let cache = {
                    let inner = self.inner.read().expect("market source poisoned");
                    inner.kline_cache.clone()
                };
                return Some(read_fixture_history(
                    fixture,
                    cache.as_ref(),
                    core,
                    epoch_ms,
                    from_rel_ms,
                    to_rel_ms,
                    candle_params,
                    out,
                ));
            }
        }
        // The client's epoch is read HERE, under the guard that already resolves the client, and
        // carried down to the archive request: taking it again there would mean a second source
        // lock, a second client lookup and a third lock inside the slot, on the frame path.
        let (provider, client, client_epoch, archive) = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            let (client, epoch) = inner.clients.get(&provider)?.get_with_epoch()?;
            (provider, client, epoch, inner.archive.clone())
        };
        let snapshot = client.snapshot_versioned()?;
        let revision = client.snapshot_revision().unwrap_or(0);
        let readers = snapshot.market_history_readers(market)?;
        // This chart is open, so the core's accumulated archive for it is worth having. Asked for
        // AFTER the readers resolve, because the request is only legal for a market in the retained
        // trades scope — MoonProto re-checks that against its own fresh snapshot, so a refusal here
        // is still possible and is handled inside. One send per installed client; see `archive.rs`.
        archive.request(provider, market, &client, client_epoch);
        let from_time = moon_time_from_rel_ms(epoch_ms, from_rel_ms);
        let to_time = moon_time_from_rel_ms(epoch_ms, to_rel_ms.max(from_rel_ms + 1.0));
        // Read trade crosses and scans only from the last-K-candles display zone. INFINITY hides
        // trades entirely when K is zero; it does not constrain candle aggregation.
        let display_trades = candle_params.map_or(true, |cp| cp.trades_from_rel_ms.is_finite());
        let trades_from_rel = candle_params
            .map(|cp| cp.trades_from_rel_ms.max(from_rel_ms))
            .filter(|v| v.is_finite())
            .unwrap_or(from_rel_ms);
        let trades_from_time = moon_time_from_rel_ms(epoch_ms, trades_from_rel);
        let trades_limit = candle_params.map_or(usize::MAX, |cp| cp.trades_limit.max(1));
        let mut read = ChartHistoryRead {
            provider,
            revision,
            caught_up: true,
            ..ChartHistoryRead::default()
        };

        let trade_reader = readers.futures_trades.or(readers.spot_trades);
        if let Some(reader) = trade_reader.as_ref().filter(|_| display_trades) {
            read.combo_capacity = reader.capacity();
            let display_cap = reader.capacity().min(trades_limit);
            let reset = force_reset || cursor.trades.is_none();
            if reset {
                reader.copy_time_range(
                    trades_from_time,
                    to_time,
                    display_cap,
                    &mut cursor.trade_rows,
                );
                cursor.trades = Some(reader.cursor_from_now());
                read.combo_reset = true;
                read.caught_up = true;
            } else if let Some(cur) = cursor.trades.as_mut() {
                let meta = reader.drain_new_bounded(cur, display_cap, &mut cursor.trade_rows);
                read.clipped |= meta.clipped;
                read.caught_up &= meta.caught_up;
                if meta.clipped {
                    reader.copy_time_range(
                        trades_from_time,
                        to_time,
                        display_cap,
                        &mut cursor.trade_rows,
                    );
                    cursor.trades = Some(reader.cursor_from_now());
                    read.combo_reset = true;
                }
            }
            rows_to_ticks(&cursor.trade_rows, &mut out.ticks);
            read.combo_left_rel_ms = out
                .ticks
                .first()
                .map(|tick| (tick.time_ms - epoch_ms) as f32);
            if let Some(t) = out.ticks.last() {
                cursor.last_price = Some(t.price);
            } else if cursor.last_price.is_none() {
                cursor.trade_rows.clear();
                reader.copy_last(1, &mut cursor.trade_rows);
                if let Some(row) = cursor.trade_rows.last() {
                    cursor.last_price = Some(row.price);
                }
            }
            if scan_price {
                reader.copy_time_range(
                    trades_from_time,
                    to_time,
                    display_cap,
                    &mut cursor.scan_trade_rows,
                );
                read.tick_price_range = trade_price_range(&cursor.scan_trade_rows);
            }
        } else {
            cursor.trades = None;
            cursor.last_price = None;
            // Trades are hidden when K is zero, but the ring still supplies last_price. On reset,
            // combo_reset instructs the layer to clear its cross ring.
            if let Some(reader) = trade_reader.as_ref() {
                read.combo_capacity = reader.capacity();
                if force_reset {
                    read.combo_reset = true;
                }
                cursor.trade_rows.clear();
                reader.copy_last(1, &mut cursor.trade_rows);
                if let Some(row) = cursor.trade_rows.last() {
                    cursor.last_price = Some(row.price);
                }
            }
        }

        // Liquidations use a separate ring of the same type and stay synchronized with combo. A
        // full combo reset or first pass rereads the entire visible range; otherwise only the new
        // live edge is drained. The renderer tags them with side=2 for one shared color. Their
        // window matches normal trades: the last-K-candles zone.
        if let Some(reader) = readers.liquidations.as_ref().filter(|_| display_trades) {
            let reset = read.combo_reset || cursor.liquidations.is_none();
            if reset {
                reader.copy_time_range(
                    trades_from_time,
                    to_time,
                    reader.capacity(),
                    &mut cursor.liq_rows,
                );
                cursor.liquidations = Some(reader.cursor_from_now());
            } else if let Some(cur) = cursor.liquidations.as_mut() {
                let meta = reader.drain_new_bounded(cur, reader.capacity(), &mut cursor.liq_rows);
                if meta.clipped {
                    reader.copy_time_range(
                        trades_from_time,
                        to_time,
                        reader.capacity(),
                        &mut cursor.liq_rows,
                    );
                    cursor.liquidations = Some(reader.cursor_from_now());
                }
            }
            rows_to_ticks(&cursor.liq_rows, &mut out.liquidations);
        } else {
            cursor.liquidations = None;
        }

        // The candle series combines the server's automatic 5-minute MoonProto snapshot, which
        // covers history before connection, with a local tail built from trades. Its own trade-ring
        // cursor keeps aggregation independent of the cross display zone. Only reset or timeframe
        // changes require a full rebuild; the live edge cheaply drains new rows.
        if let Some(cp) = candle_params {
            // CoinCard deep history applies only to timeframes of at least one minute. Sub-minute
            // candles are built from trades.
            let use_deep = cp.tf_ms >= 60_000;
            let native_kind_min =
                crate::market::candles::deep_kind_min_for_tf((cp.tf_ms / 60_000) as u32);
            // The core holds one candle timeframe per core, according to the MoonBot developer on
            // 2026-07-12. Alternating kinds between windows with different timeframes made the core
            // refetch exchange history for every request or subscription and could trigger API
            // limits. The provider's effective kind is the minimum live request because the kinds
            // divide into the chain 1|5|30|60|240|1440. Coarser panels resample the finer base at the
            // cost of depth; the core ring holds about 10,000 rows of the base timeframe.
            let deep_kind_min = if use_deep {
                let inner = self.inner.read().expect("market source poisoned");
                let mut wants = inner
                    .deep_kind_wants
                    .lock()
                    .expect("deep kind wants poisoned");
                let now_i = Instant::now();
                let m = wants.entry(provider).or_default();
                m.insert(native_kind_min, now_i);
                m.retain(|_, t| now_i.duration_since(*t) < Duration::from_secs(30));
                m.keys().copied().min().unwrap_or(native_kind_min)
            } else {
                native_kind_min
            };
            let deep_kind = deep_history_kind(deep_kind_min);
            // Pair the local kline-cache handle with the exchange key so cores on one exchange share
            // cached rows and provider election can change without changing the cache address.
            // CoreId itself is a stable uid since schema v11, but it identifies one core.
            let (kline_cache, exchange_key) = {
                let inner = self.inner.read().expect("market source poisoned");
                (
                    inner.kline_cache.clone(),
                    inner
                        .provider_exchange
                        .get(&provider)
                        .map(|e| format!("{}:{:08x}", e.code, e.dex)),
                )
            };
            // Subscribe to the core's live timeframe bars. Event::LiveCandle appends or replaces
            // the last retained tf_candles row. Without it, deep rows freeze at response time and
            // coarse-timeframe series can lag by hours. The subscription is global to the client
            // and the most recent kind wins, so a shared `(provider, market)` registry lets panels
            // refresh demand while entries stale for more than 60 seconds are unsubscribed.
            {
                let inner = self.inner.read().expect("market source poisoned");
                let mut subs = inner.candle_subs.lock().expect("candle subs poisoned");
                let now_i = Instant::now();
                if use_deep {
                    let entry = subs.entry((provider, market.to_string())).or_insert(
                        super::CandleSubState {
                            kind_min: deep_kind_min,
                            last_want: now_i,
                            subscribed: false,
                        },
                    );
                    if !entry.subscribed || entry.kind_min != deep_kind_min {
                        if client
                            .streams()
                            .subscribe_candles([market], deep_kind)
                            .is_ok()
                        {
                            entry.subscribed = true;
                            entry.kind_min = deep_kind_min;
                        }
                    }
                    entry.last_want = now_i;
                }
                // Remove stale subscriptions for this provider while its client is available.
                let stale: Vec<String> = subs
                    .iter()
                    .filter(|((p, _), s)| {
                        *p == provider
                            && s.subscribed
                            && now_i.duration_since(s.last_want) > Duration::from_secs(60)
                    })
                    .map(|((_, m), _)| m.clone())
                    .collect();
                for m in stale {
                    let _ = client.streams().unsubscribe_candles([m.as_str()]);
                    subs.remove(&(provider, m));
                }
            }
            // Load authoritative native klines from prior sessions as a local-cache prefix. Read
            // SQLite once per `(market, kind, left edge)` because pan and zoom reset frequently;
            // extending the window to the left triggers another read.
            if use_deep {
                let need_from = (epoch_ms + (from_rel_ms - cp.tf_ms.max(0) as f32) as f64) as i64;
                // A read that TIMED OUT must not be remembered as a completed one. The cache has a
                // single worker thread shared by every reader and writer, so a write burst can push
                // a read past its timeout — and this block runs only when the timeframe or the left
                // edge changes, so a lost read used to stick as an empty prefix until the user
                // panned. Retry it instead, no more often than once every `CACHE_RETRY_MS` so a
                // busy worker is not asked again on every frame.
                let retry_due = cursor.cache_retry_at.map_or(true, |t| {
                    t.elapsed() >= Duration::from_millis(CACHE_RETRY_MS)
                });
                let cache_stale = (cursor.cache_kind != Some(native_kind_min)
                    || need_from < cursor.cache_from_ms)
                    && retry_due;
                if cache_stale {
                    cursor.cache_rows.clear();
                    cursor.cache_rows_5m.clear();
                    cursor.cache_rows_1d.clear();
                    cursor.cache_generation = cursor.cache_generation.wrapping_add(1);
                    if let (Some(cache), Some(ex)) = (kline_cache.as_ref(), exchange_key.as_deref())
                    {
                        // Every read of this pass must land before the window counts as loaded;
                        // one timeout leaves the whole set to be retried together, so the layers
                        // cannot end up describing different left edges.
                        let mut complete = true;
                        let mut read = |kind: u32| match cache.read_range(
                            ex,
                            market,
                            kind,
                            need_from,
                            i64::MAX,
                        ) {
                            Some(rows) => rows,
                            None => {
                                complete = false;
                                Vec::new()
                            }
                        };
                        cursor.cache_rows = read(native_kind_min);
                        cursor.cache_rows_kind = native_kind_min;
                        // If the native kind is absent, fall back first to the background recorder's
                        // 5-minute rows and then to 1-minute deep-history rows. Every supported
                        // timeframe is divisible by both, so the merge can resample them.
                        for fb in [5u32, 1] {
                            if !cursor.cache_rows.is_empty() || native_kind_min <= fb {
                                break;
                            }
                            cursor.cache_rows = read(fb);
                            cursor.cache_rows_kind = fb;
                        }
                        // Load cache-only coarser layers used to extend the historical prefix. Kind-5
                        // rows come from the recorder and possible deep-history writeback; the
                        // retained 5-minute snapshot is merged separately through `snap_part`.
                        if cp.tf_ms < 300_000 {
                            cursor.cache_rows_5m = read(5);
                        }
                        if cp.tf_ms < 86_400_000 {
                            cursor.cache_rows_1d = read(1440);
                        }
                        if complete {
                            cursor.cache_kind = Some(native_kind_min);
                            cursor.cache_from_ms = need_from;
                            cursor.cache_retry_at = None;
                        } else {
                            cursor.cache_retry_at = Some(Instant::now());
                        }
                    } else {
                        // No cache at all: nothing to retry, and the window IS loaded — as empty.
                        cursor.cache_kind = Some(native_kind_min);
                        cursor.cache_from_ms = need_from;
                    }
                    if !cursor.cache_rows.is_empty() {
                        log::log!(
                            super::SOURCE_TRACE_LEVEL,
                            "kline cache: префикс {market} kind{}: {} рядов",
                            cursor.cache_rows_kind,
                            cursor.cache_rows.len()
                        );
                    }
                }
            }
            // Backfill a coarse timeframe natively when the panel asks for a kind coarser than the
            // effective one-core timeframe and neither retained state nor the cache has native
            // depth. The core slot changes there and back as the freshness guard restores the
            // effective kind with backoff, while the response settles into the cache and avoids
            // future changes. Skip backfill without a cache because its result would be lost on
            // every restart while the slot changes remained.
            //
            // The request is deliberately NOT one-shot. It used to be, and a single disconnect then
            // killed that market's history for the whole process. It is instead claimed against a
            // bounded backoff with a claim budget, and stopped for good by the depth guard below
            // the moment the rows actually arrive.
            if use_deep && native_kind_min > deep_kind_min && kline_cache.is_some() {
                let native_kind = deep_history_kind(native_kind_min);
                let have_native = snapshot
                    .tf_candles(market, native_kind)
                    .map_or(false, |r| !r.is_empty());
                let cache_covers =
                    cursor.cache_rows_kind == native_kind_min && cursor.cache_rows.len() >= 30;
                if !have_native && !cache_covers {
                    let key = (provider, market.to_string(), native_kind_min);
                    let now_i = Instant::now();
                    // CLAIM the attempt under the locks, then send with both released. Two things
                    // ride on that order. The send is IPC to the core, and the sibling deep request
                    // below already refuses to make it under the source lock. And the claim is what
                    // keeps N panels of one coin from all sending on the same frame: recording only
                    // the OUTCOME would leave every panel seeing an absent entry at once, which is
                    // the herd the old unconditional insert prevented by accident.
                    let claimed = {
                        let inner = self.inner.read().expect("market source poisoned");
                        inner.native_backfill.claim(key, now_i)
                    };
                    // Nothing is written back after the send. `Ok` only means the core accepted the
                    // request into its queue; the outcome arrives asynchronously as a
                    // `CoinCardCandles` event, so marking the key done here would make an
                    // asynchronously failed or silently dropped request terminal. The guard above
                    // already stops the requests the moment the rows actually show up.
                    if let Some(delay_s) = claimed {
                        match client.candles().request_coin_card(market, native_kind) {
                            Ok(_) => log::log!(
                                super::SOURCE_TRACE_LEVEL,
                                "kline cache: native backfill queued {market} kind={native_kind:?}"
                            ),
                            Err(e) => super::market_diag(format!(
                                "native backfill request failed {market} kind={native_kind:?}: {e}; retrying in {delay_s}s"
                            )),
                        }
                    }
                }
            }
            // Native-backfill rows use the panel's native kind, which is coarser than the effective
            // kind, so they bypass the deep signature that tracks only the effective kind. Cache
            // them under a separate signature and force a prefix reread plus series rebuild so the
            // added depth appears immediately instead of in the next session.
            if use_deep && native_kind_min > deep_kind_min {
                if let (Some(cache), Some(ex)) = (kline_cache.as_ref(), exchange_key.as_ref()) {
                    let native_kind = deep_history_kind(native_kind_min);
                    if let Some(rows) = snapshot.tf_candles(market, native_kind) {
                        if !rows.is_empty() {
                            let sig = (rows.len() as u64).wrapping_mul(0x9e37_79b1)
                                ^ (rows.last().map_or(0, |r| r.unix_millis()) as u64);
                            if sig != cursor.cache_written_native_sig {
                                cursor.cache_written_native_sig = sig;
                                cache.merge(
                                    ex.clone(),
                                    market.to_string(),
                                    native_kind_min,
                                    rows.iter().map(deep_row_candle).collect(),
                                );
                                // The merge enters the FIFO queue before the future read, so the
                                // prefix reread sees the new rows.
                                cursor.cache_kind = None;
                                cursor.candle_series.invalidate();
                            }
                        }
                    }
                }
            }
            // Check base freshness on every pass, not only reset. When K is zero and the trade zone
            // is disabled, no resets occur between configuration changes, so a lost or expired
            // response used to freeze the series forever. Stale means the current bucket has no
            // row; the live subscription must maintain it, and a gap means a missed response or
            // disconnect. The core fetches deep history from the exchange API and consumes request
            // weight, so retries without progress use exponential backoff from 30 seconds to 10
            // minutes. New rows or a kind change reset the delay. A fixed 30-second retry against a
            // silent core or exchange previously triggered the core's API-limit auto-stop.
            if use_deep {
                let base_tf_native_ms = deep_kind_min as i64 * 60_000;
                let rows = snapshot.tf_candles(market, deep_kind);
                let now_unix_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_millis() as f64);
                let cur_bucket_ms =
                    crate::market::candles::bucket_open_ms(now_unix_ms, base_tf_native_ms);
                let deep_stale = rows
                    .and_then(|r| r.last())
                    .map_or(true, |r| (r.unix_millis() as f64) < cur_bucket_ms);
                let kind_changed = cursor.last_deep_kind != Some(deep_kind);
                let retry_delay =
                    Duration::from_secs(cursor.deep_retry_delay_s.max(HISTORY_RETRY_MIN_S) as u64);
                if deep_stale
                    && (kind_changed
                        || cursor
                            .last_deep_request
                            .map_or(true, |t| t.elapsed() > retry_delay))
                {
                    cursor.last_deep_request = Some(Instant::now());
                    cursor.last_deep_kind = Some(deep_kind);
                    // Add global deduplication above per-panel backoff. N panels for one coin share
                    // the retained response, so the application sends one `(coin, kind)` request
                    // every 30 seconds. A gated panel does not increase its backoff when another
                    // panel sent the request; its last_deep_request was already advanced above.
                    let gate_open = {
                        let inner = self.inner.read().expect("market source poisoned");
                        let mut gate = inner.deep_req_gate.lock().expect("deep req gate poisoned");
                        let key = (provider, market.to_string(), deep_kind_min);
                        let now_i = Instant::now();
                        match gate.get(&key) {
                            Some(t) if now_i.duration_since(*t) < Duration::from_secs(30) => false,
                            _ => {
                                gate.insert(key, now_i);
                                true
                            }
                        }
                    };
                    if gate_open {
                        cursor.deep_retry_delay_s = history_retry_next_delay_s(
                            (!kind_changed).then_some(cursor.deep_retry_delay_s),
                        );
                        if let Err(e) = client.candles().request_coin_card(market, deep_kind) {
                            super::market_diag(format!(
                                "coin-card request failed {market} kind={deep_kind:?}: {e}"
                            ));
                        }
                    }
                }
            }
            // Track a cheap deep-row fingerprint: row count plus the final timestamp. It detects a
            // new or advanced bucket, but not an in-place OHLC replacement at the same timestamp.
            // Such a replacement does not itself trigger rebuild or writeback; a trade-tail or
            // explicit reset can rebuild independently, while writeback waits for a later fingerprint
            // advance, normally a new bucket.
            let deep_rows_sig = if use_deep {
                snapshot.tf_candles(market, deep_kind).map_or(0u64, |rows| {
                    let last_ms = rows.last().map_or(0, |r| r.unix_millis());
                    (rows.len() as u64).wrapping_mul(0x9e37_79b1) ^ (last_ms as u64)
                })
            } else {
                0
            };
            if deep_rows_sig != cursor.last_deep_sig {
                // A response or live bar advanced the deep rows, so reset the request backoff.
                cursor.deep_retry_delay_s = HISTORY_RETRY_MIN_S;
            }
            let series_reset = force_reset
                || read.combo_reset
                || !cursor.candle_series.is_valid()
                || cursor.candle_series.tf_ms() != cp.tf_ms
                || (cursor.candle_trades.is_none() && trade_reader.is_some())
                || deep_rows_sig != cursor.last_deep_sig;
            if series_reset {
                cursor.last_deep_sig = deep_rows_sig;
                cursor.server_candle_rows.clear();
                cursor.server_candles.clear();
                let from_base_ms =
                    (epoch_ms + (from_rel_ms - cp.tf_ms.max(0) as f32) as f64) as i64;
                // Keep the base's right edge at least at now rather than at the window's right edge.
                // A reset while scrolling into the past used to truncate the base at that window.
                // Returning live does not reset because it only extends left, leaving a gap in the
                // middle from the historical position to today's live bucket until another pan.
                let now_unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as i64);
                let to_ms = ((epoch_ms + to_rel_ms as f64) as i64).max(now_unix);
                // Base 1 is CoinCard deep history with authoritative OHLC for the effective
                // one-core timeframe. Base 2 is the free 5-minute range-only snapshot, used as a
                // prefix older than the deep part or as the entire base until deep history arrives.
                // The composite therefore has older range candles and newer authoritative candles.
                // Timeframes below five minutes cannot downsample the snapshot and do not use it.
                // One merge always converts the resulting base to the series timeframe.
                let base_tf_ms = cp.tf_ms;
                let mut deep_part: Vec<ChartCandle> = Vec::new();
                if let Some(rows) = snapshot.tf_candles(market, deep_kind).filter(|_| use_deep) {
                    deep_part.extend(
                        rows.iter()
                            .filter(|r| {
                                let t = r.unix_millis();
                                t >= from_base_ms && t <= to_ms
                            })
                            .map(|r| {
                                let (open, high, low, close) =
                                    crate::market::candles::normalize_ohlc(
                                        r.open(),
                                        r.high(),
                                        r.low(),
                                        r.close(),
                                    );
                                ChartCandle {
                                    t_open_ms: r.unix_millis() as f64,
                                    open,
                                    high,
                                    low,
                                    close,
                                    volume: r.volume(),
                                }
                            }),
                    );
                }
                let have_deep = !deep_part.is_empty();
                let use_snap5 = cp.tf_ms >= 300_000 && cp.tf_ms % 300_000 == 0;
                let mut snap_part: Vec<ChartCandle> = Vec::new();
                if use_snap5 {
                    if let Some(r5) = readers.candles_5m.as_ref() {
                        let from5 =
                            moon_time_from_rel_ms(epoch_ms, from_rel_ms - cp.tf_ms.max(0) as f32);
                        r5.copy_time_range(
                            from5,
                            moonproto::MoonTime::from_unix_millis(to_ms),
                            r5.capacity(),
                            &mut cursor.server_candle_rows,
                        );
                        snap_part.extend(cursor.server_candle_rows.iter().map(|r| {
                            let (open, high, low, close) = crate::market::candles::normalize_ohlc(
                                r.open(),
                                r.high(),
                                r.low(),
                                r.close(),
                            );
                            ChartCandle {
                                // Rows in this ring are stamped at the END of their period —
                                // moonproto seals a 5-minute candle with the seal time, and the
                                // server's own snapshot is pushed in end-stamped as well. Shift
                                // back one period so the open aligns with every other base, which
                                // is what `detect_snapshot` already does for the same ring. Without
                                // it the whole snapshot layer sat one bucket late and its boundary
                                // rows resampled into the wrong coarse bucket.
                                t_open_ms: (r.time().unix_millis() - SNAP5_TF_MS) as f64,
                                open,
                                high,
                                low,
                                close,
                                volume: r.volume(),
                            }
                        }));
                        crate::market::candles::orient_range_rows(&mut snap_part);
                    }
                } else if cp.tf_ms < SNAP5_TF_MS {
                    // The same ring, kept as a COARSE FILL layer instead. A 1-minute series cannot
                    // resample a 5-minute bucket, so this ring contributes nothing to the base — but
                    // it is the only source that covers a stretch during which the CORE was running
                    // and the terminal was not, which is exactly the overnight hole a restart
                    // leaves. The local recorder cannot cover it: it aggregates from trade rings
                    // that only fill once this process is up.
                    cursor.ring_rows_5m.clear();
                    if let Some(r5) = readers.candles_5m.as_ref() {
                        r5.copy_time_range(
                            moonproto::MoonTime::from_unix_millis(from_base_ms),
                            moonproto::MoonTime::from_unix_millis(to_ms),
                            r5.capacity(),
                            &mut cursor.server_candle_rows,
                        );
                        cursor
                            .ring_rows_5m
                            .extend(cursor.server_candle_rows.iter().map(|r| {
                                let (open, high, low, close) =
                                    crate::market::candles::normalize_ohlc(
                                        r.open(),
                                        r.high(),
                                        r.low(),
                                        r.close(),
                                    );
                                ChartCandle {
                                    t_open_ms: (r.time().unix_millis() - SNAP5_TF_MS) as f64,
                                    open,
                                    high,
                                    low,
                                    close,
                                    volume: r.volume(),
                                }
                            }));
                        crate::market::candles::orient_range_rows(&mut cursor.ring_rows_5m);
                    }
                }
                // Use authoritative native klines from prior sessions as the visible cache portion.
                let cache_part: Vec<ChartCandle> = cursor
                    .cache_rows
                    .iter()
                    .filter(|c| {
                        let t = c.t_open_ms as i64;
                        t >= from_base_ms && t <= to_ms
                    })
                    .cloned()
                    .collect();
                // Write deep rows back to the cache without blocking when the cheap fingerprint
                // advances. Same-timestamp OHLC replacements do not advance it and remain unwritten
                // until a later bucket does. Persist the complete retained sequence rather than
                // `deep_part` clipped to the visible window; a narrow window used to save one
                // response candle and lose the remaining depth.
                if have_deep && deep_rows_sig != cursor.cache_written_sig {
                    match (kline_cache.as_ref(), exchange_key.as_ref()) {
                        (Some(cache), Some(ex)) => {
                            cursor.cache_written_sig = deep_rows_sig;
                            let full: Vec<ChartCandle> = snapshot
                                .tf_candles(market, deep_kind)
                                .map(|rows| rows.iter().map(deep_row_candle).collect())
                                .unwrap_or_default();
                            cache.merge(ex.clone(), market.to_string(), deep_kind_min, full);
                        }
                        (Some(_), None) => {
                            // The provider exchange identity is unavailable, so the cache cannot
                            // address these rows. Make this visible in the log once per panel. Keep
                            // the real signature unchanged, but use the initial marker to suppress
                            // repeated logging.
                            if cursor.cache_written_sig == 0 {
                                cursor.cache_written_sig = 1;
                                log::warn!(
                                    "kline cache: провайдер {provider} без ExchangeId — \
                                     ряды {market} не кэшируются"
                                );
                            }
                        }
                        _ => {}
                    }
                }
                // Merge every base source into the series timeframe in increasing priority:
                // 5-minute range-only snapshot < authoritative cached klines < live, freshest deep
                // history. Skip sources whose timeframe is coarser than or does not divide the
                // target, such as a 5-minute snapshot for a 1-minute series.
                {
                    let tf = cp.tf_ms;
                    let mut merged: std::collections::BTreeMap<i64, ChartCandle> =
                        std::collections::BTreeMap::new();
                    let mut scratch: Vec<ChartCandle> = Vec::new();
                    for (part, part_tf) in [
                        (&snap_part, 5 * 60_000i64),
                        (&cache_part, cursor.cache_rows_kind as i64 * 60_000),
                        (&deep_part, deep_kind_min as i64 * 60_000),
                    ] {
                        if part.is_empty() || part_tf <= 0 || tf < part_tf || tf % part_tf != 0 {
                            continue;
                        }
                        crate::market::candles::resample(part, tf, &mut scratch);
                        for c in scratch.drain(..) {
                            merged.insert(c.t_open_ms as i64, c);
                        }
                    }
                    cursor.server_candles.extend(merged.into_values());
                }
                cursor.candle_trade_rows.clear();
                if let Some(reader) = trade_reader.as_ref() {
                    // Extend the series tail through now, which is already included in to_ms. The
                    // follow-up cursor starts at now, so the copied range must reach the same point
                    // or a permanent gap remains between them.
                    reader.copy_time_range(
                        from_time,
                        moonproto::MoonTime::from_unix_millis(to_ms),
                        reader.capacity(),
                        &mut cursor.candle_trade_rows,
                    );
                    cursor.candle_trades = Some(reader.cursor_from_now());
                } else {
                    cursor.candle_trades = None;
                }
                rows_to_ticks(&cursor.candle_trade_rows, &mut cursor.candle_ticks);
                cursor.candle_series.rebuild(
                    cp.tf_ms,
                    &cursor.server_candles,
                    base_tf_ms,
                    &cursor.candle_ticks,
                );
            } else if let (Some(reader), Some(cur)) =
                (trade_reader.as_ref(), cursor.candle_trades.as_mut())
            {
                let meta =
                    reader.drain_new_bounded(cur, reader.capacity(), &mut cursor.candle_trade_rows);
                if meta.clipped {
                    // The cursor fell behind the ring; force a full rebuild on the next pass.
                    cursor.candle_series.invalidate();
                } else if meta.copied > 0 {
                    rows_to_ticks(&cursor.candle_trade_rows, &mut cursor.candle_ticks);
                    cursor.candle_series.push_trades(&cursor.candle_ticks);
                }
            }
            read.candles_revision = cursor.candle_series.revision();
            read.candles_changed =
                cursor.candle_series.is_valid() && read.candles_revision != cp.shipped_revision;
            // Compose the series with its cache-only coarser layers. The kind-5 layer comes from
            // the recorder and possible deep-history writeback, the daily layer from backfill and
            // cache; the retained `snap_part` separately feeds the main series. Fillers carry their
            // own timeframe for shader width and render muted against the selected one.
            //
            // Composed OUTSIDE the `candles_changed` branch on purpose: the auto-Y scan below runs
            // every frame while the upload runs only when the revision moved, so deriving the fill
            // in each of them is how the price scale and the drawn candles came to disagree.
            let fill_key = (read.candles_revision, cursor.cache_generation);
            if cursor.coarse_fill_key != Some(fill_key) {
                cursor.coarse_fill_key = Some(fill_key);
                let mut layers: Vec<crate::market::candles::CoarseLayer<'_>> = Vec::new();
                // Order is PRIORITY: each layer's coverage is subtracted before the next is
                // offered the remainder. The local cache goes first because its rows are
                // trade-derived with real OHLC; the core's ring is range-only, so it fills what
                // the cache could not — which after a restart is most of the night.
                for (rows, tf) in [
                    (&cursor.cache_rows_5m, 300_000.0f64),
                    (&cursor.ring_rows_5m, 300_000.0f64),
                    (&cursor.cache_rows_1d, 86_400_000.0f64),
                ] {
                    // A layer finer than or equal to the series has nothing to add: those rows
                    // already reach the series through `cache_part`/`snap_part` resampling, and
                    // re-adding them here would draw every bucket twice.
                    if (cp.tf_ms as f64) >= tf {
                        continue;
                    }
                    layers.push(crate::market::candles::CoarseLayer { rows, tf_ms: tf });
                }
                let mut fill = std::mem::take(&mut cursor.coarse_fill);
                crate::market::candles::compose_with_coarse(
                    cursor.candle_series.candles(),
                    cp.tf_ms as f64,
                    &layers,
                    &mut fill,
                );
                cursor.coarse_fill = fill;
            }
            if read.candles_changed {
                out.candles.reserve(cursor.coarse_fill.len());
                out.candle_tf_ms.reserve(cursor.coarse_fill.len());
                for (c, tf) in cursor.coarse_fill.iter() {
                    out.candles.push(*c);
                    out.candle_tf_ms.push(*tf);
                }
                // Diagnose candle-to-now gaps. If the last candle is older than three timeframes,
                // log layer coverage once every 30 seconds so the exhausted layer is identifiable
                // as series, deep, cache, 5-minute, or 1-day instead of debugging screenshots.
                let now_unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_millis() as f64);
                let last_ms = out.candles.last().map(|c| c.t_open_ms).unwrap_or(0.0);
                // Detect gaps inside the sequence where the next candle begins after the previous
                // one ends. This is where the scroll-to-history then return-to-live gap was hidden.
                let mut max_hole = 0.0f64;
                let mut hole_at = 0.0f64;
                for i in 1..out.candles.len() {
                    let prev = &out.candles[i - 1];
                    let prev_tf = out
                        .candle_tf_ms
                        .get(i - 1)
                        .copied()
                        .filter(|t| *t > 0.0)
                        .unwrap_or(cp.tf_ms as f32) as f64;
                    let hole = out.candles[i].t_open_ms - (prev.t_open_ms + prev_tf);
                    if hole > max_hole {
                        max_hole = hole;
                        hole_at = prev.t_open_ms + prev_tf;
                    }
                }
                if last_ms > 0.0
                    && (now_unix - last_ms > 3.0 * cp.tf_ms as f64
                        || max_hole > 3.0 * cp.tf_ms as f64)
                    && cursor
                        .last_gap_diag
                        .map_or(true, |t| t.elapsed() > Duration::from_secs(30))
                {
                    cursor.last_gap_diag = Some(Instant::now());
                    let ago_min = |ms: f64| ((now_unix - ms) / 60_000.0).round();
                    let span = |rows: &[ChartCandle]| match (rows.first(), rows.last()) {
                        (Some(f), Some(l)) => {
                            format!("{}м..{}м назад", ago_min(l.t_open_ms), ago_min(f.t_open_ms))
                        }
                        _ => "пусто".to_string(),
                    };
                    // The fill count is stated as "how many of the composed entries are NOT series
                    // candles", so a report distinguishes a fill that never ran from one that ran
                    // and fell short of the residual hole printed beside it.
                    let fill_n = cursor
                        .coarse_fill
                        .len()
                        .saturating_sub(cursor.candle_series.candles().len());
                    log::warn!(
                        "candle gap {market} tf={}с: последняя свеча {}м назад, макс. дыра \
                         {}м (кончается {}м назад); серия n={} \
                         [{}], заливка n={}, кэш kind{} n={} [{}], 5м n={} [{}], ринг5м n={} [{}],                          1д n={} [{}]",
                        cp.tf_ms / 1000,
                        ago_min(last_ms),
                        (max_hole / 60_000.0).round(),
                        ago_min(hole_at + max_hole),
                        cursor.candle_series.candles().len(),
                        span(cursor.candle_series.candles()),
                        fill_n,
                        cursor.cache_rows_kind,
                        cursor.cache_rows.len(),
                        span(&cursor.cache_rows),
                        cursor.cache_rows_5m.len(),
                        span(&cursor.cache_rows_5m),
                        cursor.ring_rows_5m.len(),
                        span(&cursor.ring_rows_5m),
                        cursor.cache_rows_1d.len(),
                        span(&cursor.cache_rows_1d),
                    );
                }
            }
            if scan_price {
                // Include visible candle highs and lows in automatic Y scaling. Trade crosses now
                // cover only their display zone, so older visible history would otherwise not affect
                // the scale.
                if let Some((lo, hi)) = cursor
                    .candle_series
                    .price_range(epoch_ms + from_rel_ms as f64, epoch_ms + to_rel_ms as f64)
                {
                    read.tick_price_range = Some(match read.tick_price_range {
                        Some((a, b)) => (a.min(lo), b.max(hi)),
                        None => (lo, hi),
                    });
                }
                // The coarse fillers are visible too, so include their highs and lows. Read from
                // the SAME composed vector the upload drew, and admit each entry through the one
                // visibility predicate the volume band and this scale already share — the scale
                // must never disagree with the drawn candles about which coarse rows exist.
                let from_abs = epoch_ms + from_rel_ms as f64;
                let to_abs = epoch_ms + to_rel_ms as f64;
                let series_tf = cp.tf_ms as f64;
                for (c, tf) in cursor.coarse_fill.iter() {
                    // Series candles are already covered by `price_range` above, and only a layer
                    // strictly coarser than the series is ever composed in.
                    if (*tf as f64) <= series_tf {
                        continue;
                    }
                    if crate::market::candles::candle_intersects_window(
                        c.t_open_ms,
                        *tf as f64,
                        from_abs,
                        to_abs,
                    ) {
                        read.tick_price_range = Some(match read.tick_price_range {
                            Some((a, b)) => (a.min(c.low), b.max(c.high)),
                            None => (c.low, c.high),
                        });
                    }
                }
            }
        }

        if let Some(reader) = readers.last_prices {
            drain_price_line(
                &reader,
                from_time,
                to_time,
                force_reset,
                &mut cursor.last_prices,
                &mut cursor.last_price_rows,
                &mut out.last_points,
                &mut read,
                price_rows_to_points,
            );
        } else {
            cursor.last_prices = None;
        }

        if let Some(reader) = readers.mark_prices {
            drain_price_line(
                &reader,
                from_time,
                to_time,
                force_reset,
                &mut cursor.mark_prices,
                &mut cursor.mark_price_rows,
                &mut out.mark_points,
                &mut read,
                price_rows_to_points,
            );
        } else {
            cursor.mark_prices = None;
        }

        read.last_price = cursor.last_price;
        Some(read)
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

/// Serve one chart-history read from the frozen bench instead of a live core.
///
/// The bench has no trade ring, no price lines and no order book: it answers with the candle
/// series alone, which is what the marker, order-line and drawing-figure work needs to look at.
///
/// The revision is derived from what was ASKED FOR rather than from incoming data, because the
/// data never changes. A chart ships its last revision back in `shipped_revision` and expects an
/// empty series when nothing moved; with a constant revision, panning or switching the timeframe
/// would be answered with "nothing changed" and the chart would keep drawing its first window.
///
/// A repeat read of an already-served window still returns the price fields from memory. The
/// caller stores `last_price` and `tick_price_range` unconditionally, so answering it with empty
/// ones would wipe the chart's Y reference on the very next frame — and, because panning re-reads
/// every frame while the revision only moves once per timeframe bucket, that is the common case.
///
/// Args:
///     fixture: The bench installed for this process.
///     cache: The application's candle cache, already open on the bench copy.
///     core: Chart's core, echoed back as the provider so callers keep one identity.
///     epoch_ms: Chart epoch the relative bounds are measured from.
///     from_rel_ms: Visible-window start relative to the epoch.
///     to_rel_ms: Visible-window end relative to the epoch.
///     candle_params: Series request; `None` means the caller wants no candles.
///     out: Buffers to fill.
///
/// Returns:
///     A read describing the served window.
#[allow(clippy::too_many_arguments)]
fn read_fixture_history(
    fixture: &crate::fixture::ChartFixture,
    cache: Option<&crate::market::kline_cache::KlineCache>,
    core: CoreId,
    epoch_ms: f64,
    from_rel_ms: f32,
    to_rel_ms: f32,
    candle_params: Option<&CandleReadParams>,
    out: &mut ChartHistoryBuffers,
) -> ChartHistoryRead {
    let mut read = ChartHistoryRead {
        provider: core,
        caught_up: true,
        ..ChartHistoryRead::default()
    };
    let Some(params) = candle_params else {
        return read;
    };
    // A non-finite bound would convert to a saturated or zero timestamp and silently ask for the
    // wrong window; there is nothing sensible to draw for one, so decline it instead.
    if !epoch_ms.is_finite() || !from_rel_ms.is_finite() || !to_rel_ms.is_finite() {
        return read;
    }
    let from_ms = (epoch_ms + from_rel_ms as f64).round() as i64;
    let to_ms = (epoch_ms + to_rel_ms.max(from_rel_ms) as f64).round() as i64;
    let to_ms = to_ms.max(from_ms + 1);
    let revision = fixture_revision(params.tf_ms, from_ms, to_ms);
    read.revision = revision;
    read.candles_revision = revision;
    // Whether the CALLER already holds this series decides if the series is sent — not whether the
    // bench happens to remember serving it. A second pane, a new tab, or a candles off→on toggle
    // arrives with `shipped_revision` reset, and answering it from the bench's memory would hand it
    // "nothing changed" plus an empty series, leaving it with no candles at all.
    if params.shipped_revision == revision {
        if let Some((last_price, price_range)) = fixture.served_window(revision) {
            read.last_price = last_price;
            read.tick_price_range = price_range;
        }
        return read;
    }
    let Some(cache) = cache else {
        // Still claim the reset: `resident_left_rel` is stamped only inside the caller's
        // combo-reset branch, and without it every later frame forces a full history re-read.
        read.combo_reset = true;
        return read;
    };
    out.candles = fixture.candles(cache, params.tf_ms, from_ms, to_ms);
    read.candles_changed = true;
    // The caller stamps `resident_left_rel` — its "how far left is this pane covered" mark — ONLY
    // inside its combo-reset branch. Without this flag it stays NaN, which the caller reads as
    // "coverage unknown" and forces a full history reset on EVERY frame.
    read.combo_reset = true;
    read.last_price = out.candles.last().map(|c| c.close);
    // The chart's automatic Y fit is built from the TICK price range — candles do not feed it. A
    // bench has no trade ring, so leaving this empty collapses the scale onto the single last
    // price and the whole series sits off-screen, which reads as "the chart is empty". The served
    // window's own low/high is the honest equivalent of what the ticks in it would have spanned.
    read.tick_price_range = out
        .candles
        .iter()
        .filter(|c| c.low.is_finite() && c.high.is_finite() && c.high > 0.0)
        .fold(None, |acc: Option<(f32, f32)>, c| {
            Some(match acc {
                None => (c.low, c.high),
                Some((lo, hi)) => (lo.min(c.low), hi.max(c.high)),
            })
        });
    fixture.remember_window(revision, read.last_price, read.tick_price_range);
    // One line per process, and only for a bench run: "the chart is open" and "the bench actually
    // answered it" are different facts, and without this the difference is invisible from outside.
    static ANNOUNCED: std::sync::Once = std::sync::Once::new();
    ANNOUNCED.call_once(|| {
        log::info!(
            "стенд {}: первая серия — {} свечей, ТФ {} мин, последняя цена {:?}",
            fixture.market(),
            out.candles.len(),
            params.tf_ms / 60_000,
            read.last_price
        );
    });
    read
}

/// Revision identifying one served bench window: timeframe plus the bucket-aligned bounds.
///
/// Aligning to the timeframe keeps the revision stable while a drag moves the window by less than
/// one candle, so an idle chart is not handed a fresh series on every frame.
fn fixture_revision(tf_ms: i64, from_ms: i64, to_ms: i64) -> u64 {
    let tf = tf_ms.max(1);
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for value in [tf, from_ms.div_euclid(tf), to_ms.div_euclid(tf)] {
        hash ^= value as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    // Zero is the default `shipped_revision` of a chart that has never been served; a window that
    // hashed to it would be answered with "nothing changed" on the very first read.
    hash.max(1)
}

#[cfg(test)]
mod tests;
