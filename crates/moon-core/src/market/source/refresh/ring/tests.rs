use super::*;

const NOW_BUCKET: i64 = 1_800_000_000_000;

fn wire(stamp_ms: i64) -> WireRow {
    WireRow {
        stamp_ms,
        open: 12.0,
        high: 12.0,
        low: 8.0,
        close: 8.0,
        volume: 5.0,
    }
}

/// The whole ring on the first pass, after a reconnect (another epoch) and when older rows have
/// appeared; otherwise only the rows after the newest one stored.
///
/// Breakage: a cursor that skips the snapshot landing after the client sealed live rows (its
/// rows are older than the cursor), or a reconnect that keeps the old client's cursor and loses
/// the outage the new snapshot covers.
#[test]
fn a_pass_takes_the_whole_ring_only_when_it_may_hold_rows_not_stored() {
    let mark = RingMark {
        epoch: 7,
        oldest_ms: 1_000,
        newest_ms: 9_000,
    };
    assert_eq!(plan(None, 7, 1_000), RingPlan::Whole);
    assert_eq!(plan(Some(mark), 7, 1_000), RingPlan::After(9_000));
    // The ring rolled forward: still only the new rows.
    assert_eq!(plan(Some(mark), 7, 2_000), RingPlan::After(9_000));
    assert_eq!(plan(Some(mark), 8, 2_000), RingPlan::Whole);
    assert_eq!(plan(Some(mark), 7, 500), RingPlan::Whole);
    assert!(RingPlan::After(9_000).takes(9_001));
    assert!(!RingPlan::After(9_000).takes(9_000));
}

/// A ring row is stored opened one period before its END stamp, in the range-only form it came
/// in, with its volume split by the venue; a zero stamp and a bucket not sealed on this clock are
/// never stored.
///
/// By hand: stamped at `NOW_BUCKET` (the end of the bucket before the forming one), the row opens
/// at `NOW_BUCKET - 5 min`; stamped 5 min later it would open AT the forming bucket and is refused.
/// On a quote venue the 5.0 is the turnover; on a base venue the turnover is its OHLC4 estimate,
/// 5 * (12 + 12 + 8 + 8) / 4 = 50.
///
/// Breakage: the stamp taken as the open (every ring bar one bucket late), the range-only form
/// normalised away, the volume read as base on a quote venue, or a `MoonTime::ZERO` row stored at
/// 1970.
#[test]
fn a_ring_row_is_stored_at_its_open_in_its_range_form() {
    let c = ring_candle(wire(NOW_BUCKET), true, NOW_BUCKET).expect("sealed row");
    assert_eq!(c.t_open_ms, (NOW_BUCKET - 300_000) as f64);
    assert!(crate::market::candles::is_range_only(&c));
    assert_eq!(c.quote_volume, 5.0);
    let base = ring_candle(wire(NOW_BUCKET), false, NOW_BUCKET).expect("sealed row");
    assert_eq!((base.volume, base.quote_volume), (5.0, 50.0));
    assert_eq!(
        ring_candle(wire(NOW_BUCKET + 300_000), true, NOW_BUCKET),
        None
    );
    assert_eq!(ring_candle(wire(0), true, NOW_BUCKET), None);
    assert!(!usable_stamp(0, NOW_BUCKET));
}
