//! OKX spot and perpetual-swap candles.
//!
//! One endpoint, one host and one row grammar serve both markets, and they are still two routes,
//! because two of OKX's facts are per-market and both are silent when wrong.

use serde_json::Value;

use super::{FetchError, cell_f32, cell_i64};
use crate::market::candles::ChartCandle;
use crate::market::trade_replay::venue_caps::KlineRoute;

/// Positional index of the cell holding BASE-asset volume on a SPOT row.
pub(super) const SPOT_VOLUME_CELL: usize = 5;

/// Positional index of the cell holding BASE-asset volume on a SWAP row.
///
/// A swap's cell 5 (`vol`) counts CONTRACTS, and cell 6 (`volCcy`) is the base-asset amount those
/// contracts represent. Measured on a recorded `BTC-USDT-SWAP` row: `vol` read `14484` where
/// `volCcy` read `144.84`, i.e. one contract is 0.01 BTC. Taking cell 5 on a swap is therefore not
/// a rounding difference but a hundredfold one, and it would be merged into the kline cache the
/// live recorder shares.
pub(super) const SWAP_VOLUME_CELL: usize = 6;

/// Fetch one page and return the decoded body.
///
/// # Bounding a window takes both cursors, and their names invite the wrong guess
///
/// `before` and `after` are OKX's pagination cursors, and supplying both bounds one window in a
/// single request. Two things about them are verified against the live endpoint rather than
/// inferred:
///
/// - **`before` takes the OLDER edge and `after` the NEWER one**, so `before < after` numerically.
///   Swapping them returns an empty array rather than an error — a wrong window that reads as a
///   market with no trades.
/// - **Both are strictly EXCLUSIVE** of the timestamp given. A page asking for exactly
///   `[from_ms, to_ms]` therefore nudges each bound one millisecond outward; a millisecond can
///   never collide with another bar's open time at this resolution.
///
/// Args:
///     agent: Shared client.
///     route: [`KlineRoute::OkxSpot`] or [`KlineRoute::OkxSwap`].
///     market: Exchange-native instrument id, e.g. `BTC-USDT` or `BTC-USDT-SWAP`.
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
    from_ms: i64,
    to_ms: i64,
    max_rows: usize,
) -> Result<Value, FetchError> {
    let response = agent
        .get(route.url())
        .query("instId", market)
        .query("bar", "1m")
        .query("before", (from_ms.saturating_sub(1)).to_string())
        .query("after", (to_ms.saturating_add(1)).to_string())
        .query("limit", max_rows.to_string())
        .call()
        .map_err(|error| FetchError::Transient(error.to_string()))?;
    super::decode_and_classify(response, "okx", classify)
}

/// Classify an OKX candle response by status and envelope `code`.
///
/// OKX answers HTTP 200 for application errors, exactly as Bybit does with `retCode` — see
/// [`super::bybit::classify`] for why that shape is worth guarding rather than trusting the
/// status. An unlisted instrument here returns 200 with `code` `51001`, so a status-only check
/// would hand the parser an envelope whose `data` is empty and the window would be reported as a
/// market that simply did not trade, then remembered as such.
///
/// `code` is a STRING in this envelope, not a number.
///
/// Args:
///     status: HTTP status.
///     body: Decoded response.
///
/// Returns:
///     `Ok(())` on success, or the classified failure.
pub(super) fn classify(status: u16, body: &Value) -> Result<(), FetchError> {
    if !(200..300).contains(&status) {
        return Err(FetchError::Transient(format!("okx HTTP {status}")));
    }
    match body.get("code").and_then(Value::as_str) {
        Some("0") => Ok(()),
        // A MISSING `code` is not success. OKX sends it on every response, so its absence means
        // the envelope is not the one this parser understands — and treating that as success
        // would hand the parser a body it reads as zero rows, which the worker then remembers as
        // "this market did not trade". Transient is the honest reading of a malformed answer.
        None => Err(FetchError::Transient(
            "okx: envelope has no code".to_string(),
        )),
        // `51001` is OKX's "instrument does not exist".
        Some("51001") => Err(FetchError::UnknownSymbol),
        Some(code) => {
            let message = body.get("msg").and_then(Value::as_str).unwrap_or("unknown");
            Err(FetchError::Transient(format!("okx {code}: {message}")))
        }
    }
}

/// Parse an OKX candle envelope into bars.
///
/// Rows live under `data` and are positional arrays of nine STRINGS:
///
/// ```text
/// [ ts_ms, open, high, low, close, vol, volCcy, volCcyQuote, confirm ]
///     0      1     2    3     4     5      6         7           8
/// ```
///
/// Cell 8 is OKX's "this bar has closed" flag, `"1"` closed and `"0"` still forming. A forming bar
/// is dropped, for the same reason Gate spot's is: these rows reach the kline cache the live
/// recorder shares, and a half-formed minute filed there outlives the minute itself. A missing
/// flag counts as closed, so a vendor that stops sending it degrades to the previous behaviour
/// rather than to an empty window.
///
/// Which cell carries base volume depends on the market, which is why the caller passes it —
/// see [`SWAP_VOLUME_CELL`].
///
/// Args:
///     body: Decoded response.
///     volume_cell: Index of the base-volume cell for this market.
///
/// Returns:
///     Bars in the response's own order, or a failure when the envelope is missing.
pub(super) fn parse_klines(
    body: &Value,
    volume_cell: usize,
) -> Result<Vec<ChartCandle>, FetchError> {
    let rows = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::Transient("okx: missing data".to_string()))?;
    Ok(rows
        .iter()
        .filter_map(|row| parse_row(row, volume_cell))
        .collect())
}

/// Parse one positional OKX candle row.
///
/// Args:
///     row: One element of `data`.
///     volume_cell: Index of the base-volume cell for this market.
///
/// Returns:
///     The bar, or `None` when the row is malformed or still forming.
fn parse_row(row: &Value, volume_cell: usize) -> Option<ChartCandle> {
    let cells = row.as_array()?;
    if cells.get(8).and_then(Value::as_str) == Some("0") {
        return None;
    }
    Some(ChartCandle {
        t_open_ms: cell_i64(cells.first()?)? as f64,
        open: cell_f32(cells.get(1)?)?,
        high: cell_f32(cells.get(2)?)?,
        low: cell_f32(cells.get(3)?)?,
        close: cell_f32(cells.get(4)?)?,
        volume: cells.get(volume_cell).and_then(cell_f32).unwrap_or(0.0),
    })
}

#[cfg(test)]
mod tests;
