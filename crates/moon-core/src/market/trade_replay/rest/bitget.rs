//! BitGet spot and USDT-perpetual candles.
//!
//! Two things about this venue are worth knowing before reading the code.
//!
//! **The endpoint is `history-candles`, never the plain `candles`.** Measured: the plain endpoint
//! answers HTTP 200 with an EMPTY `data` array once the window is older than roughly a month,
//! rather than refusing. That is an empty-but-successful answer, which
//! [`crate::market::trade_replay::worker`] would then remember as this window's authoritative
//! verdict — the single worst outcome available here, because it is indistinguishable from a
//! market that genuinely did not trade. The URL is fixed in [`KlineRoute::url`] and this comment
//! is why.
//!
//! **BitGet spells the one-minute bar differently on its two markets** — `1min` on spot, `1m` on
//! futures. Crossing them over is a hard HTTP 400, so it fails loudly, but it is still two tokens
//! and they live here beside the fetch rather than in one shared constant that would have to
//! carry a per-market exception.

use serde_json::Value;

use super::{FetchError, cell_f32, cell_i64};
use crate::market::candles::ChartCandle;
use crate::market::trade_replay::venue_caps::KlineRoute;

/// Fetch one page and return the decoded body.
///
/// Args:
///     agent: Shared client.
///     route: [`KlineRoute::BitgetSpot`] or [`KlineRoute::BitgetFutures`].
///     market: Exchange-native market name, e.g. `BTCUSDT`.
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
    let futures = matches!(route, KlineRoute::BitgetFutures);
    let mut request = agent
        .get(route.url())
        .query("symbol", market)
        // The two markets do not share this token; see the module header.
        .query("granularity", if futures { "1m" } else { "1min" })
        .query("startTime", from_ms.to_string())
        .query("endTime", to_ms.to_string())
        .query("limit", max_rows.to_string());
    if futures {
        request = request.query("productType", "USDT-FUTURES");
    }
    let response = request
        .call()
        .map_err(|error| FetchError::Transient(error.to_string()))?;
    super::decode_and_classify(response, "bitget", classify)
}

/// Classify a BitGet candle response.
///
/// BitGet answers a real HTTP status, and names the fault in `code` — as a STRING, not a number.
///
/// The two unknown-symbol codes are asymmetric. Futures answers `40034` and echoes the offending
/// symbol into `msg`, which is unambiguous. Spot answers `400172`, "Parameter verification
/// failed", which is the SAME code it returns for a malformed `granularity`. Reading it as an
/// unknown symbol is sound here and only here: `granularity` is sent as a constant chosen by the
/// route, never from user input, so on these call sites it cannot be the parameter at fault. A
/// caller that ever made granularity dynamic would have to revisit this arm.
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
    let code = body.get("code").and_then(Value::as_str).unwrap_or_default();
    match code {
        "40034" | "400172" => Err(FetchError::UnknownSymbol),
        "" => Err(FetchError::Transient(format!("bitget HTTP {status}"))),
        other => {
            let message = body.get("msg").and_then(Value::as_str).unwrap_or("unknown");
            Err(FetchError::Transient(format!(
                "bitget HTTP {status} {other}: {message}"
            )))
        }
    }
}

/// Parse a BitGet candle envelope into bars.
///
/// One parser serves both markets on purpose. Spot rows carry eight cells and futures rows seven —
/// the extra spot cell is a second quote-volume figure at the end — but indices 0 through 5 are
/// identical on both, and index 5 is the only volume this parser reads. Splitting this into two
/// functions would duplicate the positional contract without pinning anything the other does not.
///
/// ```text
/// [ open_time_ms, open, high, low, close, base_volume, quote_volume, (spot only) quote_volume ]
///        0          1     2     3     4         5            6                    7
/// ```
///
/// Every cell is a STRING, timestamp included.
///
/// Args:
///     body: Decoded response.
///
/// Returns:
///     Bars in the response's own order, or a failure when the envelope is missing.
pub(super) fn parse_klines(body: &Value) -> Result<Vec<ChartCandle>, FetchError> {
    let rows = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::Transient("bitget: missing data".to_string()))?;
    Ok(rows.iter().filter_map(parse_row).collect())
}

/// Parse one positional BitGet candle row.
///
/// Args:
///     row: One element of `data`.
///
/// Returns:
///     The bar, or `None` when the row is malformed.
fn parse_row(row: &Value) -> Option<ChartCandle> {
    let cells = row.as_array()?;
    Some(ChartCandle {
        t_open_ms: cell_i64(cells.first()?)? as f64,
        open: cell_f32(cells.get(1)?)?,
        high: cell_f32(cells.get(2)?)?,
        low: cell_f32(cells.get(3)?)?,
        close: cell_f32(cells.get(4)?)?,
        volume: cell_f32(cells.get(5)?).unwrap_or(0.0),
    })
}

#[cfg(test)]
mod tests;
