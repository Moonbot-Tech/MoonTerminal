use super::*;
use moon_core::market::trade_replay::replay_window_ms;

const ENTRY_S: i64 = 1_700_000_000;
const MINUTE_MS: i64 = 60_000;

fn replay_bounds(duration_ms: i64) -> (i64, i64, i64, i64) {
    let exit_s = ENTRY_S + duration_ms / 1_000;
    let data = replay_window_ms(ENTRY_S * 1_000, exit_s * 1_000).expect("valid replay window");
    (ENTRY_S * 1_000, exit_s * 1_000, data.from_ms, data.to_ms)
}

/// Lowering `frame.rs:MIN_CONTEXT_BARS` from `90` to `45`, or changing
/// `frame.rs:CONTEXT_DIVISOR` from `1` to `2`, must fail: short trades lose their fixed context
/// or an unclamped long trade grows from one third to one half of the viewport.
#[test]
fn trade_frames_keep_the_floor_for_short_trades_and_third_width_when_unclamped() {
    for duration_ms in [28_000, 2 * MINUTE_MS] {
        let (entry, exit, _, _) = replay_bounds(duration_ms);
        let (start, end) = trade_frame(entry, exit, MINUTE_MS).expect("short frame");
        assert_eq!(end - start, duration_ms + 2 * 90 * MINUTE_MS);
        assert_eq!(start + end, entry + exit);
    }
    let duration_ms = 3 * 60 * MINUTE_MS;
    let entry = ENTRY_S * 1_000;
    let exit = entry + duration_ms;
    let (start, end) = trade_frame(entry, exit, MINUTE_MS).expect("long frame");
    assert_eq!((start, end), (entry - duration_ms, exit + duration_ms));
    assert_eq!(end - start, 3 * duration_ms);
}

/// Reinstating a data clamp in `frame.rs:trade_frame` must fail: a wide frame intentionally runs
/// past the fetched replay interval, leaving empty margin instead of widening the download.
#[test]
fn trade_frames_may_extend_past_replay_data_without_widening_the_fetch() {
    let duration_ms = 4 * 24 * 60 * MINUTE_MS;
    let (entry, exit, from, to) = replay_bounds(duration_ms);
    let (start, end) = trade_frame(entry, exit, MINUTE_MS).expect("wide frame");
    assert_eq!((start, end), (entry - duration_ms, exit + duration_ms));
    assert!(
        start < from && end > to,
        "the frame must retain its requested context beyond fetched data: {start}..{end} vs {from}..{to}"
    );
}

/// `frame.rs:trade_frame` must frame same-second trades on the ninety-bar floor and cap a
/// coarse or unknown resolution at two hours; lowering its ceiling to one hour would make
/// the newly widened short-trade frame silently collapse.
#[test]
fn trade_frames_scale_the_floor_with_bar_width_and_accept_same_second_trades() {
    let (entry, exit, _, _) = replay_bounds(MINUTE_MS);
    let coarse = trade_frame(entry, exit, 2 * MINUTE_MS).expect("coarse frame");
    assert_eq!(
        coarse,
        (entry - 2 * 60 * MINUTE_MS, exit + 2 * 60 * MINUTE_MS),
        "ninety two-minute bars must be capped at two hours of context"
    );
    let fine = trade_frame(entry, exit, 1_000).expect("fine frame");
    assert!(fine.1 - fine.0 < coarse.1 - coarse.0);
    let unknown = trade_frame(entry, exit, 0).expect("unknown-resolution frame");
    assert_eq!(
        unknown,
        (entry - 2 * 60 * MINUTE_MS, exit + 2 * 60 * MINUTE_MS)
    );
    assert_eq!(
        trade_frame(entry, entry, MINUTE_MS),
        Some((1_699_994_600_000, 1_700_005_400_000)),
        "a same-second trade uses 90 one-minute bars of context on each side"
    );
    assert_eq!(trade_frame(exit, entry, MINUTE_MS), None);
}

/// A fitted frame is the trade itself, and takes in only the neighbours within one trade-length.
///
/// Breakage: a neighbour beyond the reach stretching the frame would make the subject a sliver;
/// one within it left out would sit off-screen while its lines are drawn.
#[test]
fn a_fitted_frame_is_the_trade_plus_the_neighbours_within_reach() {
    let entry = ENTRY_S * 1_000;
    let close = entry + 10 * MINUTE_MS;
    let subject = trade_span(None, entry, close);
    assert_eq!(subject, (entry, close));
    // Alone: exactly the trade.
    assert_eq!(fit_frame(subject, []), Some((entry, close)));
    // Within reach on both sides: taken in. Beyond it: left out.
    let near_before = (entry - 8 * MINUTE_MS, entry - 2 * MINUTE_MS);
    let near_after = (close + 3 * MINUTE_MS, close + 9 * MINUTE_MS);
    let far = (close + 11 * MINUTE_MS, close + 20 * MINUTE_MS);
    assert_eq!(
        fit_frame(subject, [near_before, far, near_after]),
        Some((near_before.0, near_after.1))
    );
    // A neighbour touching the reach's edge counts; one starting one ms past it does not.
    let edge = (close + 10 * MINUTE_MS, close + 30 * MINUTE_MS);
    assert_eq!(fit_frame(subject, [edge]), Some((entry, edge.1)));
    let past = (close + 10 * MINUTE_MS + 1, close + 30 * MINUTE_MS);
    assert_eq!(fit_frame(subject, [past]), Some((entry, close)));
}

/// The span starts where the entry order was placed only when the archive says so and that
/// instant precedes the fill; otherwise at the fill.
#[test]
fn a_trade_span_starts_at_the_placement_when_the_archive_holds_it() {
    let fill = ENTRY_S * 1_000;
    let close = fill + MINUTE_MS;
    assert_eq!(
        trade_span(Some(fill - 3 * MINUTE_MS), fill, close),
        (fill - 3 * MINUTE_MS, close)
    );
    assert_eq!(
        trade_span(Some(fill + 5_000), fill, close),
        (fill, close),
        "a later stamp is noise"
    );
    assert_eq!(trade_span(None, fill, close), (fill, close));
    // A same-instant trade frames through the ordinary rule rather than as a point.
    assert!(fit_frame(trade_span(None, fill, fill), []).is_some_and(|(s, e)| e > s));
}

/// The entry-line start is the earliest own entry point; inherited and exit lines do not count.
#[test]
fn the_entry_placement_comes_from_the_own_entry_line_only() {
    use moon_core::feed::{ArchivedLineKind, ArchivedOrderTrace};
    let line = |own: bool, kind: ArchivedLineKind, points: Vec<(f64, f64)>| ArchivedOrderTrace {
        own,
        kind,
        stop_price: None,
        stop_time_ms: None,
        points,
    };
    let lines = [
        line(true, ArchivedLineKind::Exit, vec![(10.0, 1.0)]),
        line(false, ArchivedLineKind::Entry, vec![(20.0, 1.0)]),
        line(
            true,
            ArchivedLineKind::Entry,
            vec![(50.0, 1.0), (40.0, 1.0)],
        ),
    ];
    assert_eq!(entry_set_ms(&lines), Some(40));
    assert_eq!(entry_set_ms(&lines[..2]), None);
    assert_eq!(
        fit_price_range([1.5, f32::NAN, 0.0, 2.5, 0.5]),
        Some((0.5, 2.5))
    );
    assert_eq!(fit_price_range([]), None);
}
