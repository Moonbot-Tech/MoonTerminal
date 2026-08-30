//! Public spot-candle providers and canonical historical/current rate routing.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{RateOrientation, RatePriceBasis, ResolvedRate, identity_rate};

#[cfg(test)]
mod tests;

/// Maximum lifetime of one Hyperliquid spot-universe snapshot.
const HYPERLIQUID_META_TTL: Duration = Duration::from_secs(5 * 60);

/// Route/range absence or transient failure from one provider request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FetchFailure {
    /// The symbol is invalid or the requested range currently contains no retained candle.
    Missing,
    /// Transport, rate-limit, service, or malformed-response failure that may recover.
    Transient(String),
}

/// One provider candle covering a requested UTC minute.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SpotCandle {
    /// Candle open time in Unix milliseconds.
    pub open_ms: i64,
    /// Candle close time in Unix milliseconds.
    pub close_ms: i64,
    /// Finite positive open price in the provider symbol's quote asset.
    pub open: f64,
    /// Finite positive close price in the provider symbol's quote asset.
    pub close: f64,
}

/// Boundary used by the worker to retrieve closed one-minute spot candles.
pub(crate) trait SpotRateSource: Send + Sync + 'static {
    /// Fetch closed one-minute candles over one inclusive UTC range.
    ///
    /// Args:
    ///     provider: Canonical Binance, Bybit, or Hyperliquid provider identifier.
    ///     symbol: Provider-native direct or inverse spot market.
    ///     start_minute_utc: First UTC candle-open minute in Unix seconds.
    ///     end_minute_utc: Last UTC candle-open minute in Unix seconds.
    ///
    /// Returns:
    ///     Available exact candles, route/range absence, or transient failure.
    fn candles(
        &self,
        provider: &'static str,
        symbol: &str,
        start_minute_utc: i64,
        end_minute_utc: i64,
    ) -> Result<Vec<SpotCandle>, FetchFailure>;

    /// Fetch the earliest closed candle at or after one UTC minute.
    ///
    /// Args:
    ///     provider: Canonical provider identifier.
    ///     symbol: Provider-native market or neutral `BASE/QUOTE` pair.
    ///     start_minute_utc: First eligible UTC candle-open minute.
    ///     end_minute_utc: Latest fully closed UTC candle-open minute.
    ///
    /// Returns:
    ///     Earliest retained candle, route/range absence, or transient failure.
    fn next_closed_candle(
        &self,
        provider: &'static str,
        symbol: &str,
        start_minute_utc: i64,
        end_minute_utc: i64,
    ) -> Result<SpotCandle, FetchFailure> {
        self.candles(provider, symbol, start_minute_utc, end_minute_utc)?
            .into_iter()
            .min_by_key(|candle| candle.open_ms)
            .ok_or(FetchFailure::Missing)
    }
}

/// Production HTTP implementation of the canonical spot-rate boundary.
pub(crate) struct HttpSpotRateSource {
    agent: ureq::Agent,
    /// Last public request start, shared across every provider route.
    last_request: Mutex<Option<Instant>>,
    /// Fetch time and Hyperliquid neutral pair to dynamic `@universe-index` mapping.
    hyperliquid_markets: Mutex<Option<(Instant, BTreeMap<String, String>)>>,
}

impl HttpSpotRateSource {
    /// Build an HTTPS-only client with a bounded global request timeout.
    ///
    /// Returns:
    ///     Public-market-data client suitable for the dedicated valuation worker.
    pub(crate) fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .https_only(true)
            .http_status_as_error(false)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            last_request: Mutex::new(None),
            hyperliquid_markets: Mutex::new(None),
        }
    }

    /// Pace public requests so sparse historical backfills cannot burst into exchange IP limits.
    fn pace_request(&self) {
        const MIN_INTERVAL: Duration = Duration::from_millis(100);
        let mut last = self
            .last_request
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(previous) = *last {
            std::thread::sleep(MIN_INTERVAL.saturating_sub(previous.elapsed()));
        }
        *last = Some(Instant::now());
    }

    /// Request one Binance Spot minute range and parse its array response.
    ///
    /// Args:
    ///     symbol: Uppercase spot symbol.
    ///     start_minute_utc: First UTC minute start in Unix seconds.
    ///     end_minute_utc: Last UTC minute start in Unix seconds.
    ///
    /// Returns:
    ///     Available candles or classified route failure.
    fn binance(
        &self,
        symbol: &str,
        start_minute_utc: i64,
        end_minute_utc: i64,
    ) -> Result<Vec<SpotCandle>, FetchFailure> {
        self.pace_request();
        let start_ms = start_minute_utc.saturating_mul(1_000);
        let end_ms = end_minute_utc.saturating_mul(1_000).saturating_add(59_999);
        let limit = range_limit(start_minute_utc, end_minute_utc);
        let response = self
            .agent
            .get("https://data-api.binance.vision/api/v3/klines")
            .query("symbol", symbol)
            .query("interval", "1m")
            .query("startTime", &start_ms.to_string())
            .query("endTime", &end_ms.to_string())
            .query("limit", &limit.to_string())
            .call()
            .map_err(classify_http_error)?;
        let status = response.status().as_u16();
        let value: Value = response
            .into_body()
            .read_json()
            .map_err(|error| FetchFailure::Transient(format!("binance JSON: {error}")))?;
        classify_binance_status(status, &value)?;
        parse_binance(&value, start_minute_utc, end_minute_utc)
    }

    /// Request one Bybit Spot minute range and parse its reverse-ordered envelope.
    ///
    /// Args:
    ///     symbol: Uppercase spot symbol.
    ///     start_minute_utc: First UTC minute start in Unix seconds.
    ///     end_minute_utc: Last UTC minute start in Unix seconds.
    ///
    /// Returns:
    ///     Available candles or classified route failure.
    fn bybit(
        &self,
        symbol: &str,
        start_minute_utc: i64,
        end_minute_utc: i64,
    ) -> Result<Vec<SpotCandle>, FetchFailure> {
        self.pace_request();
        let start_ms = start_minute_utc.saturating_mul(1_000);
        let end_ms = end_minute_utc.saturating_mul(1_000).saturating_add(59_999);
        let limit = range_limit(start_minute_utc, end_minute_utc);
        let response = self
            .agent
            .get("https://api.bybit.com/v5/market/kline")
            .query("category", "spot")
            .query("symbol", symbol)
            .query("interval", "1")
            .query("start", &start_ms.to_string())
            .query("end", &end_ms.to_string())
            .query("limit", &limit.to_string())
            .call()
            .map_err(classify_http_error)?;
        let status = response.status().as_u16();
        let value: Value = response
            .into_body()
            .read_json()
            .map_err(|error| FetchFailure::Transient(format!("bybit JSON: {error}")))?;
        if !(200..300).contains(&status) {
            return Err(FetchFailure::Transient(format!("bybit HTTP {status}")));
        }
        parse_bybit(&value, start_minute_utc, end_minute_utc)
    }

    /// Execute one paced Hyperliquid public-info request.
    ///
    /// Args:
    ///     payload: JSON request body accepted by the public info endpoint.
    ///
    /// Returns:
    ///     Decoded successful response or a classified provider failure.
    fn hyperliquid_info(&self, payload: Value) -> Result<Value, FetchFailure> {
        self.pace_request();
        let response = self
            .agent
            .post("https://api.hyperliquid.xyz/info")
            .send_json(payload)
            .map_err(classify_http_error)?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(FetchFailure::Transient(format!(
                "hyperliquid HTTP {status}"
            )));
        }
        response
            .into_body()
            .read_json()
            .map_err(|error| FetchFailure::Transient(format!("hyperliquid JSON: {error}")))
    }

    /// Discover one Hyperliquid spot market without relying on unstable provider indexes.
    ///
    /// Args:
    ///     pair: Neutral uppercase `BASE/QUOTE` pair.
    ///
    /// Returns:
    ///     Provider candle identifier for the requested pair.
    fn hyperliquid_coin(&self, pair: &str) -> Result<String, FetchFailure> {
        let mut cached = self
            .hyperliquid_markets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((fetched_at, markets)) = cached.as_ref() {
            if fetched_at.elapsed() < HYPERLIQUID_META_TTL {
                return markets.get(pair).cloned().ok_or(FetchFailure::Missing);
            }
        }
        let value = self.hyperliquid_info(serde_json::json!({"type": "spotMeta"}))?;
        let markets = parse_hyperliquid_markets(&value)?;
        let coin = markets.get(pair).cloned();
        *cached = Some((Instant::now(), markets));
        coin.ok_or(FetchFailure::Missing)
    }

    /// Fetch Hyperliquid candles for one dynamically discovered neutral pair.
    ///
    /// Args:
    ///     pair: Neutral uppercase `BASE/QUOTE` pair.
    ///     start_minute_utc: First eligible UTC minute.
    ///     end_minute_utc: Latest eligible UTC minute.
    ///
    /// Returns:
    ///     Provider-retained candles in the requested range.
    fn hyperliquid(
        &self,
        pair: &str,
        start_minute_utc: i64,
        end_minute_utc: i64,
    ) -> Result<Vec<SpotCandle>, FetchFailure> {
        let coin = self.hyperliquid_coin(pair)?;
        let value = self.hyperliquid_info(serde_json::json!({
            "type": "candleSnapshot",
            "req": {
                "coin": coin,
                "interval": "1m",
                "startTime": start_minute_utc.saturating_mul(1_000),
                "endTime": end_minute_utc.saturating_mul(1_000).saturating_add(59_999)
            }
        }))?;
        parse_hyperliquid(&value, start_minute_utc, end_minute_utc)
    }

    /// Find Bybit's earliest retained candle without scanning every 1,000-minute window.
    ///
    /// Args:
    ///     symbol: Uppercase spot symbol.
    ///     start_minute_utc: First eligible UTC minute.
    ///     end_minute_utc: Latest eligible UTC minute.
    ///
    /// Returns:
    ///     Earliest retained candle in the interval.
    fn bybit_next(
        &self,
        symbol: &str,
        start_minute_utc: i64,
        end_minute_utc: i64,
    ) -> Result<SpotCandle, FetchFailure> {
        let mut low = start_minute_utc;
        let mut high = end_minute_utc;
        let mut candidate = self
            .bybit(symbol, low, high)?
            .into_iter()
            .min_by_key(|candle| candle.open_ms)
            .ok_or(FetchFailure::Missing)?;
        if candidate.open_ms.div_euclid(1_000) == start_minute_utc {
            return Ok(candidate);
        }
        while low < high {
            let span_minutes = high.saturating_sub(low).div_euclid(60);
            let mid = low.saturating_add(span_minutes.div_euclid(2).saturating_mul(60));
            match self.bybit(symbol, start_minute_utc, mid) {
                Ok(candles) => {
                    if let Some(found) = candles.into_iter().min_by_key(|candle| candle.open_ms) {
                        candidate = found;
                        high = mid;
                    } else {
                        low = mid.saturating_add(60);
                    }
                }
                Err(FetchFailure::Missing) => low = mid.saturating_add(60),
                Err(error) => return Err(error),
            }
        }
        Ok(candidate)
    }
}

impl Default for HttpSpotRateSource {
    /// Build the production public spot-rate source.
    ///
    /// Returns:
    ///     Source configured for the Binance, Bybit, and Hyperliquid public spot endpoints.
    fn default() -> Self {
        Self::new()
    }
}

impl SpotRateSource for HttpSpotRateSource {
    /// Fetch one exact provider range without applying direct/inverse routing.
    ///
    /// Args:
    ///     provider: Canonical provider identifier.
    ///     symbol: Provider-native spot symbol.
    ///     start_minute_utc: First requested UTC minute.
    ///     end_minute_utc: Last requested UTC minute.
    ///
    /// Returns:
    ///     Closed spot candles inside the requested inclusive range.
    ///
    /// Errors:
    ///     Returns route/range absence or a transient provider/transport failure.
    fn candles(
        &self,
        provider: &'static str,
        symbol: &str,
        start_minute_utc: i64,
        end_minute_utc: i64,
    ) -> Result<Vec<SpotCandle>, FetchFailure> {
        match provider {
            "binance_spot" => self.binance(symbol, start_minute_utc, end_minute_utc),
            "bybit_spot" => self.bybit(symbol, start_minute_utc, end_minute_utc),
            "hyperliquid_spot" => self.hyperliquid(symbol, start_minute_utc, end_minute_utc),
            other => Err(FetchFailure::Transient(format!(
                "unsupported provider {other}"
            ))),
        }
    }

    /// Fetch one production route's earliest retained closed candle efficiently.
    fn next_closed_candle(
        &self,
        provider: &'static str,
        symbol: &str,
        start_minute_utc: i64,
        end_minute_utc: i64,
    ) -> Result<SpotCandle, FetchFailure> {
        match provider {
            "binance_spot" => self
                .binance(symbol, start_minute_utc, end_minute_utc)?
                .into_iter()
                .min_by_key(|candle| candle.open_ms)
                .ok_or(FetchFailure::Missing),
            "bybit_spot" => self.bybit_next(symbol, start_minute_utc, end_minute_utc),
            "hyperliquid_spot" => self
                .hyperliquid(symbol, start_minute_utc, end_minute_utc)?
                .into_iter()
                .min_by_key(|candle| candle.open_ms)
                .ok_or(FetchFailure::Missing),
            other => Err(FetchFailure::Transient(format!(
                "unsupported provider {other}"
            ))),
        }
    }
}

/// Maximum candle count accepted by both canonical provider endpoints.
const MAX_RANGE_MINUTES: usize = 1_000;

/// Bound an inclusive provider request to the supported candle count.
///
/// Args:
///     start_minute_utc: First requested UTC minute.
///     end_minute_utc: Last requested UTC minute.
///
/// Returns:
///     Inclusive count clamped to the shared provider limit.
fn range_limit(start_minute_utc: i64, end_minute_utc: i64) -> usize {
    end_minute_utc
        .saturating_sub(start_minute_utc)
        .div_euclid(60)
        .saturating_add(1)
        .clamp(1, MAX_RANGE_MINUTES as i64) as usize
}

/// Resolve the newest available closed rate inside one current-rate window.
///
/// Unlike historical valuation, current valuation needs the latest known price rather than one
/// exact minute. Canonical route priority still wins over recency across routes: the newest candle
/// from the first route with data is used, so an inverse or fallback market cannot outrank an
/// available direct market.
///
/// Args:
///     source: Public candle boundary.
///     quote_ordinal: Persisted MoonBot quote ordinal.
///     quote_ticker: Uppercase neutral quote ticker.
///     start_minute_utc: Oldest eligible closed UTC minute start.
///     end_minute_utc: Newest eligible closed UTC minute start.
///
/// Returns:
///     Newest validated rate on the first available route, route/range absence after every route,
///     or a transient failure that stops fallback.
pub(crate) fn resolve_latest_rate(
    source: &dyn SpotRateSource,
    quote_ordinal: i64,
    quote_ticker: &str,
    start_minute_utc: i64,
    end_minute_utc: i64,
) -> Result<ResolvedRate, FetchFailure> {
    if quote_ticker == "USDT" {
        return Ok(identity_rate(quote_ordinal, end_minute_utc));
    }
    for (provider, symbol, orientation) in canonical_routes(quote_ticker) {
        match source.candles(provider, &symbol, start_minute_utc, end_minute_utc) {
            Ok(candles) => {
                let Some(candle) = candles.into_iter().max_by_key(|candle| candle.open_ms) else {
                    continue;
                };
                let rate_usdt =
                    validated_market_rate(candle.close, orientation).map_err(|error| {
                        FetchFailure::Transient(route_transient(provider, &symbol, error))
                    })?;
                return Ok(ResolvedRate {
                    quote_ordinal,
                    minute_utc: candle.open_ms.div_euclid(60_000) * 60,
                    resolved_minute_utc: candle.open_ms.div_euclid(60_000) * 60,
                    rate_usdt,
                    provider: provider.to_string(),
                    symbol,
                    orientation,
                    price_basis: RatePriceBasis::ExactClose,
                    candle_open_ms: candle.open_ms,
                    candle_close_ms: candle.close_ms,
                    leg2_provider: None,
                    leg2_symbol: None,
                    leg2_orientation: None,
                    leg1_rate: rate_usdt,
                    leg2_rate: None,
                });
            }
            Err(FetchFailure::Missing) => continue,
            Err(FetchFailure::Transient(error)) => {
                return Err(FetchFailure::Transient(route_transient(
                    provider, &symbol, error,
                )));
            }
        }
    }
    Err(FetchFailure::Missing)
}

/// Batch result that preserves successful canonical routes before a later transient failure.
pub(crate) struct RateBatch {
    /// Validated rates resolved for requested minutes.
    pub ready: Vec<ResolvedRate>,
    /// Requested minutes proven absent across every route.
    pub missing: Vec<i64>,
    /// Transient failure that stopped fallback for remaining minutes.
    pub transient: Option<String>,
}

/// Resolve many minutes for one quote with one range request per canonical route.
///
/// The caller supplies at most one provider-sized window. Sparse provider results continue through
/// inverse and fallback routes only for unresolved minutes, while already resolved rates remain
/// usable if a later route fails transiently.
///
/// Args:
///     source: Public candle boundary.
///     quote_ordinal: Persisted MoonBot quote ordinal.
///     quote_ticker: Uppercase neutral quote ticker.
///     minutes: Sorted or unsorted closed UTC minute starts within one 1,000-minute span.
///
/// Returns:
///     Ready exact rates, unresolved exact minutes, and an optional transient stop reason.
pub(crate) fn resolve_rate_batch(
    source: &dyn SpotRateSource,
    quote_ordinal: i64,
    quote_ticker: &str,
    minutes: &[i64],
) -> RateBatch {
    let mut unresolved = minutes.iter().copied().collect::<BTreeSet<_>>();
    if quote_ticker == "USDT" {
        return RateBatch {
            ready: unresolved
                .into_iter()
                .map(|minute_utc| identity_rate(quote_ordinal, minute_utc))
                .collect(),
            missing: Vec::new(),
            transient: None,
        };
    }
    let Some(start_minute) = unresolved.first().copied() else {
        return RateBatch {
            ready: Vec::new(),
            missing: Vec::new(),
            transient: None,
        };
    };
    let end_minute = unresolved.last().copied().unwrap_or(start_minute);
    let mut ready = Vec::new();
    for (provider, symbol, orientation) in canonical_routes(quote_ticker) {
        if unresolved.is_empty() {
            break;
        }
        match source.candles(provider, &symbol, start_minute, end_minute) {
            Ok(candles) => {
                let by_minute = candles
                    .into_iter()
                    .map(|candle| (candle.open_ms.div_euclid(60_000) * 60, candle))
                    .collect::<BTreeMap<_, _>>();
                let matched = unresolved
                    .iter()
                    .filter_map(|minute| by_minute.get(minute).map(|candle| (*minute, *candle)))
                    .collect::<Vec<_>>();
                for (minute_utc, candle) in matched {
                    let rate_usdt = match validated_market_rate(candle.close, orientation) {
                        Ok(rate) => rate,
                        Err(error) => {
                            return RateBatch {
                                ready,
                                missing: Vec::new(),
                                transient: Some(route_transient(provider, &symbol, error)),
                            };
                        }
                    };
                    ready.push(ResolvedRate {
                        quote_ordinal,
                        minute_utc,
                        resolved_minute_utc: minute_utc,
                        rate_usdt,
                        provider: provider.to_string(),
                        symbol: symbol.clone(),
                        orientation,
                        price_basis: RatePriceBasis::ExactClose,
                        candle_open_ms: candle.open_ms,
                        candle_close_ms: candle.close_ms,
                        leg2_provider: None,
                        leg2_symbol: None,
                        leg2_orientation: None,
                        leg1_rate: rate_usdt,
                        leg2_rate: None,
                    });
                    unresolved.remove(&minute_utc);
                }
            }
            Err(FetchFailure::Missing) => continue,
            Err(FetchFailure::Transient(error)) => {
                return RateBatch {
                    ready,
                    missing: Vec::new(),
                    transient: Some(route_transient(provider, &symbol, error)),
                };
            }
        }
    }
    RateBatch {
        ready,
        missing: unresolved.into_iter().collect(),
        transient: None,
    }
}

/// Build the canonical direct, inverse, and provider fallback order for one quote.
///
/// Args:
///     quote_ticker: Uppercase neutral quote ticker.
///
/// Returns:
///     Binance direct/inverse followed by Bybit direct/inverse.
fn canonical_routes(quote_ticker: &str) -> [(&'static str, String, RateOrientation); 4] {
    [
        (
            "binance_spot",
            format!("{quote_ticker}USDT"),
            RateOrientation::Direct,
        ),
        (
            "binance_spot",
            format!("USDT{quote_ticker}"),
            RateOrientation::Inverse,
        ),
        (
            "bybit_spot",
            format!("{quote_ticker}USDT"),
            RateOrientation::Direct,
        ),
        (
            "bybit_spot",
            format!("USDT{quote_ticker}"),
            RateOrientation::Inverse,
        ),
    ]
}

/// Attach the canonical route to one transient provider detail.
///
/// Args:
///     provider: Canonical provider identifier.
///     symbol: Provider-native spot symbol.
///     detail: Transport, parser, service, or validation detail.
///
/// Returns:
///     Safe diagnostic text naming the route that stopped fallback.
fn route_transient(provider: &str, symbol: &str, detail: String) -> String {
    format!("{provider} {symbol}: {detail}")
}

/// Validate and orient one provider price into quote units per base unit.
///
/// Args:
///     price: Provider open or close price.
///     orientation: Direct or inverse route direction.
///
/// Returns:
///     Finite positive USDT rate, or a transient-data error description.
pub(super) fn validated_market_rate(
    price: f64,
    orientation: RateOrientation,
) -> Result<f64, String> {
    if !price.is_finite() || price <= 0.0 {
        return Err(format!("invalid market price {price}"));
    }
    let rate = match orientation {
        RateOrientation::Direct => price,
        RateOrientation::Inverse => 1.0 / price,
        RateOrientation::Identity => return Err("identity entered market routing".to_string()),
    };
    if rate.is_finite() && rate > 0.0 {
        Ok(rate)
    } else {
        Err(format!("invalid oriented rate {rate}"))
    }
}

/// Classify one ureq failure without turning service or rate-limit errors into permanent misses.
///
/// Args:
///     error: HTTP client failure.
///
/// Returns:
///     Missing only for client responses that prove the route is invalid; transient otherwise.
fn classify_http_error(error: ureq::Error) -> FetchFailure {
    FetchFailure::Transient(error.to_string())
}

/// Classify a Binance HTTP response using its structured exchange error code.
///
/// Args:
///     status: HTTP response status.
///     value: Parsed response body.
///
/// Returns:
///     Success for 2xx, route absence only for Binance's invalid-symbol code, and transient
///     failure for every other response.
fn classify_binance_status(status: u16, value: &Value) -> Result<(), FetchFailure> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    if value.get("code").and_then(Value::as_i64) == Some(-1121) {
        Err(FetchFailure::Missing)
    } else {
        Err(FetchFailure::Transient(format!("binance HTTP {status}")))
    }
}

/// Parse Binance kline rows and retain exact one-minute candles inside the requested range.
///
/// Args:
///     value: JSON response body.
///     start_minute_utc: First requested UTC minute start in Unix seconds.
///     end_minute_utc: Last requested UTC minute start in Unix seconds.
///
/// Returns:
///     Validated candles, range absence when no exact candle remains, or a classified
///     structural response failure.
fn parse_binance(
    value: &Value,
    start_minute_utc: i64,
    end_minute_utc: i64,
) -> Result<Vec<SpotCandle>, FetchFailure> {
    let rows = value
        .as_array()
        .ok_or_else(|| FetchFailure::Transient("binance response is not an array".to_string()))?;
    if rows.is_empty() {
        return Err(FetchFailure::Missing);
    }
    let start_ms = start_minute_utc.saturating_mul(1_000);
    let end_ms = end_minute_utc.saturating_mul(1_000);
    let mut candles = Vec::new();
    for value in rows {
        let row = value
            .as_array()
            .ok_or_else(|| FetchFailure::Transient("binance candle is not an array".to_string()))?;
        let open_ms = row.first().and_then(Value::as_i64).ok_or_else(|| {
            FetchFailure::Transient("binance candle has no integer open time".to_string())
        })?;
        if open_ms < start_ms || open_ms > end_ms || open_ms.rem_euclid(60_000) != 0 {
            continue;
        }
        let open = row
            .get(1)
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<f64>().ok())
            .ok_or_else(|| {
                FetchFailure::Transient("binance candle has no numeric open".to_string())
            })?;
        let close = row
            .get(4)
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<f64>().ok())
            .ok_or_else(|| {
                FetchFailure::Transient("binance candle has no numeric close".to_string())
            })?;
        let close_ms = row.get(6).and_then(Value::as_i64).ok_or_else(|| {
            FetchFailure::Transient("binance candle has no integer close time".to_string())
        })?;
        if close_ms != open_ms.saturating_add(59_999) {
            // A duration outlier cannot price this minute, but valid sibling rows and canonical
            // fallback routes must remain usable instead of stalling the whole reconciliation.
            continue;
        }
        candles.push(SpotCandle {
            open_ms,
            close_ms,
            open,
            close,
        });
    }
    if candles.is_empty() {
        Err(FetchFailure::Missing)
    } else {
        Ok(candles)
    }
}

/// Parse Bybit kline rows and retain exact minutes inside the requested range.
///
/// Args:
///     value: JSON response body.
///     start_minute_utc: First requested UTC minute start in Unix seconds.
///     end_minute_utc: Last requested UTC minute start in Unix seconds.
///
/// Returns:
///     Validated candles or classified response failure.
fn parse_bybit(
    value: &Value,
    start_minute_utc: i64,
    end_minute_utc: i64,
) -> Result<Vec<SpotCandle>, FetchFailure> {
    let code = value
        .get("retCode")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            FetchFailure::Transient("bybit response has no integer retCode".to_string())
        })?;
    if code != 0 {
        let message = value.get("retMsg").and_then(Value::as_str).map(str::trim);
        return if code == 10029 || (code == 10001 && message == Some("Not supported symbols")) {
            Err(FetchFailure::Missing)
        } else {
            let detail = message.filter(|message| !message.is_empty()).map_or_else(
                || format!("bybit retCode {code}: missing retMsg"),
                |message| format!("bybit retCode {code}: {message}"),
            );
            Err(FetchFailure::Transient(detail))
        };
    }
    let rows = value
        .pointer("/result/list")
        .and_then(Value::as_array)
        .ok_or_else(|| FetchFailure::Transient("bybit result.list is not an array".to_string()))?;
    if rows.is_empty() {
        return Err(FetchFailure::Missing);
    }
    let start_ms = start_minute_utc.saturating_mul(1_000);
    let end_ms = end_minute_utc.saturating_mul(1_000);
    let mut candles = Vec::new();
    for value in rows {
        let row = value
            .as_array()
            .ok_or_else(|| FetchFailure::Transient("bybit candle is not an array".to_string()))?;
        let open_ms = row
            .first()
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or_else(|| {
                FetchFailure::Transient("bybit candle has no integer open time".to_string())
            })?;
        if open_ms < start_ms || open_ms > end_ms || open_ms.rem_euclid(60_000) != 0 {
            continue;
        }
        let open = row
            .get(1)
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<f64>().ok())
            .ok_or_else(|| {
                FetchFailure::Transient("bybit candle has no numeric open".to_string())
            })?;
        let close = row
            .get(4)
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<f64>().ok())
            .ok_or_else(|| {
                FetchFailure::Transient("bybit candle has no numeric close".to_string())
            })?;
        candles.push(SpotCandle {
            open_ms,
            close_ms: open_ms.saturating_add(59_999),
            open,
            close,
        });
    }
    if candles.is_empty() {
        Err(FetchFailure::Missing)
    } else {
        Ok(candles)
    }
}

/// Parse Hyperliquid spot metadata into neutral pair identifiers.
///
/// Args:
///     value: `spotMeta` response body.
///
/// Returns:
///     Neutral `BASE/QUOTE` pairs mapped to dynamic `@universe-index` identifiers.
fn parse_hyperliquid_markets(value: &Value) -> Result<BTreeMap<String, String>, FetchFailure> {
    let tokens = value
        .get("tokens")
        .and_then(Value::as_array)
        .ok_or_else(|| FetchFailure::Transient("hyperliquid spotMeta has no tokens".to_string()))?;
    let mut names = BTreeMap::new();
    for token in tokens {
        let index = token.get("index").and_then(Value::as_u64).ok_or_else(|| {
            FetchFailure::Transient("hyperliquid spotMeta token has no index".to_string())
        })?;
        let name = token.get("name").and_then(Value::as_str).ok_or_else(|| {
            FetchFailure::Transient("hyperliquid spotMeta token has no name".to_string())
        })?;
        if names.insert(index, name.to_string()).is_some() {
            return Err(FetchFailure::Transient(format!(
                "hyperliquid spotMeta repeats token index {index}"
            )));
        }
    }
    let universe = value
        .get("universe")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            FetchFailure::Transient("hyperliquid spotMeta has no universe".to_string())
        })?;
    let mut markets = BTreeMap::new();
    for market in universe {
        let pair = market
            .get("tokens")
            .and_then(Value::as_array)
            .filter(|pair| pair.len() == 2)
            .ok_or_else(|| {
                FetchFailure::Transient("hyperliquid spotMeta market has no token pair".to_string())
            })?;
        let base = pair[0].as_u64().and_then(|index| names.get(&index));
        let quote = pair[1].as_u64().and_then(|index| names.get(&index));
        let index = market.get("index").and_then(Value::as_u64);
        let (Some(base), Some(quote), Some(index)) = (base, quote, index) else {
            return Err(FetchFailure::Transient(
                "hyperliquid spotMeta market references an invalid token".to_string(),
            ));
        };
        let pair = format!("{base}/{quote}");
        if markets.insert(pair.clone(), format!("@{index}")).is_some() {
            return Err(FetchFailure::Transient(format!(
                "hyperliquid spotMeta repeats pair {pair}"
            )));
        }
    }
    Ok(markets)
}

/// Parse Hyperliquid candle objects retained inside one requested range.
///
/// Args:
///     value: `candleSnapshot` response body.
///     start_minute_utc: First eligible UTC minute.
///     end_minute_utc: Last eligible UTC minute.
///
/// Returns:
///     Valid one-minute candles, or range absence when none remain.
fn parse_hyperliquid(
    value: &Value,
    start_minute_utc: i64,
    end_minute_utc: i64,
) -> Result<Vec<SpotCandle>, FetchFailure> {
    let rows = value.as_array().ok_or_else(|| {
        FetchFailure::Transient("hyperliquid candle response is not an array".to_string())
    })?;
    let start_ms = start_minute_utc.saturating_mul(1_000);
    let end_ms = end_minute_utc.saturating_mul(1_000);
    let mut candles = Vec::new();
    for row in rows {
        let open_ms = row.get("t").and_then(Value::as_i64).ok_or_else(|| {
            FetchFailure::Transient("hyperliquid candle has no integer open time".to_string())
        })?;
        if open_ms < start_ms || open_ms > end_ms || open_ms.rem_euclid(60_000) != 0 {
            continue;
        }
        let close_ms = row.get("T").and_then(Value::as_i64).ok_or_else(|| {
            FetchFailure::Transient("hyperliquid candle has no integer close time".to_string())
        })?;
        if close_ms != open_ms.saturating_add(59_999) {
            continue;
        }
        let parse_price = |field: &str| {
            row.get(field)
                .and_then(Value::as_str)
                .and_then(|price| price.parse::<f64>().ok())
                .ok_or_else(|| {
                    FetchFailure::Transient(format!("hyperliquid candle has no numeric {field}"))
                })
        };
        candles.push(SpotCandle {
            open_ms,
            close_ms,
            open: parse_price("o")?,
            close: parse_price("c")?,
        });
    }
    if candles.is_empty() {
        Err(FetchFailure::Missing)
    } else {
        Ok(candles)
    }
}
