use std::time::Duration;

use super::*;

/// Regression target: a reconciliation that keeps an override past its TTL, or past the core's
/// agreement, leaves a stale Stop Panic label, where a click recomputes from the snapshot and arms
/// panic sell.
#[test]
fn an_override_settles_on_expiry_or_on_the_cores_agreement() {
    assert!(
        panic_local_settled(true, PANIC_LOCAL_TTL, false),
        "an expired override must stop outranking a disagreeing snapshot"
    );
    assert!(
        panic_local_settled(false, Duration::ZERO, false),
        "a matching snapshot settles an override before its TTL"
    );
    assert!(
        !panic_local_settled(true, Duration::ZERO, false),
        "a fresh disagreement must retain the optimistic override"
    );
}

/// Regression target: changing `effective_panic_armed` to union a fresh local disarm with an
/// armed snapshot keeps Stop Panic visible and invites a re-press that re-arms panic sell while
/// the trader believes it is off.
#[test]
fn fresh_panic_disarm_override_precedes_an_armed_snapshot() {
    assert!(
        !effective_panic_armed(Some((false, Duration::ZERO)), || true),
        "a fresh local disarm must outrank an armed core snapshot"
    );
}

/// Regression target: changing `effective_panic_armed` to reject a fresh local arm when the core
/// has not echoed it yet makes Panic Sell look inactive after an accepted command and encourages
/// an unsafe repeat press.
#[test]
fn fresh_panic_arm_override_precedes_a_disarmed_snapshot() {
    assert!(
        effective_panic_armed(Some((true, Duration::ZERO)), || false),
        "a fresh local arm must outrank a disarmed core snapshot"
    );
}

/// A stale override no longer decides: the snapshot does, and it is read only then.
#[test]
fn a_stale_override_defers_to_the_snapshot() {
    assert!(effective_panic_armed(
        Some((false, PANIC_LOCAL_TTL)),
        || true
    ));
    assert!(!effective_panic_armed(None, || false));
}
