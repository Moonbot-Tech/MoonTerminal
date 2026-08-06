use super::ApiKeyExpiry;

/// Milliseconds in a day, for readable fixtures.
const DAY_MS: i64 = 86_400_000;

/// An answer with an absolute date `days` from `checked_ms`.
fn dated(days: i64, checked_ms: i64) -> ApiKeyExpiry {
    ApiKeyExpiry {
        unlimited: false,
        known: true,
        days_left: Some(days as i32),
        at_unix: Some((checked_ms + days * DAY_MS) / 1_000),
        checked_ms,
    }
}

/// A legacy answer: a day count with no absolute date behind it.
fn legacy(days: i32, checked_ms: i64) -> ApiKeyExpiry {
    ApiKeyExpiry {
        unlimited: false,
        known: true,
        days_left: Some(days),
        at_unix: None,
        checked_ms,
    }
}

/// The stored count is a SNAPSHOT. A core that goes down keeps its last answer forever, so reading
/// the field as-is would leave a key frozen at "10 days" while it quietly expires — the warning
/// would never fire and the panel would never stop lying.
#[test]
fn a_stored_count_ages_while_the_core_is_away() {
    let answered = dated(10, 1_000 * DAY_MS);

    assert_eq!(answered.days_left_at(1_000 * DAY_MS), Some(10));
    assert_eq!(
        answered.days_left_at(1_005 * DAY_MS),
        Some(5),
        "five days on"
    );
    assert_eq!(
        answered.days_left_at(1_010 * DAY_MS),
        Some(0),
        "its last day"
    );
    assert_eq!(
        answered.days_left_at(1_011 * DAY_MS),
        Some(-1),
        "a day past its date reads negative, which is what 'expired' means"
    );
}

/// A legacy answer carries no date, so its count is aged by whole elapsed days instead — same
/// outcome, one step coarser.
#[test]
fn a_legacy_count_ages_by_elapsed_days() {
    let answered = legacy(10, 1_000 * DAY_MS);

    assert_eq!(answered.days_left_at(1_000 * DAY_MS + 1), Some(10));
    assert_eq!(answered.days_left_at(1_004 * DAY_MS), Some(6));
    assert_eq!(answered.days_left_at(1_011 * DAY_MS), Some(-1));
}

/// Zero is "less than a day left", not "gone": both counts are whole days, so calling zero expired
/// would declare a working key dead up to a day early. Expiry begins strictly after the date, which
/// this pins on both sides of the boundary.
#[test]
fn the_last_day_is_not_yet_expired() {
    let answered = dated(1, 0);

    assert_eq!(answered.days_left_at(DAY_MS / 2), Some(0), "12 hours left");
    assert_eq!(
        answered.days_left_at(DAY_MS),
        Some(0),
        "exactly at the date is still the last day, not past it"
    );
    assert_eq!(
        answered.days_left_at(DAY_MS + 60_000),
        Some(-1),
        "a minute past it"
    );
}

/// A key with no expiration has no day count at any moment, and never reads as expired — the wire
/// puts a zero beside that answer, and letting it through would warn on every perpetual key.
#[test]
fn a_perpetual_key_never_ages_into_a_warning() {
    // What the converter builds for an unlimited key: the flag, and NO count — the zero the wire
    // carries beside it is deliberately not retained, so nothing downstream can age it into
    // "expired" and warn on a key that has nothing to expire.
    let perpetual = ApiKeyExpiry {
        unlimited: true,
        known: false,
        days_left: None,
        at_unix: None,
        checked_ms: 0,
    };

    assert_eq!(perpetual.days_left_at(0), None, "not zero days left");
    assert_eq!(
        perpetual.days_left_at(9_999 * DAY_MS),
        None,
        "and it never ages"
    );
}

/// MoonProto rebuilds the absolute date as `client_now + remaining`, so an unchanged key answers
/// with a date a few seconds apart on every poll. Comparing those exactly would report a change
/// every six hours; comparing them by day does not.
#[test]
fn a_re_answered_key_is_not_a_change() {
    let first = dated(30, 1_000 * DAY_MS);
    let same_key_later = ApiKeyExpiry {
        at_unix: first.at_unix.map(|at| at + 7),
        checked_ms: first.checked_ms + 6 * 3_600_000,
        ..first
    };

    assert!(first.answer_eq(&same_key_later));

    let replaced = ApiKeyExpiry {
        days_left: Some(365),
        at_unix: first.at_unix.map(|at| at + 335 * 86_400),
        ..first
    };
    assert!(!first.answer_eq(&replaced));
}
