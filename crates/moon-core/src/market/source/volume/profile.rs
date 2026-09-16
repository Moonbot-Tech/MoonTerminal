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
//! # Why a rebuild, not a cursor
//!
//! The sides band advances from a cursor because its slots are keyed by TIME, and a trailing window
//! over a time-keyed series is two moving pointers. Rows keyed by PRICE have no such order: a
//! trade leaving the window must be found and subtracted from the row it once landed in, which
//! means keeping every row's contributions by time — the whole ring again, twice. A rebuild walks
//! at most the aggregates inside the window plus the raw tail, which is bounded by the retained
//! capacities (tens of thousands of rows at the widest), and it is asked for at the rate the
//! profile is REPAINTED — once a second or slower — not at the rate the market prints.

use std::collections::HashMap;

use moonproto::MoonTime;
use moonproto::state::{MiniCandle, SeqRingReader, TradeHistoryRow};

use super::copy_window;

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

/// How long a built profile is reused before the rings are walked again.
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
    /// `MarketRevisions::archive` the rows were built under; a different one is a rebuild.
    archive_rev: u64,
    /// When the rows were last built, for the reuse window.
    built_ms: i64,
    /// Bumped on every rebuild whose rows DIFFER from the previous ones, so a caller holding the
    /// last revision can skip the copy — and the GPU upload behind it — when nothing changed.
    revision: u64,
    /// Reusable buffers, so a rebuild that folds in a minute of trades allocates nothing.
    minis: Vec<MiniCandle>,
    trades: Vec<TradeHistoryRow>,
    bins: HashMap<i64, (f64, f64)>,
    /// When this profile was last read, for the prune that drops markets nobody charts any more.
    pub(super) used_ms: i64,
}

impl PriceProfile {
    /// An empty profile, before it has been built.
    pub(super) fn new(now: i64) -> Self {
        Self {
            rows: Vec::new(),
            window: ProfileWindow::All,
            row_width: 0.0,
            archive_rev: 0,
            built_ms: i64::MIN,
            revision: 0,
            minis: Vec::new(),
            trades: Vec::new(),
            bins: HashMap::new(),
            used_ms: now,
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

    /// Bring the profile up to date if its inputs moved or its reuse window ran out.
    ///
    /// Args:
    ///     trades: The market's raw trade ring, if it has one.
    ///     minis: Its five-second aggregates, if it has any.
    ///     window: The trailing window to cover.
    ///     row_width: Row width in price units; non-positive reads as no profile.
    ///     archive_rev: The market's `MarketRevisions::archive`; a change rebuilds.
    ///     now: Current unix time in milliseconds.
    pub(super) fn refresh(
        &mut self,
        trades: Option<&SeqRingReader<TradeHistoryRow>>,
        minis: Option<&SeqRingReader<MiniCandle>>,
        window: ProfileWindow,
        row_width: f64,
        archive_rev: u64,
        now: i64,
    ) {
        self.used_ms = now;
        let key_moved =
            self.window != window || self.row_width != row_width || self.archive_rev != archive_rev;
        if !key_moved && now.saturating_sub(self.built_ms) < rebuild_ttl_ms(window) {
            return;
        }
        self.window = window;
        self.row_width = row_width;
        self.archive_rev = archive_rev;
        self.built_ms = now;
        let mut bins = std::mem::take(&mut self.bins);
        bins.clear();
        if row_width.is_finite() && row_width > 0.0 {
            let from = match window {
                ProfileWindow::Millis(ms) => now.saturating_sub(ms.max(0)),
                // The epoch: nothing retained is stamped before it, and a time the protocol's
                // clock type can hold is safer than a sentinel it may not.
                ProfileWindow::All => 0,
            };
            // Read to the far future rather than `now`: the rings hold what the core stamped,
            // and a clock a second ahead of this machine must not drop the newest prints.
            let to = i64::MAX / 2;
            let mut newest_mini = i64::MIN;
            if let Some(reader) = minis {
                copy_window(reader, from, to, &mut self.minis);
                for row in &self.minis {
                    newest_mini = newest_mini.max(row.time.unix_millis());
                    fold_mini(&mut bins, row_width, *row);
                }
            }
            if let Some(reader) = trades {
                // The tail the aggregates do not cover; see the module docs.
                let tail_from = match newest_mini > i64::MIN {
                    true => newest_mini + 1,
                    false => from,
                };
                copy_window(reader, tail_from, to, &mut self.trades);
                for row in &self.trades {
                    fold_trade(&mut bins, row_width, *row);
                }
            }
        }
        let mut rows: Vec<PriceProfileRow> = bins
            .iter()
            .filter(|(_, (buy, sell))| *buy > 0.0 || *sell > 0.0)
            .map(|(id, (buy, sell))| PriceProfileRow {
                price_lo: (*id as f64 * row_width) as f32,
                price_hi: ((*id + 1) as f64 * row_width) as f32,
                buy_quote: *buy as f32,
                sell_quote: *sell as f32,
            })
            .collect();
        rows.sort_by(|a, b| a.price_lo.total_cmp(&b.price_lo));
        self.bins = bins;
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

/// Fold one trade into the row of its price.
fn fold_trade(bins: &mut HashMap<i64, (f64, f64)>, row_width: f64, row: TradeHistoryRow) {
    let price = f64::from(row.price);
    let value = price * f64::from(row.quantity());
    if !(price.is_finite() && price > 0.0 && value.is_finite() && value > 0.0) {
        return;
    }
    let (buy, sell) = match row.is_buy() {
        true => (value, 0.0),
        false => (0.0, value),
    };
    add(bins, row_of(price, row_width), buy, sell);
}

/// Fold one aggregate into the rows its price range crosses, in proportion to the overlap.
fn fold_mini(bins: &mut HashMap<i64, (f64, f64)>, row_width: f64, row: MiniCandle) {
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
        add(bins, row_of((lo + hi) * 0.5, row_width), buy, sell);
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
        add(bins, id, buy * share, sell * share);
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
