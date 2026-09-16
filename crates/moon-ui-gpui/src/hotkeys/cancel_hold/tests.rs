//! Pure regression tests for held-key cancellation safeguards.

use std::time::Instant;

use super::{
    CancelHold, CancelRefusal, HOLD_TTL, HoldKey, PressKind, classify_cancel,
    is_cancellable_status, reports,
};

/// Creates a probeable hold for tests that need a live armed state.
fn armed_hold() -> (CancelHold<u32>, Instant) {
    let now = Instant::now();
    let mut hold = CancelHold::default();
    hold.press(Some(HoldKey::Tab), 7, false, now);
    (hold, now)
}

/// `hotkeys/cancel_hold.rs:CancelHold::poll` must clear on an up physical probe; returning true
/// for `Some(false)` would let a stray pointer move cancel a live order with no key pressed.
#[test]
fn a_key_that_is_physically_up_kills_the_hold() {
    let (mut hold, now) = armed_hold();
    assert!(!hold.poll(now, true, Some(false)));
    assert!(!hold.is_armed());
}

/// `hotkeys/cancel_hold.rs:CancelHold::poll` must clear an inactive arm; omitting that gate would
/// let a hold armed in another window sweep and cancel the chart currently under the pointer.
#[test]
fn an_inactive_window_kills_the_hold() {
    let (mut hold, now) = armed_hold();
    assert!(!hold.poll(now, false, Some(true)));
    assert!(!hold.is_armed());
}

/// `hotkeys/cancel_hold.rs:CancelHold::poll` must expire an unprobed arm after `HOLD_TTL`; using
/// a non-expiring fallback would leave a stale held key able to cancel a live entry later.
#[test]
fn without_a_probe_the_hold_expires_after_ttl() {
    let (mut hold, now) = armed_hold();
    assert!(hold.poll(now + HOLD_TTL, true, None));
    assert!(!hold.poll(
        now + HOLD_TTL + std::time::Duration::from_nanos(1),
        true,
        None
    ));
    assert!(!hold.is_armed());
}

/// `hotkeys/cancel_hold.rs:CancelHold::poll` must trust a down physical probe beyond `HOLD_TTL`;
/// applying the fallback expiry on Windows would stop a trader from sweeping a legitimately held key.
#[test]
fn a_probe_that_is_down_outlives_the_ttl() {
    let (mut hold, now) = armed_hold();
    assert!(hold.poll(
        now + HOLD_TTL + std::time::Duration::from_secs(1),
        true,
        Some(true)
    ));
    assert!(hold.is_armed());
}

/// `hotkeys/cancel_hold.rs:CancelHold::poll` must never activate a key without a physical probe;
/// treating an arbitrary user binding as live would make that unrelated key sweep cancellations.
#[test]
fn a_press_only_key_never_polls_live() {
    let now = Instant::now();
    let mut hold = CancelHold::default();
    hold.press(None, 7, false, now);
    assert!(!hold.poll(now, true, Some(true)));
    assert!(hold.is_armed());
}

/// `hotkeys/cancel_hold.rs:HoldKey::from_key_name` must recognize only Tab and Delete; accepting
/// arbitrary bindings would let an unrelated user key arm a sweep that cancels entry orders.
#[test]
fn only_probeable_hold_keys_are_recognized() {
    assert_eq!(HoldKey::from_key_name("tab"), Some(HoldKey::Tab));
    assert_eq!(HoldKey::from_key_name("delete"), Some(HoldKey::Delete));
    for key in ["x", "escape", "Tab"] {
        assert_eq!(HoldKey::from_key_name(key), None, "key={key}");
    }
}

/// `hotkeys/cancel_hold.rs:CancelHold::address` must deduplicate one target per arm; always
/// returning true would send one cancel command per pointer pixel until the core echoes back.
#[test]
fn an_order_is_addressed_once_per_hold() {
    let (mut hold, _) = armed_hold();
    assert!(hold.address((11, 42)));
    assert!(!hold.address((11, 42)));
    assert!(hold.address((11, 43)));
}

/// `hotkeys/cancel_hold.rs:CancelHold::press` must replace deduplication on a fresh press; keeping
/// the old set would prevent a trader from retrying an entry cancel after a new key press.
#[test]
fn a_fresh_press_starts_a_new_hold_with_an_empty_addressed_set() {
    let (mut hold, now) = armed_hold();
    assert!(hold.address((11, 42)));
    assert!(matches!(
        hold.press(Some(HoldKey::Tab), 7, false, now),
        PressKind::Fresh { .. }
    ));
    assert!(hold.address((11, 42)));
}

/// `hotkeys/cancel_hold.rs:CancelHold::press` must retain an equivalent repeat's addressed set;
/// clearing it on repeat would resend cancellation continuously while a key is held.
#[test]
fn a_repeat_on_the_same_key_and_window_extends_the_hold() {
    let (mut hold, now) = armed_hold();
    assert!(hold.address((11, 42)));
    assert_eq!(
        hold.press(Some(HoldKey::Tab), 7, true, now + HOLD_TTL),
        PressKind::Repeat
    );
    assert!(!hold.address((11, 42)));
    assert!(hold.poll(now + HOLD_TTL + HOLD_TTL, true, None));
}

/// `hotkeys/cancel_hold.rs:CancelHold::press` must report a cross-window OS repeat as `Repeat`
/// while starting a new arm; relabelling it Fresh would let a released key cancel a live order.
#[test]
fn a_repeat_on_a_different_window_starts_a_new_hold() {
    let (mut hold, now) = armed_hold();
    assert!(hold.address((11, 42)));
    assert_eq!(
        hold.press(Some(HoldKey::Tab), 8, true, now),
        PressKind::Repeat
    );
    assert_eq!(hold.armed_window(), Some(8));
    assert!(hold.address((11, 42)));
}

/// `hotkeys/cancel_hold.rs:CancelHold::press` must retain an OS repeat after its arm was cleared;
/// returning `Fresh { explicit: true }` for that queued repeat would cancel an entry after the
/// physical key was released.
#[test]
fn a_repeat_after_its_arm_was_cleared_is_not_a_fresh_press() {
    let (mut hold, now) = armed_hold();
    hold.clear();

    assert_eq!(
        hold.press(Some(HoldKey::Tab), 7, true, now),
        PressKind::Repeat
    );
}

/// `hotkeys/cancel_hold.rs:is_cancellable_status` must allow only None and BuySet; accepting
/// BuyDone would send a command MoonProto drops silently while the user sees no cancellation.
#[test]
fn only_none_and_buyset_are_cancellable() {
    let statuses = [
        ("None", true),
        ("BuyFail", false),
        ("BuySet", true),
        ("BuyCancel", false),
        ("BuyDone", false),
        ("SellFail", false),
        ("SellSet", false),
        ("SellCancel", false),
        ("SellDone", false),
        ("SellAlmostDone", false),
        ("Unknown", false),
    ];
    for (status, expected) in statuses {
        assert_eq!(is_cancellable_status(status), expected, "status={status}");
    }
}

/// `hotkeys/cancel_hold.rs:reports` must suppress empty repeat sweeps; reporting NoTarget on a
/// repeat would flood the Log panel while the user moves over empty chart space.
#[test]
fn no_target_is_reported_only_on_an_explicit_press() {
    assert!(reports(
        &CancelRefusal::NoTarget,
        PressKind::Fresh { explicit: true }
    ));
    assert!(!reports(
        &CancelRefusal::NoTarget,
        PressKind::Fresh { explicit: false }
    ));
    assert!(!reports(&CancelRefusal::NoTarget, PressKind::Repeat));
}

/// `hotkeys/cancel_hold.rs:classify_cancel` must check workspace authorization before status;
/// reversing the checks would hide why an unauthorized core's entry cancellation was refused.
#[test]
fn a_refused_core_outranks_the_status_check() {
    assert_eq!(
        classify_cancel(Some((11, 42)), false, Some("BuySet")),
        Err(CancelRefusal::CoreNotAllowed { core: 11, uid: 42 })
    );
    assert_eq!(
        classify_cancel(None, true, Some("BuySet")),
        Err(CancelRefusal::NoTarget)
    );
}

/// `hotkeys/cancel_hold.rs:reports` and `classify_cancel` must report each addressed refusal;
/// suppressing one would make a dropped entry cancellation invisible to the user.
#[test]
fn every_per_order_refusal_is_always_reported() {
    let refusals = [
        CancelRefusal::CoreNotAllowed { core: 11, uid: 42 },
        CancelRefusal::NoOrderRow { core: 11, uid: 42 },
        CancelRefusal::NotCancellable {
            core: 11,
            uid: 42,
            status: "BuyDone".to_string(),
        },
    ];
    for refusal in &refusals {
        assert!(reports(refusal, PressKind::Fresh { explicit: true }));
        assert!(reports(refusal, PressKind::Fresh { explicit: false }));
        assert!(reports(refusal, PressKind::Repeat));
    }
    assert_eq!(
        classify_cancel(Some((11, 42)), true, Some("BuyDone")),
        Err(CancelRefusal::NotCancellable {
            core: 11,
            uid: 42,
            status: "BuyDone".to_string(),
        })
    );
    assert_eq!(
        classify_cancel(Some((11, 42)), true, None),
        Err(CancelRefusal::NoOrderRow { core: 11, uid: 42 })
    );
    assert_eq!(
        classify_cancel(Some((11, 42)), true, Some("BuySet")),
        Ok((11, 42))
    );
}
