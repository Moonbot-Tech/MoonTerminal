//! The automatic update's decision, on synthetic versions and times only.

use super::*;

/// `vMAJOR.MINOR.PATCH` for the tests.
fn v(major: u64, minor: u64, patch: u64) -> ReleaseVersion {
    ReleaseVersion {
        major,
        minor,
        patch,
    }
}

const NOW: u64 = 1_800_000_000;

#[test]
fn a_newer_release_is_requested() {
    assert_eq!(
        decide(true, Some(v(0, 30, 0)), Some(v(0, 31, 0)), None, NOW),
        Decision::Update(v(0, 31, 0))
    );
}

/// An equal or older release is never installed over the running one: otherwise the station
/// would restart itself every few hours, or roll itself back.
#[test]
fn an_equal_or_older_release_is_not_requested() {
    assert_eq!(
        decide(true, Some(v(0, 31, 0)), Some(v(0, 31, 0)), None, NOW),
        Decision::Current
    );
    assert_eq!(
        decide(true, Some(v(0, 31, 0)), Some(v(0, 30, 2)), None, NOW),
        Decision::Current
    );
    assert_eq!(
        decide(true, Some(v(0, 31, 0)), None, None, NOW),
        Decision::Current
    );
}

#[test]
fn the_switch_off_or_a_dev_build_never_updates() {
    assert_eq!(
        decide(false, Some(v(0, 30, 0)), Some(v(0, 31, 0)), None, NOW),
        Decision::Off
    );
    assert_eq!(
        decide(true, None, Some(v(0, 31, 0)), None, NOW),
        Decision::Unversioned
    );
}

/// A version asked for less than a day ago is not asked for again: a failed update restarts the
/// old binary, and without this it would file the same request at every start.
#[test]
fn a_tried_version_waits_a_day() {
    let tried = Attempt {
        version: v(0, 31, 0),
        at_s: NOW - (RETRY_AFTER_S - 1),
    };
    assert_eq!(
        decide(true, Some(v(0, 30, 0)), Some(v(0, 31, 0)), Some(tried), NOW),
        Decision::Tried(v(0, 31, 0))
    );
    let day_old = Attempt {
        at_s: NOW - RETRY_AFTER_S,
        ..tried
    };
    assert_eq!(
        decide(
            true,
            Some(v(0, 30, 0)),
            Some(v(0, 31, 0)),
            Some(day_old),
            NOW
        ),
        Decision::Update(v(0, 31, 0))
    );
    // A still newer release is not held back by the attempt at an older one.
    assert_eq!(
        decide(true, Some(v(0, 30, 0)), Some(v(0, 32, 0)), Some(tried), NOW),
        Decision::Update(v(0, 32, 0))
    );
}

#[test]
fn the_attempt_file_reads_back_what_was_written() {
    let written = format!("{} {NOW}\n", v(0, 31, 0));
    assert_eq!(
        parse_attempt(&written),
        Some(Attempt {
            version: v(0, 31, 0),
            at_s: NOW
        })
    );
    assert_eq!(parse_attempt(""), None);
    assert_eq!(parse_attempt("garbage 12"), None);
}
