//! Exact-set and payload-shape regressions for report rec-id compression.

use super::*;
use std::collections::BTreeSet;

/// Expand an emitted payload back into the id set it actually addresses.
///
/// The oracle is deliberately independent of the folding logic: it walks each range end to end
/// rather than trusting the run boundaries the compressor chose.
fn addressed(ranges: &[ReportRecIdRange], singles: &[i64]) -> BTreeSet<i64> {
    let mut out: BTreeSet<i64> = singles.iter().copied().collect();
    for range in ranges {
        for id in range.from_rec_id..=range.to_rec_id {
            out.insert(id);
        }
    }
    out
}

/// Removing sort/dedup or widening a run would make the reconstructed address set differ.
#[test]
fn compression_covers_exactly_the_input_set() {
    // Unsorted, duplicated, and mixing an isolated id, a pair, a long run, and a run that ends
    // one short of the next block — the shapes a real strategy history produces.
    let mut input = vec![
        50, 12, 11, 10, 13, 14, 99, 30, 31, 12, 7, 200, 201, 202, 203, 50,
    ];
    let expected: BTreeSet<i64> = input.iter().copied().collect();

    let (ranges, singles) = compress_rec_ids(&mut input);

    assert_eq!(
        addressed(&ranges, &singles),
        expected,
        "the payload must address the input set exactly, never a superset"
    );
}

/// Widening a range across a gap would soft-delete an unrelated report row.
#[test]
fn a_gap_is_never_swallowed_by_a_range() {
    // 5 is absent; folding 1..=4 and 6..=9 into one range would soft-delete a foreign trade.
    let mut input = vec![1, 2, 3, 4, 6, 7, 8, 9];

    let (ranges, singles) = compress_rec_ids(&mut input);

    assert!(
        !addressed(&ranges, &singles).contains(&5),
        "an id the caller did not ask for must never be addressed"
    );
    assert_eq!(ranges.len(), 2, "each side of the gap is its own range");
}

/// Lowering `MIN_RUN` would spend a range on a pair without reducing the payload.
#[test]
fn singletons_and_pairs_stay_singles() {
    // A pair costs the same either way, so a range must only appear where it removes ids.
    let mut input = vec![4, 9, 10, 40, 41, 42];

    let (ranges, singles) = compress_rec_ids(&mut input);

    assert_eq!(
        ranges,
        vec![ReportRecIdRange::new(40, 42)],
        "only the three-long run earns a range"
    );
    assert_eq!(singles, vec![4, 9, 10]);
}

/// The normalization is what makes the folding work at all: the reader hands back whatever order
/// the scan produced, and two passes of the purge can re-offer the same id.
///
/// Both halves are load-bearing, and neither is visible in the addressed SET — an unsorted or
/// duplicated input still covers exactly its own ids, it just covers them as thousands of singles.
/// That is the whole cost this module exists to avoid: a strategy's history is one long consecutive
/// block, and unfolded it travels as one wire batch per trade.
#[test]
fn an_unsorted_duplicated_history_still_folds_into_one_range() {
    let mut input = vec![202, 200, 203, 201, 202];

    let (ranges, singles) = compress_rec_ids(&mut input);

    assert_eq!(
        ranges,
        vec![ReportRecIdRange::new(200, 203)],
        "a consecutive block must fold whatever order it arrived in"
    );
    assert!(
        singles.is_empty(),
        "nothing may be left over — including the repeated id: {singles:?}"
    );
}

/// An empty selection must remain a no-op instead of producing a synthetic range or single.
#[test]
fn an_empty_input_emits_nothing() {
    // Pairs with `set_report_rows_deleted`, which treats an empty payload as a no-op.
    let mut input: Vec<i64> = Vec::new();

    let (ranges, singles) = compress_rec_ids(&mut input);

    assert!(ranges.is_empty() && singles.is_empty());
}
