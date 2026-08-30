use super::*;

/// `OffsetEstimator::observe` -- a reconnect replay burst that hands N historical log lines the
/// SAME fresh `recv_ms` (arriving well past [`QUARANTINE_MS`], so quarantine alone cannot be what
/// defends this test) must adopt NOTHING, even though a naive rule counting raw sample COUNT per
/// bucket -- rather than DISTINCT arrival count -- would see enough agreeing samples and adopt.
///
/// `samples()` is asserted alongside `adopted()` to prove the burst genuinely entered the window
/// (so this is not accidentally testing the quarantine gate instead) and was rejected once inside
/// it, by the distinct-arrival requirement alone.
#[test]
fn a_reconnect_backlog_replay_adopts_no_offset() {
    let mut est = OffsetEstimator::new();
    est.note_ready(0);
    // Past QUARANTINE_MS (15_000ms after ready_at), so these samples are not quarantine-blocked.
    let recv_ms = 20_000;

    for i in 0..4 {
        // Different historical core times, but close enough together to round into ONE bucket --
        // the replayed lines all originate from the same (real) clock-offset session.
        let core_time_ms = recv_ms + 3_600_000 + i * 1_000;
        assert_eq!(
            est.observe(core_time_ms, recv_ms),
            None,
            "a burst sharing one recv_ms must never adopt, sample {i}"
        );
    }

    assert_eq!(
        est.samples(),
        4,
        "the burst must have entered the window (proving quarantine did not block it) for the \
         distinct-arrival rejection to be the thing actually under test"
    );
    assert_eq!(
        est.adopted(),
        None,
        "a replay burst sharing one arrival instant must never be adopted as a real offset"
    );
}

/// `OffsetEstimator::observe` -- proves the defense above is not simply "never adopt": samples
/// that agree on the same clock offset AND genuinely arrive [`MIN_SPREAD_MS`] apart in real time
/// must still be adopted, exactly like a real UTC+1 core reporting consistently over time.
#[test]
fn distinct_arrivals_spanning_real_time_do_adopt() {
    let mut est = OffsetEstimator::new();
    // raw_ms = core_time_ms - recv_ms is held at exactly 3_600_000ms (1h) across every sample,
    // rounding to bucket 4 (4 * BUCKET_SECS = 3_600 seconds), a real and ordinary UTC+1 offset.
    const RAW_MS: i64 = 3_600_000;

    assert_eq!(
        est.observe(RAW_MS, 0),
        None,
        "only 1 distinct arrival so far"
    );
    assert_eq!(
        est.observe(RAW_MS + 25_000, 25_000),
        None,
        "only 2 distinct arrivals so far"
    );
    assert_eq!(
        est.observe(RAW_MS + 50_000, 50_000),
        Some(3_600),
        "3 distinct arrivals spanning 50_000ms (> MIN_SPREAD_MS) and agreeing on one bucket must \
         adopt exactly the offset that bucket represents"
    );
}

/// `OffsetEstimator::observe` -- returns `Some` ONLY when a sample causes the ADOPTED value to
/// change, never merely because a bucket re-qualifies with the same answer; and once the window
/// also holds enough distinct, sufficiently spread samples agreeing on a DIFFERENT offset, that
/// new offset must win and be reported exactly once.
#[test]
fn observe_returns_some_only_on_change() {
    let mut est = OffsetEstimator::new();
    const FIRST_RAW_MS: i64 = 3_600_000; // 1h -> bucket 4 -> 3_600s
    const SECOND_RAW_MS: i64 = 5_400_000; // 1.5h -> bucket 6 -> 5_400s

    assert_eq!(est.observe(FIRST_RAW_MS, 0), None);
    assert_eq!(est.observe(FIRST_RAW_MS + 25_000, 25_000), None);
    assert_eq!(
        est.observe(FIRST_RAW_MS + 50_000, 50_000),
        Some(3_600),
        "first adoption"
    );
    assert_eq!(
        est.observe(FIRST_RAW_MS + 60_000, 60_000),
        None,
        "a 4th sample merely re-confirming the SAME already-adopted offset must return None"
    );

    // A second, higher-offset bucket accumulates its own 3 distinct, sufficiently spread samples.
    // The first two do not yet make it the winner: the still-qualifying first bucket's lower raw
    // value stays the adopted one, so this must keep returning None.
    assert_eq!(
        est.observe(SECOND_RAW_MS + 100_000, 100_000),
        None,
        "the new bucket has only 1 distinct arrival so far"
    );
    assert_eq!(
        est.observe(SECOND_RAW_MS + 130_000, 130_000),
        None,
        "the new bucket has only 2 distinct arrivals so far; the old bucket still wins the max"
    );
    assert_eq!(
        est.observe(SECOND_RAW_MS + 160_000, 160_000),
        Some(5_400),
        "the new bucket now has 3 distinct arrivals spanning 60_000ms and its raw value is the \
         larger of the two qualifying buckets, so it must displace the previously adopted offset"
    );
}

/// `OffsetEstimator::observe` -- a candidate whose bucket resolves outside
/// [`MIN_OFFSET_SECS`]..=[`MAX_OFFSET_SECS`] is a broken clock, not a real time zone, and must be
/// refused even when the samples otherwise satisfy every agreement rule.
#[test]
fn offset_outside_the_clamp_band_is_refused() {
    let mut est = OffsetEstimator::new();
    // 15h -> bucket 60 -> 54_000s, past MAX_OFFSET_SECS (14h = 50_400s).
    const OUT_OF_BAND_RAW_MS: i64 = 15 * 3_600 * 1_000;

    assert_eq!(est.observe(OUT_OF_BAND_RAW_MS, 0), None);
    assert_eq!(est.observe(OUT_OF_BAND_RAW_MS + 25_000, 25_000), None);
    assert_eq!(
        est.observe(OUT_OF_BAND_RAW_MS + 50_000, 50_000),
        None,
        "3 distinct, well-spread samples agreeing on an out-of-band offset must still be refused"
    );
    assert_eq!(
        est.adopted(),
        None,
        "an out-of-band candidate must never become the adopted offset"
    );
}
