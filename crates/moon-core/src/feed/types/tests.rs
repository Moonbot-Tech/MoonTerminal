use super::{ApiKeyExpiry, CoreStartupState, CoreStartupStatus, DetectRow};

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

/// A HIP-3 DEX the terminal has never seen must key, caption and group correctly with no code
/// change — a new one appears whenever a builder deploys it, and Hyperliquid already carries more
/// than a dozen.
///
/// Breakage: replacing the hash with something that ignores order or case (a byte sum, a
/// `to_lowercase` fold) collapses two DEXes onto one `ExchangeId`, which silently merges their
/// market universes onto one provider — the exact failure `with_dex` exists to prevent. An empty
/// name colliding with a real one would do the same to plain Hyperliquid futures.
#[test]
fn every_hip3_dex_name_keys_its_own_venue() {
    // Names as a core reports them: the protocol caps them at 15 bytes, and nothing promises they
    // are ASCII, lowercase, or unpadded.
    let names = [
        "xyz",
        "ventuals",
        "felix",
        "hyena",
        "kinetiq",
        "dreamcash",
        "sekai",
        "nunchi",
        "liquid",
        "onlyvibes",
        "aura",
        "paragon",
        "XYZ",
        "Xyz",
        "x y z",
        "стакан",
        "123456789012345",
    ];
    let mut ids: Vec<u32> = names
        .iter()
        .map(|name| super::ExchangeId::with_dex(13, name).dex)
        .collect();
    let distinct = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        distinct,
        "two DEX names must never share one discriminator"
    );

    // The plain venue is `dex = 0`, and no named DEX may land on it.
    assert_eq!(super::ExchangeId::with_dex(13, "").dex, 0);
    assert_eq!(super::ExchangeId::new(13), super::ExchangeId::with_dex(13, ""));
    assert!(
        !ids.contains(&0),
        "a named DEX must not key as the plain venue"
    );

    // Same DEX, different platform code: futures and spot stay separate venues.
    assert_ne!(
        super::ExchangeId::with_dex(13, "xyz"),
        super::ExchangeId::with_dex(12, "xyz")
    );
    // And the discriminator is stable across calls, which is what makes it a grouping key.
    assert_eq!(
        super::ExchangeId::with_dex(13, "ventuals"),
        super::ExchangeId::with_dex(13, "ventuals")
    );
}

/// Two snapshots in the SAME terminal phase compare equal even when every byte/block counter
/// differs — without this a config of 200 already-started cores would bump `startup_rev` forever.
///
/// Breakage: dropping the terminal short-circuit makes the full field-by-field comparison run
/// instead, so two `Ready` snapshots whose counters merely kept accumulating after settling would
/// compare unequal.
#[test]
fn two_snapshots_in_the_same_terminal_phase_compare_equal_regardless_of_counters() {
    let a = CoreStartupStatus {
        state: CoreStartupState::Ready,
        received_sliced_bytes: 1_000,
        received_sliced_blocks: 10,
        ..Default::default()
    };
    let b = CoreStartupStatus {
        state: CoreStartupState::Ready,
        received_sliced_bytes: 99_999,
        received_sliced_blocks: 500,
        ..Default::default()
    };
    assert!(a.progress_eq(&b));
}

/// `elapsed_ms` compares at WHOLE-SECOND resolution while the core is still starting.
///
/// Breakage: comparing `elapsed_ms` exactly instead of by whole second would bump the panel at
/// poll rate (every 500 ms) instead of once a second.
#[test]
fn elapsed_ms_in_the_same_second_counts_as_equal_progress() {
    let a = CoreStartupStatus {
        state: CoreStartupState::Connecting,
        elapsed_ms: 1_200,
        ..Default::default()
    };
    let b = CoreStartupStatus {
        state: CoreStartupState::Connecting,
        elapsed_ms: 1_800,
        ..Default::default()
    };
    assert!(a.progress_eq(&b));
}

/// Crossing a whole-second boundary still counts as changed progress — the resolution above must
/// not collapse into "elapsed never matters".
#[test]
fn elapsed_ms_crossing_a_second_counts_as_different_progress() {
    let a = CoreStartupStatus {
        state: CoreStartupState::Connecting,
        elapsed_ms: 1_900,
        ..Default::default()
    };
    let b = CoreStartupStatus {
        state: CoreStartupState::Connecting,
        elapsed_ms: 2_000,
        ..Default::default()
    };
    assert!(!a.progress_eq(&b));
}

/// A `current_step` change while non-terminal compares unequal — the panel must repaint when the
/// core moves from one startup step to the next.
///
/// Breakage: dropping `current_step` from the comparison would silently freeze the "step" line in
/// the hover while the core keeps advancing.
#[test]
fn a_current_step_change_counts_as_different_progress() {
    let a = CoreStartupStatus {
        state: CoreStartupState::Initializing,
        current_step: Some(super::CoreInitStep::BaseCheck),
        ..Default::default()
    };
    let b = CoreStartupStatus {
        state: CoreStartupState::Initializing,
        current_step: Some(super::CoreInitStep::AuthCheck),
        ..Default::default()
    };
    assert!(!a.progress_eq(&b));
}

/// Build a detect row carrying `keep_in_chart_secs`; the rest is filler this test never reads.
fn detect_row(keep_in_chart_secs: u32) -> DetectRow {
    DetectRow {
        seq: 1,
        market: "BTCUSDT".to_string(),
        time_ms: 0.0,
        sound_alert: false,
        keep_alert_secs: 60,
        add_to_chart: 1,
        keep_in_chart_secs,
        sound_name: None,
        is_alert: false,
        kind: 0,
        is_short: false,
        msg: String::new(),
        strat_name: String::new(),
    }
}

/// `KeepInChart = 0` is Moonbot's "keep it forever", NOT "close it in a moment".
///
/// Breaks on: any caller reading `keep_in_chart_secs` directly. Multiplying the field by 1000 makes
/// the chart live one millisecond; clamping it with `.max(1)` — which is what shipped — makes it
/// live one second. Both read as "the tab closed by itself the instant it opened".
#[test]
fn zero_keep_in_chart_means_forever() {
    assert_eq!(detect_row(0).keep_in_chart_ttl_ms(), f64::INFINITY);
}

/// Every other value is still plain seconds, so the fix cannot have made everything eternal.
#[test]
fn nonzero_keep_in_chart_is_seconds() {
    assert_eq!(detect_row(60).keep_in_chart_ttl_ms(), 60_000.0);
    assert_eq!(detect_row(1).keep_in_chart_ttl_ms(), 1_000.0);
}
