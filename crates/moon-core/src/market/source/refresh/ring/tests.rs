use super::*;

/// The open of the bucket still forming at the reference moment: 2027-01-15 08:00 UTC, on the
/// 5-minute grid.
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

/// The whole ring is taken only when it may hold a snapshot not stored yet: the first pass, another
/// client, a ring cleared and refilled (the old cursor clipped), or older rows than last time. A
/// ring that only gained live seals is not.
///
/// Breakage: a reconnect that keeps the old client's mark (the outage its snapshot covers is never
/// stored), a re-snapshot under the same client missed, or every live seal re-reading the ring.
#[test]
fn a_pass_takes_the_ring_only_when_it_holds_a_new_snapshot() {
    let mark = RingMark {
        epoch: 7,
        oldest_ms: 1_000,
        cursor: moonproto::state::SeqRingCursor::default(),
    };
    assert!(holds_new_snapshot(None, 7, 1_000, false));
    // Live seals since: the oldest stayed or rolled forward, the old cursor is inside the ring.
    assert!(!holds_new_snapshot(Some(mark), 7, 1_000, false));
    assert!(!holds_new_snapshot(Some(mark), 7, 2_000, false));
    // Another client.
    assert!(holds_new_snapshot(Some(mark), 8, 2_000, false));
    // Cleared and refilled.
    assert!(holds_new_snapshot(Some(mark), 7, 2_000, true));
    // A snapshot reaching further back.
    assert!(holds_new_snapshot(Some(mark), 7, 500, false));
}

/// A ring row is filed at the grid bucket its period opened in, whatever second it was sealed at.
///
/// By hand: sealed at 07:07:20, the period opened no later than 07:02:20, which floors to 07:00;
/// sealed exactly at 07:05:00 it opened at 07:00 as well; sealed at 07:12:25, the next row, it
/// files under 07:05 — never the bucket of the row before it.
///
/// Breakage: the open taken as `stamp - 5 min` unaligned (07:02:20, a bar between the real
/// candles' buckets that no reader can match to one), or rounded up into the next bucket.
#[test]
fn an_off_grid_stamp_is_filed_at_its_bucket() {
    let base = NOW_BUCKET - 3_600_000; // 07:00
    let at = |m: i64, s: i64| base + m * 60_000 + s * 1_000;
    let later = NOW_BUCKET + 3_600_000;
    assert_eq!(ring_open_ms(at(5, 0), later), Some(at(0, 0)));
    assert_eq!(ring_open_ms(at(7, 20), later), Some(at(0, 0)));
    assert_eq!(ring_open_ms(at(12, 25), later), Some(at(5, 0)));
    let c = ring_candle(wire(at(7, 20)), later).expect("sealed row");
    assert_eq!(c.t_open_ms, at(0, 0) as f64);
}

/// A sealed bucket is stored, the forming one and a zero stamp never are.
///
/// By hand: sealed at `NOW_BUCKET` the row opened at `NOW_BUCKET - 5 min`, a closed bucket;
/// sealed one period later it would open AT the forming bucket and is refused.
///
/// Breakage: a core clock running ahead planting a bar in the future, or a `MoonTime::ZERO` row
/// stored at 1970.
#[test]
fn only_sealed_buckets_are_stored() {
    let c = ring_candle(wire(NOW_BUCKET), NOW_BUCKET).expect("sealed row");
    assert_eq!(c.t_open_ms, (NOW_BUCKET - 300_000) as f64);
    assert_eq!(ring_candle(wire(NOW_BUCKET + 300_000), NOW_BUCKET), None);
    assert_eq!(ring_candle(wire(0), NOW_BUCKET), None);
}

/// A snapshot row's wire volume is the quote, on every venue; the base is its OHLC4 estimate.
///
/// By hand: the wire 5.0 is the quote and the base is 5 / OHLC4 = 5 / ((12 + 12 + 8 + 8) / 4) =
/// 0.5.
///
/// Breakage: the wire figure read as base coins, which made the stored quote 5 * 10 = 50 — the
/// price times too large, measured as a 225 000 B $ day for Bybit BTCUSDT on 2026-11-25.
#[test]
fn a_snapshot_rows_wire_volume_is_the_quote() {
    let row = ring_candle(wire(NOW_BUCKET), NOW_BUCKET).expect("row");
    assert_eq!((row.volume, row.quote_volume), (0.5, 5.0));
}

/// A row with a price that is not a positive finite number, or a volume that is negative or not
/// finite, is not stored at all.
///
/// Breakage: a NaN high passed through `normalize_ohlc` (whose `max` skips it) and stored as the
/// open, a zero low stored as a price, or a negative volume summed into a profile for 15 days.
#[test]
fn a_broken_ring_row_is_not_stored() {
    let broken = [
        WireRow {
            high: f32::NAN,
            ..wire(NOW_BUCKET)
        },
        WireRow {
            low: 0.0,
            ..wire(NOW_BUCKET)
        },
        WireRow {
            high: f32::INFINITY,
            ..wire(NOW_BUCKET)
        },
        WireRow {
            open: f32::NAN,
            ..wire(NOW_BUCKET)
        },
        WireRow {
            volume: -1.0,
            ..wire(NOW_BUCKET)
        },
        WireRow {
            volume: f32::NAN,
            ..wire(NOW_BUCKET)
        },
    ];
    for row in broken {
        assert_eq!(ring_candle(row, NOW_BUCKET), None, "{row:?}");
    }
    let zero_volume = WireRow {
        volume: 0.0,
        ..wire(NOW_BUCKET)
    };
    assert!(ring_candle(zero_volume, NOW_BUCKET).is_some());
}
