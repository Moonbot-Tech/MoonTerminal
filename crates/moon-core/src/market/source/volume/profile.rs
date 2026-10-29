//! A market's BOUGHT and SOLD turnover by PRICE over a trailing window — the data behind the
//! chart's horizontal volumes, Moonbot's `HVol`.
//!
//! # Shape
//!
//! Rows of one width in price, each holding what was bought and what was sold inside it over the
//! window that ends now. The row grid is anchored at price zero (`row = floor(price / width)`), so
//! a row keeps its place while the market moves and two reads with the same width line up bar for
//! bar. The width is the caller's: the chart bins at the market's tick or a fraction of Moonbot's
//! `PriceFrame` window, whichever is wider, and draws ROLLING sums over that window from these
//! bins — the bins are the resolution, the window is the smoothing.
//!
//! # Sources
//!
//! The same two rings the sides band reads, and for the same reason they cannot double count: a
//! mini-candle exists only because its trades were EVICTED from the trade ring, so the aggregates
//! are read over the whole window and the raw trades strictly after the newest aggregate. A trade
//! lands in the row of its own price; an aggregate carries only its price RANGE, so its two sides
//! are spread over the rows that range crosses, in proportion to how much of the range each row
//! holds — a five-second candle that walked three ticks is three thin rows, not one fat one.
//!
//! Cursor drains bin only new inputs. Each bin retains its ordered addends: expiry re-sums that
//! bin in mini-then-trade order, rather than subtracting floating-point amounts and accumulating
//! rounding dust. Cold seeds and advances run on the shared volume readers, outside VolumeBook.

use std::collections::HashMap;

use super::prepare::HistoryKey;
use moonproto::MoonTime;
use moonproto::state::{MiniCandle, SeqRingReader, TradeHistoryRow};

mod incremental;
mod prepare;

pub(super) use prepare::{ProfileCache, ProfileRequest};

/// The window a profile covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProfileWindow {
    /// The last this many milliseconds.
    Millis(i64),
    /// Everything the retained history holds — Moonbot's `Max`.
    All,
}

/// One row of the profile: a price band and what traded inside it, quote currency.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PriceProfileRow {
    /// Lower price edge of the row, inclusive.
    pub price_lo: f32,
    /// Upper price edge of the row, exclusive.
    pub price_hi: f32,
    /// Bought inside the row over the window, quote currency.
    pub buy_quote: f32,
    /// Sold inside the row over the window, quote currency.
    pub sell_quote: f32,
}

/// Most rows one aggregate may be spread over.
///
/// An aggregate whose range crosses more rows than this — a corrupt row, or a row width far below
/// the market's tick — lands whole in the row of its midpoint instead of spending the frame on a
/// walk; the profile it draws is then coarser than the truth for that one candle, not wrong.
const MAX_SPREAD_ROWS: i64 = 4_096;

/// How long a published profile is reused before its retained cursors advance.
///
/// A short window changes visibly within a second; a long one is dominated by history that does
/// not move, and walking a day of aggregates four times a second to redraw a bar the eye cannot
/// see moving is what turns a chart drag into hitches. `All` sits on the slow clock: it is the
/// widest walk there is.
fn rebuild_ttl_ms(window: ProfileWindow) -> i64 {
    match window {
        ProfileWindow::Millis(ms) if ms <= 3_600_000 => 1_000,
        _ => 5_000,
    }
}

/// One market's built profile and the inputs it was built from.
pub(super) struct PriceProfile {
    /// Sorted by price, one row per non-empty band.
    rows: Vec<PriceProfileRow>,
    /// The window and row width the rows were built for; a different pair is a rebuild.
    window: ProfileWindow,
    row_width: f64,
    /// Retained source identity the rows were built under; a different one is a rebuild.
    source_key: HistoryKey,
    /// When the rows were last built, for the reuse window.
    built_ms: i64,
    /// Bumped on every rebuild whose rows DIFFER from the previous ones, so a caller holding the
    /// last revision can skip the copy — and the GPU upload behind it — when nothing changed.
    revision: u64,
    /// Reusable buffers, so a rebuild that folds in a minute of trades allocates nothing.
    minis: Vec<MiniCandle>,
    trades: Vec<TradeHistoryRow>,
    bins: incremental::Bins,
    /// Independent retained cursors and selected sequence prefixes for the two history sources.
    mini_tape: incremental::Tape,
    trade_tape: incremental::Tape,
}

impl PriceProfile {
    /// An empty profile, before it has been built.
    pub(super) fn new() -> Self {
        Self {
            rows: Vec::new(),
            window: ProfileWindow::All,
            row_width: 0.0,
            source_key: HistoryKey::default(),
            built_ms: i64::MIN,
            revision: 0,
            minis: Vec::new(),
            trades: Vec::new(),
            bins: incremental::Bins::default(),
            mini_tape: incremental::Tape::default(),
            trade_tape: incremental::Tape::default(),
        }
    }

    /// The revision of the rows as they stand.
    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    /// The rows as they stand.
    pub(super) fn rows(&self) -> &[PriceProfileRow] {
        &self.rows
    }

    /// Bin newly retained inputs and expire old sequence prefixes on the profile's reuse clock.
    ///
    /// Args:
    ///     trades: The market's raw trade ring, if it has one.
    ///     minis: Its five-second aggregates, if it has any.
    ///     window: The trailing window to cover.
    ///     row_width: Row width in price units; non-positive reads as no profile.
    ///     source_key: Provider, client, catalog and archive identity; a change rebuilds.
    ///     now: Current unix time in milliseconds.
    pub(super) fn refresh(
        &mut self,
        trades: Option<&SeqRingReader<TradeHistoryRow>>,
        minis: Option<&SeqRingReader<MiniCandle>>,
        window: ProfileWindow,
        row_width: f64,
        source_key: HistoryKey,
        now: i64,
    ) {
        let key_moved =
            self.window != window || self.row_width != row_width || self.source_key != source_key;
        if !key_moved && now.saturating_sub(self.built_ms) < rebuild_ttl_ms(window) {
            return;
        }
        if key_moved {
            self.bins = incremental::Bins::default();
            self.mini_tape = incremental::Tape::default();
            self.trade_tape = incremental::Tape::default();
        }
        self.window = window;
        self.row_width = row_width;
        self.source_key = source_key;
        self.built_ms = now;
        if row_width.is_finite() && row_width > 0.0 {
            let from = match window {
                ProfileWindow::Millis(ms) => now.saturating_sub(ms.max(0)),
                // The epoch: nothing retained is stamped before it, and a time the protocol's
                // clock type can hold is safer than a sentinel it may not.
                ProfileWindow::All => 0,
            };
            self.mini_tape.drain(minis, &mut self.minis);
            let tail_from = self.fold_minis(from, row_width, minis.map_or(0, |r| r.capacity()));
            if self.trade_tape.moved_back(tail_from) {
                self.trade_tape = incremental::Tape::default();
                self.bins.clear_trades();
            }
            self.trade_tape.drain(trades, &mut self.trades);
            self.fold_trades(tail_from, row_width, trades.map_or(0, |r| r.capacity()));
        }
        self.publish(row_width);
    }

    /// Bin new aggregate addends and return the same raw-tail seam as the retained full fold.
    fn fold_minis(&mut self, from: i64, row_width: f64, capacity: usize) -> i64 {
        for row in &self.minis {
            if let Some(seq) = self.mini_tape.push(row.time.unix_millis(), from) {
                visit_mini(row_width, *row, |id, buy, sell| {
                    self.bins.add(true, seq, id, buy, sell)
                });
            }
        }
        self.mini_tape.trim(capacity, from);
        self.mini_tape.newest().map_or(from, |at| at + 1)
    }

    /// Bin new raw prints and expire exactly the retained prefix outside the aggregate seam.
    fn fold_trades(&mut self, from: i64, row_width: f64, capacity: usize) {
        for row in &self.trades {
            if let Some(seq) = self.trade_tape.push(row.time.unix_millis(), from) {
                visit_trade(row_width, *row, |id, buy, sell| {
                    self.bins.add(false, seq, id, buy, sell)
                });
            }
        }
        self.trade_tape.trim(capacity, from);
    }

    /// Materialize sorted bins only after an advance; unchanged rows keep their old revision.
    fn publish(&mut self, row_width: f64) {
        let mut rows: Vec<PriceProfileRow> = self
            .bins
            .totals(self.mini_tape.first(), self.trade_tape.first())
            .filter(|(_, (buy, sell))| *buy > 0.0 || *sell > 0.0)
            .map(|(id, (buy, sell))| PriceProfileRow {
                price_lo: (id as f64 * row_width) as f32,
                price_hi: ((id + 1) as f64 * row_width) as f32,
                buy_quote: buy as f32,
                sell_quote: sell as f32,
            })
            .collect();
        rows.sort_by(|a, b| a.price_lo.total_cmp(&b.price_lo));
        if rows != self.rows {
            self.rows = rows;
            self.revision = self.revision.wrapping_add(1).max(1);
        }
    }
}

/// The row a price falls in.
fn row_of(price: f64, row_width: f64) -> i64 {
    (price / row_width)
        .floor()
        .clamp(i64::MIN as f64 / 4.0, i64::MAX as f64 / 4.0) as i64
}

/// Add to one row.
fn add(bins: &mut HashMap<i64, (f64, f64)>, id: i64, buy: f64, sell: f64) {
    let slot = bins.entry(id).or_insert((0.0, 0.0));
    slot.0 += buy;
    slot.1 += sell;
}

/// Emit the one price contribution of a valid raw print, without choosing its storage.
fn visit_trade(row_width: f64, row: TradeHistoryRow, mut emit: impl FnMut(i64, f64, f64)) {
    let price = f64::from(row.price);
    let value = price * f64::from(row.quantity());
    if !(price.is_finite() && price > 0.0 && value.is_finite() && value > 0.0) {
        return;
    }
    let (buy, sell) = match row.is_buy() {
        true => (value, 0.0),
        false => (0.0, value),
    };
    emit(row_of(price, row_width), buy, sell);
}

/// Fold one aggregate into the rows its price range crosses, in proportion to the overlap.
fn fold_mini(bins: &mut HashMap<i64, (f64, f64)>, row_width: f64, row: MiniCandle) {
    visit_mini(row_width, row, |id, buy, sell| add(bins, id, buy, sell));
}

/// Emit an aggregate's original overlap-weighted addends for incremental or synthetic storage.
fn visit_mini(row_width: f64, row: MiniCandle, mut emit: impl FnMut(i64, f64, f64)) {
    let buy = f64::from(row.buy_vol);
    let sell = f64::from(row.sell_vol);
    let buy = if buy.is_finite() { buy.max(0.0) } else { 0.0 };
    let sell = if sell.is_finite() { sell.max(0.0) } else { 0.0 };
    if buy <= 0.0 && sell <= 0.0 {
        return;
    }
    // Tested BEFORE the min/max: `f32::min` ignores a NaN operand, so a NaN edge would otherwise
    // pass as a one-price range and land the candle on a row nobody traded.
    if !(row.min_price.is_finite() && row.max_price.is_finite()) {
        return;
    }
    let lo = f64::from(row.min_price.min(row.max_price));
    let hi = f64::from(row.min_price.max(row.max_price));
    if hi <= 0.0 {
        return;
    }
    let first = row_of(lo.max(0.0), row_width);
    let last = row_of(hi, row_width);
    let span = hi - lo;
    if first == last || span <= 0.0 || last - first > MAX_SPREAD_ROWS {
        // One row, or too many to walk: the midpoint's row takes it all.
        emit(row_of((lo + hi) * 0.5, row_width), buy, sell);
        return;
    }
    for id in first..=last {
        let row_lo = id as f64 * row_width;
        let row_hi = row_lo + row_width;
        let overlap = (hi.min(row_hi) - lo.max(row_lo)).max(0.0);
        if overlap <= 0.0 {
            continue;
        }
        let share = overlap / span;
        emit(id, buy * share, sell * share);
    }
}

/// A bench's stand-in for the profile: each candle's turnover divided by its direction and spread
/// over the rows between its low and high, exactly as a live aggregate is.
///
/// The fixture holds candles and no trade history, so it has no profile to serve. This gives the
/// zone something to draw on the bench and is NOT a measurement: `docs-internal/FIXTURES.md`
/// lists it among what the bench makes up. The direction split is the same 62/38 the sides band's
/// stand-in uses, so the two bands agree about which way a bar leaned.
///
/// Args:
///     candles: The candles inside the window, any timeframe.
///     row_width: Row width in price units.
///
/// Returns:
///     Rows sorted by price, empty ones omitted.
pub fn synthetic_profile(
    candles: &[crate::market::ChartCandle],
    row_width: f64,
) -> Vec<PriceProfileRow> {
    if !(row_width.is_finite() && row_width > 0.0) {
        return Vec::new();
    }
    let mut bins: HashMap<i64, (f64, f64)> = HashMap::new();
    for c in candles {
        if !(c.quote_volume.is_finite() && c.quote_volume > 0.0) {
            continue;
        }
        let buy_share: f32 = if c.close >= c.open { 0.62 } else { 0.38 };
        fold_mini(
            &mut bins,
            row_width,
            MiniCandle {
                time: MoonTime::ZERO,
                cnt: 1,
                min_price: c.low,
                max_price: c.high,
                buy_vol: c.quote_volume * buy_share,
                sell_vol: c.quote_volume * (1.0 - buy_share),
            },
        );
    }
    let mut rows: Vec<PriceProfileRow> = bins
        .into_iter()
        .filter(|(_, (buy, sell))| *buy > 0.0 || *sell > 0.0)
        .map(|(id, (buy, sell))| PriceProfileRow {
            price_lo: (id as f64 * row_width) as f32,
            price_hi: ((id + 1) as f64 * row_width) as f32,
            buy_quote: buy as f32,
            sell_quote: sell as f32,
        })
        .collect();
    rows.sort_by(|a, b| a.price_lo.total_cmp(&b.price_lo));
    rows
}

#[cfg(test)]
mod tests;
