//! Binance spot, USD-M and COIN-M klines.
//!
//! One grammar serves all three: the same query parameters against three different hosts, and the
//! same positional row. That is why they share this module while every other brand has its own.

use serde_json::Value;

use super::{FetchError, TradeCursor, TradePage, cell_f32};
use crate::feed::types::{Side, Tick};
use crate::market::candles::{ChartCandle, estimate_quote_volume};
use crate::market::trade_replay::venue_caps::{KlineRoute, TradeRoute};

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

/// Fetch one page of public aggregate trades and return the decoded body.
///
/// # Continuation is by id, and the FIRST page is the only one carrying a time range
///
/// The first page of a slice is asked with `startTime`/`endTime`. Every later page instead
/// carries `fromId` ALONE, Binance's own forward cursor — an aggregate-trade id is unambiguous
/// where a millisecond timestamp is not, since more than one trade can share a millisecond — and
/// `startTime`/`endTime` are dropped rather than kept alongside it. That is the vendor's own
/// documented contract, not a preference: Binance's `aggTrades` page states "Sending both
/// startTime/endTime and fromId might cause response timeout, please send either fromId or
/// startTime/endTime." A dense slice needing a second page would otherwise take a documented
/// timeout-prone path on exactly the busy markets where ticks matter most. The client-side clip
/// against `to_ms` in [`parse_agg_trades`] is what bounds a continuation page instead.
///
/// **Resolves the earlier open assumption** ("nothing says `endTime` is rejected alongside
/// `fromId`"): the vendor's own page does warn against the combination, so the assumption is
/// refuted by the docs, not merely tidied away.
///
/// Args:
///     agent: Shared client.
///     route: One of the three Binance trade routes.
///     market: Exchange-native market name.
///     from_ms: Left edge of this slice, inclusive; used only on the first page.
///     to_ms: Right edge of this slice, inclusive; used only on the first page.
///     max_rows: Row cap for this request.
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
    max_rows: usize,
    cursor: Option<TradeCursor>,
) -> Result<Value, FetchError> {
    let request = agent
        .get(route.url())
        .query("symbol", market)
        .query("limit", max_rows.to_string());
    let request = match cursor {
        Some(TradeCursor::FromId(id)) => request.query("fromId", id.to_string()),
        None => request
            .query("startTime", from_ms.to_string())
            .query("endTime", to_ms.to_string()),
        Some(_) => {
            debug_assert!(false, "binance trade routes hand back only FromId");
            request
                .query("startTime", from_ms.to_string())
                .query("endTime", to_ms.to_string())
        }
    };
    let response = request
        .call()
        .map_err(|error| FetchError::Transient(error.to_string()))?;
    super::decode_and_classify(response, "binance", classify)
}

/// Parse a Binance aggregate-trade array into a page of ticks.
///
/// Args:
///     body: Decoded response.
///     to_ms: Right edge of the slice this page belongs to, so completeness is judged from the
///         last row's own timestamp rather than assumed from the row count alone.
///     max_rows: Row cap that was sent, so a FULL page can be told from a short, final one.
///
/// Returns:
///     The page, or a failure when the envelope is not an array.
pub(super) fn parse_agg_trades(
    body: &Value,
    to_ms: i64,
    max_rows: usize,
) -> Result<TradePage, FetchError> {
    let rows = body
        .as_array()
        .ok_or_else(|| FetchError::Transient("binance: response is not an array".to_string()))?;
    let ticks: Vec<Tick> = rows.iter().filter_map(parse_trade_row).collect();
    // A page holding a malformed row alongside valid ones can still finish pagination with a
    // non-empty tick vector that is silently missing rows — dropping one bad row in a thousand is
    // fine for candles, where a missing bar is visible, but not here, where a hole must send the
    // window to candles instead of drawing a partial tape as if it were whole.
    if ticks.len() < rows.len() {
        return Err(FetchError::Transient(format!(
            "binance: page held {} unparseable row(s) of {} (parsed {})",
            rows.len() - ticks.len(),
            rows.len(),
            ticks.len()
        )));
    }
    let last_id = rows.last().and_then(|r| r.get("a")).and_then(Value::as_u64);
    let last_time_ms = rows.last().and_then(|r| r.get("T")).and_then(Value::as_i64);
    let full = rows.len() >= max_rows;
    let covered = last_time_ms.is_some_and(|t| t >= to_ms);
    let next = match (full, covered, last_id) {
        (true, false, Some(id)) => Some(TradeCursor::FromId(id + 1)),
        // The page is full and the window is not yet covered, but the last row's own `a` (trade
        // id) did not parse: the cursor to continue from is unknowable. Treating this as
        // completion would silently ship a truncated window as a whole one — every field name
        // here is inferred with no fixture behind it, so a wrong name yields exactly this shape
        // on every page.
        (true, false, None) => {
            return Err(FetchError::Transient(
                "binance: full page, window not covered, but the last row's `a` did not parse"
                    .to_string(),
            ));
        }
        _ => None,
    };
    Ok(TradePage { ticks, next })
}

/// Parse one Binance aggregate-trade row.
///
/// **COIN-M unit note**: on a `BinanceCoinMAggTrades` row, `q` is a CONTRACT count, not a
/// base-currency amount — `aggTrades` exposes no `baseQty` alternative for COIN-M, the same fact
/// [`COIN_M_BASE_VOLUME_CELL`] already states for klines. `Tick::qty` therefore holds contracts
/// rather than base currency for that one route; no base-asset amount is available from this
/// endpoint. The chart's volume bars stay shape-correct regardless: the chart normalises the
/// visible window against its own maximum `qty`, and a per-instrument contract multiplier is a
/// constant that cancels out of that normalisation — only an ABSOLUTE volume figure would be
/// wrong, and a tick series' aggregated candles never reach the shared SQLite kline cache where
/// one could be read. See `venue_caps.rs`'s trade-route table for the same note.
///
/// Args:
///     row: One element of the response array.
///
/// Returns:
///     The tick, or `None` when the row is malformed.
fn parse_trade_row(row: &Value) -> Option<Tick> {
    let price = cell_f32(row.get("p")?)?;
    let qty = cell_f32(row.get("q")?)?;
    let time_ms = row.get("T")?.as_i64()? as f64;
    // `m == true`: the BUYER was the maker, meaning the TAKER — the side this tape reports — SOLD.
    // Time is read from `T`, never from the aggregate id `a`.
    let side = match row.get("m")?.as_bool()? {
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
