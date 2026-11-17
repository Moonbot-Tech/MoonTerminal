//! What a closed trade's replay says was traded over one stretch of its window.
//!
//! The trade window refuses the live tape on purpose — that tape is the market now, not the deal on
//! screen — so the percent ruler and the cursor-anchored volume captions read the replay instead.
//! The replay already holds the answer: the per-second split of the raw prints
//! ([`TradeReplaySeries::side_slots`], the volume band's own data) inside the stretches the tick walk
//! proved exhaustive ([`TradeReplaySeries::covered`]), and the venue's one-minute bars everywhere
//! else. This module sums those two sources over a period without counting any millisecond twice,
//! and says how far the sum can be trusted.

use super::TradeReplaySeries;
use crate::market::source::SIDE_BUCKET_MS;

/// How much of a period the replay's prints answer, which decides what a reader may be shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayVolumeFit {
    /// The period lies wholly inside the tick coverage and takes every traded second it touches
    /// whole: the bought and sold amounts are measured.
    Exact,
    /// Inside the coverage, but an end cuts a second that traded. The stored split is one second,
    /// so that second contributes its time share — an estimate of the sides, not a measurement.
    CutSecond,
    /// Part of the period lies outside the coverage, where only the bars answer. Their turnover
    /// has no buy/sell split (the band's 62/38 lean is a picture, not a measurement), so only the
    /// total means anything, and it is an estimate.
    Bars,
}

/// Traded value over one period of a replay, in the market's quote currency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReplayVolume {
    /// Bought by the prints over the covered part of the period.
    pub buy_quote: f64,
    /// Sold by the prints over the covered part of the period.
    pub sell_quote: f64,
    /// The bars' turnover over the uncovered part, each bar spread evenly over its own time.
    pub bars_quote: f64,
    /// How far these figures can be trusted.
    pub fit: ReplayVolumeFit,
}

impl ReplayVolume {
    /// Everything traded over the period: both sides of the prints plus the bars' share.
    pub fn total_quote(self) -> f64 {
        self.buy_quote + self.sell_quote + self.bars_quote
    }
}

impl TradeReplaySeries {
    /// What this replay says was traded over `[from_ms, to_ms)`.
    ///
    /// Inside [`Self::covered`] the prints' per-second slots answer; a slot only partly inside the
    /// period contributes the share of its covered milliseconds the period takes. Outside it the
    /// bars answer, each one's turnover spread evenly over its own time and only over the
    /// uncovered milliseconds — the volume band adds a boundary second from both sources, this
    /// does not. With no slots at all (no prints, or a contract size this build cannot value) the
    /// whole period goes to the bars, as the band does.
    ///
    /// Args:
    ///     from_ms: Start of the period, unix milliseconds, inclusive.
    ///     to_ms: End of the period, unix milliseconds, exclusive.
    ///
    /// Returns:
    ///     The figures and their fit, or `None` when the period is empty or neither the prints nor
    ///     a bar reach any of it.
    pub fn volume_between(&self, from_ms: i64, to_ms: i64) -> Option<ReplayVolume> {
        if to_ms <= from_ms {
            return None;
        }
        // Half-open covered spans, clipped to the period. No slots means the prints cannot answer
        // even where the walk covered the ground, so nothing counts as covered.
        let spans: Vec<(i64, i64)> = match self.side_slots.is_empty() {
            true => Vec::new(),
            false => self
                .covered
                .spans()
                .iter()
                .map(|&(lo, hi)| (lo, hi.saturating_add(1)))
                .collect(),
        };
        let inside: Vec<(i64, i64)> = spans
            .iter()
            .filter_map(|&span| intersect(span, (from_ms, to_ms)))
            .collect();
        let covered_ms: i64 = inside.iter().map(|&(a, b)| b - a).sum();

        let (mut buy, mut sell, mut cut) = (0.0f64, 0.0f64, false);
        let first_id = from_ms.div_euclid(SIDE_BUCKET_MS);
        let start = self.side_slots.partition_point(|s| s.id < first_id);
        for slot in &self.side_slots[start..] {
            let second = (slot.id * SIDE_BUCKET_MS, (slot.id + 1) * SIDE_BUCKET_MS);
            if second.0 >= to_ms {
                break;
            }
            // The slot holds the prints of its second's covered milliseconds only — the walk is
            // exhaustive there and fetched nothing outside — so its share is taken against those.
            let held = overlap_ms(second, &spans);
            let taken = overlap_ms(second, &inside);
            if held <= 0 || taken <= 0 {
                continue;
            }
            let share = match taken >= held {
                true => 1.0,
                false => {
                    cut = true;
                    taken as f64 / held as f64
                }
            };
            buy += slot.buy * share;
            sell += slot.sell * share;
        }

        let (mut bars, mut bar_reached) = (0.0f64, false);
        let tf = self.tf_ms.max(1);
        for candle in &self.candles {
            if !candle.t_open_ms.is_finite() {
                continue;
            }
            let open = candle.t_open_ms as i64;
            let Some(piece) = intersect((open, open.saturating_add(tf)), (from_ms, to_ms)) else {
                continue;
            };
            let uncovered = (piece.1 - piece.0) - overlap_ms(piece, &inside);
            if uncovered <= 0 {
                continue;
            }
            bar_reached = true;
            let turnover = f64::from(candle.quote_volume);
            if turnover.is_finite() && turnover > 0.0 {
                bars += turnover * uncovered as f64 / tf as f64;
            }
        }

        if covered_ms == 0 && !bar_reached {
            return None;
        }
        let fit = if covered_ms < to_ms - from_ms {
            ReplayVolumeFit::Bars
        } else if cut {
            ReplayVolumeFit::CutSecond
        } else {
            ReplayVolumeFit::Exact
        };
        Some(ReplayVolume {
            buy_quote: buy,
            sell_quote: sell,
            bars_quote: bars,
            fit,
        })
    }
}

/// The common part of two half-open intervals, or `None` when they do not meet.
fn intersect(a: (i64, i64), b: (i64, i64)) -> Option<(i64, i64)> {
    let (from, to) = (a.0.max(b.0), a.1.min(b.1));
    (to > from).then_some((from, to))
}

/// How many milliseconds of the half-open `span` the disjoint half-open `pieces` cover.
fn overlap_ms(span: (i64, i64), pieces: &[(i64, i64)]) -> i64 {
    pieces
        .iter()
        .filter_map(|&piece| intersect(span, piece))
        .map(|(a, b)| b - a)
        .sum()
}

#[cfg(test)]
mod tests;
