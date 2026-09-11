//! Conservative display-time estimates for second-resolution live report records.
//!
//! A public print is not an execution receipt. Only a matching price in the reported second
//! supplies a display-time estimate; reported prices and the source records remain unchanged.

use std::collections::BTreeMap;

use super::{TapePrint, TradeMark};

/// Per-pane matches, bounded by twice the report row count rather than by live tape volume.
/// Retaining matches across ring eviction keeps old arrows stable while the chart scrolls.
#[derive(Default)]
pub struct LiveTradeSnap {
    marks: Vec<TradeMark>,
    matches: BTreeMap<(i64, u32), Option<i64>>,
    seed_pending: bool,
}

/// Match at the feed's f32 price precision, without accepting a neighbouring price level.
fn key(time: i64, price: f64) -> Option<(i64, u32)> {
    let feed_price = price as f32;
    (time > 0 && feed_price.is_finite() && feed_price > 0.0)
        .then_some((time.div_euclid(1_000), feed_price.to_bits()))
}

impl LiveTradeSnap {
    /// Smallest half-open interval covering all retained second-resolution report ends.
    pub fn window(&self) -> Option<(i64, i64)> {
        let (&(first, _), _) = self.matches.first_key_value()?;
        let (&(last, _), _) = self.matches.last_key_value()?;
        Some((
            first.saturating_mul(1_000),
            last.saturating_add(1).saturating_mul(1_000),
        ))
    }
    /// Retire the previous provider's estimates while keeping report ends ready for the first batch.
    pub fn reset_matches(&mut self) {
        self.matches.values_mut().for_each(|time| *time = None);
        self.seed_pending = !self.matches.is_empty();
    }

    /// Whether new report inputs or a changed source still need one exact retained-ring scan.
    pub fn needs_seed(&self) -> bool {
        self.seed_pending
    }

    /// Newly received archive rows may precede the visible tick range; recheck retained history.
    pub fn request_seed(&mut self) {
        self.seed_pending = !self.matches.is_empty();
    }

    /// A successful source read, even an empty ring, ends the one-shot seed attempt.
    /// Missing source/readers leave it pending until another market revision arrives.
    pub fn seed_complete(&mut self) {
        self.seed_pending = false;
    }

    /// Replace the filtered report inputs, retaining estimates only for unchanged time/price keys.
    /// Returns whether a resident-tape scan is needed for newly published or revised records.
    pub fn set_marks(&mut self, marks: &[TradeMark]) -> bool {
        if self.marks == marks {
            return false;
        }
        let mut previous = std::mem::take(&mut self.matches);
        for mark in marks {
            for (time, price) in [
                (mark.buy_ms, mark.buy_price),
                (mark.close_ms, mark.sell_price),
            ] {
                // This correction belongs only to second-resolution report stamps.
                if time % 1_000 == 0 {
                    if let Some(key) = key(time, price) {
                        self.matches
                            .entry(key)
                            .or_insert_with(|| previous.remove(&key).flatten());
                    }
                }
            }
        }
        self.marks = marks.to_vec();
        self.seed_pending = !self.matches.is_empty();
        true
    }

    /// Observe ordinary prints from this pane's exact market, including late history batches.
    /// Earliest matching time is deterministic across batch order; unrelated ticks cost one lookup.
    pub fn observe(&mut self, tape: impl IntoIterator<Item = TapePrint>) -> bool {
        if self.matches.is_empty() {
            return false;
        }
        let mut changed = false;
        for print in tape {
            let Some(key) = key(print.t_ms, print.price) else {
                continue;
            };
            let Some(found) = self.matches.get_mut(&key) else {
                continue;
            };
            if found.is_none_or(|time| print.t_ms < time) {
                *found = Some(print.t_ms);
                changed = true;
            }
        }
        changed
    }

    /// Return display marks with original prices and a non-reversed entry/exit interval.
    /// Missing matches retain report time; conflicting estimates revert the whole pair.
    pub fn marks(&self) -> Vec<TradeMark> {
        self.marks
            .iter()
            .copied()
            .map(|mut mark| {
                let estimate = |time: i64, price| {
                    if time % 1_000 != 0 {
                        return time;
                    }
                    key(time, price)
                        .and_then(|key| self.matches.get(&key).copied().flatten())
                        .unwrap_or(time)
                };
                let entry = estimate(mark.buy_ms, mark.buy_price);
                let exit = estimate(mark.close_ms, mark.sell_price);
                if entry <= exit {
                    mark.buy_ms = entry;
                    mark.close_ms = exit;
                }
                mark
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
