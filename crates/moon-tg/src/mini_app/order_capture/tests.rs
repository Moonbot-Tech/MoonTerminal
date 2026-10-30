//! Revision, lifetime and trace exclusion contracts for shared Mini App captures.

use super::*;

/// Reusing a mutable table, omitting a revision, or keeping a removed core leaks stale prices.
#[test]
fn captures_are_frozen_reused_and_invalidated_by_revision_and_removal() {
    let mut store = CoreStore::default();
    store.ensure(1);
    let mut row = super::super::tests::resting_order(100.0, 101.0);
    row.uid = 3;
    row.sell_trace = Some(Default::default());
    store
        .core_mut(1)
        .unwrap()
        .apply(moon_core::feed::FeedMsg::Orders(vec![row.clone()].into()));
    let listed = [(1, "CORE_A".into(), "EX-XX".into())];
    let mut cache = OrderCaptures::default();
    let old = cache.capture(&store, &listed);
    let same = cache.capture(&store, &listed);
    assert!(Arc::ptr_eq(&old[0].rows, &same[0].rows));
    assert!(old[0].rows[0].sell_trace.is_none());
    row.price = 102.0;
    store
        .core_mut(1)
        .unwrap()
        .apply(moon_core::feed::FeedMsg::Orders(vec![row.clone()].into()));
    let new = cache.capture(&store, &listed);
    assert!(!Arc::ptr_eq(&old[0].rows, &new[0].rows));
    assert_eq!(old[0].rows[0].price, 101.0);
    assert_eq!(new[0].rows[0].price, 102.0);
    store.remove(1);
    store.ensure(1);
    row.price = 103.0;
    store
        .core_mut(1)
        .unwrap()
        .apply(moon_core::feed::FeedMsg::Orders(vec![row].into()));
    let replaced = cache.capture(&store, &listed);
    assert_eq!(replaced[0].rows[0].price, 103.0);
    store.remove(1);
    assert!(cache.capture(&store, &listed).is_empty());
    assert!(cache.cores.is_empty());
}

/// Dropping a consumed field from the trace-free capture silently changes the displayed row.
#[test]
fn compact_capture_preserves_all_order_display_inputs() {
    for short in [false, true] {
        for filled in [false, true] {
            let mut row = super::super::tests::resting_order(100.0, 110.0);
            row.is_short = short;
            row.filled = filled;
            row.fill_pct = 30.0;
            row.remaining_size = 2.0;
            row.emulator = true;
            row.quote = "BTC".into();
            row.take_profit = Some(120.0);
            row.stop_loss = Some(90.0);
            let expected = super::super::dto::order_draft(
                7,
                "CORE_A".into(),
                "EX-XX".into(),
                &row,
                |_| Some(2.0),
                true,
            )
            .render();
            let copied = capture_row(&row);
            let actual = super::super::dto::order_draft(
                7,
                "CORE_A".into(),
                "EX-XX".into(),
                &copied,
                |_| Some(2.0),
                true,
            )
            .render();
            assert_eq!(actual, expected);
        }
    }
}

/// Removing and restoring a core can reuse its table counter while its prices have changed.
#[test]
fn orders_response_key_includes_removal_epoch() {
    let mut store = CoreStore::default();
    store.ensure(1);
    let row = super::super::tests::resting_order(100.0, 101.0);
    store
        .core_mut(1)
        .unwrap()
        .apply(moon_core::feed::FeedMsg::Orders(vec![row.clone()].into()));
    let listed = [(1, "CORE_A".into(), "EX-XX".into())];
    let old = super::super::reads::orders_inputs(&store, &listed, |_, _| Some(1.0), |_, _| false);
    store.remove(1);
    store.ensure(1);
    let mut changed = row;
    changed.price = 102.0;
    store
        .core_mut(1)
        .unwrap()
        .apply(moon_core::feed::FeedMsg::Orders(vec![changed].into()));
    let new = super::super::reads::orders_inputs(&store, &listed, |_, _| Some(1.0), |_, _| false);
    assert_eq!(old.cores[0].revision, new.cores[0].revision);
    assert_ne!(old, new);
}
