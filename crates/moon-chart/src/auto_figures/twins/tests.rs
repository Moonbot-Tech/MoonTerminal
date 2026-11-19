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
