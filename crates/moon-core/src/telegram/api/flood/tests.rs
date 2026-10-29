//! Flood deadlines are tested with an injected clock, never by sleeping.

use super::*;

/// The notification client's 429 must hold its reply peer, while an unrelated bot remains free.
#[test]
fn retry_after_is_shared_only_with_the_same_bot() {
    let sender = FloodWait::default();
    let reply = sender.clone();
    let other_bot = FloodWait::default();
    let now = Instant::now();
    sender.observe(&limited(7), now);
    assert_eq!(
        reply.remaining(now + Duration::from_secs(2)),
        Some(Duration::from_secs(5))
    );
    assert_eq!(other_bot.remaining(now), None);
    assert_eq!(reply.remaining(now + Duration::from_secs(7)), None);
}

/// A concurrent shorter flood response or an unrelated local failure must not release a longer wait.
#[test]
fn concurrent_results_cannot_shorten_the_deadline() {
    let sender = FloodWait::default();
    let reply = sender.clone();
    let now = Instant::now();
    sender.observe(&limited(10), now);
    reply.observe(&limited(1), now + Duration::from_secs(1));
    reply.observe(&ApiError::Transport, now + Duration::from_secs(2));
    assert_eq!(
        reply.remaining(now + Duration::from_secs(2)),
        Some(Duration::from_secs(8))
    );
    reply.observe(&limited(20), now + Duration::from_secs(3));
    assert_eq!(
        sender.remaining(now + Duration::from_secs(4)),
        Some(Duration::from_secs(19))
    );
}

/// Construct the typed Bot API flood result without credentials or HTTP requests.
fn limited(seconds: u32) -> ApiError {
    ApiError::Telegram {
        description: "Too Many Requests".into(),
        retry_after_secs: Some(seconds),
    }
}
