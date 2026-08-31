//! Regression tests for the recurring updater schedule and lifecycle authority.

use std::{fs, time::Duration};

use moon_core::config::paths;
use moon_core::update::DiscoveryRetry;

use super::{
    POLL_WINDOW_SECONDS, PollSchedule, STARTUP_POLL_GAP_SECONDS, UpdateState, claim_polling,
    failure_backoff, next_regular_poll, polling_continues_after, read_helper_failure,
    record_helper_failure, same_installed_target,
};

/// Removing the five-minute startup gap would issue a duplicate request when an immediate scan
/// lands just before this process's UTC phase; delaying by more than one window would exceed the
/// promised 20-minute startup-edge discovery bound. The same phase is a lower bound after a
/// transient failure, so local recovery cannot pull a request ahead of the regular cadence, but a
/// 30-minute transient backoff may skip the next 15-minute slot.
#[test]
fn startup_scan_keeps_a_minimum_gap_without_missing_two_windows() {
    let attempt = 3_600;
    let completed = attempt + 1;
    let deadline = next_regular_poll(completed, attempt, 100);
    assert_eq!(deadline, 4_600);
    assert!(deadline >= attempt + STARTUP_POLL_GAP_SECONDS);
    assert!(deadline - completed <= POLL_WINDOW_SECONDS + STARTUP_POLL_GAP_SECONDS);

    let mut schedule = PollSchedule::new(100);
    assert_eq!(
        schedule.after_failure_hint(3_601, DiscoveryRetry::Transient, None),
        5_401
    );
}

/// Replacing the explicit UTC phase with a fixed sleep from completion would drift forever when
/// GitHub is slow. These fixed instants independently name the next quarter-hour phase, including
/// :15 / :45 so a regression to an 1800-second window is visible.
#[test]
fn regular_polling_stays_on_its_utc_phase() {
    assert_eq!(next_regular_poll(3_700, 3_400, 600), 4_200);
    assert_eq!(next_regular_poll(5_999, 5_600, 600), 6_000);
    assert_eq!(next_regular_poll(6_000, 5_600, 600), 6_900);
    assert_eq!(next_regular_poll(1, 0, 0), 900);
    assert_eq!(next_regular_poll(900, 0, 0), 1_800);
    assert_eq!(next_regular_poll(1_800, 1_500, 0), 2_700);
    assert_eq!(next_regular_poll(2_700, 2_400, 0), 3_600);
}

/// Removing the idempotent claim would let a second startup path create a parallel discovery loop
/// and double every process's shared-IP GitHub traffic.
#[test]
fn polling_start_can_be_claimed_exactly_once() {
    let mut started = false;
    assert!(claim_polling(&mut started));
    assert!(!claim_polling(&mut started));
    assert!(started);
}

/// Shortening or unbounding the retry tables would either hammer GitHub after failures or leave a
/// recovered terminal asleep indefinitely. Literal policy durations are the independent oracle.
#[test]
fn failure_backoff_grows_to_the_documented_caps() {
    let hours = |hours: u64| Duration::from_secs(hours * 60 * 60);
    assert_eq!(failure_backoff(DiscoveryRetry::RateLimited, 1), hours(1));
    assert_eq!(failure_backoff(DiscoveryRetry::RateLimited, 2), hours(2));
    assert_eq!(failure_backoff(DiscoveryRetry::RateLimited, 3), hours(4));
    assert_eq!(failure_backoff(DiscoveryRetry::RateLimited, 4), hours(8));
    assert_eq!(failure_backoff(DiscoveryRetry::RateLimited, 20), hours(8));
    assert_eq!(
        failure_backoff(DiscoveryRetry::Transient, 1),
        Duration::from_secs(30 * 60)
    );
    assert_eq!(failure_backoff(DiscoveryRetry::Transient, 2), hours(1));
    assert_eq!(failure_backoff(DiscoveryRetry::Transient, 3), hours(2));
    assert_eq!(failure_backoff(DiscoveryRetry::Transient, 4), hours(4));
    assert_eq!(failure_backoff(DiscoveryRetry::Transient, 20), hours(4));
    assert_eq!(failure_backoff(DiscoveryRetry::Protocol, 1), hours(8));
}

/// Ignoring a later server deadline would retry while GitHub still forbids it, while failing to
/// reset on success would keep a recovered connection on an hours-long local backoff.
#[test]
fn server_deadlines_win_and_success_resets_the_failure_streak() {
    let mut schedule = PollSchedule::new(0);
    assert_eq!(
        schedule.after_failure_hint(1_000, DiscoveryRetry::RateLimited, Some(10_000)),
        10_060
    );
    assert_eq!(
        schedule.after_failure_hint(20_000, DiscoveryRetry::RateLimited, None),
        27_200
    );
    let success = schedule.after_success(30_000, 29_900, None);
    assert!(success <= 31_800);
    assert_eq!(
        schedule.after_failure_hint(40_000, DiscoveryRetry::RateLimited, None),
        43_600
    );
}

/// Stopping after a successful `Current` result would recreate the original bug: a release
/// published later would remain invisible until restart. Success must always produce a future
/// anchored deadline, independent of whether that complete scan found a candidate.
#[test]
fn a_successful_current_scan_schedules_the_next_attempt() {
    let mut schedule = PollSchedule::new(300);
    let completed = 10_000;
    assert!(polling_continues_after(&UpdateState::Hidden));
    let deadline = schedule.after_success(completed, completed - 2, None);
    assert!(deadline > completed);
    assert!(deadline - completed <= POLL_WINDOW_SECONDS + STARTUP_POLL_GAP_SECONDS);
}

/// Removing `update.rs:record_helper_failure` or its parent-side read would leave a failed helper
/// opaque to the user instead of surfacing its bounded diagnostic; a missing file must retain the
/// existing opaque readiness error.
#[test]
fn helper_failure_reason_round_trips_and_absence_stays_opaque() {
    let nonce = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
    let missing_nonce = "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2";
    let transaction = paths::update_transaction_paths(nonce).expect("derive transaction paths");
    let transaction_dir = transaction
        .manifest
        .parent()
        .expect("transaction manifest has a parent");
    fs::create_dir_all(transaction_dir).expect("create nonce-bound transaction directory");
    fs::write(&transaction.manifest, b"{}").expect("create canonical transaction manifest");

    record_helper_failure(
        &transaction.manifest,
        nonce,
        &anyhow::anyhow!("replacement helper could not open the installed executable"),
    );

    assert_eq!(
        read_helper_failure(nonce).as_deref(),
        Some("replacement helper could not open the installed executable")
    );
    assert_eq!(read_helper_failure(missing_nonce), None);

    fs::remove_file(&transaction.reason).expect("remove helper failure diagnostic");
    fs::remove_file(&transaction.manifest).expect("remove transaction manifest");
    fs::remove_dir(transaction_dir).expect("remove empty transaction directory");
}

/// Requiring `update.rs:same_installed_target` to retain the literal `MoonTerminal.exe` name
/// rejects a legitimate renamed portable install even though the manifest still binds it to the
/// derived install root.
#[test]
fn same_installed_target_accepts_a_renamed_install_name() {
    let canonical = std::path::Path::new(r"C:\install\MoonTerminal.exe");
    let renamed = std::path::Path::new(r"C:\install\MoonTerminalX.exe");

    assert!(same_installed_target(renamed, canonical));
}

/// Removing `update.rs:same_installed_target`'s parent-directory comparison accepts a manifest
/// target outside its derived install root, allowing the updater to validate an unrelated
/// executable.
#[test]
fn same_installed_target_rejects_a_target_outside_the_derived_install_root() {
    let canonical = std::path::Path::new(r"C:\install\MoonTerminal.exe");
    let outside = std::path::Path::new(r"C:\other\MoonTerminal.exe");

    assert!(!same_installed_target(outside, canonical));
}
