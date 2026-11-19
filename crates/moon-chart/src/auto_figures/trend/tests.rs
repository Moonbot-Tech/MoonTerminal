use super::super::test_bars::{M5_MS, ace, bar, from_hl};
use super::super::within;
use super::*;
use moon_core::config::auto_figures::{BrokenPolicy, TrendParams};

/// Lows at 90, 92, 94 on bars 2, 6, 10 — a support rising 0.5 per bar, 0.556 % of the 90 base —
/// with every other low above it and only two pivot highs.
fn rising_lows() -> Vec<ChartCandle> {
    from_hl(&[
        (97.0, 93.0),
        (96.0, 92.0),
        (95.0, 90.0),
        (97.0, 92.0),
        (98.0, 93.0),
        (97.0, 93.0),
        (96.0, 92.0),
        (98.0, 94.0),
        (100.0, 95.0),
        (99.0, 95.0),
        (98.0, 94.0),
        (99.0, 96.0),
        (101.0, 97.0),
    ])
}

fn params() -> TrendParams {
    TrendParams {
        lookback_bars: 1_000,
        pivot_left: 1,
        pivot_right: 1,
        side: TrendSide::Both,
        tolerance_pct: 0.15,
        min_touches: 3,
        max_body_breaks: 0,
        min_slope_pct_per_bar: 0.005,
        max_slope_pct_per_bar: 2.0,
        broken: BrokenPolicy::Keep { bars: 3 },
        ..TrendParams::default()
    }
}

fn run(bars: &[ChartCandle], p: TrendParams) -> Vec<Found> {
    Trend.detect(
        bars,
        &DetectorParams::Trend(p),
        &DetectCtx {
            tf_ms: M5_MS,
            now_ms: 0.0,
        },
    )
}

/// Three collinear lows are one support line through its first and last touch.
///
/// Breakage: a line built in bar index instead of time, endpoints at the wrong pivots, or the pair
/// counted twice as two lines.
#[test]
fn three_rising_lows_make_one_support() {
    let bars = rising_lows();
    let found = run(&bars, params());
    assert_eq!(found.len(), 1, "{found:?}");
    let FoundShape::Sloped {
        t0_ms,
        p0,
        t1_ms,
        p1,
    } = found[0].shape
    else {
        panic!("a trend line is sloped");
    };
    assert_eq!((t0_ms, p0), (bars[2].t_open_ms, 90.0));
    assert_eq!((t1_ms, p1), (bars[10].t_open_ms, 94.0));
    assert_eq!(found[0].touches.len(), 3);
    assert_eq!(found[0].shape.price_at(bars[6].t_open_ms), Some(92.0));
}

/// The slope bounds are by absolute value, in % of the line's first-touch price per bar.
///
/// Breakage: a slope measured per millisecond, against the wrong base, or a NaN bound that lets
/// everything through.
#[test]
fn slope_bounds_apply() {
    let bars = rising_lows();
    let mut p = params();
    p.max_slope_pct_per_bar = 0.55;
    assert!(run(&bars, p).is_empty(), "0.556 %/bar is over 0.55");
    p.max_slope_pct_per_bar = 0.56;
    assert_eq!(run(&bars, p).len(), 1);
    p.min_slope_pct_per_bar = f32::NAN;
    assert!(run(&bars, p).is_empty());
}

/// A body crossing the line between touches rejects it unless a break is allowed; a wick does not.
///
/// Breakage: counting wicks, or ignoring `max_body_breaks`.
#[test]
fn bodies_break_a_line_and_wicks_do_not() {
    let mut body = rising_lows();
    body[4] = bar(4, 97.0, 98.0, 90.0, 90.0); // closes at 90 under the 91 line
    assert!(run(&body, params()).is_empty());
    let mut one = params();
    one.max_body_breaks = 1;
    assert_eq!(run(&body, one).len(), 1);

    let mut wick = rising_lows();
    wick[8] = bar(8, 95.0, 100.0, 92.0, 96.0); // wick to 92 under the 93 line, body above
    assert_eq!(run(&wick, params()).len(), 1);
}

/// No usable timeframe or too little input: nothing, never a panic.
///
/// Breakage: dividing by a zero bar length, or indexing pivots that do not exist.
#[test]
fn degenerate_inputs_are_safe() {
    let bars = rising_lows();
    assert!(
        Trend
            .detect(
                &bars,
                &DetectorParams::Trend(params()),
                &DetectCtx {
                    tf_ms: 0,
                    now_ms: 0.0
                }
            )
            .is_empty()
    );
    assert!(run(&bars[..3], params()).is_empty());
    let mut zero = params();
    zero.min_touches = 0;
    zero.pivot_left = 0;
    zero.pivot_right = 0;
    let _ = run(&bars, zero);
}

/// On the real `chart-ace` candles: every touch lies within the tolerance of the line at its own
/// time, every line has enough touches and a slope inside the bounds, and its endpoints are its
/// first and last touch.
///
/// Breakage: touches tested against the line at the wrong time, a slope bound applied to a signed
/// slope, or endpoints that drift off the touches.
#[test]
fn real_candles_keep_the_structural_invariants() {
    let mut lines = 0;
    for kind in [1u32, 5] {
        let bars = ace(kind);
        let tf_ms = i64::from(kind) * 60_000;
        let p = TrendParams {
            lookback_bars: 1_000,
            ..TrendParams::default()
        };
        let found = Trend.detect(
            &bars,
            &DetectorParams::Trend(p),
            &DetectCtx { tf_ms, now_ms: 0.0 },
        );
        let frac = f64::from(p.tolerance_pct) / 100.0;
        for f in &found {
            let FoundShape::Sloped {
                t0_ms,
                p0,
                t1_ms,
                p1,
            } = f.shape
            else {
                panic!("sloped");
            };
            assert!(t0_ms < t1_ms);
            assert!(f.touches.len() >= usize::from(p.min_touches));
            assert_eq!(f.touches.first().map(|t| t.t_ms), Some(t0_ms));
            assert_eq!(f.touches.last().map(|t| t.t_ms), Some(t1_ms));
            for t in &f.touches {
                let at = f.shape.price_at(t.t_ms).expect("a positive line price");
                assert!(within(t.price, at, frac + 1e-12), "{} vs {at}", t.price);
            }
            let bars_between = (t1_ms - t0_ms) / tf_ms as f64;
            let slope_pct = ((p1 - p0) / bars_between / p0 * 100.0).abs();
            assert!(
                slope_pct >= f64::from(p.min_slope_pct_per_bar) * (1.0 - 1e-9)
                    && slope_pct <= f64::from(p.max_slope_pct_per_bar) * (1.0 + 1e-9),
                "slope {slope_pct} %/bar"
            );
        }
        lines += found.len();
    }
    assert!(
        lines > 0,
        "1000 real bars of two timeframes hold a trend line"
    );
}

/// A close under the support after its last touch breaks it, and the row's policy decides:
/// `Remove` drops the line, `Keep{3}` keeps it flagged.
///
/// Breakage: the trend policy ignored (a broken line drawn forever, or dropped under `Keep`).
#[test]
fn a_broken_line_follows_the_policy() {
    let mut bars = rising_lows();
    bars.push(bar(13, 96.0, 96.5, 93.0, 93.5)); // closes at 93.5, under the 95.5 the line reaches
    let mut remove = params();
    remove.broken = BrokenPolicy::Remove;
    assert!(run(&bars, remove).is_empty());
    let kept = run(&bars, params());
    assert_eq!(kept.len(), 1, "{kept:?}");
    let broken = kept[0].broken.expect("flagged broken");
    assert_eq!((broken.at_ms, broken.bars_since), (bars[13].t_open_ms, 0));
}

/// The pair search reads only the most recent `MAX_LINE_PIVOTS` pivots of a kind, whatever the
/// lookback: 5 000 bars of a perfect zig-zag whose 2 499 peaks all lie on one line give a
/// resistance through exactly the newest 200 of them.
///
/// Breakage: the cap removed (this input then costs ~8·10⁹ touch checks and the test does not
/// finish), or applied to the oldest pivots instead of the newest.
#[test]
fn the_line_search_is_capped_to_the_newest_pivots() {
    // Even bars peak at base + 1, odd bars dip to base − 0.5, base rising 0.002 per bar: every even
    // bar from 2 on is a pivot high, every odd bar from 1 on a pivot low, all collinear.
    let base = |i: usize| 100.0 + 0.002 * i as f32;
    let bars: Vec<ChartCandle> = (0..5_000)
        .map(|i| {
            let (high, low) = if i % 2 == 0 {
                (base(i) + 1.0, base(i))
            } else {
                (base(i) + 0.5, base(i) - 0.5)
            };
            bar(i, low, high, low, (high + low) / 2.0)
        })
        .collect();
    let p = TrendParams {
        lookback_bars: 5_000,
        side: TrendSide::Resistance,
        min_slope_pct_per_bar: 0.0,
        ..params()
    };
    let found = run(&bars, p);
    assert_eq!(found.len(), 1, "one line through every capped peak");
    assert_eq!(found[0].touches.len(), MAX_LINE_PIVOTS);
    // The newest peak is bar 4 998 (bar 4 999 has no right neighbour); 200 peaks back, two bars
    // apart, starts at bar 4 600.
    assert_eq!(found[0].first_ms, bars[4_600].t_open_ms);
    assert_eq!(found[0].last_ms, bars[4_998].t_open_ms);
}
