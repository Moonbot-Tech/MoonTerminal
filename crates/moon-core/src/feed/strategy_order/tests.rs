//! Unit tests for applying a desired sequence to a list that has moved on.

use super::resequence;

/// Runs one resequence over ids, ranking the ones the desired sequence names.
fn apply(items: &[u64], desired: &[u64]) -> (Vec<u64>, usize) {
    let mut items = items.to_vec();
    let moved = resequence(&mut items, |id| desired.iter().position(|want| want == id));
    (items, moved)
}

/// The ordinary case: every row is named, and the list comes out in exactly that sequence.
#[test]
fn a_fully_named_list_takes_the_desired_sequence() {
    assert_eq!(apply(&[1, 2, 3], &[3, 1, 2]), (vec![3, 1, 2], 3));
}

/// Nothing moved means nothing to send, and that zero is what a caller uses to stay off the wire.
#[test]
fn an_order_the_list_already_holds_moves_nothing() {
    assert_eq!(apply(&[1, 2, 3], &[1, 2, 3]), (vec![1, 2, 3], 0));
}

/// The point of the module. A strategy created after the sequence was chosen keeps the row it
/// follows instead of being flung to the end, where it would leave its folder split in two and be
/// sent back to the core as a deliberate arrangement by the next press.
#[test]
fn an_unnamed_row_keeps_the_row_it_follows() {
    assert_eq!(apply(&[1, 9, 2, 3], &[3, 1, 2]), (vec![3, 1, 9, 2], 4));
}

/// Several unnamed rows behind the same named one keep their own order behind it.
#[test]
fn unnamed_rows_behind_one_row_keep_their_relative_order() {
    assert_eq!(apply(&[1, 8, 9, 2], &[2, 1]), (vec![2, 1, 8, 9], 4));
}

/// A row that precedes every named one has nothing to follow, so it stays at the front — which is
/// also where it already is.
#[test]
fn a_row_before_every_named_one_stays_at_the_front() {
    assert_eq!(apply(&[7, 1, 2], &[2, 1]), (vec![7, 2, 1], 2));
}

/// Ids the sequence names but the list does not hold are simply absent; they must not create gaps,
/// duplicate anything, or drop a row.
#[test]
fn ids_the_list_no_longer_holds_are_ignored() {
    assert_eq!(apply(&[1, 3], &[3, 2, 1]).0, vec![3, 1]);
}

/// An empty desired sequence names nothing at all, so every row keeps its place.
#[test]
fn an_empty_sequence_leaves_the_list_alone() {
    assert_eq!(apply(&[1, 2, 3], &[]), (vec![1, 2, 3], 0));
}
