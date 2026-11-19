use super::super::test_bars::{ace, bar};
use super::super::{FoundShape, Touch};
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
