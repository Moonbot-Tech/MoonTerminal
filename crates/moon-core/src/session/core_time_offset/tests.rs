use super::*;

/// `OffsetEstimator::observe` -- a burst of samples sharing ONE `recv_ms` (arriving well past
/// [`QUARANTINE_MS`], so quarantine alone cannot be what defends this test) must adopt NOTHING,
/// even though a naive rule counting raw sample COUNT -- rather than DISTINCT arrival count --
/// would see enough agreeing samples and adopt.
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
        // Different core times, but within AGREE_MS of each other, so they agree on one offset.
        let core_time_ms = recv_ms + 3_600_000 + i * 500;
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
        "a burst sharing one arrival instant must never be adopted as a real offset"
    );
}

/// `OffsetEstimator::observe` -- proves the defense above is not simply "never adopt": samples
/// that agree on the same clock offset AND genuinely arrive [`MIN_SPREAD_MS`] apart in real time
/// must still be adopted, exactly like a real UTC+1 core reporting consistently over time.
#[test]
fn distinct_arrivals_spanning_real_time_do_adopt() {
    let mut est = OffsetEstimator::new();
    // raw_ms = core_time_ms - recv_ms is held at exactly 3_600_000ms (1h) across every sample,
    // a real and ordinary UTC+1 offset.
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
        "3 distinct arrivals spanning 50_000ms (> MIN_SPREAD_MS) and agreeing on one value must \
         adopt exactly that offset"
    );
}

/// `OffsetEstimator::observe` -- returns `Some` ONLY when a sample causes the ADOPTED value to
/// change, never merely because the window re-qualifies with the same answer; and once the window
/// also holds enough distinct, sufficiently spread samples agreeing on a DIFFERENT offset, that
/// new offset must win and be reported exactly once.
#[test]
fn observe_returns_some_only_on_change() {
    let mut est = OffsetEstimator::new();
    const FIRST_RAW_MS: i64 = 3_600_000; // 1h -> 3_600s
    const SECOND_RAW_MS: i64 = 5_400_000; // 1.5h -> 5_400s

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

    // A second, higher offset accumulates its own 3 distinct, sufficiently spread samples. The
    // first two do not yet make it the winner: the still-qualifying first value stays the adopted
    // one, so this must keep returning None.
    assert_eq!(
        est.observe(SECOND_RAW_MS + 100_000, 100_000),
        None,
        "the new offset has only 1 distinct arrival so far"
    );
    assert_eq!(
        est.observe(SECOND_RAW_MS + 130_000, 130_000),
        None,
        "the new offset has only 2 distinct arrivals so far; the old one still qualifies"
    );
    assert_eq!(
        est.observe(SECOND_RAW_MS + 160_000, 160_000),
        Some(5_400),
        "the new offset now has 3 distinct arrivals spanning 60_000ms and is the higher of the \
         two qualifying values, so it must displace the previously adopted offset"
    );
}

/// `OffsetEstimator::observe` -- a candidate that resolves outside
/// [`MIN_OFFSET_SECS`]..=[`MAX_OFFSET_SECS`] is a broken clock, not a real time zone, and must be
/// refused even when the samples otherwise satisfy every agreement rule.
#[test]
fn offset_outside_the_clamp_band_is_refused() {
    let mut est = OffsetEstimator::new();
    // 15h -> 54_000s, past MAX_OFFSET_SECS (14h = 50_400s).
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

/// Feed `raw_ms` minus a wandering 15–80 ms delay at receipts `from_ms`, `from_ms + 25 s`, ... —
/// distinct, well-spread arrivals. Returns every adoption.
fn feed(est: &mut OffsetEstimator, raw_ms: i64, from_ms: i64, count: i64) -> Vec<i32> {
    (0..count)
        .filter_map(|i| {
            let recv_ms = from_ms + i * 25_000;
            let lag_ms = 15 + (i * 37) % 66;
            est.observe(recv_ms + raw_ms - lag_ms, recv_ms)
        })
        .collect()
}

/// Breakage guarded: a clock that runs fast rounded away — to the hour or the quarter hour —
/// leaving every report row of that core off by exactly that much. A UTC+3 core forty seconds
/// fast is `+03:00:40`, and a UTC−5 core twelve seconds slow is `−05:00:12`.
#[test]
fn a_clock_off_by_seconds_is_adopted_to_the_second() {
    let mut est = OffsetEstimator::new();
    assert_eq!(feed(&mut est, 10_840_000, 0, 4), vec![10_840]);
    let mut est = OffsetEstimator::new();
    assert_eq!(feed(&mut est, -18_012_000, 0, 4), vec![-18_012]);
}

/// Breakage guarded: every reconnect, late Ping or second rounded the other way opening a new
/// segment — each one re-values the core's whole history — or, the other way, a corrected clock
/// or a zone change never adopted because the deadband swallowed it.
#[test]
fn only_a_change_past_the_deadband_is_adopted() {
    let mut est = OffsetEstimator::new();
    est.seed(Some(10_840));
    assert_eq!(
        feed(&mut est, 10_840_000, 0, 6),
        Vec::<i32>::new(),
        "the stored offset re-measured is no adoption"
    );
    // Past the window, so the earlier samples no longer compete.
    assert_eq!(
        feed(&mut est, 10_843_000, 400_000, 6),
        Vec::<i32>::new(),
        "three seconds is inside DEADBAND_SECS"
    );
    assert_eq!(
        feed(&mut est, 10_846_000, 800_000, 6),
        vec![10_846],
        "six seconds is a corrected clock"
    );
    assert_eq!(est.adopted(), Some(10_846));
}

/// Breakage guarded: a core never measured (nothing seeded) waiting for a change from a value it
/// never had, so its first offset is never adopted at all.
#[test]
fn an_unseeded_core_adopts_its_first_offset_whatever_it_is() {
    let mut est = OffsetEstimator::new();
    est.seed(None);
    // Seven seconds: past ZONE_SNAP_SECS, so the value itself rather than the zone.
    assert_eq!(feed(&mut est, 7_000, 0, 4), vec![7]);
}

/// Breakage guarded: one sample above the rest — a broken reading — taken as the best estimate
/// while nothing agrees with it, either adopting it or blocking the true offset beneath it for
/// as long as it stays in the window.
#[test]
fn a_lone_high_sample_does_not_block_the_offset_beneath_it() {
    let mut est = OffsetEstimator::new();
    assert_eq!(est.observe(3_700_000, 0), None);
    assert_eq!(feed(&mut est, 3_600_000, 1_000, 4), vec![3_600]);
}

/// Breakage guarded: delay read as clock error — an honest UTC core behind a slow link adopted
/// as `-1 s`, losing its identity axis and splitting the report query into a branch of its own —
/// or the snap swallowing a clock that is genuinely off. Within ZONE_SNAP_SECS of a whole zone the
/// zone wins; beyond it the seconds stay.
#[test]
fn a_value_near_a_whole_zone_is_that_zone() {
    // 600 ms of delay on a UTC core: rounds to -1 s, snaps to 0.
    let mut est = OffsetEstimator::new();
    assert_eq!(feed(&mut est, -600, 0, 4), vec![0]);
    // A UTC+5:45 core two seconds fast is UTC+5:45; three seconds fast is three seconds fast.
    let mut est = OffsetEstimator::new();
    assert_eq!(feed(&mut est, 20_702_000, 0, 4), vec![20_700]);
    let mut est = OffsetEstimator::new();
    assert_eq!(feed(&mut est, 20_703_000, 0, 4), vec![20_703]);
    // Forty seconds fast is forty seconds fast.
    let mut est = OffsetEstimator::new();
    assert_eq!(feed(&mut est, 10_840_000, 0, 4), vec![10_840]);
}
