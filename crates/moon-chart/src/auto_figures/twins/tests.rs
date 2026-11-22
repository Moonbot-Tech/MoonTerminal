use moon_core::config::auto_figures::{EventMarks, TfId};

use super::*;
use crate::auto_figures::Seg;

/// A formation tagged `tag` whose upper side starts at `upper_t0` and lower side at `lower_t0`,
/// both running to 10 000 ms; the prices are the same for every call, so only the anchors and
/// the tag tell two of them apart.
fn pair(tag: LineTag, upper_t0: f64, lower_t0: f64) -> Found {
    let side = |t0_ms: f64, p: f64| Seg {
        t0_ms,
        p0: p,
        t1_ms: 10_000.0,
        p1: p,
    };
    Found {
        shape: FoundShape::Pair {
            upper: side(upper_t0, 110.0),
            lower: side(lower_t0, 100.0),
        },
        touches: Vec::new(),
        strength: 1.0,
        first_ms: upper_t0.min(lower_t0),
        last_ms: 10_000.0,
        broken: None,
        touch_volume: 0.0,
        tolerance_frac: 0.0,
        tag: Some(tag),
        origin: None,
    }
}

fn winners(lines: &[(Found, TfId)]) -> Vec<Option<usize>> {
    let twins: Vec<Option<Twin>> = lines.iter().map(|(f, tf)| Twin::of(f, *tf)).collect();
    let mut out = vec![Some(99); 7];
    twin_winners(&twins, &mut Vec::new(), &mut out);
    out
}

fn losers(lines: &[(Found, TfId)]) -> Vec<bool> {
    winners(lines).iter().map(Option::is_some).collect()
}

/// The same pair of sides found as a channel, a flag and a triangle by three rows: the flag is
/// drawn, the other two are its twins. Ties go to the earlier row, a double is nobody's twin but
/// another double's, and other timeframes, other anchors and non-formations never pair.
///
/// Breakage: the names ranked the other way (a channel drawing over the flag), a tie dropping
/// both or neither, twins judged by one side's anchor or across timeframes, or a double top
/// swallowed by the channel on its bars.
#[test]
fn the_more_precise_name_draws_a_formation_once() {
    let h1 = TfId::H1;
    assert_eq!(
        losers(&[
            (pair(LineTag::Channel, 1_000.0, 2_000.0), h1),
            (pair(LineTag::Flag, 1_000.0, 2_000.0), h1),
            (pair(LineTag::Triangle, 1_000.0, 2_000.0), h1),
        ]),
        [true, false, true]
    );
    assert_eq!(
        losers(&[
            (pair(LineTag::Wedge, 1_000.0, 2_000.0), h1),
            (pair(LineTag::Channel, 1_000.0, 2_000.0), h1),
        ]),
        [false, true]
    );
    // The same kind from two rows: the earlier one stays.
    assert_eq!(
        losers(&[
            (pair(LineTag::Channel, 1_000.0, 2_000.0), h1),
            (pair(LineTag::Channel, 1_000.0, 2_000.0), h1),
        ]),
        [false, true]
    );
    // Different formations — another lower start, another timeframe, a double on the same bars
    // — all stay.
    assert_eq!(
        losers(&[
            (pair(LineTag::Channel, 1_000.0, 2_000.0), h1),
            (pair(LineTag::Flag, 1_000.0, 3_000.0), h1),
            (pair(LineTag::Flag, 1_000.0, 2_000.0), TfId::H4),
            (pair(LineTag::DoubleTop, 1_000.0, 2_000.0), h1),
        ]),
        [false, false, false, false]
    );
    assert_eq!(
        losers(&[
            (pair(LineTag::DoubleTop, 1_000.0, 2_000.0), h1),
            (pair(LineTag::DoubleTop, 1_000.0, 2_000.0), h1),
            (pair(LineTag::DoubleBottom, 1_000.0, 2_000.0), h1),
        ]),
        [false, true, false]
    );
    // A level is not a formation; the output is one flag per line, whatever it held before.
    let level = Found {
        shape: FoundShape::Horizontal { price: 100.0 },
        ..pair(LineTag::Channel, 1_000.0, 2_000.0)
    };
    assert!(Twin::of(&level, h1).is_none());
    assert_eq!(losers(&[(level, h1)]), [false]);
}

/// Every loser of a set names the set's ONE survivor, never a fellow loser: a channel and a
/// triangle on the flag's pair of sides both point at the flag, wherever it sits in the order.
///
/// Breakage: a loser naming the first line that outranks it (the triangle) instead of the set's
/// survivor, which would hand the channel row's marks to a line that is not drawn.
#[test]
fn every_twin_names_the_survivor() {
    let h1 = TfId::H1;
    assert_eq!(
        winners(&[
            (pair(LineTag::Channel, 1_000.0, 2_000.0), h1),
            (pair(LineTag::Triangle, 1_000.0, 2_000.0), h1),
            (pair(LineTag::Flag, 1_000.0, 2_000.0), h1),
        ]),
        [Some(2), Some(2), None]
    );
}

/// The survivor shows every event either row marks, each event sized by the row that asked for
/// it: here the winner marks bounces and stop hunts at 0.2 %, the twin approaches at 0.7 %,
/// breakouts, confluence at 1.5 % and stop hunts at 0.9 % — so the union has all five, with the
/// twin's approach and confluence distances and the winner's own stop-hunt depth.
///
/// Breakage: the twin's marks dropped with the twin, the union taking the twin's distance for an
/// event the winner already sized, or an event only the twin marks sized by the winner's value.
#[test]
fn the_survivor_shows_the_union_of_the_marks() {
    let none = EventMarks {
        approach: false,
        squeeze: false,
        breakout: false,
        false_break: false,
        bounce: false,
        role_flip: false,
        stop_hunt: false,
        confluence: false,
        approach_pct: 0.3,
        stop_hunt_pct: 0.1,
        confluence_pct: 0.5,
    };
    let winner = EventMarks {
        bounce: true,
        stop_hunt: true,
        stop_hunt_pct: 0.2,
        ..none
    };
    let twin = EventMarks {
        approach: true,
        approach_pct: 0.7,
        breakout: true,
        confluence: true,
        confluence_pct: 1.5,
        stop_hunt: true,
        stop_hunt_pct: 0.9,
        ..none
    };
    let u = union_marks(&winner, &twin);
    assert!(u.bounce && u.approach && u.breakout && u.confluence && u.stop_hunt);
    assert!(!(u.squeeze || u.false_break || u.role_flip));
    assert_eq!((u.approach_pct, u.confluence_pct), (0.7, 1.5));
    assert_eq!(u.stop_hunt_pct, 0.2);
}

/// A row's selection for [`select_rows`]: its first `count` found lines that are not skipped, or
/// nothing at all for a row that keeps nothing (a `take` of the wrong side, `count = 0`, no
/// reference price) — `take::select_skipping`'s contract without the geometry.
fn first(count: usize, found: &[Found], skip: &[bool]) -> Vec<usize> {
    (0..found.len())
        .filter(|&i| !skip.get(i).copied().unwrap_or(false))
        .take(count)
        .collect()
}

/// Marks with only `breakout` (when `true`) or only `bounce` on.
fn marks(breakout: bool) -> EventMarks {
    EventMarks {
        approach: false,
        squeeze: false,
        breakout,
        false_break: false,
        bounce: !breakout,
        role_flip: false,
        stop_hunt: false,
        confluence: false,
        ..EventMarks::default()
    }
}

/// The flag row does not keep the formation (its side, its `count`): the channel row keeps it and
/// draws it under its own name, with its own marks.
///
/// Breakage: twins judged over the found lines — the unkept flag takes the formation away from
/// the channel row, and nobody draws it.
#[test]
fn a_twin_the_winner_does_not_keep_stays_with_the_row_that_keeps_it() {
    let h1 = TfId::H1;
    let channels = [pair(LineTag::Channel, 1_000.0, 2_000.0)];
    let flags = [pair(LineTag::Flag, 1_000.0, 2_000.0)];
    let (m0, m1) = (marks(true), marks(false));
    let rows = [
        TwinRow {
            tf: h1,
            found: &channels,
            marks: &m0,
        },
        TwinRow {
            tf: h1,
            found: &flags,
            marks: &m1,
        },
    ];
    let picks = select_rows(&rows, |r, skip| match r {
        0 => first(1, rows[r].found, skip),
        _ => Vec::new(),
    });
    assert_eq!(picks[0].kept, vec![0]);
    assert!(picks[1].kept.is_empty());
    assert!(picks[0].joined.is_empty(), "{picks:?}");
}

/// Both rows keep the formation: the flag row draws it, once, with the union of both rows' marks,
/// and the channel row takes its next line — once, and not a second reading of the same flag.
///
/// By hand: the channel row finds the shared formation X, the same sides as a triangle (X again)
/// and another channel Y, and keeps one line. After losing X it may not take the triangle (the
/// flag row draws that formation), so its slot goes to Y.
///
/// Breakage: the channel row's slot left empty, the refill bringing X back under another name, or
/// the channel row's breakout mark lost with its twin.
#[test]
fn the_row_that_loses_a_twin_takes_its_next_line() {
    let h1 = TfId::H1;
    let channels = [
        pair(LineTag::Channel, 1_000.0, 2_000.0),
        pair(LineTag::Triangle, 1_000.0, 2_000.0),
        pair(LineTag::Channel, 3_000.0, 4_000.0),
    ];
    let flags = [pair(LineTag::Flag, 1_000.0, 2_000.0)];
    let (m0, m1) = (marks(true), marks(false));
    let rows = [
        TwinRow {
            tf: h1,
            found: &channels,
            marks: &m0,
        },
        TwinRow {
            tf: h1,
            found: &flags,
            marks: &m1,
        },
    ];
    let mut calls = vec![0; 2];
    let picks = select_rows(&rows, |r, skip| {
        calls[r] += 1;
        first(1, rows[r].found, skip)
    });
    assert_eq!(picks[0].kept, vec![2]);
    assert_eq!(picks[1].kept, vec![0]);
    assert_eq!(calls, [2, 1], "one refill, for the row that lost");
    let joined = picks[1]
        .joined_of(0)
        .expect("the flag shows both rows' marks");
    assert!(joined.breakout && joined.bounce);
    assert!(picks[0].joined.is_empty());
}

/// Two rows that refill with the same formation draw it once, the more precise reading, and the
/// other row is not refilled again.
///
/// By hand: rows A and B both lose X to the flag row C; A's next line is the channel Y, B's the
/// wedge Y on the same sides — the wedge stays, A's channel goes, and A ends with nothing.
///
/// Breakage: a refill pair drawn twice, or a second refill (a loop with no exit).
#[test]
fn twins_among_the_refills_are_drawn_once() {
    let h1 = TfId::H1;
    let a = [
        pair(LineTag::Channel, 1_000.0, 2_000.0),
        pair(LineTag::Channel, 3_000.0, 4_000.0),
    ];
    let b = [
        pair(LineTag::Triangle, 1_000.0, 2_000.0),
        pair(LineTag::Wedge, 3_000.0, 4_000.0),
    ];
    let c = [pair(LineTag::Flag, 1_000.0, 2_000.0)];
    let m = marks(false);
    let rows = [
        TwinRow {
            tf: h1,
            found: &a,
            marks: &m,
        },
        TwinRow {
            tf: h1,
            found: &b,
            marks: &m,
        },
        TwinRow {
            tf: h1,
            found: &c,
            marks: &m,
        },
    ];
    let mut calls = vec![0; 3];
    let picks = select_rows(&rows, |r, skip| {
        calls[r] += 1;
        first(1, rows[r].found, skip)
    });
    assert!(picks[0].kept.is_empty(), "{picks:?}");
    assert_eq!(picks[1].kept, vec![1]);
    assert_eq!(picks[2].kept, vec![0]);
    assert_eq!(calls, [2, 2, 1]);
}
