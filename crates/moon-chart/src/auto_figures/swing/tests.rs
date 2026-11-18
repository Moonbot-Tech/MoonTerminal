use super::super::test_bars::{M5_MS, ace, bar, from_hl, range};
use super::super::within;
use super::*;

fn params(min_touches: u8, broken: BrokenPolicy) -> DetectorParams {
    DetectorParams::Swing(SwingParams {
        lookback_bars: 10_000,
        pivot_left: 1,
        pivot_right: 1,
        tolerance_pct: 0.5,
        min_touches,
        broken,
    })
}

fn run(bars: &[ChartCandle], p: &DetectorParams) -> Vec<Found> {
    Swing.detect(
        bars,
        p,
        &DetectCtx {
            tf_ms: M5_MS,
            now_ms: 0.0,
        },
    )
}

/// Three highs at 99.8, 100, 100.2 under a 0.5 % tolerance — every bar between them lower.
fn three_tops() -> Vec<ChartCandle> {
    from_hl(&[
        (95.0, 90.0),
        (99.8, 96.0),
        (95.0, 90.0),
        (96.0, 91.0),
        (100.0, 96.0),
        (96.0, 91.0),
        (97.0, 92.0),
        (100.2, 96.0),
        (97.0, 92.0),
    ])
}

/// Three highs within the tolerance are one level at their mean with three touches.
///
/// Breakage: clustering by a running anchor that chains past the tolerance, a level at a single
/// pivot's price instead of the mean, or counting the 90–92 lows into the same group.
#[test]
fn three_close_highs_make_one_level() {
    let found = run(&three_tops(), &params(3, BrokenPolicy::default()));
    assert_eq!(found.len(), 1, "{found:?}");
    let FoundShape::Horizontal { price } = found[0].shape else {
        panic!("a swing level is horizontal");
    };
    assert!((price - 100.0).abs() < 1e-4, "level {price}");
    assert_eq!(found[0].touches.len(), 3);
    assert!(found[0].broken.is_none());
    assert_eq!(found[0].first_ms, found[0].touches[0].t_ms);
}

/// A tolerance too tight for the spread splits the tops, and `min_touches` then drops them.
///
/// Breakage: ignoring `tolerance_pct` or `min_touches`.
#[test]
fn tolerance_and_min_touches_are_honoured() {
    let mut p = params(3, BrokenPolicy::default());
    if let DetectorParams::Swing(s) = &mut p {
        s.tolerance_pct = 0.05;
    }
    assert!(run(&three_tops(), &p).is_empty());
    // A NaN tolerance reads as zero, not as "everything matches".
    if let DetectorParams::Swing(s) = &mut p {
        s.tolerance_pct = f32::NAN;
    }
    assert!(run(&three_tops(), &p).is_empty());
}

/// A close above the resistance after its last touch breaks it: `Remove` drops it, `Keep{3}` keeps
/// it flagged for three bars and drops it on the fourth, unless price has come back below.
///
/// Breakage: testing the high instead of the close, counting bars from the wrong end, or applying
/// the policy the wrong way round.
#[test]
fn broken_policy_remove_vs_keep() {
    let mut bars = three_tops();
    let n = bars.len();
    bars.push(bar(n, 99.0, 103.0, 99.0, 102.0)); // closes above 100: the break
    let found = run(&bars, &params(3, BrokenPolicy::Remove));
    assert!(found.is_empty(), "Remove drops a broken level: {found:?}");

    let keep = params(3, BrokenPolicy::Keep { bars: 3 });
    let flagged = run(&bars, &keep);
    assert_eq!(flagged.len(), 1);
    let broken = flagged[0].broken.expect("flagged broken");
    assert_eq!(broken.bars_since, 0);
    assert_eq!(broken.at_ms, bars[n].t_open_ms);
    assert!(!broken.returned);

    for k in 1..=3 {
        bars.push(bar(n + k, 102.0, 104.0, 101.0, 103.0));
    }
    let three_after = run(&bars, &keep);
    assert_eq!(three_after.len(), 1, "kept for exactly three more bars");
    assert_eq!(three_after[0].broken.map(|b| b.bars_since), Some(3));

    let mut four_after = bars.clone();
    four_after.push(bar(n + 4, 103.0, 104.0, 101.0, 103.0));
    assert!(
        run(&four_after, &keep).is_empty(),
        "dropped on the fourth bar"
    );

    let mut returned = bars;
    returned.push(bar(n + 4, 101.0, 101.0, 97.0, 98.0));
    let back = run(&returned, &keep);
    assert_eq!(back.len(), 1, "price came back below: the level stands");
    assert!(back[0].broken.is_some_and(|b| b.returned));
}

/// Short and empty input, a zero lookback, and zero-width pivots never panic.
///
/// Breakage: slicing the lookback window past the input, or indexing an empty group.
#[test]
fn edge_inputs_are_safe() {
    let p = params(1, BrokenPolicy::default());
    assert!(run(&[], &p).is_empty());
    assert!(run(&from_hl(&[(1.0, 0.5)]), &p).is_empty());
    let mut zero = p;
    if let DetectorParams::Swing(s) = &mut zero {
        s.lookback_bars = 0;
    }
    assert!(run(&three_tops(), &zero).is_empty());
    let mut flat = p;
    if let DetectorParams::Swing(s) = &mut flat {
        s.pivot_left = 0;
        s.pivot_right = 0;
        s.min_touches = 0;
    }
    assert!(!run(&three_tops(), &flat).is_empty());
}

/// On the real `chart-ace` candles: every level lies inside the series' range, every touch is
/// within the tolerance of its level, and touches are in time order inside `[first_ms, last_ms]`.
///
/// Breakage: a level drifting outside its touches (a wrong mean), a touch joined to a level past
/// the tolerance (anchor chaining), or the lookback window mis-sliced.
#[test]
fn real_candles_keep_the_structural_invariants() {
    for kind in [1, 5] {
        let bars = ace(kind);
        let sp = SwingParams {
            lookback_bars: 2_000,
            ..SwingParams::default()
        };
        let found = Swing.detect(
            &bars,
            &DetectorParams::Swing(sp),
            &DetectCtx {
                tf_ms: i64::from(kind) * 60_000,
                now_ms: 0.0,
            },
        );
        assert!(
            !found.is_empty(),
            "kind {kind}: 2000 real bars hold a level"
        );
        let (lo, hi) = range(&bars[bars.len() - 2_000..]);
        let frac = f64::from(sp.tolerance_pct) / 100.0;
        for f in &found {
            let FoundShape::Horizontal { price } = f.shape else {
                panic!("horizontal");
            };
            assert!(
                price >= lo && price <= hi,
                "level {price} outside [{lo}, {hi}]"
            );
            assert!(f.touches.len() >= usize::from(sp.min_touches));
            for t in &f.touches {
                assert!(
                    within(t.price, price, frac + 1e-12),
                    "touch {} vs level {price}",
                    t.price
                );
                assert!(t.t_ms >= f.first_ms && t.t_ms <= f.last_ms);
            }
            assert!(f.touches.windows(2).all(|w| w[0].t_ms < w[1].t_ms));
        }
    }
}

/// A level that broke, came back and broke again reports the SECOND break: `Keep{3}` counts its
/// bars from there, so the level is still kept two bars after the new break even though the first
/// one was six bars ago.
///
/// Breakage: reporting the first-ever break, which would count six bars and drop the level, and
/// would call the fresh break "returned".
#[test]
fn a_second_break_restarts_the_keep_countdown() {
    let mut bars = three_tops();
    let n = bars.len();
    bars.push(bar(n, 99.0, 103.0, 99.0, 102.0)); // first break
    bars.push(bar(n + 1, 101.0, 101.0, 97.0, 98.0)); // back under 100
    bars.push(bar(n + 2, 98.0, 99.0, 96.0, 97.5));
    bars.push(bar(n + 3, 97.5, 99.0, 96.5, 98.0));
    bars.push(bar(n + 4, 99.0, 103.5, 99.0, 102.5)); // second break
    bars.push(bar(n + 5, 102.5, 104.0, 101.5, 103.0));
    bars.push(bar(n + 6, 103.0, 104.5, 102.0, 103.5));
    let found = run(&bars, &params(3, BrokenPolicy::Keep { bars: 3 }));
    assert_eq!(found.len(), 1, "{found:?}");
    let broken = found[0].broken.expect("flagged broken");
    assert_eq!(broken.at_ms, bars[n + 4].t_open_ms);
    assert_eq!(broken.bars_since, 2);
    assert!(!broken.returned);
}
