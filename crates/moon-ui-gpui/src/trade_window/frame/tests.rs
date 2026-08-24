use super::*;
use moon_core::market::trade_replay::replay_window;

const ENTRY_S: i64 = 1_700_000_000;
const MINUTE_MS: i64 = 60_000;

fn replay_bounds(duration_ms: i64) -> (i64, i64, i64, i64) {
    let exit_s = ENTRY_S + duration_ms / 1_000;
    let data = replay_window(ENTRY_S, exit_s).expect("valid replay window");
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
