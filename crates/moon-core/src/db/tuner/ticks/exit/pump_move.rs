//! PumpsDetection's one sell move — a field of that kind's own settings, not of the sell
//! sections the other kinds share.
//!
//! **PumpMove** (`PumpMoveTimer` non-zero) — once, `PumpMoveTimer` seconds after the take is
//! placed, the sell moves to `PumpMovePersent` per cent of the way from the pump's peak back to
//! the buy (FAQ: "учитывается процент между пиковой ценой и ценой покупки"), the peak read over
//! [`PUMP_PEAK_LOOKBACK_MS`] before the take up to the move. `docs-internal/STRATEGY_FORMULAS/
//! pumpsdetection.md` has the archive it was read off.

use super::line::Line;
use super::{ExitParams, Side, due_by};
use crate::db::tuner::ticks::Fill;
use crate::feed::types::Tick;

/// How far past `PumpMoveTimer` the core's pump move lands, less the model's own placement
/// latency: over 32 archived PumpsDetection lines (2026-09-22, every live Pump strategy runs
/// `PumpMoveTimer` 2 with `PumpMovePersent` 1) the move came 575–704 ms past the timer, a
/// median of 610 ms — counted from the take, not from the buy's report stamp: one take placed
/// 32 s after `buydatems` still moved 2.6 s after itself.
pub const PUMP_MOVE_LAG_MS: i64 = 500;

/// How far before the take the pump's peak is looked for. The FAQ counts the peak from the
/// detect, which the report does not stamp on older rows, and the peak itself is the print that
/// triggered it — a median 70 ms before the buy, up to 6 s when the buy order waited for the
/// retrace. Over the same 32 lines a window from 10 s before the take reproduces the moved level
/// on 31; from the buy it reproduces 1, from 4 s before the take 27.
pub const PUMP_PEAK_LOOKBACK_MS: i64 = 10_000;

/// The pump move's clock: one move, timed off the take (see [`PUMP_MOVE_LAG_MS`]).
pub(super) struct PumpMove<'a> {
    params: &'a ExitParams,
    fill: Fill,
    side: Side,
    /// When the move is due; `None` when the rule is off or the move went.
    next: Option<i64>,
    /// Where the look-back for the pump's peak starts: [`PUMP_PEAK_LOOKBACK_MS`] (by default)
    /// before the take.
    peak_from: i64,
}

impl<'a> PumpMove<'a> {
    pub(super) fn new(params: &'a ExitParams, fill: Fill, side: Side, armed_at: i64) -> Self {
        Self {
            params,
            fill,
            side,
            next: (params.pump_move_timer_s > 0.0).then(|| {
                armed_at
                    + (params.pump_move_timer_s * 1000.0) as i64
                    + params.model.pump_move_lag_ms
            }),
            peak_from: armed_at - params.model.pump_peak_lookback_ms,
        }
    }

    /// The move due by the print at `t_ms`, if any.
    pub(super) fn due(&self, t_ms: i64) -> Option<i64> {
        due_by(self.next, t_ms)
    }

    /// The move due at `due`: to the peak less `PumpMovePersent` of its distance to the buy.
    ///
    /// Args:
    ///     seen: The prints up to and including the one the move is due by.
    pub(super) fn step(&mut self, due: i64, seen: &[Tick], line: &mut Line) {
        self.next = None;
        if let Some(peak) = self.side.extreme_between(seen, self.peak_from, due) {
            let next = peak + (self.fill.price - peak) * self.params.pump_move_pct / 100.0;
            line.place(due, next);
        }
    }
}

#[cfg(test)]
mod tests;
