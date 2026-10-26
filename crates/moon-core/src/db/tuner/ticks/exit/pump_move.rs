//! PumpsDetection's one sell move matches the core's rule: a rounded timer from the sell-phase
//! start, then a share of the profitable distance from the buy to the farther of the peak and ASK.
//! The tape's last print at or before the move proxies the book's side for longs and shorts.
//! The core's floor buy*(2*commission - 1) is not modelled: the deal has no commission rate,
//! and that floor can bind only when PumpMovePersent exceeds 100. The core's pump detector keeps the
//! peak from the last confirmed detect, raised by later trades, without a fixed window. The
//! report has no detect stamp, so [`PUMP_PEAK_LOOKBACK_MS`] defines the model's proxy window.
//! Source: https://moonbot.pro/documentation/advanced-strategy-parameters/pumpdetection-strategy-and-its-parameters

use super::line::Line;
use super::{ExitParams, Side, due_by};
use crate::db::tuner::ticks::Fill;
use crate::feed::types::Tick;

/// Default proxy look-back before the take for the detector's peak since its last confirmed
/// detect. The report has no detect stamp; the core itself uses no fixed look-back window.
pub const PUMP_PEAK_LOOKBACK_MS: i64 = 10_000;

/// The pump move's clock: one move, timed off the sell-phase start with rounded elapsed seconds.
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
    /// Arm once using round-half-to-even: Round(elapsed/1000) > N first reaches k=floor(N)+1
    /// at k-0.5 seconds for even k, and one millisecond later for odd k.
    pub(super) fn new(params: &'a ExitParams, fill: Fill, side: Side, armed_at: i64) -> Self {
        Self {
            params,
            fill,
            side,
            next: (params.pump_move_timer_s > 0.0).then(|| {
                let k = params.pump_move_timer_s.floor() as i64 + 1;
                armed_at + k * 1000 - 500 + k % 2
            }),
            peak_from: armed_at - params.model.pump_peak_lookback_ms,
        }
    }

    /// The move due by the print at `t_ms`, if any.
    pub(super) fn due(&self, t_ms: i64) -> Option<i64> {
        due_by(self.next, t_ms)
    }

    /// Place the profitable peak/ASK distance less `PumpMovePersent`, mirrored for a short.
    ///
    /// Args:
    ///     seen: The prints up to and including the one the move is due by.
    pub(super) fn step(&mut self, due: i64, seen: &[Tick], line: &mut Line) {
        self.next = None;
        let ask = seen
            .iter()
            .rev()
            .find(|t| t.time_ms as i64 <= due && t.price > 0.0);
        if let Some(ask) = ask {
            let peak = self
                .side
                .extreme_between(seen, self.peak_from, due)
                .unwrap_or(f64::from(ask.price));
            let peak = self.side.farther(peak, f64::from(ask.price));
            let sign = self.side.sign();
            let distance = (sign * (peak - self.fill.price)).max(0.0);
            let next =
                self.fill.price + sign * distance * (1.0 - self.params.pump_move_pct / 100.0);
            line.place(due, next);
        }
    }
}

#[cfg(test)]
mod tests;
