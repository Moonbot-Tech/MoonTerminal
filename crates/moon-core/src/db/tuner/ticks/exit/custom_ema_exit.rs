//! A boundary B evaluates second B/1000 - 1, whose entire bucket precedes B.
//! The walk aggregates its tape once; advancing a check never reads a later bucket.
//! SellEMADelay postpones checks after purchase; a true check places the sale immediately.
//! https://moonbot.pro/documentation/sell-order-tab/sell-order-tab-parameters

use super::super::custom_ema::{EmaHistoryPath, Formula, FormulaState, Refusal, SecondSeries};
use super::super::deltas::Bar;
use crate::feed::types::Tick;

/// First UTC-second boundary strictly after the supplied check-start time.
/// https://moonbot.pro/documentation/sell-order-tab/sell-order-tab-parameters
pub(crate) fn first_check_ms(fill_ms: i64) -> i64 {
    fill_ms.saturating_add(1000 - fill_ms.rem_euclid(1000))
}

/// Nonnegative delay before checks start, retaining fractional inputs conservatively.
/// https://moonbot.pro/documentation/sell-order-tab/sell-order-tab-parameters
pub(crate) fn sell_delay_ms(delay_s: f64) -> i64 {
    (delay_s.max(0.0) * 1000.0).ceil() as i64
}

/// An independently armed formula clock; missing initial history disables it permanently.
pub struct CustomEmaClock<'a> {
    formula: &'a Formula,
    state: FormulaState<'a>,
    armed_ms: i64,
    next_ms: Option<i64>,
    checked: bool,
    /// The first check could not be evaluated with the available history.
    pub short_history: bool,
    /// History serving the first evaluable check; absent when the clock could not arm.
    pub history_path: Option<EmaHistoryPath>,
    /// True boundary retained separately from the market fill's transport latency.
    pub fired_ms: Option<i64>,
}

impl<'a> CustomEmaClock<'a> {
    /// Arm at the first UTC-second boundary after the supplied check-start time.
    /// https://moonbot.pro/documentation/sell-order-tab/sell-order-tab-parameters
    pub fn new(
        formula: &'a Formula,
        check_start_ms: i64,
        prehistory: &[Bar],
        series: &'a SecondSeries,
    ) -> Self {
        let first = first_check_ms(check_start_ms);
        let mut state = FormulaState::new(formula, series);
        state.warm_from_bars(prehistory, first.div_euclid(1000) - 1);
        Self {
            formula,
            state,
            armed_ms: first,
            next_ms: Some(first),
            checked: false,
            short_history: false,
            history_path: None,
            fired_ms: None,
        }
    }

    /// Whether a live formula may depend on the missing prints by this moment.
    pub fn armed_by(&self, t_ms: i64) -> bool {
        self.next_ms.is_some() && self.armed_ms <= t_ms
    }

    /// Return the first true boundary through `upto_ms`, excluding prints at that boundary.
    /// https://moonbot.pro/documentation/sell-order-tab/sell-order-tab-parameters
    pub fn next_fire(&mut self, upto_ms: i64, ticks_seen: &[Tick]) -> Option<i64> {
        while let Some(boundary) = self.next_ms.filter(|&next| next <= upto_ms) {
            self.next_ms = boundary.checked_add(1000);
            let end = ticks_seen.partition_point(|tick| tick.time_ms < boundary as f64);
            let held = &ticks_seen[..end];
            let at_s = boundary.div_euclid(1000) - 1;
            let warm_start_ms = boundary.saturating_sub(i64::from(self.formula.warm_up_s()) * 1000);
            let warm_hole = !self.checked
                && (held.windows(2).any(|pair| {
                    pair[1].time_ms > warm_start_ms as f64
                        && pair[1].time_ms - pair[0].time_ms > 30_000.0
                }) || held
                    .last()
                    .is_some_and(|tick| boundary as f64 - tick.time_ms > 30_000.0));
            let result = if warm_hole {
                Err(Refusal::ShortHistory)
            } else {
                self.state.advance_to(at_s)
            };
            if !self.checked && result == Err(Refusal::ShortHistory) {
                self.short_history = true;
                self.next_ms = None;
            }
            if !self.checked && result.is_ok() {
                self.history_path = Some(self.state.history_path());
            }
            self.checked = true;
            if result == Ok(true) {
                self.next_ms = None;
                self.fired_ms = Some(boundary);
                return Some(boundary);
            }
        }
        None
    }
}
