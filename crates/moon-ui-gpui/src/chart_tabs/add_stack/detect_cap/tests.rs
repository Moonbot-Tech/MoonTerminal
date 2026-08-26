//! Regression coverage for the detect cap: who gets in, and who gives way when nobody can.

// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::{Admission, admit, resolved_max_charts, resolved_max_charts_evict};

/// A live slot that may be evicted, carrying how stale it is, as `admit_detect` collects it.
fn ttl(ix: usize, stale_ms: f64) -> (usize, Option<f64>) {
    (ix, Some(stale_ms))
}

/// A live slot that may not be evicted: pinned, or opened by hand. Both reach `admit` as `None`.
fn held(ix: usize) -> (usize, Option<f64>) {
    (ix, None)
}

#[test]
fn no_cap_accepts_everything() {
    let full = [ttl(0, 1.0), ttl(1, 2.0), ttl(2, 3.0)];
    assert_eq!(admit(None, false, &full), Admission::Accept);
    assert_eq!(admit(None, true, &full), Admission::Accept);
}

/// `detect_cap.rs` must not restore a zero default cap, a zero fallback, or disabled default
/// eviction; otherwise an unconfigured DETECT storm either opens unbounded charts or drops the
/// next chart instead of replacing the stalest one.
#[test]
fn an_unconfigured_tab_evicts_the_stalest_slot_at_the_default_boundary() {
    let full = [
        ttl(0, 800.0),
        ttl(1, 700.0),
        ttl(2, 600.0),
        ttl(3, 500.0),
        ttl(4, 400.0),
        ttl(5, 300.0),
        ttl(6, 200.0),
        ttl(7, 100.0),
    ];

    assert_eq!(
        admit(
            Some(resolved_max_charts(None)),
            resolved_max_charts_evict(None),
            &full,
        ),
        Admission::Evict(7)
    );
}

/// `detect_cap.rs:resolved_max_charts` must preserve `Some(0)` instead of filtering it into the
/// default; otherwise a reader who chose "0 - no limit" is silently re-capped despite the popup
/// promise.
#[test]
fn an_explicit_zero_stays_uncapped_while_none_and_positive_caps_stay_distinct() {
    let at_default_cap = [
        ttl(0, 800.0),
        ttl(1, 700.0),
        ttl(2, 600.0),
        ttl(3, 500.0),
        ttl(4, 400.0),
        ttl(5, 300.0),
        ttl(6, 200.0),
        ttl(7, 100.0),
    ];
    let beyond_default = [
        ttl(0, 900.0),
        ttl(1, 800.0),
        ttl(2, 700.0),
        ttl(3, 600.0),
        ttl(4, 500.0),
        ttl(5, 400.0),
        ttl(6, 300.0),
        ttl(7, 200.0),
        ttl(8, 100.0),
    ];

    assert_eq!(
        admit(Some(resolved_max_charts(None)), false, &at_default_cap),
        Admission::Drop
    );
    assert_eq!(
        admit(Some(resolved_max_charts(Some(0))), false, &beyond_default),
        Admission::Accept
    );
    assert_eq!(
        admit(
            Some(resolved_max_charts(Some(3))),
            false,
            &at_default_cap[..3]
        ),
        Admission::Drop
    );
}

/// Zero is what an emptied field reads as, and it must mean "no cap" rather than "show nothing".
#[test]
fn a_zero_cap_is_no_cap() {
    assert_eq!(admit(Some(0), false, &[ttl(0, 1.0)]), Admission::Accept);
}

#[test]
fn below_the_cap_accepts() {
    assert_eq!(admit(Some(3), false, &[ttl(0, 1.0)]), Admission::Accept);
    assert_eq!(
        admit(Some(3), false, &[ttl(0, 1.0), ttl(1, 2.0)]),
        Admission::Accept
    );
}

#[test]
fn at_the_cap_without_eviction_drops() {
    let full = [ttl(0, 1.0), ttl(1, 2.0)];
    assert_eq!(admit(Some(2), false, &full), Admission::Drop);
}

/// The victim is the stalest chart: the one that has gone longest without a fresh detect.
#[test]
fn eviction_takes_the_stalest_chart() {
    let full = [ttl(0, 900.0), ttl(1, 100.0), ttl(2, 500.0)];
    assert_eq!(admit(Some(3), true, &full), Admission::Evict(1));
}

/// A pin — or a hand-opened chart — is the user saying this one stays, so the cap may not overrule
/// it. It still holds its place on screen, so it still counts toward the cap.
#[test]
fn a_held_chart_counts_but_is_never_evicted() {
    let full = [held(0), ttl(1, 700.0), held(2)];
    assert_eq!(admit(Some(3), true, &full), Admission::Evict(1));
    assert_eq!(admit(Some(4), true, &full), Admission::Accept);
}

/// Nothing left to give way: eviction degrades to dropping rather than closing a chart the user
/// pinned or opened by hand. Without this a cap of one plus a hand-typed coin would mean the detect
/// feed closes what the reader just opened, on every single detect.
#[test]
fn everything_held_at_the_cap_drops_even_with_eviction_on() {
    let full = [held(0), held(1)];
    assert_eq!(admit(Some(2), true, &full), Admission::Drop);
    assert_eq!(admit(Some(1), true, &[held(0)]), Admission::Drop);
}

/// Charts already open past a cap that was just lowered do not make the rule misbehave: the next
/// detect evicts one instead of adding to the excess.
#[test]
fn over_the_cap_still_resolves() {
    let over = [ttl(0, 300.0), ttl(1, 100.0), ttl(2, 200.0)];
    assert_eq!(admit(Some(2), true, &over), Admission::Evict(1));
    assert_eq!(admit(Some(2), false, &over), Admission::Drop);
}
