//! Binance spot, USD-M and COIN-M klines.
//!
//! One grammar serves all three: the same query parameters against three different hosts, and the
//! same positional row. That is why they share this module while every other brand has its own.

use serde_json::Value;

use super::{FetchError, cell_f32};
use crate::market::candles::{ChartCandle, estimate_quote_volume};
use crate::market::trade_replay::venue_caps::KlineRoute;

/// Positional index of the cell holding QUOTE-asset turnover on a spot or USD-M row.
///
/// `quoteAssetVolume`, present on both — verified against a recorded response for each.
pub(super) const QUOTE_VOLUME_CELL: usize = 7;

/// Positional index of the cell holding a BASE-asset amount on a COIN-M row.
///
/// COIN-M's own index 5 (`volume`) is a CONTRACT count, not a currency amount at all — the
/// dapi contract multiplier lives on a different endpoint — so it cannot feed
/// [`crate::market::candles::estimate_quote_volume`], which needs a base-currency quantity.
/// Index 7 (`quoteAssetVolume` on spot/USD-M) is instead the BASE-asset amount on COIN-M —
/// verified against a recorded COIN-M response — which is exactly what that estimator wants.
pub(super) const COIN_M_BASE_VOLUME_CELL: usize = 7;

/// How to obtain one row's quote-currency turnover — the two are NOT interchangeable, because
/// which cell answers which question differs per Binance market (see [`QUOTE_VOLUME_CELL`] and
/// [`COIN_M_BASE_VOLUME_CELL`]).
#[derive(Clone, Copy)]
pub(super) enum QuoteSource {
    /// Read this cell directly as quote-asset turnover.
    Cell(usize),
    /// This cell is a BASE-currency quantity; estimate turnover from it via
    /// [`crate::market::candles::estimate_quote_volume`].
    EstimateFromBase(usize),
}

/// Fetch one page and return the decoded body.
///
/// Args:
///     agent: Shared client.
///     route: One of the three Binance routes.
///     market: Exchange-native market name.
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
        .query("symbol", market)
        .query("interval", "1m")
        .query("startTime", from_ms.to_string())
        .query("endTime", to_ms.to_string())
        .query("limit", max_rows.to_string())
        .call()
        .map_err(|error| FetchError::Transient(error.to_string()))?;
    super::decode_and_classify(response, "binance", classify)
}

/// Classify a Binance kline response by status and error code.
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
    // `-1121` is Binance's own "invalid symbol"; every other code is something that may recover.
    if body.get("code").and_then(Value::as_i64) == Some(-1121) {
        return Err(FetchError::UnknownSymbol);
    }
    Err(FetchError::Transient(format!("binance HTTP {status}")))
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
///     quote_source: How to obtain each row's turnover — see [`QuoteSource`].
///
/// Returns:
///     Bars in the response's own order, or a failure when the envelope is not an array.
pub(super) fn parse_klines(
    body: &Value,
    quote_source: QuoteSource,
) -> Result<Vec<ChartCandle>, FetchError> {
    let rows = body
        .as_array()
        .ok_or_else(|| FetchError::Transient("binance: response is not an array".to_string()))?;
    Ok(rows
        .iter()
        .filter_map(|row| parse_row(row, quote_source))
        .collect())
}

/// Parse one positional Binance kline row.
///
/// Args:
///     row: One element of the response array.
///     quote_source: How to obtain this row's turnover — see [`QuoteSource`].
///
/// Returns:
///     The bar, or `None` when the row is malformed.
fn parse_row(row: &Value, quote_source: QuoteSource) -> Option<ChartCandle> {
    let cells = row.as_array()?;
    let open_ms = cells.first()?.as_i64()? as f64;
    let open = cell_f32(cells.get(1)?)?;
    let high = cell_f32(cells.get(2)?)?;
    let low = cell_f32(cells.get(3)?)?;
    let close = cell_f32(cells.get(4)?)?;
    // Index 5 is base-currency volume on spot and USD-M. On COIN-M it is actually a CONTRACT
    // count, not a currency amount at all — a separate, pre-existing mislabel this goal does not
    // touch (see `QuoteSource::EstimateFromBase`'s doc for the cell that DOES hold a base amount
    // there).
    let volume = cell_f32(cells.get(5)?).unwrap_or(0.0);
    let quote_volume = match quote_source {
        QuoteSource::Cell(i) => cells.get(i).and_then(cell_f32).unwrap_or(0.0),
        QuoteSource::EstimateFromBase(i) => {
            let base = cells.get(i).and_then(cell_f32).unwrap_or(0.0);
            estimate_quote_volume(base, open, high, low, close)
        }
    };
    Some(ChartCandle {
        t_open_ms: open_ms,
        open,
        high,
        low,
        close,
        volume,
        quote_volume,
    })
}
