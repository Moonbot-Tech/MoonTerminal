//! Linger membership for chart markets and order-book subscriptions.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use super::reconcile_linger;

fn set(markets: &[&str]) -> HashSet<String> {
    markets.iter().map(|m| (*m).to_string()).collect()
}

/// A market that leaves and comes back inside the linger must not expire, or a 1-second reopen
/// would drop the subscription the linger exists to keep.
#[test]
fn reconcile_linger_cancels_a_pending_drop_when_the_market_is_wanted_again() {
    let mut wanted: HashMap<u64, HashSet<String>> = HashMap::from([(1, set(&["BTCUSDT"]))]);
    let now = Instant::now();
    let delay = Duration::from_secs(5);
    let mut pending = HashMap::new();

    let empty: HashMap<u64, HashSet<String>> = HashMap::new();
    let (inserted, expired) = reconcile_linger(&mut wanted, &mut pending, &empty, now, delay);
    assert!(inserted.is_empty());
    assert!(expired.is_empty());
    assert_eq!(pending.len(), 1);

    let again = HashMap::from([(1, set(&["BTCUSDT"]))]);
    let (inserted, expired) = reconcile_linger(
        &mut wanted,
        &mut pending,
        &again,
        now + Duration::from_secs(1),
        delay,
    );
    assert!(inserted.is_empty(), "already served, must not reset");
    assert!(expired.is_empty());
    assert!(pending.is_empty(), "re-open must cancel the linger clock");
    assert!(wanted.get(&1).is_some_and(|s| s.contains("BTCUSDT")));
}

/// Past the linger the market leaves `wanted`, which is what actually unsubscribes it.
#[test]
fn reconcile_linger_expires_only_after_the_delay() {
    let mut wanted: HashMap<u64, HashSet<String>> = HashMap::from([(1, set(&["ETHUSDT"]))]);
    let now = Instant::now();
    let delay = Duration::from_secs(5);
    let mut pending = HashMap::new();
    let empty: HashMap<u64, HashSet<String>> = HashMap::new();

    let (_, expired) = reconcile_linger(&mut wanted, &mut pending, &empty, now, delay);
    assert!(expired.is_empty());
    assert!(wanted.get(&1).is_some_and(|s| s.contains("ETHUSDT")));

    let (_, expired) = reconcile_linger(&mut wanted, &mut pending, &empty, now + delay, delay);
    assert_eq!(expired, vec![(1, "ETHUSDT".to_string())]);
    assert!(!wanted.get(&1).is_some_and(|s| s.contains("ETHUSDT")));
}

/// A brand-new market is the one `set_open` must reset, because retained history would otherwise
/// paint the previous visit's window until the next trade.
#[test]
fn reconcile_linger_reports_a_first_insert() {
    let mut wanted = HashMap::new();
    let mut pending = HashMap::new();
    let desired = HashMap::from([(7, set(&["SOLUSDT"]))]);
    let (inserted, expired) = reconcile_linger(
        &mut wanted,
        &mut pending,
        &desired,
        Instant::now(),
        Duration::from_secs(5),
    );
    assert_eq!(inserted, vec![(7, "SOLUSDT".to_string())]);
    assert!(expired.is_empty());
}
