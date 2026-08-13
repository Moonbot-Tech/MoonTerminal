//! Public spot-candle providers and canonical historical/current rate routing.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{RateOrientation, ResolvedRate};

#[cfg(test)]
mod tests;

/// Permanent absence or transient failure from one provider route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FetchFailure {
    /// The symbol or requested closed candle is definitively unavailable.
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
    /// Finite positive close price in the provider symbol's quote asset.
    pub close: f64,
}

/// Boundary used by the worker to retrieve closed one-minute spot candles.
pub(crate) trait SpotRateSource: Send + Sync + 'static {
    /// Fetch closed one-minute candles over one inclusive UTC range.
    ///
    /// Args:
    ///     provider: `binance_spot` or `bybit_spot`.
    ///     symbol: Uppercase direct or inverse spot market.
    ///     start_minute_utc: First UTC candle-open minute in Unix seconds.
    ///     end_minute_utc: Last UTC candle-open minute in Unix seconds.
    ///
    /// Returns:
    ///     Available exact candles, permanent route absence, or transient failure.
    fn candles(
        &self,
        provider: &'static str,
        symbol: &str,
        start_minute_utc: i64,
        end_minute_utc: i64,
    ) -> Result<Vec<SpotCandle>, FetchFailure>;
}

/// Production HTTP implementation of the canonical spot-rate boundary.
pub(crate) struct HttpSpotRateSource {
    agent: ureq::Agent,
    /// Last public request start, shared across Binance and Bybit routes.
    last_request: Mutex<Option<Instant>>,
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
}

impl Default for HttpSpotRateSource {
    /// Build the production public spot-rate source.
    ///
    /// Returns:
    ///     Source configured for the canonical Binance and Bybit public spot endpoints.
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
    ///     Returns a permanent absence or transient provider/transport failure.
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

/// Resolve one quote minute through Binance direct/inverse, then Bybit direct/inverse.
///
/// Transient failure stops the route: falling through to another provider would turn an outage or
/// throttling response into a permanent negative cache. Only proven symbol/candle absence advances.
///
/// Args:
///     source: Public candle boundary.
///     quote_ordinal: Persisted MoonBot quote ordinal.
///     quote_ticker: Uppercase neutral quote ticker.
///     minute_utc: Closed UTC minute start in Unix seconds.
///
/// Returns:
///     Validated rate, permanent absence after all routes, or transient failure.
pub(crate) fn resolve_rate(
    source: &dyn SpotRateSource,
    quote_ordinal: i64,
    quote_ticker: &str,
    minute_utc: i64,
) -> Result<ResolvedRate, FetchFailure> {
    let batch = resolve_rate_batch(source, quote_ordinal, quote_ticker, &[minute_utc]);
    if let Some(rate) = batch.ready.into_iter().next() {
        return Ok(rate);
    }
    if let Some(error) = batch.transient {
        return Err(FetchFailure::Transient(error));
    }
    Err(FetchFailure::Missing)
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
///     Newest validated rate on the first available route, permanent absence after every route,
///     or a transient failure that stops fallback.
pub(crate) fn resolve_latest_rate(
    source: &dyn SpotRateSource,
    quote_ordinal: i64,
    quote_ticker: &str,
    start_minute_utc: i64,
    end_minute_utc: i64,
) -> Result<ResolvedRate, FetchFailure> {
    if quote_ticker == "USDT" {
        return Ok(ResolvedRate {
            quote_ordinal,
            minute_utc: end_minute_utc,
            rate_usdt: 1.0,
            provider: "identity".to_string(),
            symbol: "USDT".to_string(),
            orientation: RateOrientation::Identity,
            candle_open_ms: end_minute_utc.saturating_mul(1_000),
            candle_close_ms: end_minute_utc.saturating_mul(1_000).saturating_add(59_999),
        });
    }
    for (provider, symbol, orientation) in canonical_routes(quote_ticker) {
        match source.candles(provider, &symbol, start_minute_utc, end_minute_utc) {
            Ok(candles) => {
                let Some(candle) = candles.into_iter().max_by_key(|candle| candle.open_ms) else {
                    continue;
                };
                let rate_usdt = validated_rate(candle.close, orientation).map_err(|error| {
                    FetchFailure::Transient(route_transient(provider, &symbol, error))
                })?;
                return Ok(ResolvedRate {
                    quote_ordinal,
                    minute_utc: candle.open_ms.div_euclid(60_000) * 60,
                    rate_usdt,
                    provider: provider.to_string(),
                    symbol,
                    orientation,
                    candle_open_ms: candle.open_ms,
                    candle_close_ms: candle.close_ms,
                });
            }
            Err(FetchFailure::Missing) => continue,
            Err(FetchFailure::Transient(error)) => {
                return Err(FetchFailure::Transient(route_transient(
                    provider, &symbol, error,
                )))
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
///     Ready rates, permanent misses, and an optional transient stop reason.
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
                .map(|minute_utc| ResolvedRate {
                    quote_ordinal,
                    minute_utc,
                    rate_usdt: 1.0,
                    provider: "identity".to_string(),
                    symbol: "USDT".to_string(),
                    orientation: RateOrientation::Identity,
                    candle_open_ms: minute_utc.saturating_mul(1_000),
                    candle_close_ms: minute_utc.saturating_mul(1_000).saturating_add(59_999),
                })
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
                    let rate_usdt = match validated_rate(candle.close, orientation) {
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
                        rate_usdt,
                        provider: provider.to_string(),
                        symbol: symbol.clone(),
                        orientation,
                        candle_open_ms: candle.open_ms,
                        candle_close_ms: candle.close_ms,
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

/// Validate and orient one provider close into USDT per quote unit.
///
/// Args:
///     close: Provider close price.
///     orientation: Direct or inverse route direction.
///
/// Returns:
///     Finite positive USDT rate, or a transient-data error description.
fn validated_rate(close: f64, orientation: RateOrientation) -> Result<f64, String> {
    if !close.is_finite() || close <= 0.0 {
        return Err(format!("invalid close {close}"));
    }
    let rate = match orientation {
        RateOrientation::Direct => close,
        RateOrientation::Inverse => 1.0 / close,
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
///     Success for 2xx, permanent absence only for Binance's invalid-symbol code, and transient
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
///     Validated candles, permanent absence when no exact candle remains, or a classified
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
            close,
        });
    }
    if candles.is_empty() {
        Err(FetchFailure::Missing)
    } else {
        Ok(candles)
    }
}
