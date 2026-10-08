use super::*;

/// A Unix instant in 2026, in milliseconds.
const T1_MS: i64 = 1_791_000_000_123;

/// A server reply to a request sent at `T1_MS`, from a server whose clock is `error_ms` ahead of
/// this machine, with `delay_ms` each way and 1 ms spent on the server.
fn reply(error_ms: i64, delay_ms: i64) -> ([u8; 48], [u8; 8], i64) {
    let sent = to_ntp(T1_MS);
    let t2 = T1_MS + delay_ms + error_ms;
    let t3 = t2 + 1;
    let t4 = T1_MS + 2 * delay_ms + 1;
    let mut r = [0u8; 48];
    r[0] = 0x24; // LI 0, version 4, mode 4 (server)
    r[1] = 2; // stratum 2
    r[24..32].copy_from_slice(&sent);
    r[32..40].copy_from_slice(&to_ntp(t2));
    r[40..48].copy_from_slice(&to_ntp(t3));
    (r, sent, t4)
}

/// Breakage guarded: the error's sign flipped or the delay left in it — a PC ten seconds behind
/// read as ten seconds ahead, which doubles the very error the correction exists to remove.
#[test]
fn the_error_is_true_minus_local_with_the_delay_cancelled() {
    for (error, delay) in [(10_000, 40), (-7_250, 120), (0, 300)] {
        let (r, sent, t4) = reply(error, delay);
        let got = parse(&r, &sent, T1_MS, t4).expect("a valid reply");
        assert!(
            (got - error).abs() <= 1,
            "error {error} ms with {delay} ms delay read as {got}"
        );
    }
}

/// Breakage guarded: a reply that is no clock adopted as one — a kiss-o'-death (stratum 0), an
/// unsynchronized server, a client-mode echo, or an answer to some other request — any of which
/// would move every core's offset by an arbitrary amount.
#[test]
fn a_reply_that_is_no_clock_is_refused() {
    let (good, sent, t4) = reply(5_000, 50);
    assert!(parse(&good, &sent, T1_MS, t4).is_some());

    let mut kod = good;
    kod[1] = 0;
    assert_eq!(parse(&kod, &sent, T1_MS, t4), None, "stratum 0");

    let mut unsynced = good;
    unsynced[0] = 0xE4; // LI 3
    assert_eq!(parse(&unsynced, &sent, T1_MS, t4), None, "leap indicator 3");

    let mut client = good;
    client[0] = 0x23; // mode 3
    assert_eq!(parse(&client, &sent, T1_MS, t4), None, "not a server reply");

    let other = to_ntp(T1_MS - 60_000);
    assert_eq!(
        parse(&good, &other, T1_MS, t4),
        None,
        "another request's echo"
    );
}

/// Breakage guarded: NTP timestamps converted with the wrong epoch or fraction — every error off
/// by seventy years or by up to a second — or the 2036 seconds wrap read as 1900.
#[test]
fn ntp_timestamps_round_trip_across_the_2036_wrap() {
    for unix_ms in [T1_MS, 2_085_978_496_000 + 1_500, 1_000] {
        assert_eq!(from_ntp(&to_ntp(unix_ms)), unix_ms, "{unix_ms}");
    }
}

/// Breakage guarded: a measurement kept across a step of this PC's clock or a sleep — the
/// monotonic clock stops during suspend on macOS and Linux — and applied to the new clock, which
/// moves every core's offset by the step until the next round. Ordinary scheduling jitter must
/// not throw a good measurement away.
#[test]
fn a_clock_step_or_a_sleep_ends_a_measurement() {
    assert!(holds(600_000, 600_000), "ten quiet minutes");
    assert!(holds(600_300, 600_000), "jitter under STEP_MS");
    assert!(!holds(590_000, 600_000), "clock set back 10 s");
    assert!(!holds(4_200_000, 600_000), "an hour asleep");
}

/// Breakage guarded: an answer taken over a slow path, whose error can be anything up to half
/// its round trip, adopted as this PC's clock error.
#[test]
fn a_slow_answer_is_refused() {
    let (fast, sent, t4) = reply(3_000, 400);
    assert!(
        parse(&fast, &sent, T1_MS, t4).is_some(),
        "800 ms round trip"
    );
    let (slow, sent, t4) = reply(3_000, 600);
    assert_eq!(parse(&slow, &sent, T1_MS, t4), None, "1.2 s round trip");
}
