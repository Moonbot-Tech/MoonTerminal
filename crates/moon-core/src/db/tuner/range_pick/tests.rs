//! Equivalence tests for the shared range picker.
//!
//! The linear scan replaced an exhaustive pair loop under a promise that it returns the SAME
//! slice, ties included. That promise is only worth anything if the exhaustive loop stays around
//! to be compared against, so it lives here permanently as the oracle.

use super::*;

/// Deterministic fixture generator; not the search's PRNG, so fixtures do not move when that
/// stream changes.
struct Lcg(u64);

impl Lcg {
    /// Advance and return the next 64-bit state.
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }

    /// Uniform value in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform integer in `0..n`.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// The exhaustive pair scan the linear picker replaced, kept as the oracle.
///
/// Enumerates `i` then `j` ascending and keeps the first strict maximum — which is precisely the
/// tie-breaking rule [`best_pair`] has to reproduce.
fn best_pair_reference(
    profit_at: &[f64],
    count_at: &[usize],
    min_n: usize,
    tot_c: usize,
    floor: f64,
) -> Option<(usize, usize)> {
    let last = profit_at.len() - 1;
    let mut best: Option<(usize, usize)> = None;
    let mut best_profit = floor;
    for i in 0..last {
        for j in (i + 1)..=last {
            let count = count_at[j] - count_at[i];
            if count < min_n {
                continue;
            }
            if count == tot_c {
                continue;
            }
            let profit = profit_at[j] - profit_at[i];
            if profit > best_profit {
                best_profit = profit;
                best = Some((i, j));
            }
        }
    }
    best
}

/// The linear picker must reproduce the exhaustive oracle across sparse and tied fixtures.
#[test]
fn linear_pick_matches_the_exhaustive_scan() {
    let mut rng = Lcg(0xC0FFEE);
    let mut saw_full_coverage = 0usize;
    let mut saw_a_pick = 0usize;
    for case in 0..4000 {
        let edges = 1 + rng.below(12);
        // Bin contents, a third of them empty on purpose: repeated cumulative counts are what
        // the admissible-prefix pointer has to survive.
        let mut bin_count = vec![0usize; edges + 1];
        let mut bin_profit = vec![0.0f64; edges + 1];
        for k in 0..=edges {
            bin_count[k] = if rng.unit() < 0.33 { 0 } else { rng.below(5) };
            // Few distinct profits, so equal-profit slices are frequent rather than incidental.
            bin_profit[k] = (rng.below(5) as f64 - 2.0) * bin_count[k] as f64;
        }
        // Half the cases leave the unreachable bucket empty, which is what lets a slice cover
        // the whole sample and triggers the full-coverage rejection.
        if case % 2 == 0 {
            bin_count[edges] = 0;
            bin_profit[edges] = 0.0;
        }
        let tot_c: usize = bin_count.iter().sum();
        let mut profit_at = vec![0.0f64; edges + 1];
        let mut count_at = vec![0usize; edges + 1];
        for k in 0..edges {
            profit_at[k + 1] = profit_at[k] + bin_profit[k];
            count_at[k + 1] = count_at[k] + bin_count[k];
        }
        if count_at[edges] == tot_c {
            saw_full_coverage += 1;
        }
        for &min_n in &[1usize, 2, 5] {
            for &floor in &[f64::NEG_INFINITY, 0.0, 1.0] {
                let fast = best_pair(&profit_at, &count_at, min_n, tot_c, floor);
                let slow = best_pair_reference(&profit_at, &count_at, min_n, tot_c, floor);
                assert_eq!(
                    fast, slow,
                    "case {case}: edges={edges} min_n={min_n} floor={floor} \
                     counts={bin_count:?} profits={bin_profit:?}"
                );
                if fast.is_some() {
                    saw_a_pick += 1;
                }
            }
        }
    }
    // Without these, a run where every case returned `None` would pass while proving nothing.
    assert!(
        saw_full_coverage > 100,
        "fixtures never exercised full coverage ({saw_full_coverage} cases)"
    );
    assert!(
        saw_a_pick > 1000,
        "fixtures almost never selected a range ({saw_a_pick} selections)"
    );
}

/// A range that changes no trade membership is not a usable filter.
#[test]
fn a_slice_covering_every_trade_is_rejected() {
    // One populated bin and nothing unreachable: the only slices that keep anything also keep
    // everything, so there is no filter to suggest. This is the shape the all-fields search hits
    // on a column whose values are all identical, where quantile edges collapse onto one number.
    let profit_at = vec![0.0, 0.0, 0.0, 50.0, 50.0];
    let count_at = vec![0usize, 0, 0, 10, 10];
    assert_eq!(
        best_pair(&profit_at, &count_at, 1, 10, f64::NEG_INFINITY),
        None
    );
    // With one trade parked outside the edges, the same slice no longer covers everything and
    // becomes a legitimate filter.
    assert_eq!(
        best_pair(&profit_at, &count_at, 1, 11, f64::NEG_INFINITY),
        Some((0, 3))
    );
}
