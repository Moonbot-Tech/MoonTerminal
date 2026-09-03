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

use super::{FetchError, TradeCursor, TradePage, cell_f32, cell_i64};
use crate::feed::types::{Side, Tick};
use crate::market::candles::ChartCandle;
use crate::market::trade_replay::venue_caps::{KlineRoute, TradeRoute};

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
/// the extra spot cell is a second quote-volume figure at the end — but indices 0 through 6 are
/// identical on both, and this parser reads index 5 (base volume) and index 6 (quote volume);
/// the spot-only trailing cell at index 7 is redundant with index 6 and is never read. Splitting
/// this into two functions would duplicate the positional contract without pinning anything the
/// other does not.
///
/// The vendor names index 6 `quoteVolume` and the spot-only index 7 `usdtVolume`. They agree for a
/// USDT-quoted market — every recorded fixture beside this module is one, which is why it cannot
/// distinguish them — and differ only on a non-USDT-quoted spot market, where `quoteVolume` (index
/// 6) is the one that actually matches [`ChartCandle::quote_volume`]'s contract.
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
        quote_volume: cell_f32(cells.get(6)?).unwrap_or(0.0),
    })
}

/// Fetch one page of public fills and return the decoded body.
///
/// Args:
///     agent: Shared client.
///     route: [`TradeRoute::BitgetSpotFills`] or [`TradeRoute::BitgetMixFills`].
///     market: Exchange-native market name, e.g. `BTCUSDT`.
///     from_ms: First millisecond of this slice, inclusive.
///     to_ms: Last millisecond of this slice, inclusive; the vendor caps one request's span at
///         7 days, see [`super::super::venue_caps::TradeRoute::max_query_ms`].
///     cursor: Continuation from a previous page, or `None` for the first page.
///
/// Returns:
///     The decoded response, or a classified failure.
pub(super) fn fetch_trades(
    agent: &ureq::Agent,
    route: TradeRoute,
    market: &str,
    from_ms: i64,
    to_ms: i64,
    cursor: Option<TradeCursor>,
) -> Result<Value, FetchError> {
    let futures = matches!(route, TradeRoute::BitgetMixFills);
    let mut request = agent
        .get(route.url())
        .query("symbol", market)
        .query("startTime", from_ms.to_string())
        .query("endTime", to_ms.to_string())
        .query("limit", route.max_rows().to_string());
    if futures {
        request = request.query("productType", "USDT-FUTURES");
    }
    if let Some(TradeCursor::LessThanId(id)) = cursor {
        request = request.query("idLessThan", id.to_string());
    }
    let response = request
        .call()
        .map_err(|error| FetchError::Transient(error.to_string()))?;
    super::decode_and_classify(response, "bitget", classify_fills)
}

/// Classify a BitGet fills-history response by status and envelope `code`.
///
/// **Assumption, not settled by vendor docs read for this task**: `fills-history` shares its
/// unknown-symbol codes (`40034` mix, `400172` spot) with `history-candles`. No vendor page for
/// this specific endpoint documents its own unknown-symbol code the way the candle endpoint's
/// pages do — see [`classify`] for the pair this borrows. If a later reader finds this endpoint
/// answers a different code for an unknown symbol, that market falls through to `Transient`
/// below rather than being misclassified as permanent, so the failure mode of a wrong guess here
/// is a retry loop, not a silently wrong "this market does not exist".
///
/// Args:
///     status: HTTP status.
///     body: Decoded response.
///
/// Returns:
///     `Ok(())` on success, or the classified failure.
pub(super) fn classify_fills(status: u16, body: &Value) -> Result<(), FetchError> {
    if !(200..300).contains(&status) {
        return Err(FetchError::Transient(format!("bitget HTTP {status}")));
    }
    let code = body.get("code").and_then(Value::as_str).unwrap_or_default();
    match code {
        "00000" => Ok(()),
        "40034" | "400172" => Err(FetchError::UnknownSymbol),
        "" => Err(FetchError::Transient(format!(
            "bitget HTTP {status}: empty code"
        ))),
        other => {
            let message = body.get("msg").and_then(Value::as_str).unwrap_or("unknown");
            Err(FetchError::Transient(format!("bitget {other}: {message}")))
        }
    }
}

/// Parse a BitGet fills envelope into a page of ticks.
///
/// Rows arrive DESCENDING (newest first, per the vendor's own doc), so the oldest row in this
/// page is the LAST one — that is what both the next cursor and the window-covered check key on.
///
/// Args:
///     body: Decoded response.
///     max_rows: Row cap that was sent, so a FULL page can be told from a short, final one.
///     from_ms: Left edge of the slice, so completeness is judged from the oldest row's own
///         timestamp rather than assumed from the row count alone.
///
/// Returns:
///     The page, or a failure when the envelope is missing.
pub(super) fn parse_fills(
    body: &Value,
    max_rows: usize,
    from_ms: i64,
) -> Result<TradePage, FetchError> {
    let rows = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::Transient("bitget: missing data".to_string()))?;
    let ticks: Vec<Tick> = rows.iter().filter_map(parse_fill_row).collect();
    // A page holding a malformed row alongside valid ones can still finish pagination with a
    // non-empty tick vector that is silently missing rows — a hole must send the window to
    // candles instead of drawing a partial tape as if it were whole.
    if ticks.len() < rows.len() {
        return Err(FetchError::Transient(format!(
            "bitget: page held {} unparseable row(s) of {} (parsed {})",
            rows.len() - ticks.len(),
            rows.len(),
            ticks.len()
        )));
    }
    let oldest_id = rows
        .last()
        .and_then(|r| r.get("tradeId"))
        .and_then(cell_i64)
        .map(|v| v as u64);
    let oldest_time_ms = rows.last().and_then(|r| r.get("ts")).and_then(cell_i64);
    let full = rows.len() >= max_rows;
    let covered = oldest_time_ms.is_some_and(|t| t <= from_ms);
    let next = match (full, covered, oldest_id) {
        (true, false, Some(id)) => Some(TradeCursor::LessThanId(id)),
        // Full page, window not covered, but the oldest row's own `tradeId` did not parse: the
        // cursor to continue from is unknowable. Treating this as completion would silently ship
        // a truncated window as a whole one.
        (true, false, None) => {
            return Err(FetchError::Transient(
                "bitget: full page, window not covered, but the oldest row's tradeId did not parse"
                    .to_string(),
            ));
        }
        _ => None,
    };
    Ok(TradePage { ticks, next })
}

/// Parse one BitGet fills row.
///
/// The vendor's own `side` casing contradicts itself between its field table and its worked
/// example, so it is read case-insensitively here rather than trusted literally.
///
/// Args:
///     row: One element of `data`.
///
/// Returns:
///     The tick, or `None` when the row is malformed.
fn parse_fill_row(row: &Value) -> Option<Tick> {
    let price = cell_f32(row.get("price")?)?;
    let qty = cell_f32(row.get("size")?)?;
    let time_ms = cell_i64(row.get("ts")?)? as f64;
    let side = match row
        .get("side")
        .and_then(Value::as_str)?
        .eq_ignore_ascii_case("sell")
    {
        true => Side::Sell,
        false => Side::Buy,
    };
    Some(Tick {
        time_ms,
        price,
        qty,
        side,
    })
}

#[cfg(test)]
mod tests;
