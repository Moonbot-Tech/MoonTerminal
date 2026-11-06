//! The shared writer must wait for evidence, then release without replaying an uncertain import.

use super::*;
use moonproto::shared_config::SharedConfig;
use std::sync::mpsc;

/// Install a sent import with a real typed target and a deterministic observation deadline.
fn pending(
    deadline: Instant,
) -> (
    SharedConfigSequence,
    mpsc::Receiver<std::result::Result<Receipt, String>>,
) {
    let (reply, read) = mpsc::sync_channel(2);
    let mut sequence = SharedConfigSequence::new();
    sequence.automation = Some(PendingAutomation {
        expected: json!({"trading.x_sell":77}),
        reply,
        deadline,
        updated: false,
    });
    (sequence, read)
}

/// A matching retained value alone cannot acknowledge a write; a new full update is necessary.
#[test]
fn confirmation_requires_a_new_matching_full_snapshot() {
    let now = Instant::now();
    let (mut sequence, read) = pending(now + ECHO_TIMEOUT);
    let mut full = SharedConfig::default();
    full.trading.x_sell = 77;
    assert!(sequence.observe_automation(Some(&full), now));
    assert!(read.try_recv().is_err());
    sequence.observe_update();
    full.trading.x_sell = 55;
    assert!(sequence.observe_automation(Some(&full), now));
    assert!(read.try_recv().is_err());
    sequence.observe_update();
    full.trading.x_sell = 77;
    assert!(!sequence.observe_automation(Some(&full), now));
    assert_eq!(
        read.try_recv().unwrap().unwrap().value["status"],
        "confirmed"
    );
    assert!(!sequence.automation_pending());
    assert!(!sequence.observe_automation(Some(&full), now));
    assert!(read.try_recv().is_err());
}

/// Missing echoes end with an unknown result and release both settings channels without retry.
#[test]
fn timeout_and_disconnect_end_observation_once() {
    let now = Instant::now();
    let (mut sequence, read) = pending(now);
    assert!(!sequence.observe_automation(None, now));
    assert_eq!(read.try_recv().unwrap().unwrap().value["status"], "unknown");
    assert!(!sequence.automation_pending());
    sequence.observe_update();
    assert!(!sequence.observe_automation(None, now));
    assert!(read.try_recv().is_err());

    let (mut sequence, read) = pending(now + ECHO_TIMEOUT);
    sequence.prepare_reconnect();
    assert!(!sequence.automation_pending());
    assert_eq!(read.try_recv().unwrap().unwrap().value["status"], "unknown");
    sequence.prepare_reconnect();
    assert!(read.try_recv().is_err());
}
