//! Randomized equality and work-bound tests for the auto-Y trade price index.

use moonproto::MoonTime;

use super::*;

/// SplitMix64: a seeded generator so every fixture is reproducible from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn row(ms: i64, price: f32) -> TradeHistoryRow {
    TradeHistoryRow {
        time: MoonTime::from_unix_millis(ms),
        price,
        qty: 1.0,
    }
}

/// Time-ordered rows (ties allowed) with random prices.
fn fixture(rng: &mut Rng, n: usize) -> Vec<TradeHistoryRow> {
    let mut t = 1_000_000 + rng.below(1_000) as i64;
    (0..n)
        .map(|_| {
            t += rng.below(4) as i64;
            row(t, 1.0 + rng.below(1_000_000) as f32 / 1_000.0)
        })
        .collect()
}

/// The oracle: a plain scan of every row ever fed, filtered by the half-open window.
fn linear(rows: &[TradeHistoryRow], from: i64, to: i64) -> (Option<(f32, f32)>, usize) {
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    let mut n = 0;
    for r in rows {
        let t = r.unix_millis();
        if t >= from && t < to {
            lo = lo.min(r.price);
            hi = hi.max(r.price);
            n += 1;
        }
    }
    ((n > 0).then_some((lo, hi)), n)
}

/// Feed `rows` the way the cursor does: one full copy, then drained deltas, then window trims.
/// Returns the covered bound the trims left.
fn feed(rng: &mut Rng, index: &mut PriceFitIndex, rows: &[TradeHistoryRow]) -> i64 {
    let first = rows[0].unix_millis();
    let copy = 1 + rng.below(rows.len() as u64) as usize;
    let mut covered = first - rng.below(50) as i64;
    index.replace_from(&rows[..copy], true, covered);
    let mut at = copy;
    while at < rows.len() {
        let step = 1 + rng.below(700) as usize;
        let end = (at + step).min(rows.len());
        index.append(&rows[at..end]);
        at = end;
        if rng.below(3) == 0 {
            let live = &rows[..at];
            let t = live[rng.below(live.len() as u64) as usize].unix_millis();
            index.trim_before(t);
            covered = covered.max(t);
        }
    }
    covered
}

/// `market/source/price_fit.rs:PriceFitIndex::range` (fed by `history.rs`) dropping the
/// `covered_from` watermark check, or trimming rows the watermark still claims, makes auto-Y fit
/// only a subset of the visible trades after a right pan then a left pan inside the pan budget,
/// so drawn trade crosses are clipped. Every `Some` answer must equal a linear scan of all rows
/// fed, and a window reaching below the trimmed bound must answer `None`.
#[test]
fn range_equals_a_linear_scan_and_refuses_below_the_covered_bound() {
    let mut worst_visited = 0u64;
    for seed in 0..200u64 {
        let mut rng = Rng(seed);
        let n = if seed % 50 == 0 {
            98_000
        } else {
            1_000 + rng.below(5_000) as usize
        };
        let rows = fixture(&mut rng, n);
        let mut index = PriceFitIndex::default();
        let covered = feed(&mut rng, &mut index, &rows);
        let t_lo = rows[0].unix_millis() - 10;
        let t_hi = rows[n - 1].unix_millis() + 10;
        let blocks = n.div_ceil(BLOCK) as u64;
        for q in 0..60 {
            let mut a = t_lo + rng.below((t_hi - t_lo) as u64) as i64;
            let mut b = t_lo + rng.below((t_hi - t_lo) as u64) as i64;
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            if q % 4 != 0 {
                a = a.max(covered);
                b = b.max(a);
            }
            take_price_fit_visited();
            let got = index.range(a, b);
            let visited = take_price_fit_visited();
            if a < covered {
                assert_eq!(
                    got, None,
                    "seed {seed}: window [{a},{b}) starts below {covered}"
                );
                continue;
            }
            let got =
                got.unwrap_or_else(|| panic!("seed {seed}: complete index refused [{a},{b})"));
            assert_eq!(got, linear(&rows, a, b), "seed {seed}: window [{a},{b})");
            assert!(
                visited <= blocks + 2 * BLOCK as u64,
                "seed {seed}: visited {visited} > {blocks} blocks + two edge blocks"
            );
            if n == 98_000 && got.1 > 0 {
                worst_visited = worst_visited.max(visited);
                if q % 10 == 1 {
                    println!("[price_fit] before={} after={}", got.1, visited);
                }
            }
        }
    }
    assert!(worst_visited > 0);
}

/// `market/source/price_fit.rs:PriceFitIndex::range` dropping the `covered_from` check answers a
/// window left of a trim from the rows that survived it, so auto-Y shrinks after a left pan.
#[test]
fn reset_then_trim_right_then_query_left_refuses() {
    let mut rng = Rng(7);
    let rows = fixture(&mut rng, 3_000);
    let t0 = rows[0].unix_millis();
    let mid = rows[1_500].unix_millis();
    let end = rows[2_999].unix_millis() + 1;
    let mut index = PriceFitIndex::default();
    index.replace_from(&rows, true, t0);
    assert_eq!(index.range(t0, end), Some(linear(&rows, t0, end)));
    index.trim_before(mid);
    assert_eq!(index.range(t0, end), None);
    assert_eq!(index.range(mid - 1, end), None);
    assert_eq!(index.range(mid, end), Some(linear(&rows, mid, end)));
    index.clear();
    assert_eq!(index.range(mid, end), None);
}
