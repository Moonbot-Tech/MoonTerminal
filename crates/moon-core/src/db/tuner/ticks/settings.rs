//! The model's own settings — what the replay assumes about the core and the exchange, and how
//! close a replay must come to the fact to count as reproducing it. None of them is a strategy
//! field: they are the model's, set once by the user of the tuner and read by the verdict, the
//! variant columns and the search alike, so the three can never judge a trade by different rules.
//!
//! Every default is the measured constant it replaces; the measurement stays on the constant
//! (`mshot::DEFAULT_LATENCY_MS`, `exit::stops::TICKER_PERIOD_MS`, `verify::POINT_TIME_TOLERANCE_MS`, …).

use serde::{Deserialize, Serialize};

use super::exit::pump_move::{PUMP_MOVE_LAG_MS, PUMP_PEAK_LOOKBACK_MS};
use super::exit::sell_order::STEP_FLOOR_MS;
use super::exit::stops::{FAST_TRADES_MAX, SERIES_TICK_MS, TICKER_PERIOD_MS};
use super::mshot::{
    DEFAULT_LATENCY_MS, EntryMethod, FAST_ALGO_WINDOW_MS, PRE_SPIKE_LOOKBACK_MS, SHIFT_WINDOW_MS,
};
use super::verify::{
    BOOK_STOP_TIME_TOLERANCE_MS, FILL_IMPROVEMENT_TOLERANCE, POINT_TIME_TOLERANCE_MS,
    STOP_PRICE_TOLERANCE,
};

/// The code-level price estimate compared by a fast stop at the series tick's end.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum FastPrice {
    /// The tick's last print, preserving the current model.
    #[default]
    LastOfTick,
    /// Quantity-weighted price of the last N trades, grouping same-time, same-side prints.
    VwapLastTrades(u8),
    /// One EMA step per grouped trade, initialized by the first trade's price.
    /// The step weight is alpha, or min(1, alpha * qty / cumulative mean qty) when weighted.
    TradeEma { alpha: f64, qty_weighted: bool },
}

/// Where a non-factual stop's panic sell fills, without changing its fire moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StopFill {
    /// Comparison fill at the trigger's check price (`MOON_TICKS_STOP_FILL=check` on the bench).
    AtCheck,
    /// Model the core's marketable Panic Sell after StopLoss AutoActivated and order latency.
    /// Logs show Panic Sell Replacing; use the bid for longs and the ask for shorts.
    /// A check-price fill can sell a positive stop at a shot entry's rebound print,
    /// overstating its return before the panic replacement reaches the book.
    /// Fall back to the last post-fill stop-side print, else the fire price.
    #[default]
    MarketAfterLatency,
}

/// The model's settings. Times in milliseconds, tolerances in per cent.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelSettings {
    /// Not persisted; defaults to a marketable panic sell after latency, matching core stops.
    #[serde(skip)]
    pub stop_fill: StopFill,
    /// Not persisted; the code default is the model's answer.
    #[serde(skip)]
    pub fast_price: FastPrice,
    /// How a MoonShot variant's entry is replayed.
    pub entry_method: EntryMethod,
    /// How long a replacement — of the sell, and of the entry order on a core with no measured
    /// round trip (`Deal::round_trip_ms`) — takes to reach the book.
    pub latency_ms: f64,
    /// The window a re-placed entry order's price is read off.
    pub replace_window_ms: i64,
    /// How far past the fact's fill a shifted order may still be reached by the same spike.
    pub shift_window_ms: i64,
    /// How far before the fill the tape's "price before the spike" is read, when the archive
    /// does not give the ask.
    pub pre_spike_lookback_ms: i64,
    /// How often the core's REST ticker brings the price a non-fast stop watches.
    pub ticker_period_ms: i64,
    /// The core's price-series tick, which a stop at `StopLossEMA` 0 also fires on and at whose
    /// end the fast stop checks `fast_price` (default: the last print).
    pub series_tick_ms: i64,
    /// The floor on a sell-line step delay of zero.
    pub step_floor_ms: i64,
    /// How far past `PumpMoveTimer` the pump move lands.
    pub pump_move_lag_ms: i64,
    /// How far before the take the pump's peak is looked for.
    pub pump_peak_lookback_ms: i64,
    /// Verdict: how far apart in time a modelled and an archived move may be and still be one.
    pub point_time_ms: i64,
    /// Verdict: how far apart in time a modelled and a factual book-watching stop may fire.
    pub book_stop_time_ms: i64,
    /// Verdict: how far a modelled price may sit from the fact's.
    pub price_pct: f64,
    /// Verdict: how far a modelled stop level may sit from the one the core fixed.
    pub stop_price_pct: f64,
    /// Verdict: how much better than the modelled level the fact's fill may be.
    pub fill_improvement_pct: f64,
}

impl Default for ModelSettings {
    /// Keep model defaults in one place, including the unpersisted fast trigger rule.
    fn default() -> Self {
        Self {
            stop_fill: StopFill::default(),
            fast_price: FastPrice::default(),
            entry_method: EntryMethod::default(),
            latency_ms: DEFAULT_LATENCY_MS,
            replace_window_ms: FAST_ALGO_WINDOW_MS,
            shift_window_ms: SHIFT_WINDOW_MS,
            pre_spike_lookback_ms: PRE_SPIKE_LOOKBACK_MS,
            ticker_period_ms: TICKER_PERIOD_MS,
            series_tick_ms: SERIES_TICK_MS,
            step_floor_ms: STEP_FLOOR_MS,
            pump_move_lag_ms: PUMP_MOVE_LAG_MS,
            pump_peak_lookback_ms: PUMP_PEAK_LOOKBACK_MS,
            point_time_ms: POINT_TIME_TOLERANCE_MS,
            book_stop_time_ms: BOOK_STOP_TIME_TOLERANCE_MS,
            price_pct: super::PRICE_TOLERANCE * 100.0,
            stop_price_pct: STOP_PRICE_TOLERANCE * 100.0,
            fill_improvement_pct: FILL_IMPROVEMENT_TOLERANCE * 100.0,
        }
    }
}

/// The longest time any setting may hold, milliseconds — a day. Every time is added to the
/// trade's own millisecond stamps, and a value near `i64::MAX` would wrap them silently (the
/// workspace builds without overflow checks); nothing the model reads is ever longer than the
/// tape around one trade.
pub const MAX_SETTING_MS: i64 = 24 * 60 * 60 * 1000;

/// The widest tolerance any setting may hold, per cent.
pub const MAX_SETTING_PCT: f64 = 100.0;

impl ModelSettings {
    /// The settings with every value inside the range the model can run on: no negative time or
    /// tolerance, the two clocks the walk divides by at least a millisecond, no time past
    /// [`MAX_SETTING_MS`] and no tolerance past [`MAX_SETTING_PCT`]. A value that is not a number
    /// takes the default. Applied wherever settings enter the model — a saved file or a typed
    /// box can hold anything.
    pub fn sanitized(self) -> Self {
        let d = Self::default();
        let ms = |v: i64, floor: i64| v.clamp(floor, MAX_SETTING_MS);
        let num = |v: f64, fallback: f64, ceiling: f64| {
            if v.is_finite() {
                v.clamp(0.0, ceiling)
            } else {
                fallback
            }
        };
        let pct = |v: f64, fallback: f64| num(v, fallback, MAX_SETTING_PCT);
        Self {
            fast_price: match self.fast_price {
                FastPrice::VwapLastTrades(n) => {
                    FastPrice::VwapLastTrades(n.clamp(1, FAST_TRADES_MAX as u8))
                }
                FastPrice::TradeEma {
                    alpha,
                    qty_weighted,
                } if alpha.is_finite() && alpha > 0.0 => FastPrice::TradeEma {
                    alpha: alpha.min(1.0),
                    qty_weighted,
                },
                _ => FastPrice::LastOfTick,
            },
            stop_fill: self.stop_fill,
            entry_method: self.entry_method,
            latency_ms: num(self.latency_ms, d.latency_ms, MAX_SETTING_MS as f64),
            replace_window_ms: ms(self.replace_window_ms, 0),
            shift_window_ms: ms(self.shift_window_ms, 0),
            pre_spike_lookback_ms: ms(self.pre_spike_lookback_ms, 0),
            ticker_period_ms: ms(self.ticker_period_ms, 1),
            series_tick_ms: ms(self.series_tick_ms, 1),
            step_floor_ms: ms(self.step_floor_ms, 0),
            pump_move_lag_ms: ms(self.pump_move_lag_ms, 0),
            pump_peak_lookback_ms: ms(self.pump_peak_lookback_ms, 0),
            point_time_ms: ms(self.point_time_ms, 0),
            book_stop_time_ms: ms(self.book_stop_time_ms, 0),
            price_pct: pct(self.price_pct, d.price_pct),
            stop_price_pct: pct(self.stop_price_pct, d.stop_price_pct),
            fill_improvement_pct: pct(self.fill_improvement_pct, d.fill_improvement_pct),
        }
    }

    /// The replacement latency in whole milliseconds, never negative nor past
    /// [`MAX_SETTING_MS`], whether or not the settings were sanitized.
    pub fn latency_whole_ms(&self) -> i64 {
        (self.latency_ms.max(0.0) as i64).min(MAX_SETTING_MS)
    }
}

#[cfg(test)]
mod tests;
