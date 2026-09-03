//! Gate spot and USDT-perpetual candlesticks.
//!
//! Named `gateio` rather than `gate` on purpose: [`super::super::gate`] is this feature's RATE
//! gate, and two modules called `gate` one level apart would be a reading trap for no gain.
//!
//! Gate is the one brand here whose two markets share neither a row SHAPE nor a cell type. Spot
//! answers positional arrays of strings; futures answers objects mixing strings and numbers. Both
//! quirks are pinned in the two parsers below rather than reconciled into one, because a shared
//! parser is exactly where a fix for one market silently changes the other.

use serde_json::Value;

use super::{FetchError, cell_f32, cell_i64};
use crate::market::candles::ChartCandle;
use crate::market::trade_replay::venue_caps::KlineRoute;

/// Milliseconds per second, for Gate's second-resolution window and timestamps.
const MS_PER_S: i64 = 1_000;

/// Fetch one page and return the decoded body.
///
/// Gate's window is in SECONDS while every caller here works in milliseconds, so the bounds are
/// divided on the way out and the row timestamps multiplied on the way back.
///
/// `limit` is deliberately NOT sent. Gate documents it as conflicting with `from`/`to`, and the
/// pager has already sized the window to [`KlineRoute::max_rows`], so the range alone is both
/// sufficient and unambiguous.
///
/// Args:
///     agent: Shared client.
///     route: [`KlineRoute::GateSpot`] or [`KlineRoute::GateFutures`].
///     market: Exchange-native market name, e.g. `BTC_USDT`.
///     from_ms: First millisecond of the page, inclusive.
///     to_ms: Last millisecond of the page, inclusive.
///
/// Returns:
///     The decoded response, or a classified failure.
pub(super) fn fetch(
    agent: &ureq::Agent,
    route: KlineRoute,
    market: &str,
    from_ms: i64,
    to_ms: i64,
) -> Result<Value, FetchError> {
    // Spot names the market `currency_pair`; futures names the same string `contract`. Asked as a
    // boolean rather than as a `match` with a fallback arm, so this reads as the one question it
    // is and no route can silently fall through to the wrong parameter name.
    let futures = matches!(route, KlineRoute::GateFutures);
    let market_param = match futures {
        true => "contract",
        false => "currency_pair",
    };
    let response = agent
        .get(route.url())
        .query(market_param, market)
        .query("interval", "1m")
        .query("from", (from_ms / MS_PER_S).to_string())
        .query("to", (to_ms / MS_PER_S).to_string())
        .call()
        .map_err(|error| FetchError::Transient(error.to_string()))?;
    super::decode_and_classify(response, "gate", classify)
}

/// Classify a Gate candlestick response.
///
/// Gate answers a real HTTP status — unlike Bybit and OKX, a 2xx here genuinely means success —
/// and names the fault in `label`. The two labels below are the ones an unlisted market produces,
/// spot and futures respectively; everything else may recover and is reported as such.
///
/// Args:
///     status: HTTP status.
///     body: Decoded response.
///
/// Returns:
///     `Ok(())` on success, or the classified failure.
pub(super) fn classify(status: u16, body: &Value) -> Result<(), FetchError> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    let label = body
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match label {
        "INVALID_CURRENCY_PAIR" | "CONTRACT_NOT_FOUND" => Err(FetchError::UnknownSymbol),
        "" => Err(FetchError::Transient(format!("gate HTTP {status}"))),
        other => Err(FetchError::Transient(format!(
            "gate HTTP {status}: {other}"
        ))),
    }
}

/// Parse a Gate SPOT candlestick array into bars.
///
/// # The cell order is not OHLC, and that is the whole hazard
///
/// A spot row is a positional array of eight STRINGS:
///
/// ```text
/// [ open_time_s, quote_volume, close, high, low, open, base_volume, window_closed ]
///        0             1         2      3    4     5         6            7
/// ```
///
/// Indices 2 through 5 are **close, high, low, open** — not open, high, low, close. Reading them
/// in the familiar order swaps each bar's open with its close, which produces candles that still
/// look like candles: the highs and lows are right, the bodies are inverted. Nothing downstream
/// can notice, which is why the order is written out here rather than left to the reader.
///
/// Index 7 is Gate's own "this window has closed" flag, as the string `"true"` or `"false"`. A
/// still-forming bar is dropped: [`crate::market::trade_replay::worker`] merges these rows into
/// the kline cache the live recorder shares, and a half-formed minute filed there would be read
/// back as final long after it stopped being true.
///
/// Args:
///     body: Decoded response.
///
/// Returns:
///     Bars in the response's own order, or a failure when the envelope is not an array.
pub(super) fn parse_spot_klines(body: &Value) -> Result<Vec<ChartCandle>, FetchError> {
    let rows = body
        .as_array()
        .ok_or_else(|| FetchError::Transient("gate: spot response is not an array".to_string()))?;
    Ok(rows.iter().filter_map(parse_spot_row).collect())
}

/// Parse one positional Gate spot row.
///
/// Args:
///     row: One element of the response array.
///
/// Returns:
///     The bar, or `None` when the row is malformed or still forming.
fn parse_spot_row(row: &Value) -> Option<ChartCandle> {
    let cells = row.as_array()?;
    // A missing flag is treated as closed, so a vendor that stops sending it degrades to the
    // previous behaviour rather than to an empty window.
    if cells.get(7).and_then(Value::as_str) == Some("false") {
        return None;
    }
    Some(ChartCandle {
        t_open_ms: (cell_i64(cells.first()?)? * MS_PER_S) as f64,
        open: cell_f32(cells.get(5)?)?,
        high: cell_f32(cells.get(3)?)?,
        low: cell_f32(cells.get(4)?)?,
        close: cell_f32(cells.get(2)?)?,
        volume: cell_f32(cells.get(6)?).unwrap_or(0.0),
        quote_volume: cell_f32(cells.get(1)?).unwrap_or(0.0),
    })
}

/// Parse a Gate USDT-perpetual candlestick array into bars.
///
/// Rows are OBJECTS with mixed cell types — `t` and `v` arrive as JSON numbers while `o`, `h`,
/// `l`, `c` and `sum` arrive as strings — which is why every cell goes through the shared readers
/// that accept either.
///
/// # This route reports NO base volume, deliberately — but DOES report quote turnover
///
/// Gate's futures candle carries `v`, a count of CONTRACTS, and `sum`, the turnover in the quote
/// asset. It carries no base-asset amount at all, and the contract multiplier that would convert
/// one is a property of the contract served by a different endpoint. [`ChartCandle::volume`] is
/// documented as base-currency volume, and these rows are merged into the kline cache the live
/// recorder shares, so filing a contract count there would be a wrong number under a field whose
/// type promises a different one — off by the multiplier, and silently so. Deriving it from
/// `sum / close` was rejected for the same reason: that is an estimate, not a measurement.
///
/// `sum`, unlike `v`, needs no multiplier: it is already quote-asset turnover and feeds
/// [`ChartCandle::quote_volume`] directly.
///
/// Zero is this module's existing spelling of "no base volume for this bar" — every other parser
/// here falls back to it — so the candles are served and the base-volume histogram is simply
/// empty for this one route.
///
/// Args:
///     body: Decoded response.
///
/// Returns:
///     Bars in the response's own order, or a failure when the envelope is not an array.
pub(super) fn parse_futures_klines(body: &Value) -> Result<Vec<ChartCandle>, FetchError> {
    let rows = body.as_array().ok_or_else(|| {
        FetchError::Transient("gate: futures response is not an array".to_string())
    })?;
    Ok(rows.iter().filter_map(parse_futures_row).collect())
}

/// Parse one Gate futures candle object.
///
/// Args:
///     row: One element of the response array.
///
/// Returns:
///     The bar, or `None` when the object is malformed.
fn parse_futures_row(row: &Value) -> Option<ChartCandle> {
    Some(ChartCandle {
        t_open_ms: (cell_i64(row.get("t")?)? * MS_PER_S) as f64,
        open: cell_f32(row.get("o")?)?,
        high: cell_f32(row.get("h")?)?,
        low: cell_f32(row.get("l")?)?,
        close: cell_f32(row.get("c")?)?,
        // Not `v`: see this module's `parse_futures_klines` docstring.
        volume: 0.0,
        quote_volume: cell_f32(row.get("sum")?).unwrap_or(0.0),
    })
}

#[cfg(test)]
mod tests;
