use moon_core::config::auto_figures::BrokenPolicy;

use super::super::test_bars::{M5_MS, MONDAY_2024_01_01_MS, ace, bar};
use super::*;

/// Half the range of every synthetic bar, as a fraction of its price: a bar at `p` spans
/// `p·(1 − H) .. p·(1 + H)`, so a vertex at `v` makes a pivot high of `v·(1 + H)` (or a pivot low of
/// `v·(1 − H)`), and a line through vertices scales by the same factor — still exactly linear.
const H: f64 = 0.0005;

/// Bars along the piecewise-linear path through `vertices` (`(bar index, price)`, ascending, the
/// first at index 0), one bar per index up to the last vertex. Each bar opens and closes on the
/// path and spans ±[`H`] of it, so the turning points are exactly the vertices and every close
/// lies on the path.
fn path(vertices: &[(usize, f64)]) -> Vec<ChartCandle> {
    let last = vertices.last().map_or(0, |v| v.0);
    (0..=last)
        .map(|i| {
            let k = vertices.iter().rposition(|v| v.0 <= i).unwrap_or(0);
            let (i0, p0) = vertices[k];
            let p = match vertices.get(k + 1) {
                Some(&(i1, p1)) => p0 + (p1 - p0) * (i - i0) as f64 / (i1 - i0) as f64,
                None => p0,
            };
            let (hi, lo) = (p * (1.0 + H), p * (1.0 - H));
            bar(i, p as f32, hi as f32, lo as f32, p as f32)
        })
        .collect()
}

/// Open time of bar `i` of a [`path`].
fn t(i: usize) -> f64 {
    MONDAY_2024_01_01_MS + (i as i64 * M5_MS) as f64
}

fn ctx() -> DetectCtx {
    DetectCtx {
        tf_ms: M5_MS,
        now_ms: 0.0,
    }
}

fn run(bars: &[ChartCandle], p: FormationParams) -> Vec<Found> {
    Formation.detect(bars, &DetectorParams::Formation(p), &ctx())
}

fn tags(found: &[Found]) -> Vec<Option<LineTag>> {
    found.iter().map(|f| f.tag).collect()
}

/// The two sides of a pair.
fn sides(f: &Found) -> (Seg, Seg) {
    let FoundShape::Pair { upper, lower } = f.shape else {
        panic!("not a pair: {f:?}");
    };
    (upper, lower)
}

/// `a` equals `b` to the f32 precision the candles carry.
fn close_to(a: f64, b: f64) -> bool {
    (a - b).abs() <= b.abs() * 1e-6
}

/// Only two-line formations, every kind on.
fn sloped_only() -> FormationParams {
    FormationParams {
        double: false,
        ..FormationParams::default()
    }
}

/// A symmetric triangle, by hand: tops 110 / 108 / 106 at bars 5 / 15 / 25 (−0.2 per bar), bottoms
/// 92 / 94 / 96 at 10 / 20 / 30 (+0.2 per bar), price back at 100 by bar 34. Width at the first
/// touch (bar 5): 110 − 91 = 19; at the last (bar 30): 105 − 96 = 9 — a ratio of 0.47, far below
/// the 20 % parallel band, so converging; the top falls 4.5 % and the bottom rises 5.5 % over the
/// span, both far past twice the 0.2 % tolerance and of opposite sign: a triangle. The sides meet
/// at bar 52.5, after the last bar. The rise into bar 5 (100 → 110) is a 10 % pole, but the
/// triangle is 19 wide at its start, more than half that pole: not a pennant.
///
/// Breakage: the wrong side kind (a wedge), a pole read without the retrace bound (a flag), a side
/// through the wrong pivots, or the default switches dropping a kind.
#[test]
fn a_symmetric_triangle() {
    let bars = path(&[
        (0, 100.0),
        (5, 110.0),
        (10, 92.0),
        (15, 108.0),
        (20, 94.0),
        (25, 106.0),
        (30, 96.0),
        (34, 100.0),
    ]);
    let found = run(&bars, FormationParams::default());
    assert_eq!(tags(&found), [Some(LineTag::Triangle)], "{found:?}");
    let f = &found[0];
    let (upper, lower) = sides(f);
    assert_eq!((upper.t0_ms, upper.t1_ms), (t(5), t(25)));
    assert_eq!((lower.t0_ms, lower.t1_ms), (t(10), t(30)));
    assert!(close_to(upper.p0, 110.0 * (1.0 + H)) && close_to(upper.p1, 106.0 * (1.0 + H)));
    assert!(close_to(lower.p0, 92.0 * (1.0 - H)) && close_to(lower.p1, 96.0 * (1.0 - H)));
    assert_eq!(f.touches.len(), 6);
    assert_eq!((f.first_ms, f.last_ms), (t(5), t(30)));
    assert_eq!(f.broken, None);
    // Six touches over 25 of the 35 bars searched.
    assert!((f.strength - (6.0 + 25.0 / 35.0) as f32).abs() < 1e-5);
    assert!((f.tolerance_frac - 0.002).abs() < 1e-9);

    // The kind switched off: nothing else claims the pair.
    let off = FormationParams {
        triangle: false,
        ..FormationParams::default()
    };
    assert!(run(&bars, off).is_empty());
}

/// An ascending triangle, by hand: a flat top at 110 (bars 5 / 15 / 25) over lows rising 100 / 103
/// / 106 (bars 10 / 20 / 30, +0.3 per bar). The top moves 0 % — flat — and the width falls from
/// 110 − 98.5 = 11.5 at bar 5 to 110 − 106 = 4 at bar 30: converging with one flat side, a
/// triangle and not a wedge.
///
/// The flat top is three equal highs: with doubles on it is still only the triangle — its first
/// and last high are 20 bars apart over a 9 % trough, but the middle one between them makes it a
/// triple top, which is no double.
///
/// Breakage: a flat side read as sloping (a wedge), a zero slope rejected by the line search, or
/// a double paired across a third touch of its level.
#[test]
fn an_ascending_triangle() {
    let bars = path(&[
        (0, 104.0),
        (5, 110.0),
        (10, 100.0),
        (15, 110.0),
        (20, 103.0),
        (25, 110.0),
        (30, 106.0),
        (34, 108.0),
    ]);
    let found = run(&bars, sloped_only());
    assert_eq!(tags(&found), [Some(LineTag::Triangle)], "{found:?}");
    let (upper, _) = sides(&found[0]);
    assert_eq!(upper.p0, upper.p1, "the top is flat");

    let with_doubles = run(&bars, FormationParams::default());
    assert_eq!(tags(&with_doubles), [Some(LineTag::Triangle)]);
}

/// A rising wedge, by hand: tops 105 / 109 / 113 at bars 5 / 15 / 25 (+0.4 per bar), bottoms 96 /
/// 102 / 108 at 10 / 20 / 30 (+0.6 per bar). Width 105 − 93 = 12 at bar 5, 115 − 108 = 7 at bar
/// 30: converging, both sides rising (9.5 % and 16 %): a wedge. The sides meet at bar 65.
///
/// Breakage: same-sign converging sides reported as a triangle.
#[test]
fn a_rising_wedge() {
    let bars = path(&[
        (0, 100.0),
        (5, 105.0),
        (10, 96.0),
        (15, 109.0),
        (20, 102.0),
        (25, 113.0),
        (30, 108.0),
        (34, 111.0),
    ]);
    let found = run(&bars, FormationParams::default());
    assert_eq!(tags(&found), [Some(LineTag::Wedge)], "{found:?}");
    let (upper, lower) = sides(&found[0]);
    assert!(upper.p1 > upper.p0 && lower.p1 > lower.p0);
}

/// A falling channel, by hand: tops 110 / 108 / 106 (bars 5 / 15 / 25) and bottoms 100 / 98 / 96
/// (bars 10 / 20 / 30), both −0.2 per bar. The width is 110 − 101 = 9 at bar 5 and 105 − 96 = 9 at
/// bar 30: a ratio of exactly 1, parallel. The 104 → 110 rise into bar 5 is a 5.8 % pole, but the
/// channel is 9 wide, more than half of it: not a flag.
///
/// Breakage: parallel sides read as converging, or the parallel band measured on the wrong width.
#[test]
fn a_falling_channel() {
    let bars = path(&[
        (0, 104.0),
        (5, 110.0),
        (10, 100.0),
        (15, 108.0),
        (20, 98.0),
        (25, 106.0),
        (30, 96.0),
        (34, 100.0),
    ]);
    let found = run(&bars, FormationParams::default());
    assert_eq!(tags(&found), [Some(LineTag::Channel)], "{found:?}");
    let (upper, lower) = sides(&found[0]);
    let slope = |s: Seg| (s.p1 - s.p0) / (s.t1_ms - s.t0_ms);
    // Equal slopes but for the ±H the pivots stand off the path by — to the f32 rounding of the
    // candle prices, which a slope (a difference of two prices) magnifies.
    let ratio = slope(upper) / slope(lower);
    assert!((ratio - (1.0 + H) / (1.0 - H)).abs() < 1e-3, "{ratio}");
}

/// A bull flag, by hand: a pole 100 → 112 over bars 0..8 (12 %), then a falling channel — tops 112
/// / 111 / 110 at bars 8 / 14 / 20, bottoms 108 / 107 / 106 at 11 / 17 / 23 (−1/6 per bar). Width
/// 112 − 108.5 = 3.5 at bar 8 and 109.5 − 106 = 3.5 at bar 23: parallel. Span 15 bars (the
/// `min_bars` floor, inside the 30-bar flag cap); the pole is 12 % ≥ 3 % and 12 high, and the flag
/// 3.5 wide ≤ half of it: a flag. With flags off the same pair is the channel it is; with channels
/// off and flags on it is still a flag.
///
/// Breakage: the pole looked for after the formation, the retrace bound inverted, or the flag
/// reading lost when its geometric kind is switched off.
#[test]
fn a_bull_flag_after_an_impulse() {
    let bars = path(&[
        (0, 100.0),
        (8, 112.0),
        (11, 108.0),
        (14, 111.0),
        (17, 107.0),
        (20, 110.0),
        (23, 106.0),
        (26, 107.5),
    ]);
    let found = run(&bars, FormationParams::default());
    assert_eq!(tags(&found), [Some(LineTag::Flag)], "{found:?}");
    assert_eq!((found[0].first_ms, found[0].last_ms), (t(8), t(23)));

    let no_flags = FormationParams {
        flag: false,
        ..FormationParams::default()
    };
    assert_eq!(tags(&run(&bars, no_flags)), [Some(LineTag::Channel)]);
    let flags_only = FormationParams {
        triangle: false,
        wedge: false,
        channel: false,
        double: false,
        ..FormationParams::default()
    };
    assert_eq!(tags(&run(&bars, flags_only)), [Some(LineTag::Flag)]);

    // The pole needs `impulse_pct`: at 15 % the 12 % move is no pole, and the pair is a channel.
    let weak_pole = FormationParams {
        impulse_pct: 15.0,
        ..FormationParams::default()
    };
    assert_eq!(tags(&run(&bars, weak_pole)), [Some(LineTag::Channel)]);
    // And the flag cap: at 10 bars the 15-bar consolidation is too long to be a flag.
    let short_cap = FormationParams {
        flag_max_bars: 10,
        ..FormationParams::default()
    };
    assert_eq!(tags(&run(&bars, short_cap)), [Some(LineTag::Channel)]);
}

/// A double top, by hand: tops 110 (bar 6) and 110.2 (bar 22), 0.18 % apart — inside the 0.5 %
/// equality band — and 16 bars apart (≥ 15), over a trough of 104 at bar 14: 5.5 % below the
/// lower top, past the 1.5 % depth. The level is the higher top, 110.2·(1 + H); the neckline the
/// trough's low, 104·(1 − H). Only one pivot low exists, so no sloped pair either.
///
/// Breakage: the level at the lower top or the mean (a close between would read as a break), the
/// neckline at a close instead of the trough's low, or the depth measured from the wrong top.
#[test]
fn a_double_top() {
    let bars = path(&[
        (0, 100.0),
        (6, 110.0),
        (14, 104.0),
        (22, 110.2),
        (28, 106.0),
    ]);
    let found = run(&bars, FormationParams::default());
    assert_eq!(tags(&found), [Some(LineTag::DoubleTop)], "{found:?}");
    let f = &found[0];
    let (upper, lower) = sides(f);
    assert!(close_to(upper.p0, 110.2 * (1.0 + H)) && upper.p0 == upper.p1);
    assert!(close_to(lower.p0, 104.0 * (1.0 - H)) && lower.p0 == lower.p1);
    assert_eq!((upper.t0_ms, upper.t1_ms), (t(6), t(22)));
    assert_eq!(f.touches.len(), 3);
    assert_eq!(
        f.touches[1].t_ms,
        t(14),
        "the neckline's touch is the trough"
    );
    assert!((f.tolerance_frac - 0.005).abs() < 1e-9);

    // Too shallow: at a 6 % depth floor the 5.5 % trough makes no double.
    let deep = FormationParams {
        double_depth_pct: 6.0,
        ..FormationParams::default()
    };
    assert!(run(&bars, deep).is_empty());
    // Too unequal: a 0.1 % band does not hold tops 0.18 % apart.
    let strict = FormationParams {
        double_tolerance_pct: 0.1,
        ..FormationParams::default()
    };
    assert!(run(&bars, strict).is_empty());
    // Too close: tops 16 bars apart are not a double under a 20-bar floor.
    let long = FormationParams {
        min_bars: 20,
        ..FormationParams::default()
    };
    assert!(run(&bars, long).is_empty());
}

/// The mirror: bottoms 100 (bar 6) and 99.8 (bar 22), a peak of 106 between (6.2 % above the
/// higher bottom). The level is the LOWER bottom, 99.8·(1 − H), drawn as the lower side; the
/// neckline the peak's high, the upper side.
#[test]
fn a_double_bottom() {
    let bars = path(&[(0, 110.0), (6, 100.0), (14, 106.0), (22, 99.8), (28, 104.0)]);
    let found = run(&bars, FormationParams::default());
    assert_eq!(tags(&found), [Some(LineTag::DoubleBottom)], "{found:?}");
    let (upper, lower) = sides(&found[0]);
    assert!(close_to(lower.p0, 99.8 * (1.0 - H)));
    assert!(close_to(upper.p0, 106.0 * (1.0 + H)));
}

/// A higher high between two equal tops is no double top: the series made a new top in between,
/// and the pair across it is two shoulders, not a double.
///
/// By hand: tops 110 at bars 6 and 30 with a 114 peak at bar 18 between them.
#[test]
fn a_higher_top_between_breaks_the_double() {
    let bars = path(&[
        (0, 100.0),
        (6, 110.0),
        (12, 104.0),
        (18, 114.0),
        (24, 104.0),
        (30, 110.0),
        (36, 106.0),
    ]);
    let found = run(
        &bars,
        FormationParams {
            triangle: false,
            wedge: false,
            channel: false,
            flag: false,
            ..FormationParams::default()
        },
    );
    assert!(found.is_empty(), "{found:?}");
}

/// The double top of [`a_double_top`] completed: after the second top price falls through the
/// neckline (104·(1 − H) ≈ 103.95) — the path crosses it between bars 28 and 29 and closes below
/// from bar 29 on. Two bars have closed after the breaking one at the last bar (31).
///
/// Breakage: a break not looked for after the second extreme, or the policy ignored — the pattern
/// must vanish under `Remove` and stay, flagged, under `Keep { 3 }`.
#[test]
fn a_broken_formation_follows_the_policy() {
    let bars = path(&[
        (0, 100.0),
        (6, 110.0),
        (14, 104.0),
        (22, 110.2),
        (30, 102.0),
        (31, 102.5),
    ]);
    let keep = run(&bars, FormationParams::default());
    assert_eq!(tags(&keep), [Some(LineTag::DoubleTop)]);
    let broken = keep[0].broken.expect("the neckline broke");
    assert_eq!(broken.at_ms, t(29));
    assert_eq!(broken.bars_since, 2);
    assert!(!broken.returned);

    let remove = FormationParams {
        broken: BrokenPolicy::Remove,
        ..FormationParams::default()
    };
    assert!(run(&bars, remove).is_empty());

    // The same for a two-line formation: the symmetric triangle broken upward. The upper side is
    // 110.055 − 0.2·(i − 5)·(1 + H) — 104.2 at bar 34 — and the path closes at 106 there.
    let bars = path(&[
        (0, 100.0),
        (5, 110.0),
        (10, 92.0),
        (15, 108.0),
        (20, 94.0),
        (25, 106.0),
        (30, 96.0),
        (34, 106.0),
    ]);
    let keep = run(&bars, FormationParams::default());
    assert_eq!(tags(&keep), [Some(LineTag::Triangle)]);
    assert!(keep[0].broken.is_some());
    assert!(run(&bars, remove).is_empty());
}

/// A broadening pattern — tops rising 104 / 106 / 108, bottoms falling 96 / 94 / 92 — is none of
/// the kinds: its width grows from 8 to 16.
#[test]
fn a_broadening_pattern_is_not_reported() {
    let bars = path(&[
        (0, 100.0),
        (5, 104.0),
        (10, 96.0),
        (15, 106.0),
        (20, 94.0),
        (25, 108.0),
        (30, 92.0),
        (34, 100.0),
    ]);
    let found = run(&bars, sloped_only());
    assert!(found.is_empty(), "{found:?}");
}

/// A steady trend has no turning points and so no formation; a converging pair the price has run
/// PAST — the apex before the last bar — is over and is not reported either.
#[test]
fn no_pivots_and_a_passed_apex_find_nothing() {
    let rising: Vec<(usize, f64)> = vec![(0, 100.0), (120, 160.0)];
    assert!(run(&path(&rising), FormationParams::default()).is_empty());

    // The symmetric triangle of `a_symmetric_triangle` meets at bar 52.5; price drifting on to
    // bar 60 inside what is left of it has outlived the pattern.
    let bars = path(&[
        (0, 100.0),
        (5, 110.0),
        (10, 92.0),
        (15, 108.0),
        (20, 94.0),
        (25, 106.0),
        (30, 96.0),
        (34, 100.0),
        (60, 100.5),
    ]);
    let found = run(&bars, sloped_only());
    assert!(found.is_empty(), "{found:?}");
}

/// 600 pseudo-random bars from `seed` (xorshift64): with `walk`, a random walk of ±0.3 % steps
/// bounded to 80..120; without, every close drawn afresh in 98.5..101.5 — choppy noise with no
/// memory. Each bar opens at the previous close and reaches up to 0.1 % past its body either way:
/// the scale of a five-minute bar, which the default tolerances are set for.
fn noise(seed: u64, walk: bool) -> Vec<ChartCandle> {
    let mut state = seed;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut price = 100.0f64;
    (0..600)
        .map(|i| {
            let open = price;
            price = if walk {
                (price * (1.0 + (next() - 0.5) * 0.006)).clamp(80.0, 120.0)
            } else {
                98.5 + next() * 3.0
            };
            let (hi, lo) = (
                open.max(price) * (1.0 + next() * 0.001),
                open.min(price) * (1.0 - next() * 0.001),
            );
            bar(i, open as f32, hi as f32, lo as f32, price as f32)
        })
        .collect()
}

/// Noise under the default parameters, eight seeds of each kind. What the rules let through on
/// noise is a FACT measured here, not a promise — a pattern detector cannot tell a chance pattern
/// from a real one, and a random walk is well known to draw them:
///
/// - Choppy noise in a box is a RANGE: its one sloped-free formation is the box itself, a
///   horizontal channel, and the only other finds are doubles at the box's edges — never a
///   triangle, a wedge or a flag.
/// - A random walk drifts into stretches that are triangles, wedges and doubles by every rule:
///   ten over the eight windows on 19.11.2026, pinned as a ceiling so a change that floods noise
///   with formations shows up.
///
/// Every reported formation is checked for the shape invariants: sides in order at the last bar,
/// three touches, a tag.
///
/// Breakage: a dropped span, shared-stretch or overlap rule (the walk's count jumps, the box grows
/// sloped kinds), or a malformed pair.
#[test]
fn noise_finds_few_formations_and_only_sound_ones() {
    let seeds = (1..=8u64).map(|k| k.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    let mut walked = 0;
    for seed in seeds {
        for walk in [false, true] {
            let bars = noise(seed, walk);
            let found = run(&bars, FormationParams::default());
            let last_t = bars.last().map(|b| b.t_open_ms).unwrap_or_default();
            for f in &found {
                let (upper, lower) = sides(f);
                assert!(f.tag.is_some(), "{f:?}");
                assert!(upper.price_at(last_t) > lower.price_at(last_t), "{f:?}");
                assert!(f.touches.len() >= 3, "{f:?}");
            }
            if walk {
                walked += found.len();
            } else {
                // The box itself is a horizontal channel; its edges hold doubles; nothing slopes.
                let t = tags(&found);
                assert_eq!(t.first(), Some(&Some(LineTag::Channel)), "{t:?}");
                assert!(
                    t[1..]
                        .iter()
                        .all(|t| matches!(t, Some(LineTag::DoubleTop | LineTag::DoubleBottom))),
                    "{t:?}"
                );
                let (upper, lower) = sides(&found[0]);
                assert!(upper.p0 == upper.p1 || (upper.p1 / upper.p0 - 1.0).abs() < 0.004);
                assert!(lower.p0 == lower.p1 || (lower.p1 / lower.p0 - 1.0).abs() < 0.004);
            }
        }
    }
    // Measured on 19.11.2026: 10 formations over the eight 150-bar windows of the walks.
    assert!(walked <= 10, "{walked}");
}

/// An ATR tolerance with fewer than `ATR_PERIOD + 1` bars has no band, and the detector finds
/// nothing rather than falling back to a percentage.
#[test]
fn an_atr_row_without_an_atr_finds_nothing() {
    let bars = path(&[(0, 100.0), (6, 110.0), (14, 104.0)]);
    let atr_row = FormationParams {
        tolerance_unit: ToleranceUnit::Atr,
        ..FormationParams::default()
    };
    assert!(run(&bars[..10], atr_row).is_empty());
}

/// The committed `chart-ace` candles, 5- and 1-minute, on every formation preset's parameters
/// and on a deep worst case (2 000 bars, pivots of width 1, two touches a side, every kind on):
/// the detector neither panics nor reports a malformed figure, and the real series is not empty.
///
/// What the presets find there, measured on 19.11.2026 at their own lookback: on 5-minute bars a
/// double top and a double bottom (`DoubleTops`) and nothing else; on 1-minute bars one wedge
/// (`Triangles`). Pinned loosely — a formation on each — so a rewrite that floods or empties a
/// real series is noticed without freezing the exact picture.
#[test]
fn bench_formations_are_sound() {
    use moon_core::config::auto_figures::{AutoFigurePreset, TfId};
    let presets = [
        AutoFigurePreset::Triangles,
        AutoFigurePreset::Channels,
        AutoFigurePreset::Flags,
        AutoFigurePreset::DoubleTops,
    ];
    let worst = DetectorParams::Formation(FormationParams {
        lookback_bars: 2_000,
        pivot_left: 1,
        pivot_right: 1,
        min_touches: 2,
        ..FormationParams::default()
    });
    for kind in [5u32, 1] {
        let all = ace(kind);
        let tf_ms = i64::from(kind) * 60_000;
        let ctx = DetectCtx { tf_ms, now_ms: 0.0 };
        let mut runs: Vec<(DetectorParams, usize)> = presets
            .iter()
            .filter_map(|p| p.row().detector.known().copied())
            .map(|p| {
                let need = p.bars_needed(TfId::M5) as usize;
                (p, need)
            })
            .collect();
        runs.push((worst, 2_000));
        let mut preset_total = 0;
        for (ix, (params, depth)) in runs.iter().enumerate() {
            let bars = &all[all.len().saturating_sub(*depth)..];
            let found = Formation.detect(bars, params, &ctx);
            let last_t = bars.last().map(|b| b.t_open_ms).unwrap_or_default();
            for f in &found {
                let (upper, lower) = sides(f);
                assert!(f.tag.is_some(), "{f:?}");
                assert!(
                    upper
                        .price_at(last_t)
                        .zip(lower.price_at(last_t))
                        .is_some_and(|(u, l)| u > l),
                    "{params:?} {f:?}"
                );
                assert!(f.first_ms <= f.last_ms && f.strength.is_finite());
                assert!(f.touch_volume.is_finite() && f.touch_volume >= 0.0);
            }
            if ix < presets.len() {
                preset_total += found.len();
            }
        }
        assert!(preset_total > 0, "kind {kind}: the presets find nothing");
    }
}

/// A horizontal range, by hand: tops 110 / 110.1 / 109.9 at bars 5 / 15 / 25 and bottoms 100 /
/// 99.9 / 100.1 at 10 / 20 / 30 — each side's line moves by under 0.4 % (twice the tolerance), so
/// both are flat and the channel is a RANGE. Its sides come out as exact levels at the highest top,
/// 110.1·(1 + H), and the lowest bottom, 99.9·(1 − H), over each side's own touches. No double:
/// equal tops ten bars apart are too close, and the nearest one is the only candidate.
///
/// Breakage: a range left as two lines through first and last touch (which tilt by the scatter and
/// never draw the zone box), or levels at the mean that the range's own extremes poke through.
#[test]
fn a_horizontal_range_is_two_levels() {
    let bars = path(&[
        (0, 105.0),
        (5, 110.0),
        (10, 100.0),
        (15, 110.1),
        (20, 99.9),
        (25, 109.9),
        (30, 100.1),
        (34, 105.0),
    ]);
    let found = run(&bars, FormationParams::default());
    assert_eq!(tags(&found), [Some(LineTag::Channel)], "{found:?}");
    let (upper, lower) = sides(&found[0]);
    assert_eq!(upper.p0, upper.p1);
    assert_eq!(lower.p0, lower.p1);
    assert!(close_to(upper.p0, 110.1 * (1.0 + H)), "{upper:?}");
    assert!(close_to(lower.p0, 99.9 * (1.0 - H)), "{lower:?}");
    assert_eq!((upper.t0_ms, upper.t1_ms), (t(5), t(25)));
    assert_eq!((lower.t0_ms, lower.t1_ms), (t(10), t(30)));
}

/// A rising channel right after a rise is the trend going on, not a pause in it: with the same
/// pole and width as the bull flag above, but sides climbing (+1/6 per bar), it is reported as the
/// channel it is. The mirror — the same channel after a FALL — is a bear flag.
///
/// By hand: pole 100 → 112 over bars 0..8 (12 %), tops 112 / 113 / 114 at 8 / 14 / 20, bottoms
/// 109.5 / 110.5 / 111.5 at 11 / 17 / 23. Width 3 at both ends (parallel), 3 ≤ half the 12-high
/// pole, 15 bars ≤ the 30-bar cap — everything a flag needs but the direction: the midline rises
/// 110.5 → 113, 2.3 %, far past twice the 0.2 % tolerance, the same way as the pole. Mirrored:
/// pole 112 → 100, bottoms 100 / 101 / 102 at 8 / 14 / 20, tops 103.5 / 104.5 / 105.5 at
/// 11 / 17 / 23 — the midline rises against a falling pole: a flag.
///
/// Breakage: the flag read from the pole and the width alone, with no look at where it runs.
#[test]
fn a_channel_running_with_its_pole_is_not_a_flag() {
    let with_pole = path(&[
        (0, 100.0),
        (8, 112.0),
        (11, 109.5),
        (14, 113.0),
        (17, 110.5),
        (20, 114.0),
        (23, 111.5),
        (26, 113.0),
    ]);
    let found = run(&with_pole, FormationParams::default());
    assert_eq!(tags(&found), [Some(LineTag::Channel)], "{found:?}");

    let against_pole = path(&[
        (0, 112.0),
        (8, 100.0),
        (11, 103.5),
        (14, 101.0),
        (17, 104.5),
        (20, 102.0),
        (23, 105.5),
        (26, 104.0),
    ]);
    let found = run(&against_pole, FormationParams::default());
    assert_eq!(tags(&found), [Some(LineTag::Flag)], "{found:?}");
}

/// The pole is looked for in exactly `impulse_bars` bars ending at the first touch, is the net move
/// from the window's open to that bar, and never one bar's own range.
///
/// By hand, on flat bars at 100 (range ±0.05 %, open and close 100): bar 2 opens at 90 (low 90,
/// high 100.05, close 100), and bar 9 is the start. With `bars = 7` the window is bars 3..=9,
/// opens at 100 and ends at 100 — no move; with `bars = 8` it is bars 2..=9, opens at 90 and bar 9
/// closes above it: a rise to its high, (100.05 − 90) / 90 = 11.2 %. A bar opening 100, low 94,
/// high 106, closing 105 after a bar that opened at 100: a rise of (106 − 100) / 100 = 6 % from
/// the window's open — not the bar's own 12 / 94 = 12.8 %, nor the 7.07 % from the earlier bar's
/// low; and one bar alone is no pole. A window that dips to 90 and closes its last bar at 98,
/// below its open of 100, is a FALL to that bar's low (2 %), however far the dip went.
///
/// Breakage: the window one bar too long (the old `start - bars ..= start`), a bar's own range
/// counted as a move, or a low inside the window taken as the pole's base.
#[test]
fn the_pole_window_is_impulse_bars_long_and_spans_bars() {
    let flat = |i: usize, lo: f32, hi: f32| bar(i, (lo + hi) / 2.0, hi, lo, (lo + hi) / 2.0);
    let mut win: Vec<ChartCandle> = (0..10).map(|i| flat(i, 99.95, 100.05)).collect();
    win[2] = bar(2, 90.0, 100.05, 90.0, 100.0);
    let short = pole(&win, 9, 7).expect("seven valid bars");
    assert!(short.frac < 0.002, "{short:?}");
    let long = pole(&win, 9, 8).expect("eight valid bars");
    assert!((long.frac - 10.05 / 90.0).abs() < 1e-6, "{long:?}");
    assert_eq!(long.dir, 1);

    let pair = [flat(0, 99.0, 101.0), bar(1, 100.0, 106.0, 94.0, 105.0)];
    let p = pole(&pair, 1, 2).expect("two valid bars");
    assert!((p.frac - 0.06).abs() < 1e-6, "{p:?}");
    assert_eq!(p.dir, 1);
    assert_eq!(pole(&pair, 1, 1), None);

    let dip = [
        flat(0, 99.95, 100.05),
        bar(1, 100.0, 100.05, 90.0, 95.0),
        bar(2, 95.0, 98.5, 94.9, 98.0),
        bar(3, 98.0, 98.2, 98.0, 98.0),
    ];
    let p = pole(&dip, 3, 4).expect("four valid bars");
    assert_eq!(p.dir, -1, "{p:?}");
    assert!((p.frac - 0.02).abs() < 1e-6, "{p:?}");
}

/// A crash bar with a long lower wick, then a FALLING channel: the move into the channel is the
/// crash, the channel runs with it, and it is the channel it is — not a flag (LinKvo, 19.11.2026:
/// the pole is the move INTO the figure, ending at its first touch; a bounce off the wick inside
/// the window is not one).
///
/// By hand: bars 0..=4 flat at 110; bar 5 opens 110, high 110·(1 + H), low 88, closes 95 (a 12
/// wick below the close); a bounce along the path 95 → 101 over bars 5..14; then a channel at
/// −1/6 per bar — tops 101 / 100 / 99 at bars 14 / 20 / 26, bottoms 98 / 97 / 96 at 17 / 23 / 29.
/// Width 101.05 − 98.45 ≈ 2.6 at bar 14 and 99.05 − 95.95 ≈ 3.1 at bar 26 (a ratio of 1.19,
/// inside the 20 % band: parallel); span 14..29 = 15 bars, inside the 30-bar flag cap. The pole
/// window is bars 5..=14 (`impulse_bars` = 10 ending at the first touch, bar 14).
///
/// - The widest swing in that window is the bounce: 88 → 101.05 = +14.8 %, larger than the fall
///   from bar 5's high to bar 6's low (110.06 → 95.62, −13.1 %) — so the old rule read a RISE
///   and took the falling channel for a pause in it: a bull flag.
/// - The move into the figure is the window's open, 110, to bar 14 closing at 101: a fall, of
///   110 − 101·(1 − H) = 9.05 (8.2 %) to the bar's low. The midline falls with it — the channel
///   runs with its pole, so it is a channel.
///
/// Breakage: the pole taken as the widest swing in the window (any low inside it, a wick
/// included, as a base), or measured in the wrong direction.
#[test]
fn a_falling_channel_after_a_wicked_crash_is_not_a_flag() {
    let mut bars = path(&[
        (0, 110.0),
        (4, 110.0),
        (5, 95.0),
        (14, 101.0),
        (17, 98.0),
        (20, 100.0),
        (23, 97.0),
        (26, 99.0),
        (29, 96.0),
        (32, 97.5),
    ]);
    bars[5] = bar(5, 110.0, (110.0 * (1.0 + H)) as f32, 88.0, 95.0);

    let found = run(&bars, sloped_only());
    assert_eq!(tags(&found), [Some(LineTag::Channel)], "{found:?}");
    assert_eq!(found[0].first_ms, t(14), "{found:?}");

    let p = pole(&bars, 14, 10).expect("ten valid bars");
    assert_eq!(p.dir, -1, "{p:?}");
    assert!(
        close_to(p.height, 110.0 - f64::from((101.0 * (1.0 - H)) as f32)),
        "{p:?}"
    );
}
