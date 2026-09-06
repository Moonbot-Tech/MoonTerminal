//! Applying a desired strategy sequence to a list that has moved on since it was chosen.
//!
//! A reorder names every strategy the operator could see when they pressed the button. By the time
//! it is applied — one frame later in the window, one round trip later in the core — the list can
//! hold strategies the sequence never named: one created, pasted or restored in between, or one
//! this side had simply not received yet.
//!
//! Where those unnamed rows land is not a detail. Sorting them to the END looks harmless and is
//! not: the core requires one folder's strategies to stay a contiguous group (moonproto
//! `docs/strats.md`, "Strategy Order"), so a new strategy flung to the tail leaves its folder split
//! in two, and the NEXT reorder sends that tail position back to the core as the deliberate
//! arrangement. One press then permanently moves a strategy nobody touched.
//!
//! So an unnamed row keeps the row it currently follows. That is the only placement that is a
//! no-op for it — it neither leaves its folder nor changes what it is adjacent to.

/// Rearrange `items` into the desired sequence, keeping rows the sequence never named in place.
///
/// Stable: rows sharing a position keep their current relative order.
///
/// Args:
///     items: The full current list, in its current order.
///     rank_of: Position of one item in the desired sequence, or `None` when it names no position.
///
/// Returns:
///     How many items ended up at a different index than they started at — zero when the list
///     already holds the desired arrangement, which callers use to send nothing at all.
pub fn resequence<T>(items: &mut Vec<T>, rank_of: impl Fn(&T) -> Option<usize>) -> usize {
    /// Where one row sorts: its desired position, and whether it is a named row, one trailing a
    /// named row, or one that precedes every named row.
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    struct Slot(usize, u8);

    let mut slots: Vec<Slot> = Vec::with_capacity(items.len());
    // Rank of the last named row seen, so an unnamed row can attach itself to it.
    let mut trailing: Option<usize> = None;
    for item in items.iter() {
        slots.push(match rank_of(item) {
            Some(rank) => {
                trailing = Some(rank);
                Slot(rank, 1)
            }
            // Directly after the named row it currently follows — or, before any named row has
            // been seen, ahead of the whole sequence, which is equally where it already sits.
            None => match trailing {
                Some(rank) => Slot(rank, 2),
                None => Slot(0, 0),
            },
        });
    }

    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&at| (slots[at], at));
    let moved = order
        .iter()
        .enumerate()
        .filter(|(now, was)| *now != **was)
        .count();
    if moved > 0 {
        // Moved, never cloned: `T` here is a whole `StrategySnapshot` with every field string it
        // carries, and a reorder that deep-copied the account's entire strategy set would spend
        // more on the copy than on the sync it exists to prepare. Taking each element out through
        // an `Option` is what lets the permutation be applied by index without `T: Clone`.
        let mut taken: Vec<Option<T>> = std::mem::take(items).into_iter().map(Some).collect();
        *items = order
            .into_iter()
            // `order` is a sorted `0..len`, so every slot is visited exactly once. Stated rather
            // than skipped: were that ever untrue, dropping the slot would quietly SHORTEN the
            // strategy list this becomes on the wire.
            .map(|at| taken[at].take().expect("resequence visits each index once"))
            .collect();
    }
    moved
}

#[cfg(test)]
mod tests;
