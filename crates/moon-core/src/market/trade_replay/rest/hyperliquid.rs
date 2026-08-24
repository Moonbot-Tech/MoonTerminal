//! Hyperliquid candles, spot and perpetual alike, from the `candleSnapshot` info request.
//!
//! The one route here that is a POST with a JSON body rather than a GET with a query string. That
//! costs the surrounding abstraction nothing — a [`KlineRoute`] has always been a request SHAPE
//! rather than a URL, and the REST layer already branches per shape — and it is not new ground in
//! this crate either: [`crate::db::valuation`] posts to this same host the same way.
//!
//! There is no market-name translation to do. The core reports Hyperliquid markets in
//! Hyperliquid's own spelling already (`crate::symbol::Exchange::Hyperliquid`: a bare `BTC`, a
//! HIP-3 `xyz:BIRD`, a spot index `@206`, or the one named spot pair `PURR/USDC`), and those are
//! exactly the strings `candleSnapshot` takes for its `coin`. The `spotMeta` lookup this would
//! otherwise need does not exist because it is not needed.

use serde_json::Value;

use super::{cell_f32, cell_i64, FetchError};
use crate::market::candles::ChartCandle;
use crate::market::trade_replay::venue_caps::KlineRoute;

/// Fetch one page and return the decoded body.
///
/// Args:
///     agent: Shared client.
///     route: [`KlineRoute::Hyperliquid`].
///     market: Coin as Hyperliquid names it.
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
    let payload = serde_json::json!({
        "type": "candleSnapshot",
        "req": {
            "coin": market,
            "interval": "1m",
            "startTime": from_ms,
            "endTime": to_ms,
        },
    });
    let response = agent
        .post(route.url())
        .send_json(&payload)
        .map_err(|error| FetchError::Transient(error.to_string()))?;
    let status = response.status().as_u16();
    classify(status)?;
    response
        .into_body()
        .read_json()
        .map_err(|error| FetchError::Transient(format!("hyperliquid JSON: {error}")))
}

/// Classify a `candleSnapshot` response by status alone.
///
/// # Why every failure here is transient, including an unknown coin
///
/// Hyperliquid answers an unknown coin with HTTP 500 and a body of literally `null`. A genuine
/// backend failure answers HTTP 500 and a body of literally `null`. There is no code, no message
/// and no field that separates them, so this classifier does not pretend to tell them apart.
///
/// Calling both transient is the direction that cannot do damage to the DATA.
/// [`crate::market::trade_replay::worker`] caches nothing on a transient failure, so no wrong
/// answer is ever remembered. The opposite choice — reading every 500 as an unknown symbol —
/// would hand a permanent verdict to a market that was merely unreachable for a moment.
///
/// # What that choice costs, stated in full
///
/// It is not free, and the cost is not confined to the market that provoked it. Only the
/// `UnknownSymbol` arm of [`crate::market::trade_replay::worker`] clears a host's refusal history
/// before returning; a transient failure returns without clearing. Since this classifier can
/// never produce `UnknownSymbol`, no Hyperliquid FAILURE ever clears the host — so a delisted or
/// renamed market, reopened a few times, walks `api.hyperliquid.xyz` up the backoff curve to its
/// ceiling. That host key covers Hyperliquid spot AND perpetual, so every other Hyperliquid
/// replay is refused with a countdown until some unrelated fetch on that host succeeds.
///
/// That is accepted rather than overlooked: the alternative trades a recoverable, self-clearing
/// throttle for an unrecoverable wrong verdict on a market that was only briefly unreachable.
///
/// The body is deliberately not inspected: reading `null` as a signal would be reading the
/// absence of information as information.
///
/// Args:
///     status: HTTP status.
///
/// Returns:
///     `Ok(())` on success, or the classified failure.
pub(super) fn classify(status: u16) -> Result<(), FetchError> {
    match (200..300).contains(&status) {
        true => Ok(()),
        false => Err(FetchError::Transient(format!("hyperliquid HTTP {status}"))),
    }
}

/// Parse a `candleSnapshot` array into bars.
///
/// Rows are OBJECTS with terse keys: `t` the open time in MILLISECONDS as a JSON number, `T` the
/// close time, `o`/`h`/`l`/`c` the prices as STRINGS, `v` base-asset volume as a string, and `n`
/// the trade count. `v` is genuinely base volume, so unlike Gate futures and OKX swaps there is no
/// contract unit to reconcile here.
///
/// The envelope carries no "this bar has closed" FLAG, so unlike Gate spot and OKX this parser
/// filters nothing here. It does carry `T`, the bar's scheduled close time, which a clock could be
/// compared against — but that comparison belongs to
/// [`crate::market::trade_replay::worker`], which drops an unfinished bar for every venue at once
/// off the bar's own open time. Two of these venues send neither a flag nor a close time, so a
/// per-venue filter could not have covered them anyway, and a clock read inside a parser is what
/// would stop the recorded fixtures beside this module from being a complete test of it.
///
/// Args:
///     body: Decoded response.
///
/// Returns:
///     Bars in the response's own order, or a failure when the envelope is not an array.
pub(super) fn parse_klines(body: &Value) -> Result<Vec<ChartCandle>, FetchError> {
    let rows = body.as_array().ok_or_else(|| {
        FetchError::Transient("hyperliquid: response is not an array".to_string())
    })?;
    Ok(rows.iter().filter_map(parse_row).collect())
}

/// Parse one `candleSnapshot` candle object.
///
/// Args:
///     row: One element of the response array.
///
/// Returns:
///     The bar, or `None` when the object is malformed.
fn parse_row(row: &Value) -> Option<ChartCandle> {
    Some(ChartCandle {
        t_open_ms: cell_i64(row.get("t")?)? as f64,
        open: cell_f32(row.get("o")?)?,
        high: cell_f32(row.get("h")?)?,
        low: cell_f32(row.get("l")?)?,
        close: cell_f32(row.get("c")?)?,
        volume: row.get("v").and_then(cell_f32).unwrap_or(0.0),
    })
}

#[cfg(test)]
mod tests;
