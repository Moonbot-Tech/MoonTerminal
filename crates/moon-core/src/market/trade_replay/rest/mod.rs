//! HTTP client and response parsers for the public replay endpoints.
//!
//! The client is built the same way the valuation provider builds its own — HTTPS-only, one
//! bounded global timeout, HTTP status returned rather than raised — because that shape is already
//! proven against these same vendors in this same process. What is deliberately NOT shared is the
//! code: the valuation provider asks a different question (one minute's rate for a conversion) and
//! its range arithmetic, its symbol routing and its failure vocabulary all answer that question.
//! Reusing its private types would couple the market layer to a report-valuation detail.
//!
//! # One module per venue, and why the file is not one file
//!
//! This module owns only what every venue shares: the client, the failure vocabulary, the two
//! cell readers, and the dispatch. Everything a VENDOR decides — its query grammar, its error
//! envelope, its row order, which cell holds base volume — lives in that vendor's own module
//! beside its route.
//!
//! That split is not filing tidiness. Each of these venues has at least one fact that is silently
//! wrong when mistaken rather than loudly wrong: Gate spot's cells are not in OHLC order, an OKX
//! swap's volume cell counts contracts, BitGet spells the one-minute bar differently on its two
//! markets, Hyperliquid cannot distinguish a bad symbol from an outage. Held in one shared
//! if-ladder, a correction aimed at one vendor sits one typo away from changing another's meaning.
//! Held per module, it cannot.
//!
//! Every parser here is PURE: it takes a decoded JSON value and returns bars or a classified
//! failure. That is deliberate — a parser is where a vendor's units, ordering and field names are
//! actually pinned, and a pure function is the only kind of parser a test can hold to those
//! without a network. It is also why the recorded fixtures beside each module are enough, and no
//! HTTP-mocking dependency is needed.

mod binance;
mod bitget;
mod bybit;
mod gateio;
mod hyperliquid;
mod okx;

use std::time::Duration;

use serde_json::Value;

use super::venue_caps::KlineRoute;
use crate::market::candles::ChartCandle;

/// Bounded lifetime of one HTTP request.
///
/// Per REQUEST, not per job: a replay that pages a wide window makes several of these, and the
/// job-level deadline that bounds the whole thing lives with the worker instead.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Why one request did not produce rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// The venue does not know this symbol. Permanent for this market; retrying cannot help.
    UnknownSymbol,
    /// Transport, service, rate-limit or malformed-response failure that may recover.
    Transient(String),
}

/// Build the HTTPS client used for every replay request.
///
/// `http_status_as_error(false)` is required rather than stylistic: a 4xx body carries the
/// vendor's own error code, and the classifiers need to read it to tell an unknown symbol from a
/// service blip. Raising on status would throw that body away.
///
/// Returns:
///     A client suitable for the dedicated replay worker.
pub fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .https_only(true)
        .http_status_as_error(false)
        .build();
    ureq::Agent::new_with_config(config)
}

/// Fetch one page of one-minute bars.
///
/// The two matches below are deliberately exhaustive and carry no `_` arm, so adding a route to
/// [`KlineRoute`] without teaching this layer to speak its grammar is a compile error rather than
/// a request that quietly goes to the wrong vendor.
///
/// Args:
///     agent: Shared client.
///     route: Which endpoint family to ask.
///     market: Exchange-native market name, as the core reports it.
///     category: Bybit product category; ignored by every other route.
///     from_ms: First millisecond of the page, inclusive.
///     to_ms: Last millisecond of the page, inclusive.
///     max_rows: Row cap for this request.
///
/// Returns:
///     Bars in ascending open time, or a classified failure.
pub fn fetch_klines(
    agent: &ureq::Agent,
    route: KlineRoute,
    market: &str,
    category: Option<&str>,
    from_ms: i64,
    to_ms: i64,
    max_rows: usize,
) -> Result<Vec<ChartCandle>, FetchError> {
    let value = match route {
        KlineRoute::BinanceSpot | KlineRoute::BinanceUsdM | KlineRoute::BinanceCoinM => {
            binance::fetch(agent, route, market, from_ms, to_ms, max_rows)?
        }
        KlineRoute::Bybit => {
            bybit::fetch(agent, route, market, category, from_ms, to_ms, max_rows)?
        }
        KlineRoute::GateSpot | KlineRoute::GateFutures => {
            gateio::fetch(agent, route, market, from_ms, to_ms)?
        }
        KlineRoute::BitgetSpot | KlineRoute::BitgetFutures => {
            bitget::fetch(agent, route, market, from_ms, to_ms, max_rows)?
        }
        KlineRoute::OkxSpot | KlineRoute::OkxSwap => {
            okx::fetch(agent, route, market, from_ms, to_ms, max_rows)?
        }
        KlineRoute::Hyperliquid => hyperliquid::fetch(agent, route, market, from_ms, to_ms)?,
    };
    let mut rows = match route {
        // COIN-M's row carries no quote-turnover cell, only a base-asset one to estimate from —
        // see `binance::QuoteSource`'s doc.
        KlineRoute::BinanceSpot | KlineRoute::BinanceUsdM => binance::parse_klines(
            &value,
            binance::QuoteSource::Cell(binance::QUOTE_VOLUME_CELL),
        )?,
        KlineRoute::BinanceCoinM => binance::parse_klines(
            &value,
            binance::QuoteSource::EstimateFromBase(binance::COIN_M_BASE_VOLUME_CELL),
        )?,
        KlineRoute::Bybit => bybit::parse_klines(&value, category)?,
        KlineRoute::GateSpot => gateio::parse_spot_klines(&value)?,
        KlineRoute::GateFutures => gateio::parse_futures_klines(&value)?,
        KlineRoute::BitgetSpot | KlineRoute::BitgetFutures => bitget::parse_klines(&value)?,
        // The base-volume cell differs between the two OKX markets, and this match is already
        // exhaustive, so the choice is made HERE rather than by a second route match inside the
        // venue module that would need a wildcard arm to compile.
        KlineRoute::OkxSpot => okx::parse_klines(&value, okx::SPOT_VOLUME_CELL)?,
        KlineRoute::OkxSwap => okx::parse_klines(&value, okx::SWAP_VOLUME_CELL)?,
        KlineRoute::Hyperliquid => hyperliquid::parse_klines(&value)?,
    };
    // A route that answers newest-first would otherwise compose a series whose bars walk
    // backwards. Sorting rather than reversing also absorbs a vendor quietly changing direction.
    rows.sort_by(|a, b| {
        a.t_open_ms
            .partial_cmp(&b.t_open_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(rows)
}

/// Decode one response body and put it through that venue's classifier.
///
/// The five GET venues repeat this exact sequence, differing only in which classifier runs and in
/// the tag a decode failure is reported under. That repetition is TRANSPORT scaffolding, not the
/// per-vendor grammar this package deliberately keeps apart: the classifier stays in the venue's
/// own module and is passed in, so collapsing the scaffolding costs the split nothing.
///
/// Reading the status BEFORE the body is not stylistic — `into_body` consumes the response.
///
/// Args:
///     response: The response returned by the venue's request.
///     venue: Short tag naming the venue in a decode-failure diagnostic.
///     classify: That venue's own status-and-body classifier.
///
/// Returns:
///     The decoded body, or a classified failure.
pub(super) fn decode_and_classify(
    response: ureq::http::Response<ureq::Body>,
    venue: &str,
    classify: impl FnOnce(u16, &Value) -> Result<(), FetchError>,
) -> Result<Value, FetchError> {
    let status = response.status().as_u16();
    let body: Value = response
        .into_body()
        .read_json()
        .map_err(|error| FetchError::Transient(format!("{venue} JSON: {error}")))?;
    classify(status, &body)?;
    Ok(body)
}

/// Read a numeric cell that a vendor may send as either a JSON number or a quoted string.
///
/// Args:
///     cell: One response cell.
///
/// Returns:
///     A finite positive value, or `None`.
pub(super) fn cell_f32(cell: &Value) -> Option<f32> {
    let value = match cell {
        Value::String(text) => text.parse::<f64>().ok()?,
        other => other.as_f64()?,
    };
    // A non-finite or non-positive price is not a price; letting one through would poison the Y
    // fit for the whole window, since the fit takes a min and a max.
    //
    // The test is applied to the NARROWED value, not the `f64` behind it, because the narrowing is
    // itself a way to become non-finite: an `f64` like `1e300` passes every check as an `f64` and
    // then saturates to `f32::INFINITY` on the way into a `ChartCandle`. Checking after the cast
    // is what makes this function's promise true of the value it actually returns.
    let narrowed = value as f32;
    match narrowed.is_finite() && narrowed > 0.0 {
        true => Some(narrowed),
        false => None,
    }
}

/// Read an integer cell that a vendor may send as either a JSON number or a quoted string.
///
/// Args:
///     cell: One response cell.
///
/// Returns:
///     The value, or `None`.
pub(super) fn cell_i64(cell: &Value) -> Option<i64> {
    match cell {
        Value::String(text) => text.parse::<i64>().ok(),
        other => other.as_i64(),
    }
}
