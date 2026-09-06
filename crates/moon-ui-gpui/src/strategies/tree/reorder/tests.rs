//! Unit tests for the unconfirmed-order overlay.
//!
//! Imports are explicit rather than `use super::*`: this module's ancestors re-export `gpui::*`,
//! whose `test` shadows the attribute and makes `#[test]` recurse (see `presentation/tests.rs`).

use super::PendingOrder;

/// The core has applied exactly what was sent, so the overlay has done its job and must go — a
/// confirmed overlay that stayed would keep outranking the core on every later change.
#[test]
fn an_echo_of_the_sent_sequence_confirms_it() {
    let pending = PendingOrder::new(vec![3, 1, 2]);
    assert!(pending.confirmed_by([3, 1, 2].into_iter()));
    assert!(!pending.confirmed_by([1, 2, 3].into_iter()));
}

/// A strategy created, deleted or restored between the press and the echo is not a disagreement
/// about order. Comparing the raw lists instead would leave the overlay open for its full window
/// after any unrelated create, and the tree would keep drawing a stale arrangement all that time.
#[test]
fn a_created_or_deleted_strategy_does_not_block_confirmation() {
    let pending = PendingOrder::new(vec![3, 1, 2]);
    // 9 was created after the press; the shared ids still read 3, 1, 2.
    assert!(pending.confirmed_by([3, 1, 9, 2].into_iter()));
    // 1 was deleted; what remains is still in the order that was asked for.
    assert!(pending.confirmed_by([3, 2].into_iter()));
    // ... but a genuine disagreement among the survivors is still one.
    assert!(!pending.confirmed_by([2, 3].into_iter()));
}

/// The sequence is what the tree cache hashes, and its one job there is to separate two orders of
/// the SAME ids — the case a second press before the core answers produces.
#[test]
fn the_sequence_is_exposed_in_order_for_the_cache() {
    assert_eq!(PendingOrder::new(vec![1, 3, 2]).ids(), &[1, 3, 2]);
    assert_ne!(
        PendingOrder::new(vec![1, 2, 3]).ids(),
        PendingOrder::new(vec![1, 3, 2]).ids()
    );
}

/// Rank is what places a row, and an id the sequence never named has none — such a row keeps the
/// row it follows instead of taking a position the operator never chose.
#[test]
fn an_unnamed_id_has_no_rank() {
    let pending = PendingOrder::new(vec![7, 4]);
    assert_eq!(pending.rank(7), Some(0));
    assert_eq!(pending.rank(4), Some(1));
    assert_eq!(pending.rank(5), None);
}
