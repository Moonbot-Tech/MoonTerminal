//! Synthetic pairing regressions independent of any user's report population.

use super::*;

/// Generic report facts with distinct identities and equal settings (control pairs stay).
fn side(core: u64, uid: i64, buy_ms: i64) -> Side {
    Side {
        core,
        uid,
        buy_ms,
        venue: "venue".into(),
        coin: "synthetic".into(),
        short: false,
        kind: "MoonShot".into(),
        values: HashMap::new(),
    }
}

/// Removing any identity guard must admit an unrelated trade and fail this table.
#[test]
fn rejects_each_identity_mismatch() {
    let a = side(1, 1, 100);
    let mut cases = vec![
        side(1, 2, 100),
        side(2, 2, 100),
        side(2, 2, 100),
        side(2, 2, 100),
        side(2, 2, 100),
    ];
    cases[1].venue = "other".into();
    cases[2].coin = "other".into();
    cases[3].short = true;
    cases[4].kind = "EMA".into();
    for b in cases {
        assert!(match_pairs(&[a.clone(), b], 3).is_empty());
    }
}

/// An inclusive time boundary must retain the exact edge and reject one millisecond beyond it.
#[test]
fn window_is_inclusive_and_symmetric() {
    assert_eq!(
        match_pairs(&[side(1, 1, 100), side(2, 2, 103)], 3),
        vec![(0, 1), (1, 0)]
    );
    assert!(match_pairs(&[side(1, 1, 100), side(2, 2, 104)], 3).is_empty());
}

/// A B row keeps its nearest A per core; tie choice must not depend on input iteration order.
#[test]
fn keeps_nearest_per_core_and_deterministic_ties() {
    let sides = [
        side(1, 9, 90),
        side(1, 5, 99),
        side(1, 3, 101),
        side(2, 8, 100),
        side(3, 7, 98),
    ];
    let got = match_pairs(&sides, 20);
    let for_b: Vec<_> = got.into_iter().filter(|&(_, b)| b == 3).collect();
    assert_eq!(for_b, vec![(2, 3), (4, 3)]);
}

/// Non-knobs, hidden kind fields and known explicit defaults must not masquerade as treatments.
#[test]
fn effective_difference_respects_applicability_and_defaults() {
    let a = side(1, 1, 100);
    let mut b = side(2, 2, 100);
    let defaults = HashMap::from([("trailingpercent".to_owned(), 0.0)]);
    b.values.insert("Unrelated".into(), "changed".into());
    b.values.insert("HookSellLevel".into(), "3".into());
    b.values.insert("TrailingPercent".into(), "0.0".into());
    assert!(!differs(&a, &b, &defaults));
    b.values.insert("TrailingPercent".into(), "2".into());
    assert!(differs(&a, &b, &defaults));
    assert_eq!(match_pairs(&[a, b], 0), vec![(0, 1), (1, 0)]);
}
