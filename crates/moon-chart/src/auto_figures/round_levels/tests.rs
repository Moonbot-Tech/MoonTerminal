use super::super::test_bars::{ace, bar};
use super::*;

/// (price, strength) of every level around a last close of `close`.
fn levels(close: f32, grid: RoundGrid, range_pct: f32) -> Vec<(f64, f32)> {
    let bars = [bar(0, close, close, close, close)];
    RoundLevels
        .detect(
            &bars,
            &DetectorParams::RoundLevels(RoundLevelsParams { grid, range_pct }),
            &DetectCtx {
                tf_ms: 300_000,
                now_ms: 0.0,
                price_tick: None,
            },
        )
        .into_iter()
        .map(|f| {
            let FoundShape::Horizontal { price } = f.shape else {
                panic!("a level");
            };
            (price, f.strength)
        })
        .collect()
}

fn prices(v: &[(f64, f32)]) -> Vec<f64> {
    v.iter().map(|l| l.0).collect()
}

/// The contract's own example, by hand: on 3.4 (magnitude 1) within 10 % (3.06..3.74) the `Half`
/// grid has 3.5 alone, `Major` nothing (3 and 4 are outside), `Minor` 3.1 … 3.7 with 3.5 the
/// roundest (a half step, 1.5) above the rest (1).
///
/// Breakage: the magnitude off by one, the grid step not halved or tenthed, or the range applied
/// to one side only.
#[test]
fn grids_on_three_point_four() {
    assert_eq!(levels(3.4, RoundGrid::Half, 10.0), vec![(3.5, 1.5)]);
    assert!(levels(3.4, RoundGrid::Major, 10.0).is_empty());
    let minor = levels(3.4, RoundGrid::Minor, 10.0);
    assert_eq!(prices(&minor), vec![3.1, 3.2, 3.3, 3.4, 3.5, 3.6, 3.7]);
    let strengths: Vec<f32> = minor.iter().map(|l| l.1).collect();
    assert_eq!(strengths, vec![1.0, 1.0, 1.0, 1.0, 1.5, 1.0, 1.0]);
}

/// Large and tiny prices land on exact round values, and the rounder of two multiples ranks
/// higher. By hand: 65 000 (magnitude 10 000) within 10 % has 60 000, 65 000, 70 000 on the `Half`
/// grid, the thousands-of-ten multiples scoring 2 against the half's 1.5. 0.000123 (magnitude
/// 0.0001) within 10 % has 0.00012 and 0.00013 on the `Minor` grid — the decimals themselves, not
/// 0.00012000000000000001. On 0.95 the `Minor` grid runs 0.86 … 1.04, and 1.00 outranks 0.90
/// outranks 0.95 outranks 0.91.
///
/// Breakage: a step accumulated by repeated addition (the last digits drift), roundness counted
/// on the grid index instead of the price, or `Strongest` unable to tell 1.00 from 0.90.
#[test]
fn big_and_tiny_prices_and_roundness() {
    assert_eq!(
        levels(65_000.0, RoundGrid::Half, 10.0),
        vec![(60_000.0, 2.0), (65_000.0, 1.5), (70_000.0, 2.0)]
    );
    assert_eq!(
        prices(&levels(0.000_123, RoundGrid::Minor, 10.0)),
        vec![0.000_12, 0.000_13]
    );
    let around_one = levels(0.95, RoundGrid::Minor, 10.0);
    assert_eq!(around_one.len(), 19, "0.86 … 1.04");
    let strength_at = |p: f64| {
        around_one
            .iter()
            .find(|l| l.0 == p)
            .map(|l| l.1)
            .expect("on the grid")
    };
    assert!(strength_at(1.0) > strength_at(0.9));
    assert!(strength_at(0.9) > strength_at(0.95));
    assert!(strength_at(0.95) > strength_at(0.91));
}

/// An exact power of ten is its own magnitude: 1 000 within 10 % on the `Major` grid is 1 000
/// alone. A magnitude read one low (`log10` landing at 2.999…) would give 900, 1 000 and 1 100.
///
/// Breakage: the exponent trusted from `log10` without checking it against the powers around it.
#[test]
fn an_exact_power_of_ten() {
    assert_eq!(
        prices(&levels(1_000.0, RoundGrid::Major, 10.0)),
        vec![1_000.0]
    );
    assert_eq!(prices(&levels(100.0, RoundGrid::Major, 10.0)), vec![100.0]);
    assert_eq!(prices(&levels(1.0, RoundGrid::Major, 10.0)), vec![1.0]);
}

/// A range that skipped `sanitize` cannot run away: at most `MAX_LEVELS` levels, every one a
/// positive price; no valid bar, no level.
///
/// Breakage: a loop over every multiple a huge range reaches, or a non-positive "round" price.
#[test]
fn bounded_and_empty() {
    let many = levels(3.4, RoundGrid::Minor, 1.0e9);
    assert!(!many.is_empty() && many.len() <= MAX_LEVELS as usize + 1);
    assert!(many.iter().all(|l| l.0 > 0.0 && l.0.is_finite()));
    let nan = [bar(0, f32::NAN, f32::NAN, f32::NAN, f32::NAN)];
    let found = RoundLevels.detect(
        &nan,
        &DetectorParams::RoundLevels(RoundLevelsParams::default()),
        &DetectCtx {
            tf_ms: 300_000,
            now_ms: 0.0,
            price_tick: None,
        },
    );
    assert!(found.is_empty());
}

/// On the real `chart-ace` close every `Minor` level within 10 % is a multiple of a tenth of the
/// close's magnitude — 0.01, the bench being a 0.1-priced coin — checked in integer
/// hundred-thousandths, and lies within 10 % of the close.
///
/// Breakage: the grid read from the wrong bar, or levels outside the range.
#[test]
fn bench_close_grid() {
    let bars = ace(5);
    let close = f64::from(bars[bars.len() - 1].close);
    assert!(
        (0.1..1.0).contains(&close),
        "the bench trades in tenths: {close}"
    );
    let found = RoundLevels.detect(
        &bars,
        &DetectorParams::RoundLevels(RoundLevelsParams {
            grid: RoundGrid::Minor,
            range_pct: 10.0,
        }),
        &DetectCtx {
            tf_ms: 300_000,
            now_ms: 0.0,
            price_tick: None,
        },
    );
    assert!(!found.is_empty());
    for f in &found {
        let p = f.shape.price_at(0.0).expect("price");
        // A tenth of 0.1 is 0.01 = 1 000 hundred-thousandths.
        let units = (p * 100_000.0).round() as i64;
        assert_eq!(units % 1_000, 0, "{p}");
        assert!((p - close).abs() <= close * 0.1 + 1e-12, "{p} vs {close}");
    }
}
