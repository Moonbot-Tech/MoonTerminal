use super::super::test_bars::{ace, bar};
use super::super::{FoundShape, Seg, Touch};
use super::*;

fn line(shape: FoundShape) -> Found {
    Found {
        shape,
        touches: vec![Touch {
            t_ms: 0.0,
            price: 100.0,
        }],
        strength: 1.0,
        first_ms: 0.0,
        last_ms: 0.0,
        broken: None,
        touch_volume: 0.0,
        tolerance_frac: 0.0,
        tag: None,
    }
}

fn level(price: f64) -> Found {
    line(FoundShape::Horizontal { price })
}

/// Bars closing at each of `closes`, with a range of ±0.2 that never reaches a line 1 away.
fn closes(cs: &[f32]) -> Vec<ChartCandle> {
    cs.iter()
        .enumerate()
        .map(|(i, &c)| bar(i, c, c + 0.2, c - 0.2, c))
        .collect()
}

fn events_of(lines: &[Found], bars: &[ChartCandle], price: f64, policy: BrokenPolicy) -> EventSet {
    let mut out = Vec::new();
    line_events(
        lines,
        bars,
        price,
        bars.last().map_or(0.0, |b| b.t_open_ms),
        &EventMarks::default(),
        policy,
        &mut out,
    );
    assert_eq!(out.len(), lines.len());
    out[0]
}

const KEEP3: BrokenPolicy = BrokenPolicy::Keep { bars: 3 };

/// A close crossing the level is a breakout; crossing back within the window is a false break and
/// not a second breakout; crossing back too late is a plain breakout the other way.
///
/// Breakage: a false break measured from the wrong end of the run, the window ignored, or both
/// events set on the return bar.
#[test]
fn breakout_and_false_break() {
    let l = [level(100.0)];
    let e = events_of(&l, &closes(&[99.0, 101.0]), 101.0, KEEP3);
    assert!(e.contains(AutoEvent::Breakout) && !e.contains(AutoEvent::FalseBreak));

    let e = events_of(&l, &closes(&[99.0, 101.0, 99.0]), 99.0, KEEP3);
    assert!(e.contains(AutoEvent::FalseBreak) && !e.contains(AutoEvent::Breakout));

    let e = events_of(&l, &closes(&[99.0, 101.0, 101.0, 101.0, 99.0]), 99.0, KEEP3);
    assert!(
        e.contains(AutoEvent::FalseBreak),
        "run began three bars back: inside"
    );

    let e = events_of(
        &l,
        &closes(&[99.0, 101.0, 101.0, 101.0, 101.0, 99.0]),
        99.0,
        KEEP3,
    );
    assert!(
        !e.contains(AutoEvent::FalseBreak),
        "run began four bars back: outside"
    );
    assert!(e.contains(AutoEvent::Breakout));
}

/// Under `Remove` a false break is never shown — removing the level at once is what that policy is
/// for.
///
/// Breakage: a default window leaking into the `Remove` case.
#[test]
fn remove_shows_no_false_break() {
    assert_eq!(false_break_window(BrokenPolicy::Remove), 0);
    let e = events_of(
        &[level(100.0)],
        &closes(&[99.0, 101.0, 99.0]),
        99.0,
        BrokenPolicy::Remove,
    );
    assert!(!e.contains(AutoEvent::FalseBreak));
}

/// A bar reaching the line and closing back on its side is a bounce; one that does not reach it is
/// not.
///
/// Breakage: testing the close instead of the range, or the side of the wrong bar.
#[test]
fn bounce() {
    let l = [level(100.0)];
    let mut bars = closes(&[99.0, 99.0]);
    bars[1] = bar(1, 99.0, 100.5, 98.8, 99.5);
    let e = events_of(&l, &bars, 99.5, KEEP3);
    assert!(e.contains(AutoEvent::Bounce) && !e.contains(AutoEvent::Breakout));
    let e = events_of(&l, &closes(&[99.0, 99.0]), 99.0, KEEP3);
    assert!(!e.contains(AutoEvent::Bounce));
}

/// Approach is a distance in % of the LINE's price, and an unmarked event is never shown.
///
/// Breakage: measuring against the current price, or ignoring `EventMarks`.
#[test]
fn approach_and_marks() {
    let l = [level(100.0)];
    let bars = closes(&[99.0, 99.0]);
    assert!(events_of(&l, &bars, 99.71, KEEP3).contains(AutoEvent::Approach));
    assert!(!events_of(&l, &bars, 99.69, KEEP3).contains(AutoEvent::Approach));
    assert!(!events_of(&l, &bars, f64::NAN, KEEP3).contains(AutoEvent::Approach));

    let mut out = Vec::new();
    let marks = EventMarks {
        approach: false,
        ..EventMarks::default()
    };
    line_events(&l, &bars, 99.9, 0.0, &marks, KEEP3, &mut out);
    assert!(out[0].is_empty());
}

/// A pressed squeeze shows the squeeze event; a sloped line is judged by its projection at each
/// bar, not by its endpoints.
///
/// Breakage: a squeeze event from an unpressed figure, or a sloped line read at `p0`.
#[test]
fn squeeze_and_sloped_lines() {
    let bars = closes(&[99.0, 99.0]);
    let pressed = line(FoundShape::Squeeze {
        level: 100.0,
        t0_ms: 0.0,
        p0: 90.0,
        t1_ms: 1.0,
        p1: 91.0,
        pressed: true,
    });
    assert!(events_of(&[pressed], &bars, 99.0, KEEP3).contains(AutoEvent::Squeeze));

    // A line falling from 100 at bar 0 to 98 at bar 1: bar 0 closed under it, bar 1 above it.
    let falling = line(FoundShape::Sloped {
        t0_ms: bars[0].t_open_ms,
        p0: 100.0,
        t1_ms: bars[1].t_open_ms,
        p1: 98.0,
    });
    assert!(events_of(&[falling], &bars, 99.0, KEEP3).contains(AutoEvent::Breakout));
}

/// Empty bars, NaN bars, a huge window and an empty line list: no panic, one set per line.
///
/// Breakage: indexing the last bar of an empty slice, or a `window + 2` overflow.
#[test]
fn degenerate_inputs() {
    let l = [level(100.0)];
    assert!(events_of(&l, &[], 99.0, KEEP3).is_empty());
    let nan = vec![bar(0, f32::NAN, f32::NAN, f32::NAN, f32::NAN); 3];
    let e = events_of(&l, &nan, 99.0, BrokenPolicy::Keep { bars: u16::MAX });
    assert!(!e.contains(AutoEvent::Breakout));
    let mut out = vec![EventSet::default(); 3];
    line_events(&[], &nan, 1.0, 0.0, &EventMarks::default(), KEEP3, &mut out);
    assert!(out.is_empty(), "the output is cleared, one set per line");
}

/// On the real `chart-ace` candles, an independent scan agrees on the breakout of the last bar for
/// a level placed at every one of the last 50 closes' midpoints.
///
/// Breakage: the side of a close computed against the wrong bar's line price, or an off-by-one in
/// which bar is "last".
#[test]
fn real_candles_breakout_matches_an_independent_crossing_check() {
    let bars = ace(1);
    let n = bars.len();
    for k in 0..50 {
        let (a, b) = (bars[n - 2 - k].close, bars[n - 1 - k].close);
        let mid = (f64::from(a) + f64::from(b)) / 2.0;
        let slice = &bars[..n - k];
        let crossed = (f64::from(a) - mid) * (f64::from(b) - mid) < 0.0;
        let e = events_of(&[level(mid)], slice, f64::from(b), KEEP3);
        assert_eq!(
            e.contains(AutoEvent::Breakout) || e.contains(AutoEvent::FalseBreak),
            crossed,
            "bar {k} back: {a} -> {b} around {mid}"
        );
    }
}

/// `closes` followed by one bar opening and closing at `close` whose range is `low..high`.
fn then_touch(cs: &[f32], low: f32, high: f32, close: f32) -> Vec<ChartCandle> {
    let mut bars = closes(cs);
    bars.push(bar(cs.len(), close, high, low, close));
    bars
}

/// The four close-side events of one set, for checking which one a bar chose.
fn side_story(e: EventSet) -> Vec<AutoEvent> {
    [
        AutoEvent::Breakout,
        AutoEvent::FalseBreak,
        AutoEvent::Bounce,
        AutoEvent::RoleFlip,
    ]
    .into_iter()
    .filter(|&x| e.contains(x))
    .collect()
}

/// A resistance at 100 held for five closes below it, broke (two closes above), and the last bar
/// dips to 99.9 and closes above again: the broken resistance holds as support — a role flip, and
/// NOT a bounce. A wick to 99.9 is 0.1 % through, under the 0.3 % stop-hunt depth.
///
/// Breakage: a role flip never found (the old bounce-only reading), or reported with a bounce.
#[test]
fn a_broken_level_retested_from_the_new_side_flips_its_role() {
    let l = [level(100.0)];
    let bars = then_touch(
        &[99.0, 99.0, 99.0, 99.0, 99.0, 101.0, 101.0],
        99.9,
        101.2,
        101.0,
    );
    let e = events_of(&l, &bars, 101.0, KEEP3);
    assert_eq!(side_story(e), vec![AutoEvent::RoleFlip]);
    assert!(!e.contains(AutoEvent::StopHunt));

    // A support at 100 broken downward and retested from below flips the same way.
    let mut cs = vec![101.0f32; 5];
    cs.extend([99.0, 99.0]);
    let broken_support = then_touch(&cs, 98.8, 100.1, 99.0);
    assert_eq!(
        side_story(events_of(&l, &broken_support, 99.0, KEEP3)),
        vec![AutoEvent::RoleFlip]
    );
    // Without the break it is a plain bounce of a resistance seen from below.
    let held = then_touch(&[99.0; 7], 98.8, 100.1, 99.0);
    assert_eq!(
        side_story(events_of(&l, &held, 99.0, KEEP3)),
        vec![AutoEvent::Bounce]
    );
}

/// A support held above 100, dipped under it for ONE close (a false break, inside the 3-bar
/// window), recovered, and the last bar touches and holds: the line held its side — a bounce, not
/// a role flip. Under `Remove` a single close beyond is already a real break, so the same bars are
/// a role flip there.
///
/// Breakage: any close beyond read as a role change whatever the policy, or `Remove`'s zero window
/// ignored.
#[test]
fn a_false_break_does_not_flip_the_role() {
    let l = [level(100.0)];
    let bars = then_touch(
        &[101.0, 101.0, 101.0, 101.0, 101.0, 99.0, 101.0, 101.0],
        99.9,
        101.2,
        101.0,
    );
    assert_eq!(
        side_story(events_of(&l, &bars, 101.0, KEEP3)),
        vec![AutoEvent::Bounce]
    );
    assert_eq!(
        side_story(events_of(&l, &bars, 101.0, BrokenPolicy::Remove)),
        vec![AutoEvent::RoleFlip]
    );
}

/// A break older than `ROLE_FLIP_LOOKBACK` bars is settled history: the touch is a bounce.
///
/// Breakage: the lookback ignored, so every touch of a line that ever changed sides is a flip.
#[test]
fn an_old_break_is_a_settled_role() {
    let l = [level(100.0)];
    let mut cs = vec![99.0f32; 5];
    cs.extend(std::iter::repeat_n(101.0, ROLE_FLIP_LOOKBACK + 1));
    let bars = then_touch(&cs, 99.9, 101.2, 101.0);
    assert_eq!(
        side_story(events_of(&l, &bars, 101.0, KEEP3)),
        vec![AutoEvent::Bounce]
    );
    // The same break inside the lookback is a flip.
    let mut cs = vec![99.0f32; 5];
    cs.extend(std::iter::repeat_n(101.0, ROLE_FLIP_LOOKBACK - 2));
    let bars = then_touch(&cs, 99.9, 101.2, 101.0);
    assert_eq!(
        side_story(events_of(&l, &bars, 101.0, KEEP3)),
        vec![AutoEvent::RoleFlip]
    );
}

/// A stop hunt: the wick goes MORE than `stop_hunt_pct` of the line's price through it and the bar
/// still closes back — under a support (99.5 < 99.7) or over a resistance (100.5 > 100.3). It
/// comes with the bar's bounce, never instead of it, and never with a crossing. A wick of exactly
/// the depth (99.7 at 0.3 %) is not deeper than it; a deeper threshold (0.6 %) turns the first one
/// off.
///
/// Breakage: the pierce measured from the close or the current price, the threshold read as a
/// fraction, `>=` for "more than", or a stop hunt on a bar that closed beyond.
#[test]
fn a_stop_hunt_pierces_deeper_than_the_threshold_and_closes_back() {
    let l = [level(100.0)];
    let support = then_touch(&[101.0; 5], 99.5, 101.2, 101.0);
    let e = events_of(&l, &support, 101.0, KEEP3);
    assert!(e.contains(AutoEvent::StopHunt));
    assert_eq!(side_story(e), vec![AutoEvent::Bounce]);

    let resistance = then_touch(&[99.0; 5], 98.8, 100.5, 99.0);
    let e = events_of(&l, &resistance, 99.0, KEEP3);
    assert!(e.contains(AutoEvent::StopHunt));
    assert_eq!(side_story(e), vec![AutoEvent::Bounce]);

    let shallow = then_touch(&[101.0; 5], 99.75, 101.2, 101.0);
    assert!(!events_of(&l, &shallow, 101.0, KEEP3).contains(AutoEvent::StopHunt));

    let mut out = Vec::new();
    let deep = EventMarks {
        stop_hunt_pct: 0.6,
        ..EventMarks::default()
    };
    line_events(&l, &support, 101.0, 0.0, &deep, KEEP3, &mut out);
    assert!(!out[0].contains(AutoEvent::StopHunt));

    // Closed beyond: a breakout with a long wick back is not a stop hunt.
    let through = then_touch(&[101.0; 5], 98.0, 101.2, 99.0);
    let e = events_of(&l, &through, 99.0, KEEP3);
    assert!(!e.contains(AutoEvent::StopHunt));
    assert_eq!(side_story(e), vec![AutoEvent::Breakout]);
}

/// On the real `chart-ace` candles, for a level at each of the last 200 bars' midpoints: one bar
/// never tells two side stories (at most one of breakout / false break / bounce / role flip), a
/// stop hunt always comes with a bounce or a role flip, and the last `bars_read` valid bars give
/// exactly the answer the whole series does.
///
/// Breakage: two exclusive events set on one bar, a stop hunt on a crossing, or a walk reading
/// further back than `bars_read` admits — which a caller keeping only that tail would miss.
#[test]
fn bench_events_are_exclusive_and_bounded() {
    let bars = ace(1);
    let n = bars.len();
    let mut touched = 0;
    for policy in [KEEP3, BrokenPolicy::Remove] {
        for k in 0..200 {
            let slice = &bars[..n - k];
            let b = &slice[slice.len() - 1];
            let mid = (f64::from(b.high) + f64::from(b.low)) / 2.0;
            let price = f64::from(b.close);
            let e = events_of(&[level(mid)], slice, price, policy);
            assert!(side_story(e).len() <= 1, "bar {k}: {e:?}");
            if e.contains(AutoEvent::StopHunt) {
                assert!(e.contains(AutoEvent::Bounce) || e.contains(AutoEvent::RoleFlip));
            }
            if e.contains(AutoEvent::Bounce) || e.contains(AutoEvent::RoleFlip) {
                touched += 1;
            }
            let tail = &slice[slice.len() - bars_read(policy)..];
            assert_eq!(events_of(&[level(mid)], tail, price, policy), e, "bar {k}");
        }
    }
    assert!(touched > 20, "the bench touches its midpoints: {touched}");
}

/// The stop-hunt depth is a strict bound: a wick reaching EXACTLY the threshold is a touch, one a
/// hair deeper is a hunt — below a support and above a resistance alike.
///
/// The threshold here is 0.390625 % (2^-8 of price) rather than the default 0.3 %: the bars are
/// `f32`, and no `f32` price lies exactly 0.3 % from a line (`0.3f32` as a fraction has more bits
/// than either side can hold), so a "wick at exactly 0.3 %" is always a little short of it or past
/// it. With 2^-8 the line at 100 puts the boundary at 99.609375 and 100.390625, both exact.
///
/// Breakage: `<=` / `>=` in place of the strict comparison, or the depth measured from the close.
#[test]
fn a_wick_exactly_at_the_stop_hunt_depth_is_not_a_hunt() {
    let l = [level(100.0)];
    let marks = EventMarks {
        stop_hunt_pct: 0.390_625,
        ..EventMarks::default()
    };
    let hunt = |bars: &[ChartCandle], price: f64| {
        let mut out = Vec::new();
        line_events(&l, bars, price, 0.0, &marks, KEEP3, &mut out);
        out[0]
    };
    let at_support = then_touch(&[101.0; 5], 99.609_375, 101.2, 101.0);
    let e = hunt(&at_support, 101.0);
    assert!(!e.contains(AutoEvent::StopHunt));
    assert_eq!(side_story(e), vec![AutoEvent::Bounce]);
    let past_support = then_touch(&[101.0; 5], 99.609, 101.2, 101.0);
    assert!(hunt(&past_support, 101.0).contains(AutoEvent::StopHunt));

    let at_resistance = then_touch(&[99.0; 5], 98.8, 100.390_625, 99.0);
    assert!(!hunt(&at_resistance, 99.0).contains(AutoEvent::StopHunt));
    let past_resistance = then_touch(&[99.0; 5], 98.8, 100.391, 99.0);
    assert!(hunt(&past_resistance, 99.0).contains(AutoEvent::StopHunt));
}

/// A role flip is a particular bounce: a row that marks bounces but not role flips sees the flip
/// bar as a bounce; a row that marks neither sees nothing; one that marks only role flips sees the
/// flip and no bounce.
///
/// Breakage: the unmarked role flip filtered away together with the bounce it replaced (the bar
/// then showed no touch at all), or a bounce added beside a marked flip.
#[test]
fn an_unmarked_role_flip_shows_as_a_bounce() {
    let l = [level(100.0)];
    let mut cs = vec![99.0f32; 5];
    cs.extend([101.0, 101.0]);
    let bars = then_touch(&cs, 99.9, 101.2, 101.0);
    let story = |role_flip: bool, bounce: bool| {
        let marks = EventMarks {
            role_flip,
            bounce,
            ..EventMarks::default()
        };
        let mut out = Vec::new();
        line_events(&l, &bars, 101.0, 0.0, &marks, KEEP3, &mut out);
        side_story(out[0])
    };
    assert_eq!(story(true, true), vec![AutoEvent::RoleFlip]);
    assert_eq!(story(false, true), vec![AutoEvent::Bounce]);
    assert_eq!(story(true, false), vec![AutoEvent::RoleFlip]);
    assert_eq!(story(false, false), Vec::<AutoEvent>::new());
}

/// A horizontal two-line formation from `lower` to `upper`.
fn pair(upper: f64, lower: f64) -> Found {
    let side = |p: f64| Seg {
        t0_ms: 0.0,
        p0: p,
        t1_ms: 1.0,
        p1: p,
    };
    line(FoundShape::Pair {
        upper: side(upper),
        lower: side(lower),
    })
}

/// A formation breaks out through EITHER side, bounces off either, and a bar that does both tells
/// one story — the breakout.
///
/// By hand, a channel 99..101 and bars of ±0.2 around their closes: a close of 101.5 after 100
/// crosses the top, one of 98.5 the bottom; a close of 99.1 after 99.5 dips its low to 98.9, onto
/// the bottom, and holds above it. The last bar of the fourth case closes at 98.5 below the bottom
/// while its high of 101.2 reached the top from below and its close stayed below the top too: a
/// touch-and-hold of the top AND a crossing of the bottom, of which only the crossing is shown.
///
/// Breakage: only the upper side judged (the downside breakout and the bounce off the bottom go
/// unmarked), or the two sides' stories shown together.
#[test]
fn a_formation_breaks_and_bounces_on_either_side() {
    let f = [pair(101.0, 99.0)];
    let e = events_of(&f, &closes(&[100.0, 100.0, 101.5]), 101.5, KEEP3);
    assert!(e.contains(AutoEvent::Breakout), "{e:?}");
    let e = events_of(&f, &closes(&[100.0, 100.0, 98.5]), 98.5, KEEP3);
    assert!(e.contains(AutoEvent::Breakout), "{e:?}");
    let e = events_of(&f, &closes(&[100.0, 99.5, 99.1]), 99.1, KEEP3);
    assert!(
        e.contains(AutoEvent::Bounce) && !e.contains(AutoEvent::Breakout),
        "{e:?}"
    );

    let mut bars = closes(&[100.0, 100.0]);
    bars.push(bar(2, 100.0, 101.2, 98.3, 98.5));
    let e = events_of(&f, &bars, 98.5, KEEP3);
    assert!(e.contains(AutoEvent::Breakout), "{e:?}");
    assert!(
        !e.contains(AutoEvent::Bounce) && !e.contains(AutoEvent::StopHunt),
        "{e:?}"
    );
}

/// A formation is approached at its NEARER side: a price of 99.1 is 0.1 % from the bottom of a
/// 99..101 channel, inside the default 0.3 %, though 1.9 % from its top; a price of 100 is 1 % from
/// both and approaches neither.
///
/// Breakage: the approach measured to the upper line only.
#[test]
fn a_formation_is_approached_at_its_nearer_side() {
    let f = [pair(101.0, 99.0)];
    let e = events_of(&f, &closes(&[100.0, 100.0]), 99.1, KEEP3);
    assert!(e.contains(AutoEvent::Approach), "{e:?}");
    let e = events_of(&f, &closes(&[100.0, 100.0]), 100.0, KEEP3);
    assert!(!e.contains(AutoEvent::Approach), "{e:?}");
}

/// A bar that runs through a narrow formation tells ONE story: the story of the side its close
/// ended up beyond.
///
/// By hand, a 99.5..100.5 formation: closes 100, 100, then 101 (out through the top), then 99 —
/// one bar from above the top to below the bottom. Judged alone, the top calls that bar a false
/// break (a one-bar run above, back below it) and the bottom a breakout; the close is below the
/// formation, so it is the breakout down, alone. The mirror (98.5 then 101) is the breakout up.
/// Closing back INSIDE (101 then 100) is the false break of the top, alone.
///
/// Breakage: the two sides' crossings merged as they come (the old union showed Breakout AND
/// FalseBreak on the one bar).
#[test]
fn a_bar_through_a_narrow_formation_is_one_crossing() {
    let f = [pair(100.5, 99.5)];
    let story = |cs: &[f32]| {
        let e = events_of(&f, &closes(cs), f64::from(*cs.last().unwrap()), KEEP3);
        (
            e.contains(AutoEvent::Breakout),
            e.contains(AutoEvent::FalseBreak),
        )
    };
    assert_eq!(story(&[100.0, 100.0, 101.0, 99.0]), (true, false));
    assert_eq!(story(&[100.0, 100.0, 98.5, 101.0]), (true, false));
    assert_eq!(story(&[100.0, 100.0, 101.0, 100.0]), (false, true));
}

/// Bars without a close (the core's ring bars) tell a bar event by their whole range: a bar wholly
/// beyond a line is on that side, exactly as a real candle of that range would be whatever its
/// close, and a bar straddling the line is on no side.
///
/// By hand, a level at 100: ranges 98–99.8 then 100.5–102 cross it upward (the breakout); back to
/// 98.5–99.5 within the false-break window (the false break); a last range of 99–101 spans the
/// line, so no bar event at all. A 99..101 channel left by a bar spanning 101.5–102 after two
/// inside it is the breakout through its top.
///
/// Breakage: a ring bar that never decides a side (no breakout, ever, on a market known only from
/// the ring), or a straddling ring bar given a side.
#[test]
fn ring_bars_cross_a_line_their_whole_range_crosses() {
    let ring = |ranges: &[(f32, f32)]| -> Vec<ChartCandle> {
        ranges
            .iter()
            .enumerate()
            .map(|(i, &(h, l))| bar(i, f32::NAN, h, l, f32::NAN))
            .collect()
    };
    let l = [level(100.0)];
    let e = events_of(&l, &ring(&[(99.8, 98.0), (102.0, 100.5)]), 101.0, KEEP3);
    assert!(
        e.contains(AutoEvent::Breakout) && !e.contains(AutoEvent::FalseBreak),
        "{e:?}"
    );
    let e = events_of(
        &l,
        &ring(&[(99.8, 98.0), (102.0, 100.5), (99.5, 98.5)]),
        99.0,
        KEEP3,
    );
    assert!(
        e.contains(AutoEvent::FalseBreak) && !e.contains(AutoEvent::Breakout),
        "{e:?}"
    );
    let e = events_of(&l, &ring(&[(99.8, 98.0), (101.0, 99.0)]), 100.2, KEEP3);
    for bar_event in [
        AutoEvent::Breakout,
        AutoEvent::FalseBreak,
        AutoEvent::Bounce,
        AutoEvent::RoleFlip,
        AutoEvent::StopHunt,
    ] {
        assert!(!e.contains(bar_event), "{bar_event:?} in {e:?}");
    }

    let f = [pair(101.0, 99.0)];
    let e = events_of(
        &f,
        &ring(&[(100.5, 99.5), (100.5, 99.5), (102.0, 101.5)]),
        101.8,
        KEEP3,
    );
    assert!(e.contains(AutoEvent::Breakout), "{e:?}");
}
