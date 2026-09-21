//! Pure-decision tests for the startup `RLIMIT_NOFILE` raise.
//!
//! The syscall path is Unix-only and is not exercised on this Windows builder.

use super::{desired_soft, request_hard};

/// A 256-file soft limit against a higher hard limit is the macOS GUI default
/// this change exists to lift. Returning 256 would leave overlapping report
/// readers able to exhaust the budget and surface `SQLITE_CANTOPEN`.
#[test]
fn raises_soft_to_hard_when_hard_is_higher() {
    assert_eq!(desired_soft(256, 10_240), 10_240);
}

/// A process already at its hard limit must keep that value. Returning a
/// higher number would make `setrlimit` fail; returning a lower one would
/// shrink the budget.
#[test]
fn keeps_soft_when_already_equal_to_hard() {
    assert_eq!(desired_soft(4_096, 4_096), 4_096);
}

/// A soft limit already above the hard limit is unusual but must not be
/// lowered: shrinking the descriptor budget at startup is worse than leaving
/// the pair alone, and the kernel would reject a raise past hard.
#[test]
fn keeps_soft_when_already_above_hard() {
    assert_eq!(desired_soft(8_192, 4_096), 8_192);
}

/// Darwin `OPEN_MAX` is 10240. Passing an unbounded hard limit through
/// [`request_hard`] on macOS must produce that ceiling, otherwise
/// `setrlimit` returns `EINVAL` and the raise never happens.
#[test]
fn macos_caps_unbounded_hard_limit_at_open_max() {
    assert_eq!(request_hard(u64::MAX, true), 10_240);
    assert_eq!(desired_soft(256, request_hard(u64::MAX, true)), 10_240);
}

/// Other Unix platforms accept the reported hard limit, including an
/// unbounded one. Capping them would leave descriptors on the table.
#[test]
fn other_unix_keeps_the_reported_hard_limit() {
    assert_eq!(request_hard(u64::MAX, false), u64::MAX);
    assert_eq!(desired_soft(256, request_hard(u64::MAX, false)), u64::MAX);
}

/// A macOS process whose soft limit is already above `OPEN_MAX` must keep it.
/// Folding the ceiling in as the hard argument reuses the soft-above-hard
/// rule so the raise never shrinks the budget.
#[test]
fn macos_does_not_lower_a_soft_limit_already_above_open_max() {
    assert_eq!(desired_soft(20_000, request_hard(u64::MAX, true)), 20_000);
}

/// Deleting the `raise_to_hard_limit` call, or moving it below the portable
/// data migration, would open stores against the inherited 256-file budget.
#[test]
fn run_raises_the_unix_limit_before_opening_stores() {
    let source = include_str!("../../startup.rs");
    let raise = source
        .find("open_file_limit::raise_to_hard_limit()")
        .expect("run must raise RLIMIT_NOFILE on Unix");
    let stores = source
        .find("moon_core::config::paths::migrate_bundle_data()")
        .expect("run must still migrate portable data");
    assert!(
        raise < stores,
        "the rlimit raise must run before startup opens or migrates on-disk stores"
    );
}
