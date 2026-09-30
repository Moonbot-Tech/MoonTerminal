use super::*;

const MIN: i64 = 60_000;

/// A fresh open is an entry anywhere; a resend of a known trade changes nothing.
#[test]
fn a_fresh_open_is_recorded_everywhere() {
    for station in [false, true] {
        for known in [false, true] {
            assert_eq!(open_verdict(station, 0, known), OpenVerdict::Open);
            assert_eq!(
                open_verdict(station, FRESH_OPEN_MS, known),
                OpenVerdict::Open
            );
        }
    }
}

/// An old open is a resent row: the terminal skips it, a station takes up a trade it does not
/// know yet — one open across its own restart — and skips it once known or past the horizon.
#[test]
fn only_a_station_resumes_an_old_open_it_does_not_know() {
    let old = 30 * MIN;
    assert_eq!(open_verdict(false, old, false), OpenVerdict::Skip);
    assert_eq!(open_verdict(true, old, false), OpenVerdict::Resume);
    assert_eq!(open_verdict(true, old, true), OpenVerdict::Skip);
    assert_eq!(
        open_verdict(true, OPEN_HORIZON_MS, false),
        OpenVerdict::Resume
    );
    assert_eq!(
        open_verdict(true, OPEN_HORIZON_MS + 1, false),
        OpenVerdict::Skip
    );
}
