use super::*;

/// A UTC instant the synthetic runs start at, in milliseconds.
const T0_MS: i64 = 1_800_000_000_000;

/// Drive a Ready sampler for `secs` seconds from second `from_s`, one loop pass per second, with
/// the core's clock `offset_secs` east of UTC and a delay that wanders between 15 and 80 ms like
/// the 2026-10-08 shadow run — so every Ping reads a different delta. Returns every adoption with
/// the second it happened at.
fn run(
    clock: &mut PingClock,
    start: Instant,
    from_s: u64,
    secs: u64,
    offset_secs: i64,
) -> Vec<(u64, i32)> {
    let mut adopted = Vec::new();
    for s in from_s..from_s + secs {
        let now = start + Duration::from_secs(s);
        let recv_ms = T0_MS + s as i64 * 1000;
        let lag_ms = 15 + (s as i64 * 37) % 66;
        let delta = Some(offset_secs * 1000 - lag_ms);
        if let Some(PingOffset::Adopted(offset)) = clock.poll(now, recv_ms, delta, None, true) {
            adopted.push((s, offset));
        }
    }
    adopted
}

/// Breakage guarded: a non-UTC core corrected by the wrong amount — the sign flipped, the
/// timezone dropped, or a half- or quarter-hour zone rounded to the hour — which would move every
/// report row of that core by hours. UTC+3, UTC-5, UTC+5:30, UTC+5:45 and a UTC+3 clock forty
/// seconds fast must each adopt exactly their offset, once, after the quarantine.
#[test]
fn a_non_utc_core_adopts_its_own_zone() {
    for offset_secs in [10_800, -18_000, 19_800, 20_700, 10_840] {
        let start = Instant::now();
        let mut clock = PingClock::new(start);
        clock.note_ready(T0_MS);
        let adopted = run(&mut clock, start, 0, 120, offset_secs);
        assert_eq!(
            adopted.len(),
            1,
            "UTC{offset_secs:+}s adopts once: {adopted:?}"
        );
        let (at, offset) = adopted[0];
        assert_eq!(i64::from(offset), offset_secs);
        assert!(
            at >= 15,
            "nothing is adopted inside the quarantine (at {at} s)"
        );
    }
}

/// Breakage guarded: a daylight-saving switch or a timezone change on the core's machine left on
/// the old offset for the rest of the connection — every row closed after it lands an hour off.
/// Both directions must be adopted on the running connection, without a reconnect.
#[test]
fn a_zone_change_on_the_running_connection_is_adopted() {
    for (before, after) in [(7_200, 10_800), (10_800, 7_200)] {
        let start = Instant::now();
        let mut clock = PingClock::new(start);
        clock.note_ready(T0_MS);
        assert_eq!(
            run(&mut clock, start, 0, 120, before),
            vec![(35, before as i32)]
        );
        // The estimator keeps a 5-minute window, so going back waits for the old samples to age
        // out; ten minutes is enough either way.
        let changed = run(&mut clock, start, 120, 600, after);
        assert_eq!(
            changed
                .iter()
                .map(|&(_, o)| i64::from(o))
                .collect::<Vec<_>>(),
            vec![after],
            "{before} -> {after}"
        );
    }
}

/// Breakage guarded: this PC's own clock error folded into the core's offset — a PC twenty
/// seconds slow would make a UTC+3 core forty seconds fast read `+03:01:00` and move its report
/// rows twenty seconds off the exchange's trades. Corrected, the core is measured against true
/// UTC.
#[test]
fn this_pcs_clock_error_is_taken_out() {
    let start = Instant::now();
    let mut clock = PingClock::new(start);
    clock.note_ready(T0_MS);
    let mut adopted = Vec::new();
    for s in 0..120 {
        let now = start + Duration::from_secs(s);
        let lag_ms = 15 + (s as i64 * 37) % 66;
        // The PC is 20 s behind true UTC: the core reads 20 s further ahead of it.
        let delta = Some(10_840_000 + 20_000 - lag_ms);
        if let Some(PingOffset::Adopted(o)) =
            clock.poll(now, T0_MS + s as i64 * 1000, delta, Some(20_000), true)
        {
            adopted.push(o);
        }
    }
    assert_eq!(adopted, vec![10_840]);
}

/// Breakage guarded: a reconnecting core whose offset did not move showing a stale status for
/// the life of the connection — "Observed" frozen at the stored segment's start, zero samples,
/// the log as its source — because a re-confirmation stores nothing and so said nothing.
/// The first confirmation on each connection is reported once; nothing is stored for it.
#[test]
fn an_unchanged_offset_is_confirmed_once_per_connection() {
    let start = Instant::now();
    let mut clock = PingClock::new(start);
    clock.seed(Some(10_800));
    clock.note_ready(T0_MS);
    let mut outcomes = Vec::new();
    for s in 0..120 {
        let now = start + Duration::from_secs(s);
        let lag_ms = 15 + (s as i64 * 37) % 66;
        if let Some(o) = clock.poll(
            now,
            T0_MS + s as i64 * 1000,
            Some(10_800_000 - lag_ms),
            None,
            true,
        ) {
            outcomes.push(o);
        }
    }
    assert_eq!(outcomes, vec![PingOffset::Confirmed(10_800)]);

    // A reconnect confirms again.
    clock.note_ready(T0_MS + 120_000);
    let mut again = Vec::new();
    for s in 120..240 {
        let now = start + Duration::from_secs(s);
        let lag_ms = 15 + (s as i64 * 37) % 66;
        if let Some(o) = clock.poll(
            now,
            T0_MS + s as i64 * 1000,
            Some(10_800_000 - lag_ms),
            None,
            true,
        ) {
            again.push(o);
        }
    }
    assert_eq!(again, vec![PingOffset::Confirmed(10_800)]);
}

/// Breakage guarded: the Ping MoonProto holds through a reconnect re-read every 5 s and counted
/// as a new arrival each time, until one Ping alone clears the agreement and spread rules and
/// stores a segment nothing confirmed. Neither before Ready nor after it may a held value count
/// more than once.
#[test]
fn one_held_ping_is_one_sample() {
    let start = Instant::now();
    let mut clock = PingClock::new(start);
    // A reconnect: the getter keeps returning the last Ping of the old connection.
    for s in 0..120 {
        let now = start + Duration::from_secs(s);
        assert_eq!(
            clock.poll(now, T0_MS + s as i64 * 1000, Some(10_800_000), None, false),
            None
        );
    }
    assert_eq!(clock.samples(), 0, "nothing is fed before Ready");

    clock.note_ready(T0_MS + 120_000);
    // Ready, but the first new Ping is late: the held value is read for two more minutes. Its
    // first reading falls inside the quarantine and is consumed there, so it never counts.
    for s in 120..240 {
        let now = start + Duration::from_secs(s);
        assert_eq!(
            clock.poll(now, T0_MS + s as i64 * 1000, Some(10_800_000), None, true),
            None,
            "one Ping must not adopt (at {s} s)"
        );
    }
    assert_eq!(clock.samples(), 0, "the held Ping never counts");

    // The next genuine Ping moves the delta by its own delay and counts at once.
    let now = start + Duration::from_secs(240);
    clock.poll(now, T0_MS + 240_000, Some(10_800_000 - 37), None, true);
    assert_eq!(clock.samples(), 1, "a new Ping after the held one counts");
}

/// Breakage guarded: a wrong value adopted before the first Ping (`None` read as zero), or the
/// loop sampling on every pass — sixty times the window walks on a core whose loop wakes often.
#[test]
fn no_ping_and_not_due_take_no_sample() {
    let start = Instant::now();
    let mut clock = PingClock::new(start);
    for s in 0..120 {
        assert_eq!(
            clock.poll(
                start + Duration::from_secs(s),
                T0_MS + s as i64 * 1000,
                None,
                None,
                true
            ),
            None
        );
    }
    assert_eq!(clock.samples(), 0, "no Ping, no sample");

    let mut clock = PingClock::new(start);
    for ms in (0..10_000).step_by(100) {
        let now = start + Duration::from_millis(ms);
        // A fresh Ping every pass: the delay, and so the delta, moves by a millisecond each time.
        clock.poll(now, T0_MS + ms as i64, Some(-(ms as i64) / 100), None, true);
    }
    assert_eq!(
        clock.samples(),
        2,
        "one sample per SAMPLE_EVERY: at 0 s and at 5 s"
    );
    assert_eq!(clock.wait(start + Duration::from_secs(5)), SAMPLE_EVERY);
}

/// Breakage guarded: a delta near `i64::MAX` overflowing the core-clock sum and panicking the
/// feed thread instead of being dropped.
#[test]
fn an_overflowing_delta_is_dropped() {
    let start = Instant::now();
    let mut clock = PingClock::new(start);
    assert_eq!(clock.poll(start, T0_MS, Some(i64::MAX), None, true), None);
    assert_eq!(clock.samples(), 0);
}
