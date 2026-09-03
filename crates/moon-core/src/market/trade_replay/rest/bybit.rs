//! Bybit spot, linear and inverse klines — one host, one grammar, `category` per request.

use serde_json::Value;

use super::{FetchError, cell_f32, cell_i64};
use crate::market::candles::ChartCandle;
use crate::market::trade_replay::venue_caps::KlineRoute;

/// Positional index of the cell holding QUOTE-asset turnover, on `spot` and `linear` rows.
///
/// On `inverse` rows cells 5 and 6 carry the OPPOSITE denominations from `linear` — verified
/// against a recorded `BTCUSD` (inverse) row beside a `BTCUSDT` (linear) one: cell 5 there is
/// already quote-denominated turnover and cell 6 is a base amount. See
/// [`INVERSE_TURNOVER_CELL`] and [`quote_cell_for_category`].
pub(super) const TURNOVER_CELL: usize = 6;

/// Positional index of the cell holding QUOTE-asset turnover on an `inverse` row — the mirror
/// image of [`TURNOVER_CELL`]. A REAL figure, not an estimate: `inverse` never needs
/// [`crate::market::candles::estimate_quote_volume`].
pub(super) const INVERSE_TURNOVER_CELL: usize = 5;

/// Return the quote-turnover cell for a Bybit product category.
///
/// Args:
///     category: `spot`, `linear`, `inverse`, or `None` (treated as `spot`, matching [`fetch`]'s
///         own default).
///
/// Returns:
///     [`TURNOVER_CELL`] for `spot`/`linear`, [`INVERSE_TURNOVER_CELL`] for `inverse`.
pub(super) fn quote_cell_for_category(category: Option<&str>) -> usize {
    match category.unwrap_or("spot") {
        "inverse" => INVERSE_TURNOVER_CELL,
        _ => TURNOVER_CELL,
    }
}

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
///     category: Product category resolved from the market's quote asset — see
///         [`quote_cell_for_category`].
///
/// Returns:
///     Bars in the response's own order, or a failure when the envelope is missing.
pub(super) fn parse_klines(
    body: &Value,
    category: Option<&str>,
) -> Result<Vec<ChartCandle>, FetchError> {
    let rows = body
        .get("result")
        .and_then(|r| r.get("list"))
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::Transient("bybit: missing result.list".to_string()))?;
    let quote_cell = quote_cell_for_category(category);
    Ok(rows
        .iter()
        .filter_map(|row| parse_row(row, quote_cell))
        .collect())
}

/// Parse one positional Bybit kline row.
///
/// Args:
///     row: One element of `result.list`.
///     quote_cell: Index of the quote-turnover cell — see [`quote_cell_for_category`].
///
/// Returns:
///     The bar, or `None` when the row is malformed.
fn parse_row(row: &Value, quote_cell: usize) -> Option<ChartCandle> {
    let cells = row.as_array()?;
    let open_ms = cell_i64(cells.first()?)?;
    let open = cell_f32(cells.get(1)?)?;
    let high = cell_f32(cells.get(2)?)?;
    let low = cell_f32(cells.get(3)?)?;
    let close = cell_f32(cells.get(4)?)?;
    // Bybit's index 5 is base-currency volume, matching `ChartCandle::volume`, on `spot` and
    // `linear`; on `inverse` this cell is actually quote turnover (see `INVERSE_TURNOVER_CELL`)
    // and index 5's base-currency mislabel on `inverse` is a separate, pre-existing issue this
    // goal does not touch — only `quote_volume` is fixed here.
    let volume = cell_f32(cells.get(5)?).unwrap_or(0.0);
    let quote_volume = cells.get(quote_cell).and_then(cell_f32).unwrap_or(0.0);
    Some(ChartCandle {
        t_open_ms: open_ms as f64,
        open,
        high,
        low,
        close,
        volume,
        quote_volume,
    })
}
