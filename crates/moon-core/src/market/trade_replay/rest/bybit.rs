//! Bybit spot, linear and inverse klines — one host, one grammar, `category` per request.

use serde_json::Value;

use super::{FetchError, cell_f32, cell_i64};
use crate::market::candles::ChartCandle;
use crate::market::trade_replay::venue_caps::KlineRoute;

/// Fetch one page and return the decoded body.
///
/// Args:
///     agent: Shared client.
///     route: The Bybit route.
///     market: Exchange-native market name.
///     category: Product category resolved from the market's quote asset.
///     from_ms: First millisecond of the page, inclusive.
///     to_ms: Last millisecond of the page, inclusive.
///     max_rows: Row cap for this request.
///
/// Returns:
///     The decoded response, or a classified failure.
pub(super) fn fetch(
    agent: &ureq::Agent,
    route: KlineRoute,
    market: &str,
    category: Option<&str>,
    from_ms: i64,
    to_ms: i64,
    max_rows: usize,
) -> Result<Value, FetchError> {
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
    super::decode_and_classify(response, "bybit", classify)
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
pub(super) fn classify(status: u16, body: &Value) -> Result<(), FetchError> {
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
pub(super) fn parse_klines(body: &Value) -> Result<Vec<ChartCandle>, FetchError> {
    let rows = body
        .get("result")
        .and_then(|r| r.get("list"))
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::Transient("bybit: missing result.list".to_string()))?;
    Ok(rows.iter().filter_map(parse_row).collect())
}

/// Parse one positional Bybit kline row.
///
/// Args:
///     row: One element of `result.list`.
///
/// Returns:
///     The bar, or `None` when the row is malformed.
fn parse_row(row: &Value) -> Option<ChartCandle> {
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
