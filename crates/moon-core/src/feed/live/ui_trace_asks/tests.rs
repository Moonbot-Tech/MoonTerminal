use super::*;

/// Background replies must stay off the UI channel and window replies must be consumed once.
#[test]
fn delivers_only_asked_uids_once() {
    let mut asks = UiTraceAsks::default();
    assert!(!asks.deliver(2));
    asks.ask(1);
    assert!(!asks.deliver(2));
    assert!(asks.deliver(1));
    assert!(!asks.deliver(1));
}

/// Resetting bypass when tracked asks drain would drop late overflow replies awaited by the UI.
#[test]
fn overflow_delivers_late_untracked_answers_after_drain() {
    let mut asks = UiTraceAsks::default();
    for uid in 0..1024 {
        asks.ask(uid);
    }
    assert!(!asks.deliver(-1));
    asks.ask(1024);
    assert!(asks.deliver(-1));
    for uid in 0..1024 {
        assert!(asks.deliver(uid));
    }
    assert!(asks.deliver(1024));
    assert!(asks.deliver(-1));
}
