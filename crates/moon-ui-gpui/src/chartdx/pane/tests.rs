use super::*;

/// A container holding one auto-added pane born at 1000 ms with the given TTL.
fn container_with_ttl(ttl_ms: f64) -> Container {
    let mut c = Container::new(ContainerKind::Main);
    c.push_auto(1, "BTCUSDT", 1_000.0, ttl_ms, 0.0);
    c
}

/// `KeepInChart = 0` gives an infinite TTL, and such a pane is never pruned — not even years later.
#[test]
fn infinite_ttl_pane_is_never_pruned() {
    let mut c = container_with_ttl(f64::INFINITY);
    let year_ms = 1_000.0 + 365.0 * 24.0 * 3600.0 * 1000.0;

    assert!(c.prune_ttl(year_ms).is_empty());
    assert!(c.pane.is_some());
}

/// ...and no close timer is armed for it either.
///
/// Breaks on: `next_ttl_deadline_ms` computing `born_ms + ttl_ms` without asking whether the TTL is
/// finite. That sum is infinite, and the caller turns a deadline into a sleep — leaving a task
/// parked for `u64::MAX` milliseconds for the life of the process.
#[test]
fn infinite_ttl_pane_has_no_deadline() {
    let c = container_with_ttl(f64::INFINITY);

    assert_eq!(c.next_ttl_deadline_ms(), None);
}

/// A finite TTL still closes its pane on the deadline, so the fix cannot have made every pane
/// permanent.
#[test]
fn finite_ttl_pane_still_expires() {
    let mut c = container_with_ttl(60_000.0);

    assert_eq!(c.next_ttl_deadline_ms(), Some(61_000.0));
    assert!(c.prune_ttl(60_999.0).is_empty());
    assert_eq!(c.prune_ttl(61_000.0), vec![(1, "BTCUSDT".to_string())]);
}

/// A pane held forever has no DEADLINE but must still have an eviction RANK.
///
/// Breaks on: `ChartStackEntry::eviction_rank_ms` reading the TTL deadline again. The per-tab chart
/// cap treats `None` as "this chart may never be evicted", so with `KeepInChart = 0` — the schema
/// default, and the case this whole change exists to serve — every chart on a tab would become
/// unevictable AND unprunable. The tab would fill to its cap and then silently stop showing any new
/// coin for the rest of the session.
#[test]
fn an_infinite_ttl_pane_still_has_an_eviction_rank() {
    let c = container_with_ttl(f64::INFINITY);

    assert_eq!(
        c.next_ttl_deadline_ms(),
        None,
        "nothing closes it on a timer"
    );
    assert_eq!(
        c.stalest_detect_ms(),
        Some(1_000.0),
        "but the cap can still rank it by its last detect"
    );
}

/// Pinning is what withholds a chart from the cap, and it must keep doing so for a permanent pane —
/// otherwise the pin control would protect nothing on exactly the tabs where charts never expire.
#[test]
fn a_pinned_pane_offers_no_eviction_rank() {
    let mut c = container_with_ttl(f64::INFINITY);
    c.toggle_pin(0);

    assert_eq!(c.stalest_detect_ms(), None);
}

/// A finite pane ranks by its last detect too, so the cap orders both kinds on one scale.
#[test]
fn a_finite_ttl_pane_ranks_by_its_last_detect() {
    let c = container_with_ttl(60_000.0);

    assert_eq!(c.stalest_detect_ms(), Some(1_000.0));
}
