use super::*;

/// Poll a Ready reader every five seconds for `secs` seconds from second `from_s`, each poll
/// reading `minutes`. Returns every outcome with the second it happened at.
fn run(
    clock: &mut PingClock,
    start: Instant,
    from_s: u64,
    secs: u64,
    minutes: i64,
) -> Vec<(u64, PingOffset)> {
    (from_s..from_s + secs)
        .step_by(5)
        .filter_map(|s| {
            clock
                .poll(start + Duration::from_secs(s), Some(minutes), true)
                .map(|o| (s, o))
        })
        .collect()
}

/// Breakage guarded (#931): a stable link whose fresh Pings all read the same value never
/// establishing the core's offset — a UTC+3 core with no stored segment left on the UTC axis,
/// every report date three hours off. Equal readings are usable from the first one.
#[test]
fn fresh_equal_readings_establish_offset() {
    let start = Instant::now();
    let mut clock = PingClock::new(start);
    clock.note_ready();
    assert_eq!(
        run(&mut clock, start, 0, 121, 180),
        vec![(0, PingOffset::Adopted(10_800))]
    );
}

/// Breakage guarded: a non-UTC core corrected by the wrong amount — the sign flipped, minutes
/// read as seconds, a half- or quarter-hour zone rounded to the hour — which moves every report
/// row of that core by hours; or a UTC core's `Some(0)` taken for "no reading" and left
/// unmeasured.
#[test]
fn a_core_adopts_its_own_zone_at_once() {
    for (minutes, secs) in [
        (180, 10_800),
        (-300, -18_000),
        (330, 19_800),
        (345, 20_700),
        (0, 0),
    ] {
        let start = Instant::now();
        let mut clock = PingClock::new(start);
        clock.note_ready();
        assert_eq!(
            run(&mut clock, start, 0, 60, minutes),
            vec![(0, PingOffset::Adopted(secs))],
            "UTC{minutes:+} min"
        );
    }
}

/// Breakage guarded: a daylight-saving switch or a timezone change on the core's machine left on
/// the old offset for the rest of the connection — every row closed after it lands an hour off.
/// Both directions are adopted at the next reading, without a reconnect.
#[test]
fn a_zone_change_on_the_running_connection_is_adopted() {
    for (before, after) in [(120, 180), (180, 120)] {
        let start = Instant::now();
        let mut clock = PingClock::new(start);
        clock.note_ready();
        run(&mut clock, start, 0, 60, before);
        assert_eq!(
            run(&mut clock, start, 60, 60, after),
            vec![(60, PingOffset::Adopted(after as i32 * 60))],
            "{before} -> {after}"
        );
    }
}

/// Breakage guarded: a reconnecting core whose zone did not move either re-storing a segment
/// for it (the writer then re-values nothing, but the status claims a new adoption) or showing a
/// stale status for the life of the connection, "Observed" frozen and the log as its source. The
/// first reading on each connection confirms once; nothing is stored for it.
#[test]
fn an_unchanged_offset_is_confirmed_once_per_connection() {
    let start = Instant::now();
    let mut clock = PingClock::new(start);
    clock.seed(Some(10_800));
    clock.note_ready();
    assert_eq!(
        run(&mut clock, start, 0, 120, 180),
        vec![(0, PingOffset::Confirmed(10_800))]
    );
    clock.note_ready();
    assert_eq!(
        run(&mut clock, start, 120, 120, 180),
        vec![(120, PingOffset::Confirmed(10_800))]
    );
}

/// Breakage guarded: a core whose stored offset came from the seconds-precise estimator
/// (`+03:00:40`) kept on it, the clock error the timezone reading deliberately leaves out still
/// applied to every new row. The zone differs from the stored value, so it is adopted.
#[test]
fn a_seconds_precise_stored_offset_gives_way_to_the_zone() {
    let start = Instant::now();
    let mut clock = PingClock::new(start);
    clock.seed(Some(10_840));
    clock.note_ready();
    assert_eq!(
        run(&mut clock, start, 0, 60, 180),
        vec![(0, PingOffset::Adopted(10_800))]
    );
}

/// Breakage guarded: `None` before the first Ping read as UTC and stored as a zero offset, a
/// reading taken before Ready confirming a connection that is not up, or the reader acting on
/// every loop pass instead of on its deadline.
#[test]
fn no_ping_not_ready_and_not_due_say_nothing() {
    let start = Instant::now();
    let mut clock = PingClock::new(start);
    for s in 0..60 {
        let now = start + Duration::from_secs(s);
        assert_eq!(clock.poll(now, None, true), None, "no Ping at {s} s");
    }

    let mut clock = PingClock::new(start);
    for s in 0..60 {
        let now = start + Duration::from_secs(s);
        assert_eq!(
            clock.poll(now, Some(180), false),
            None,
            "not Ready at {s} s"
        );
    }

    // Read at 0 s, then due again only at 5 s: the zone changing at 1 s is seen at 5 s.
    let mut clock = PingClock::new(start);
    clock.note_ready();
    let mut outcomes = Vec::new();
    for ms in (0..10_000u64).step_by(100) {
        let minutes = if ms < 1_000 { 120 } else { 180 };
        if let Some(o) = clock.poll(start + Duration::from_millis(ms), Some(minutes), true) {
            outcomes.push((ms, o));
        }
    }
    assert_eq!(
        outcomes,
        vec![
            (0, PingOffset::Adopted(7_200)),
            (5_000, PingOffset::Adopted(10_800))
        ]
    );
    assert_eq!(clock.wait(start + Duration::from_secs(5)), SAMPLE_EVERY);
}

/// Breakage guarded: a value outside the report axis's band stored as a segment the axis drops
/// on load, or a value near `i64::MAX` overflowing the conversion and panicking the feed thread.
#[test]
fn an_impossible_zone_is_dropped() {
    let start = Instant::now();
    for minutes in [15 * 60, -13 * 60, i64::MAX, i64::MIN] {
        let mut clock = PingClock::new(start);
        clock.note_ready();
        assert_eq!(
            clock.poll(start, Some(minutes), true),
            None,
            "{minutes} min"
        );
    }
}
