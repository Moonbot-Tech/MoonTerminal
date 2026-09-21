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

use super::{FetchError, TradeCursor, TradePage, cell_f32, cell_i64, cell_number, split_no_fill};
use crate::feed::types::{Side, Tick};
use crate::market::candles::ChartCandle;
use crate::market::trade_replay::venue_caps::{KlineRoute, TradeRoute};

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

/// Largest page number the SPOT trades endpoint's own cap allows.
///
/// Gate documents `limit*(page-1) <= 100000` for `/spot/trades`; a page beyond that is refused.
/// Futures pagination is by time and id ([`TradeCursor::Before`]) and carries no such cap.
const SPOT_PAGE_ROW_CAP: usize = 100_000;

/// Fetch one page of public trades and return the decoded body.
///
/// # Order is UNDOCUMENTED here, unlike the candle endpoint
///
/// Both routes' pagination below keys on the ROW COUNT for "is there more", never on a row's
/// position; the spot route assumes no order at all. The futures route's time cursor
/// ([`TradeCursor::Before`]) takes the page's oldest row by value, whatever its position, and
/// its drain ([`TradeCursor::Within`]) filters by a fixed id boundary, so neither loses a row
/// to the order — but the walk's EXHAUSTIVENESS does lean on the MEASURED newest-first order
/// (2026-09-21, see `venue_caps.rs`): a page's oldest row is the walk's next edge only when
/// the page held everything newer, and `walked_part` (`worker.rs`) claims an interrupted walk
/// covered from that edge up on the same ground. [`super::super::worker::serve_ticks`] sorts the complete tick vector once
/// every page of a stage is in.
///
/// Args:
///     agent: Shared client.
///     route: [`TradeRoute::GateSpotTrades`] or [`TradeRoute::GateFuturesTrades`].
///     market: Exchange-native market name, e.g. `BTC_USDT`.
///     from_ms: First millisecond of this slice, inclusive.
///     to_ms: Last millisecond of this slice, inclusive.
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
    let futures = matches!(route, TradeRoute::GateFuturesTrades);
    let market_param = match futures {
        true => "contract",
        false => "currency_pair",
    };
    let (from_s, to_s) = trade_window_seconds(from_ms, to_ms);
    let mut request = agent
        .get(route.url())
        .query(market_param, market)
        .query("limit", route.max_rows().to_string())
        .query("from", from_s.to_string())
        .query("to", to_s.to_string());
    request = match (futures, cursor) {
        (false, Some(TradeCursor::Page(page))) => request.query("page", page.to_string()),
        (false, _) => request.query("page", "1"),
        (true, _) => match offset {
            Some(offset) => request.query("offset", offset.to_string()),
            None => request,
        },
    };
    let response = request
        .call()
        .map_err(|error| FetchError::Transient(error.to_string()))?;
    super::decode_and_classify(response, "gate", classify)
}

/// The `from`/`to` pair a millisecond slice is sent as, in Gate's whole seconds.
///
/// `to` is the slice's last second PLUS ONE, never its truncation: Gate reads a second-valued
/// `to` as "prints up to `to`.000", so `to=…954` does not return a print at `…954.306` while
/// `to=…955` does (probed 2026-09-21 on both routes). A truncated `to` would leave the tail of
/// every slice's last second unasked while the walk marks the slice covered. The widening is
/// harmless: [`super::super::worker::paginate_ticks`] clips every page back to the slice.
///
/// Args:
///     from_ms: First millisecond of the slice, inclusive.
///     to_ms: Last millisecond of the slice, inclusive.
///
/// Returns:
///     `(from, to)` in seconds, `from` floored and `to` one past the slice's last second.
pub(super) fn trade_window_seconds(from_ms: i64, to_ms: i64) -> (i64, i64) {
    (from_ms / MS_PER_S, to_ms / MS_PER_S + 1)
}

/// Parse a Gate SPOT trades array into a page of ticks.
///
/// Args:
///     body: Decoded response.
///     max_rows: Row cap that was sent, so a FULL page can be told from a short, final one.
///     cursor: The cursor this request was sent with, so the NEXT page number is derived from it
///         rather than reconstructed.
///
/// Returns:
///     The page, or a failure when the envelope is not an array.
pub(super) fn parse_spot_trades(
    body: &Value,
    max_rows: usize,
    cursor: Option<TradeCursor>,
) -> Result<TradePage, FetchError> {
    let rows = body
        .as_array()
        .ok_or_else(|| FetchError::Transient("gate: spot response is not an array".to_string()))?;
    let (fills, no_fill) = split_no_fill(rows, "amount");
    let ticks: Vec<Tick> = fills
        .iter()
        .filter_map(|row| parse_spot_trade_row(row))
        .collect();
    // A page holding a malformed row alongside valid ones can still finish pagination with a
    // non-empty tick vector that is silently missing rows — a hole must send the window to
    // candles instead of drawing a partial tape as if it were whole.
    if ticks.len() < fills.len() {
        return Err(FetchError::Transient(format!(
            "gate: spot page held {} unparseable row(s) of {} (parsed {}, {no_fill} of zero size)",
            fills.len() - ticks.len(),
            rows.len(),
            ticks.len()
        )));
    }
    let page = match cursor {
        Some(TradeCursor::Page(page)) => page,
        _ => 1,
    };
    let next_page = page.saturating_add(1);
    let within_cap =
        (next_page.saturating_sub(1) as usize).saturating_mul(max_rows) <= SPOT_PAGE_ROW_CAP;
    let next = match rows.len() >= max_rows && within_cap {
        true => Some(TradeCursor::Page(next_page)),
        false => None,
    };
    Ok(TradePage { ticks, next })
}

/// Parse one Gate spot trade row.
///
/// Args:
///     row: One element of the response array.
///
/// Returns:
///     The tick, or `None` when the row is malformed.
fn parse_spot_trade_row(row: &Value) -> Option<Tick> {
    let price = cell_f32(row.get("price")?)?;
    let qty = cell_f32(row.get("amount")?)?;
    // Prefer the sub-second `create_time_ms`; fall back to `create_time`, which is SECONDS.
    let time_ms = row
        .get("create_time_ms")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| {
            row.get("create_time")
                .and_then(cell_i64)
                .map(|s| (s * MS_PER_S) as f64)
        })?;
    let side = match row.get("side").and_then(Value::as_str)? {
        "sell" => Side::Sell,
        _ => Side::Buy,
    };
    Some(Tick {
        time_ms,
        price,
        qty,
        side,
    })
}

/// Parse a Gate USDT-perpetual trades array into a page of ticks.
///
/// # No `side` field — the SIGN of `size` is the side
///
/// Positive `size` is a buyer-taker fill and negative a seller-taker one; the magnitude is the
/// contract quantity. **Assumption, not settled by vendor docs read for this task**: which sign
/// means which side. Every other Gate/Binance/Bitget/OKX parser in this module reads an explicit
/// `side`/`m` field; this is the one route with no such field at all, so the mapping below is
/// inferred from the vendor's own field name (`size`, signed) rather than confirmed against a
/// recorded response, and it is the first place to look if a Gate futures tick chart shows every
/// trade on the wrong side.
///
/// # Time cells are fractional SECONDS, not milliseconds
///
/// `create_time` and `create_time_ms` both hold `1789726954.306`-style JSON numbers: the suffix
/// names the precision. The vendor says so — gateapi-python `docs/FuturesTrade.md` types
/// `create_time_ms` as `float`, "trade time, with millisecond precision to 3 decimal places",
/// where spot's `docs/Trade.md` types it `str` — and the recorded response agrees. Spot's
/// `create_time_ms` is a millisecond string; the two parsers are deliberately not shared.
///
/// A FULL page is ALWAYS treated as incomplete regardless of any other signal: this endpoint
/// truncates SILENTLY at `limit` with no error, so a full page means "ask again", never "that was
/// all" — see [`super::super::rest::TradePage::next`]'s own doc for why that rule is frozen.
///
/// Args:
///     body: Decoded response.
///     max_rows: Row cap that was sent.
///     cursor: The cursor this request was sent with; its `below_id` is what the rows already
///         held are dropped by.
///
/// Returns:
///     The page, or a failure when the envelope is not an array.
pub(super) fn parse_futures_trades(
    body: &Value,
    max_rows: usize,
    cursor: Option<TradeCursor>,
) -> Result<TradePage, FetchError> {
    let rows = body.as_array().ok_or_else(|| {
        FetchError::Transient("gate: futures response is not an array".to_string())
    })?;
    // The `size: 0` rows a small contract prints between real fills are split off first — see
    // `split_no_fill`; this is the route they were recorded on.
    let (fills, no_fill) = split_no_fill(rows, "size");
    let ticks: Vec<Tick> = fills
        .iter()
        .filter_map(|row| parse_futures_trade_row(row))
        .collect();
    // A page holding a malformed row alongside valid ones can still finish pagination with a
    // non-empty tick vector that is silently missing rows — a hole must send the window to
    // candles instead of drawing a partial tape as if it were whole.
    if ticks.len() < fills.len() {
        return Err(FetchError::Transient(format!(
            "gate: futures page held {} unparseable row(s) of {} (parsed {}, {no_fill} of zero size)",
            fills.len() - ticks.len(),
            rows.len(),
            ticks.len()
        )));
    }
    // The oldest row this page holds — fills and zero-size rows alike, they are rows of the
    // venue's stream and a page of nothing but dead rows must still move the cursor.
    let oldest = rows
        .iter()
        .filter_map(|row| Some((futures_row_time_ms(row)?, futures_row_id(row)?)))
        .min();
    // The lowest id this page took, for the drain's own bookkeeping.
    let lowest_id = rows.iter().filter_map(|row| futures_row_id(row)).min();
    let next = match (cursor, full, oldest) {
        // A drain page: the offset moves by the venue's row count whatever was new, the
        // boundary the rows are filtered by stays; a short page ends the second, and the walk
        // resumes by time up to the second's own start, below everything the drain took.
        (
            Some(TradeCursor::Within {
                second_s,
                offset,
                below_id,
                low_id,
            }),
            true,
            _,
        ) => Some(TradeCursor::Within {
            second_s,
            offset: offset.saturating_add(raw_len as u32),
            below_id,
            low_id: lowest_id.map_or(low_id, |id| id.min(low_id)),
        }),
        (
            Some(TradeCursor::Within {
                second_s, low_id, ..
            }),
            false,
            _,
        ) => Some(TradeCursor::Before {
            boundary_ms: second_s * MS_PER_S - 1,
            below_id: lowest_id.map_or(low_id, |id| id.min(low_id)),
        }),
        (_, true, Some((boundary_ms, id))) => Some(TradeCursor::Before {
            boundary_ms,
            below_id: id,
        }),
        // Full, nothing new: the boundary second holds more prints than a page. Drain it.
        (Some(TradeCursor::Before { boundary_ms, .. }), true, None) => Some(TradeCursor::Within {
            second_s: boundary_ms.div_euclid(MS_PER_S),
            offset: 0,
            below_id,
            low_id: below_id,
        }),
        (_, true, None) => {
            return Err(FetchError::Transient(
                "gate: futures first page full of rows without an id".to_string(),
            ));
        }
        (_, false, _) => None,
    };
    Ok(TradePage { ticks, next })
}

/// The trade id of one Gate futures row, the venue's own ascending counter.
fn futures_row_id(row: &Value) -> Option<u64> {
    row.get("id")?.as_u64()
}

/// The millisecond stamp of one Gate futures row — the same two cells and the same rounding as
/// [`parse_futures_trade_row`], so a cursor's boundary is the stamp the tick carries.
fn futures_row_time_ms(row: &Value) -> Option<i64> {
    let seconds = row
        .get("create_time_ms")
        .and_then(cell_seconds)
        .or_else(|| row.get("create_time").and_then(cell_seconds))?;
    let ms = (seconds * MS_PER_S as f64).round();
    ms.is_finite().then_some(ms as i64)
}

/// Parse one Gate futures trade row.
///
/// **Unit note**: `size` is a signed CONTRACT count, not a base-currency amount — its base
/// amount depends on the contract's `quanto_multiplier`, served by a different endpoint, so no
/// base-asset amount is available here and `Tick::qty` holds contracts for this route. The
/// chart's volume bars stay shape-correct regardless: the chart normalises the visible window
/// against its own maximum `qty`, and a per-instrument contract multiplier is a constant that
/// cancels out of that normalisation — only an ABSOLUTE volume figure would be wrong, and a tick
/// series' aggregated candles never reach the shared SQLite kline cache where one could be read.
/// See `venue_caps.rs`'s trade-route table for the same note.
///
/// `size` is checked for finiteness and sign on the NARROWED `f32`, not the `f64` behind it,
/// exactly as [`cell_f32`] does and for the same reason: narrowing is itself a way to become
/// non-finite, so a check applied before the cast can pass a value that becomes infinite after
/// it.
///
/// Args:
///     row: One element of the response array.
///
/// Returns:
///     The tick, or `None` when the row is malformed.
fn parse_futures_trade_row(row: &Value) -> Option<Tick> {
    let price = cell_f32(row.get("price")?)?;
    let size_raw = cell_number(row.get("size")?)?;
    let qty = size_raw.abs() as f32;
    if !(qty.is_finite() && qty > 0.0) {
        return None;
    }
    let side = match size_raw.is_sign_negative() {
        true => Side::Sell,
        false => Side::Buy,
    };
    // Both time cells are fractional SECONDS (`1789726954.306`), as JSON numbers — the `_ms`
    // suffix names the precision, not the unit (vendor doc: `float`, "millisecond precision to
    // 3 decimal places"), and neither cell is an integer, so a `cell_i64` read would reject every
    // row the venue sends. Recorded 2026-09-21, see `gateio/tests.rs`; spot's `create_time_ms`
    // is a millisecond STRING and parses separately.
    let seconds = row
        .get("create_time_ms")
        .and_then(cell_seconds)
        .or_else(|| row.get("create_time").and_then(cell_seconds))?;
    let time_ms = (seconds * MS_PER_S as f64).round();
    Some(Tick {
        time_ms,
        price,
        qty,
        side,
    })
}

/// Read a Gate futures time cell: fractional seconds, as a JSON number or quoted text.
///
/// Args:
///     cell: One response cell.
///
/// Returns:
///     A finite positive count of seconds, or `None`.
fn cell_seconds(cell: &Value) -> Option<f64> {
    let seconds = cell_number(cell)?;
    (seconds.is_finite() && seconds > 0.0).then_some(seconds)
}

#[cfg(test)]
mod tests;
