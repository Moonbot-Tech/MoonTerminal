//! Regression tests for native market-history backfill claims.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::*;

/// `history.rs:native_backfill_due` must keep an elapsed retry due while a just-claimed or
/// clock-earlier retry remains blocked; changing the due comparison would either suppress missing
/// history or multiply requests against the MoonBot core.
#[test]
fn native_backfill_due_obeys_elapsed_retry_and_clock_order() {
    let now = Instant::now();
    let claimed = NativeBackfillAttempt {
        last_attempt: now,
        delay_s: HISTORY_RETRY_MIN_S,
        attempts: 1,
    };

    assert!(native_backfill_due(None, now));
    assert!(!native_backfill_due(Some(&claimed), now));
    assert!(native_backfill_due(
        Some(&claimed),
        now + Duration::from_secs(HISTORY_RETRY_MIN_S as u64),
    ));
    assert!(!native_backfill_due(
        Some(&claimed),
        now - Duration::from_secs(1),
    ));
}

/// `history.rs:history_retry_next_delay_s` must retain the documented 30, 60, 120, 240, 480, 600
/// sequence; removing its floor or cap would respectively hammer or silently starve history
/// recovery after a transient core disconnect.
#[test]
fn history_retry_delay_stays_floored_and_capped() {
    let actual = [
        None,
        Some(30),
        Some(60),
        Some(120),
        Some(240),
        Some(480),
        Some(600),
    ]
    .map(history_retry_next_delay_s);

    assert_eq!(actual, [30, 60, 120, 240, 480, 600, 600]);
}

/// `history.rs:history_retry_next_delay_s` must normalize a recorded zero into the retry band and
/// resume at the doubled 30-second floor; restarting below that schedule would distort recovery.
#[test]
fn history_retry_delay_normalizes_zero_to_the_doubled_floor() {
    let delay_s = history_retry_next_delay_s(Some(0));

    assert!(
        (HISTORY_RETRY_MIN_S..=HISTORY_RETRY_MAX_S).contains(&delay_s)
            && delay_s == HISTORY_RETRY_MIN_S * 2,
        "a recorded zero must resume the doubled floor inside the retry band"
    );
}

/// `history.rs:history_retry_next_delay_s` must cap a u32::MAX prior delay without overflowing;
/// losing that guard would crash the terminal's retry path instead of safely preserving history.
#[test]
fn history_retry_delay_handles_u32_max_without_overflow() {
    assert_eq!(history_retry_next_delay_s(Some(u32::MAX)), 600);
}

/// `history.rs:NativeBackfillGate::claim` must spend exactly five claims before refusing a key;
/// removing the budget would keep an unfillable market consuming the core's exchange-request
/// allowance indefinitely.
#[test]
fn native_backfill_gate_spends_the_five_claim_budget() {
    let gate = NativeBackfillGate::default();
    let key = (7, "BTCUSDT".to_string(), 60);
    let mut now = Instant::now();

    for _ in 0..NATIVE_BACKFILL_MAX_ATTEMPTS {
        let delay_s = gate
            .claim(key.clone(), now)
            .expect("an unspent key must grant its next claim");
        now += Duration::from_secs(delay_s as u64);
    }

    assert_eq!(gate.claim(key, now), None);
}

/// `history.rs:NativeBackfillGate::{claim,forget_provider,retain_providers,clear}` must claim before
/// queueing and restore only dropped providers' budgets; delaying the claim or skipping a reset
/// would respectively duplicate a request across panels or leave a replacement core without history.
#[test]
fn native_backfill_gate_claims_once_and_scopes_lifecycle_resets() {
    let gate = NativeBackfillGate::default();
    let provider_a_key = (7, "BTCUSDT".to_string(), 60);
    let provider_b_key = (8, "ETHUSDT".to_string(), 60);
    let now = Instant::now();

    assert_eq!(gate.claim(provider_a_key.clone(), now), Some(30));
    assert_eq!(gate.claim(provider_a_key.clone(), now), None);
    assert_eq!(gate.claim(provider_b_key.clone(), now), Some(30));

    gate.forget_provider(7);
    assert_eq!(gate.claim(provider_a_key.clone(), now), Some(30));
    assert_eq!(gate.claim(provider_b_key.clone(), now), None);

    let mut keep = HashSet::new();
    keep.insert(7);
    gate.retain_providers(&keep);
    assert_eq!(gate.claim(provider_a_key.clone(), now), None);
    assert_eq!(gate.claim(provider_b_key.clone(), now), Some(30));

    gate.clear();
    assert_eq!(gate.claim(provider_a_key, now), Some(30));
    assert_eq!(gate.claim(provider_b_key, now), Some(30));
}
