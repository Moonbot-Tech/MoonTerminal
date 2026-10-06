use super::*;

/// The log is process-wide and other tests file into the cache too, so this one works off the
/// revision it starts from and its own market names.
#[test]
fn a_market_filed_after_a_revision_is_reported_and_one_before_it_is_not() {
    let (start, _) = filed_since(u64::MAX);
    note("test-ex:filed", "OLDUSDT");
    let (mid, _) = filed_since(u64::MAX);
    note("test-ex:filed", "NEWUSDT");
    let (now, fresh) = filed_since(mid);
    assert!(mid > start && now > mid);
    assert!(fresh.contains(&("test-ex:filed".to_string(), "NEWUSDT".to_string())));
    assert!(!fresh.contains(&("test-ex:filed".to_string(), "OLDUSDT".to_string())));
    // A market written again moves to the later revision.
    note("test-ex:filed", "OLDUSDT");
    let (_, again) = filed_since(now);
    assert!(again.contains(&("test-ex:filed".to_string(), "OLDUSDT".to_string())));
}
