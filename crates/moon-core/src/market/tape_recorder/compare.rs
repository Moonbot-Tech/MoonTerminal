//! One settled trade, recorded twice: by the recorder's archive requests and by the terminal's
//! close-time capture from its live ring (`trades.sqlite`). Pure — the two tapes and their
//! coverage in, the numbers out — so the verdict's arithmetic is a unit test.
//!
//! The ring capture is the reference: it is what the tuner reads today. Only the stretch both
//! tapes cover is compared print for print; the rest is reported as coverage.
//!
//! The measure is the one the entry model cares about (`STATION.md` §7.7): the MoonShot model
//! reads a print's price, time and side and fills on the first print that reaches a level. So from
//! an anchor every two seconds, levels 0.05–2 % above and below the reference's last price are
//! tested for the first print reaching them within a minute, in each tape. A level one tape reaches
//! and the other does not is a fill the tuner would decide differently.

use crate::feed::Tick;
use crate::market::trade_replay::Coverage;

/// Levels a touch is tested at, as fractions of the anchor price — the set of §7.7.
const LEVELS: [f64; 6] = [0.0005, 0.001, 0.0025, 0.005, 0.01, 0.02];
/// Spacing of the anchors.
const ANCHOR_STEP_MS: i64 = 2_000;
/// How long after an anchor a touch counts.
const TOUCH_HORIZON_MS: i64 = 60_000;
/// Two touches this close in time are the same fill for the model's 100 ms extremum window.
const NEAR_MS: i64 = 100;

/// Touch tallies over every anchor and level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Touches {
    /// Reached in both tapes.
    pub both: u64,
    /// Reached in the ring capture only — a fill the recorded tape would miss.
    pub only_captured: u64,
    /// Reached in the recorded tape only — a fill it would invent.
    pub only_recorded: u64,
    /// Of `both`, reached at the same millisecond.
    pub same_ms: u64,
    /// Of `both`, reached within [`NEAR_MS`].
    pub near: u64,
}

impl Touches {
    pub(super) fn add(&mut self, other: Touches) {
        self.both += other.both;
        self.only_captured += other.only_captured;
        self.only_recorded += other.only_recorded;
        self.same_ms += other.same_ms;
        self.near += other.near;
    }
}

/// One trade's verdict.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Comparison {
    /// Milliseconds the trade needs.
    pub needed_ms: i64,
    /// Of those, covered by the recorder.
    pub recorded_ms: i64,
    /// Of those, covered by the ring capture.
    pub captured_ms: i64,
    /// Of those, covered by both — everything below is counted over this stretch alone.
    pub common_ms: i64,
    pub recorded_prints: usize,
    pub captured_prints: usize,
    /// Summed quantity, both sides.
    pub recorded_volume: f64,
    pub captured_volume: f64,
    pub touches: Touches,
}

/// A tape as read back: what it covers, and its prints ascending.
pub(super) struct Tape<'a> {
    pub covered: &'a Coverage,
    pub ticks: &'a [Tick],
}

/// Compare the two tapes of one trade over the stretches it `needed`.
pub(super) fn compare(needed: &Coverage, recorded: Tape<'_>, captured: Tape<'_>) -> Comparison {
    let recorded_cov = recorded.covered.clip(needed);
    let captured_cov = captured.covered.clip(needed);
    let common = recorded_cov.clip(&captured_cov);
    let rec = inside(recorded.ticks, &common);
    let cap = inside(captured.ticks, &common);
    Comparison {
        needed_ms: needed.width_ms(),
        recorded_ms: recorded_cov.width_ms(),
        captured_ms: captured_cov.width_ms(),
        common_ms: common.width_ms(),
        recorded_prints: rec.len(),
        captured_prints: cap.len(),
        recorded_volume: volume(&rec),
        captured_volume: volume(&cap),
        touches: touches(&cap, &rec, &common),
    }
}

/// The prints of an ascending run that fall inside `covered`.
fn inside(ticks: &[Tick], covered: &Coverage) -> Vec<Tick> {
    ticks
        .iter()
        .filter(|t| covered.contains_ms(t.time_ms as i64))
        .copied()
        .collect()
}

fn volume(ticks: &[Tick]) -> f64 {
    ticks.iter().map(|t| f64::from(t.qty.abs())).sum()
}

/// Tally the first touches of every level from every anchor of `common`, the reference's last
/// price at the anchor being the level's base. An anchor needs a full horizon inside one covered
/// stretch, or "not reached" could mean "not covered".
fn touches(reference: &[Tick], recorded: &[Tick], common: &Coverage) -> Touches {
    let mut out = Touches::default();
    for &(from_ms, to_ms) in common.spans() {
        let mut anchor = from_ms;
        while anchor.saturating_add(TOUCH_HORIZON_MS) <= to_ms {
            let end = anchor + TOUCH_HORIZON_MS;
            let at = reference.partition_point(|t| (t.time_ms as i64) <= anchor);
            let base = at
                .checked_sub(1)
                .map(|i| reference[i])
                .filter(|t| (t.time_ms as i64) >= from_ms);
            if let Some(base) = base {
                let base = f64::from(base.price);
                let targets: Vec<f64> = LEVELS
                    .iter()
                    .flat_map(|l| [base * (1.0 + l), -(base * (1.0 - l))])
                    .collect();
                let hits_ref = first_hits(reference, anchor, end, &targets);
                let hits_rec = first_hits(recorded, anchor, end, &targets);
                for (r, c) in hits_ref.iter().zip(&hits_rec) {
                    match (r, c) {
                        (Some(r), Some(c)) => {
                            out.both += 1;
                            let gap = (r - c).abs();
                            out.same_ms += u64::from(gap == 0);
                            out.near += u64::from(gap <= NEAR_MS);
                        }
                        (Some(_), None) => out.only_captured += 1,
                        (None, Some(_)) => out.only_recorded += 1,
                        (None, None) => {}
                    }
                }
            }
            anchor += ANCHOR_STEP_MS;
        }
    }
    out
}

/// For each target, the first print in `(after, until]` reaching it: a positive target is reached
/// from below (price at or above it), a negative one — a level under the price, sign-flipped —
/// from above.
fn first_hits(ticks: &[Tick], after: i64, until: i64, targets: &[f64]) -> Vec<Option<i64>> {
    let mut hits = vec![None; targets.len()];
    let start = ticks.partition_point(|t| (t.time_ms as i64) <= after);
    for tick in &ticks[start..] {
        let at = tick.time_ms as i64;
        if at > until {
            break;
        }
        let price = f64::from(tick.price);
        for (hit, &target) in hits.iter_mut().zip(targets) {
            let reached = match target >= 0.0 {
                true => price >= target,
                false => price <= -target,
            };
            if hit.is_none() && reached {
                *hit = Some(at);
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests;
