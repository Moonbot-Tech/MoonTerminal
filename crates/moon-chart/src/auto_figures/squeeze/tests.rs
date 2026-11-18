use super::super::Touch;
use super::super::test_bars::{M5_MS, ace, bar, from_hl, range};
use super::super::within;
use super::*;
use moon_core::config::auto_figures::{BrokenPolicy, SwingParams};

/// A ceiling at 100 touched on bars 4, 8, 12 and lows rising 90, 92, 94 on bars 2, 6, 10 — an
/// ascending triangle. The lows line projects to 96 on the last bar (14): 4 % under the level.
const ASCENDING: [(f32, f32); 15] = [
    (97.0, 93.0),
    (96.0, 92.0),
    (95.0, 90.0),
    (97.0, 92.0),
    (100.0, 95.0),
    (97.0, 93.0),
    (96.0, 92.0),
    (98.0, 94.0),
    (100.0, 96.0),
    (99.0, 95.0),
    (98.0, 94.0),
    (99.0, 96.0),
    (100.0, 97.0),
    (99.0, 97.5),
    (99.5, 98.0),
];

fn params(pressed_pct: f32) -> SqueezeParams {
    SqueezeParams {
        level: SwingParams {
            lookback_bars: 1_000,
            pivot_left: 1,
            pivot_right: 1,
            tolerance_pct: 0.5,
            min_touches: 3,
            ..SwingParams::default()
        },
        min_trend_touches: 3,
        trend_tolerance_pct: 0.2,
        pressed_pct,
    }
}

fn run(bars: &[ChartCandle], p: SqueezeParams) -> Vec<Found> {
    Squeeze.detect(
        bars,
        &DetectorParams::Squeeze(p),
        &DetectCtx {
            tf_ms: M5_MS,
            now_ms: 0.0,
        },
    )
}

/// The mirror image around 100: a floor at 100 and highs falling 110, 108, 106.
fn descending() -> Vec<ChartCandle> {
    let hl: Vec<(f32, f32)> = ASCENDING
        .iter()
        .map(|&(h, l)| (200.0 - l, 200.0 - h))
        .collect();
    from_hl(&hl)
}

/// Rising lows under a resistance are one squeeze: the level, the lows line through its first and
/// last touch, and all six touches; `pressed` flips exactly where the 4 % projected gap crosses
/// `pressed_pct`.
///
/// Breakage: pairing the level with a line from its own side, measuring the gap at the line's
/// last touch instead of the last bar, or measuring it against the line's price.
#[test]
fn rising_lows_under_a_ceiling() {
    let bars = from_hl(&ASCENDING);
    let found = run(&bars, params(3.9));
    assert_eq!(found.len(), 1, "{found:?}");
    let FoundShape::Squeeze {
        level,
        t0_ms,
        p0,
        t1_ms,
        p1,
        pressed,
    } = found[0].shape
    else {
        panic!("a squeeze shape");
    };
    assert!((level - 100.0).abs() < 1e-9);
    assert_eq!((t0_ms, p0), (bars[2].t_open_ms, 90.0));
    assert_eq!((t1_ms, p1), (bars[10].t_open_ms, 94.0));
    assert!(!pressed, "96 is 4 % under 100, outside 3.9 %");
    assert_eq!(found[0].touches.len(), 6);
    assert!(found[0].touches.windows(2).all(|w| w[0].t_ms <= w[1].t_ms));

    let found = run(&bars, params(4.1));
    assert!(matches!(
        found[0].shape,
        FoundShape::Squeeze { pressed: true, .. }
    ));
}

/// Falling highs over a support are the mirror formation.
///
/// Breakage: handling only the resistance case, or requiring the line to rise in both.
#[test]
fn falling_highs_over_a_floor() {
    let bars = descending();
    let found = run(&bars, params(0.5));
    assert_eq!(found.len(), 1, "{found:?}");
    let FoundShape::Squeeze { level, p0, p1, .. } = found[0].shape else {
        panic!("a squeeze shape");
    };
    assert!((level - 100.0).abs() < 1e-9);
    assert_eq!((p0, p1), (110.0, 106.0));
}

/// A close through the level is the squeeze's breakout, and the LEVEL's policy decides: `Remove`
/// drops the formation, `Keep{3}` keeps it flagged with the level's break.
///
/// Breakage: filtering broken levels out regardless of the policy (no breakout is ever visible on
/// a squeeze row), or keeping a broken formation unflagged.
#[test]
fn a_broken_level_follows_the_level_policy() {
    let mut bars = from_hl(&ASCENDING);
    bars.push(bar(15, 99.0, 102.0, 99.0, 101.0));
    let mut remove = params(5.0);
    remove.level.broken = BrokenPolicy::Remove;
    assert!(run(&bars, remove).is_empty());

    let mut keep = params(5.0);
    keep.level.broken = BrokenPolicy::Keep { bars: 3 };
    let found = run(&bars, keep);
    assert_eq!(found.len(), 1, "{found:?}");
    let broken = found[0].broken.expect("the level's break is carried");
    assert_eq!(broken.at_ms, bars[15].t_open_ms);
    assert_eq!(broken.bars_since, 0);
}

/// A pivot that is both a level touch and a line touch is counted once.
///
/// Breakage: concatenating the two touch lists, which counts that pivot twice and inflates the
/// formation's touches and strength.
#[test]
fn a_shared_pivot_is_one_touch() {
    let t = |t_ms: f64, price: f64| Touch { t_ms, price };
    let level = [t(1.0, 100.0), t(3.0, 99.7), t(5.0, 100.0)];
    let line = [t(2.0, 98.0), t(3.0, 99.7), t(4.0, 99.0)];
    let merged = merge_touches(&level, &line);
    assert_eq!(
        merged,
        vec![
            t(1.0, 100.0),
            t(2.0, 98.0),
            t(3.0, 99.7),
            t(4.0, 99.0),
            t(5.0, 100.0)
        ]
    );
    // Two different pivots of one bar (its high and its low) are two touches.
    let outside = merge_touches(&[t(1.0, 100.0)], &[t(1.0, 95.0)]);
    assert_eq!(outside.len(), 2);
}

/// Edge inputs: nothing, too few bars, no usable timeframe.
///
/// Breakage: indexing the last bar of an empty window, or a zero bar length reaching a division.
#[test]
fn degenerate_inputs_are_safe() {
    assert!(run(&[], params(0.5)).is_empty());
    assert!(run(&from_hl(&ASCENDING[..3]), params(0.5)).is_empty());
    let bars = from_hl(&ASCENDING);
    assert!(
        Squeeze
            .detect(
                &bars,
                &DetectorParams::Squeeze(params(0.5)),
                &DetectCtx {
                    tf_ms: 0,
                    now_ms: 0.0
                },
            )
            .is_empty()
    );
}

/// On the real `chart-ace` candles: the level lies in the series' range, every touch is within its
/// tolerance of the level or of the sloped line at its time, the line converges toward the level
/// from the level's other side, and `pressed` agrees with an independent projection.
///
/// Breakage: a sloped half from the level's own side, a diverging line, or `pressed` computed
/// against the wrong base.
#[test]
fn real_candles_keep_the_structural_invariants() {
    let mut formations = 0;
    for kind in [1u32, 5] {
        let bars = ace(kind);
        let mut p = SqueezeParams::default();
        p.level.lookback_bars = 2_000;
        let found = run_tf(&bars, p, i64::from(kind) * 60_000);
        let win = &bars[bars.len() - 2_000..];
        let (lo, hi) = range(win);
        let last_t = win[win.len() - 1].t_open_ms;
        let level_frac = f64::from(p.level.tolerance_pct) / 100.0;
        let line_frac = f64::from(p.trend_tolerance_pct) / 100.0;
        for f in &found {
            let FoundShape::Squeeze {
                level,
                t0_ms,
                p0,
                t1_ms,
                p1,
                pressed,
            } = f.shape
            else {
                panic!("squeeze");
            };
            assert!(level >= lo && level <= hi);
            let line_at = |t: f64| super::super::line_price_at(t0_ms, p0, t1_ms, p1, t);
            for t in &f.touches {
                let on_level = within(t.price, level, level_frac + 1e-12);
                let on_line =
                    line_at(t.t_ms).is_some_and(|l| within(t.price, l, line_frac + 1e-12));
                assert!(on_level || on_line, "touch {t:?} is on neither half");
            }
            let resistance = p0 < level;
            if resistance {
                assert!(p1 > p0, "lows must rise into a resistance");
            } else {
                assert!(p1 < p0, "highs must fall into a support");
            }
            let projected = line_at(last_t).expect("projection");
            let gap = if resistance {
                (level - projected) / level
            } else {
                (projected - level) / level
            };
            assert_eq!(pressed, gap <= f64::from(p.pressed_pct) / 100.0);
        }
        formations += found.len();
    }
    assert!(
        formations > 0,
        "2000 real bars of two timeframes hold a squeeze"
    );
}

fn run_tf(bars: &[ChartCandle], p: SqueezeParams, tf_ms: i64) -> Vec<Found> {
    Squeeze.detect(
        bars,
        &DetectorParams::Squeeze(p),
        &DetectCtx { tf_ms, now_ms: 0.0 },
    )
}
