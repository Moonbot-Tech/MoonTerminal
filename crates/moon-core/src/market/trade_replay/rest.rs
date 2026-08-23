//! HTTP client and response parsers for the public replay endpoints.
//!
//! The client is built the same way the valuation provider builds its own — HTTPS-only, one
//! bounded global timeout, HTTP status returned rather than raised — because that shape is already
//! proven against these same vendors in this same process. What is deliberately NOT shared is the
//! code: the valuation provider asks a different question (one minute's rate for a conversion) and
//! its range arithmetic, its symbol routing and its failure vocabulary all answer that question.
//! Reusing its private types would couple the market layer to a report-valuation detail.
//!
//! Every parser here is PURE: it takes a decoded JSON value and the window it asked for, and
//! returns bars or a classified failure. That is deliberate — a parser is where a vendor's units,
//! ordering and field names are actually pinned, and a pure function is the only kind of parser a
//! test can hold to those without a network.

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
/// vendor's own error code, and the classifiers below need to read it to tell an unknown symbol
/// from a service blip. Raising on status would throw that body away.
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
            let response = agent
                .get(route.url())
                .query("symbol", market)
                .query("interval", "1m")
                .query("startTime", from_ms.to_string())
                .query("endTime", to_ms.to_string())
                .query("limit", max_rows.to_string())
                .call()
                .map_err(|error| FetchError::Transient(error.to_string()))?;
            let status = response.status().as_u16();
            let body: Value = response
                .into_body()
                .read_json()
                .map_err(|error| FetchError::Transient(format!("binance JSON: {error}")))?;
            classify_binance(status, &body)?;
            body
        }
        KlineRoute::Bybit => {
            let response = agent
                .get(route.url())
                .query("category", category.unwrap_or("spot"))
                .query("symbol", market)
                .query("interval", "1")
                .query("start", from_ms.to_string())
                .query("end", to_ms.to_string())
                .query("limit", max_rows.to_string())
                .call()
                .map_err(|error| FetchError::Transient(error.to_string()))?;
            let status = response.status().as_u16();
            let body: Value = response
                .into_body()
                .read_json()
                .map_err(|error| FetchError::Transient(format!("bybit JSON: {error}")))?;
            classify_bybit(status, &body)?;
            body
        }
    };
    let mut rows = match route {
        KlineRoute::BinanceSpot | KlineRoute::BinanceUsdM | KlineRoute::BinanceCoinM => {
            parse_binance_klines(&value)?
        }
        KlineRoute::Bybit => parse_bybit_klines(&value)?,
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

/// Classify a Binance kline response by status and error code.
///
/// Args:
///     status: HTTP status.
///     body: Decoded response.
///
/// Returns:
///     `Ok(())` on success, or the classified failure.
fn classify_binance(status: u16, body: &Value) -> Result<(), FetchError> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    // `-1121` is Binance's own "invalid symbol"; every other code is something that may recover.
    if body.get("code").and_then(Value::as_i64) == Some(-1121) {
        return Err(FetchError::UnknownSymbol);
    }
    Err(FetchError::Transient(format!("binance HTTP {status}")))
}

/// Classify a Bybit kline response by status and `retCode`.
///
/// Bybit answers HTTP 200 with a non-zero `retCode` for application errors, so the status alone
/// would read every one of them as success and the parser would then report an empty window —
/// the failure mode that looks like "this market simply had no trades".
///
/// Args:
///     status: HTTP status.
///     body: Decoded response.
///
/// Returns:
///     `Ok(())` on success, or the classified failure.
fn classify_bybit(status: u16, body: &Value) -> Result<(), FetchError> {
    if !(200..300).contains(&status) {
        return Err(FetchError::Transient(format!("bybit HTTP {status}")));
    }
    match body.get("retCode").and_then(Value::as_i64) {
        Some(0) | None => Ok(()),
        // `10001` is Bybit's parameter error, which an unlisted symbol produces.
        Some(10001) => Err(FetchError::UnknownSymbol),
        Some(code) => {
            let message = body
                .get("retMsg")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            Err(FetchError::Transient(format!("bybit {code}: {message}")))
        }
    }
}

/// Parse a Binance kline array into bars.
///
/// The row is a positional array — `[openTime, open, high, low, close, volume, ...]` — with every
/// price as a STRING, so each field is read by index and parsed rather than deserialized into a
/// struct. A row whose numbers do not parse is dropped rather than failing the page: one bad bar
/// in a thousand should cost that bar, not the whole trade's picture.
///
/// Args:
///     body: Decoded response.
///
/// Returns:
///     Bars in the response's own order, or a failure when the envelope is not an array.
pub fn parse_binance_klines(body: &Value) -> Result<Vec<ChartCandle>, FetchError> {
    let rows = body
        .as_array()
        .ok_or_else(|| FetchError::Transient("binance: response is not an array".to_string()))?;
    Ok(rows.iter().filter_map(parse_binance_row).collect())
}

/// Parse one positional Binance kline row.
///
/// Args:
///     row: One element of the response array.
///
/// Returns:
///     The bar, or `None` when the row is malformed.
fn parse_binance_row(row: &Value) -> Option<ChartCandle> {
    let cells = row.as_array()?;
    let open_ms = cells.first()?.as_i64()? as f64;
    Some(ChartCandle {
        t_open_ms: open_ms,
        open: cell_f32(cells.get(1)?)?,
        high: cell_f32(cells.get(2)?)?,
        low: cell_f32(cells.get(3)?)?,
        close: cell_f32(cells.get(4)?)?,
        volume: cell_f32(cells.get(5)?).unwrap_or(0.0),
    })
}

/// Parse a Bybit kline envelope into bars.
///
/// The rows live under `result.list` and are POSITIONAL string arrays like Binance's, but they
/// arrive newest-first and the open time is itself a string. Both differences are pinned here.
///
/// Args:
///     body: Decoded response.
///
/// Returns:
///     Bars in the response's own order, or a failure when the envelope is missing.
pub fn parse_bybit_klines(body: &Value) -> Result<Vec<ChartCandle>, FetchError> {
    let rows = body
        .get("result")
        .and_then(|r| r.get("list"))
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::Transient("bybit: missing result.list".to_string()))?;
    Ok(rows.iter().filter_map(parse_bybit_row).collect())
}

/// Parse one positional Bybit kline row.
///
/// Args:
///     row: One element of `result.list`.
///
/// Returns:
///     The bar, or `None` when the row is malformed.
fn parse_bybit_row(row: &Value) -> Option<ChartCandle> {
    let cells = row.as_array()?;
    let open_ms = cell_i64(cells.first()?)?;
    Some(ChartCandle {
        t_open_ms: open_ms as f64,
        open: cell_f32(cells.get(1)?)?,
        high: cell_f32(cells.get(2)?)?,
        low: cell_f32(cells.get(3)?)?,
        close: cell_f32(cells.get(4)?)?,
        // Bybit's index 5 is base-currency volume, matching `ChartCandle::volume`; index 6 is
        // turnover in the quote asset and would be off by the price if taken instead.
        volume: cell_f32(cells.get(5)?).unwrap_or(0.0),
    })
}

/// Read a numeric cell that a vendor may send as either a JSON number or a quoted string.
///
/// Args:
///     cell: One response cell.
///
/// Returns:
///     A finite positive value, or `None`.
fn cell_f32(cell: &Value) -> Option<f32> {
    let value = match cell {
        Value::String(text) => text.parse::<f64>().ok()?,
        other => other.as_f64()?,
    };
    // A non-finite or non-positive price is not a price; letting one through would poison the Y
    // fit for the whole window, since the fit takes a min and a max.
    match value.is_finite() && value > 0.0 {
        true => Some(value as f32),
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
fn cell_i64(cell: &Value) -> Option<i64> {
    match cell {
        Value::String(text) => text.parse::<i64>().ok(),
        other => other.as_i64(),
    }
}
