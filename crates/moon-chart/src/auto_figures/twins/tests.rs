use moon_core::config::auto_figures::TfId;

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

fn losers(lines: &[(Found, TfId)]) -> Vec<bool> {
    let twins: Vec<Option<Twin>> = lines.iter().map(|(f, tf)| Twin::of(f, *tf)).collect();
    let mut out = vec![true; 7];
    twin_losers(&twins, &mut out);
    out
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
