//! The close-time index must answer exactly what a full `close_ms < cutoff` walk would.

use super::*;

/// A trade of core 1 with the given row and close stamp.
fn trade(rec_id: i64, close_ms: i64) -> CatchupTrade {
    CatchupTrade {
        id: TradeId { core: 1, rec_id },
        exchange: "ex".into(),
        market: "MKT".into(),
        venue: None,
        core: None,
        open_ms: close_ms - 10,
        close_ms,
        emulator: false,
        tunable: true,
    }
}

/// Ids `closed_before` yields, sorted for comparison.
fn before(known: &Known, cutoff: i64) -> Vec<i64> {
    let mut ids: Vec<_> = known.closed_before(cutoff).map(|t| t.id.rec_id).collect();
    ids.sort_unstable();
    ids
}

/// A close exactly at the cutoff is not before it, matching the strict `<` the expiry used.
#[test]
fn cutoff_is_strict() {
    let mut known = Known::default();
    known.insert(TradeId { core: 1, rec_id: 1 }, trade(1, 100));
    known.insert(TradeId { core: 1, rec_id: 2 }, trade(2, 200));
    assert_eq!(before(&known, 200), vec![1]);
    assert_eq!(before(&known, 201), vec![1, 2]);
    assert!(before(&known, i64::MIN).is_empty());
}

/// A relisted trade whose close moved is found by its new close only.
#[test]
fn replacing_moves_the_index_entry() {
    let mut known = Known::default();
    let id = TradeId { core: 1, rec_id: 7 };
    known.insert(id, trade(7, 100));
    let old = known.insert(id, trade(7, 500));
    assert_eq!(old.map(|t| t.close_ms), Some(100));
    assert!(before(&known, 200).is_empty());
    assert_eq!(before(&known, 501), vec![7]);
    assert_eq!(known.by_close.len(), known.len());
}

/// A forgotten trade leaves no index entry behind.
#[test]
fn removing_clears_the_index_entry() {
    let mut known = Known::default();
    let id = TradeId { core: 1, rec_id: 3 };
    known.insert(id, trade(3, 100));
    assert!(known.remove(&id).is_some());
    assert!(known.remove(&id).is_none());
    assert!(known.by_close.is_empty());
    assert!(before(&known, i64::MAX).is_empty());
}

/// Two cores' rows with one close stamp are both indexed.
#[test]
fn same_close_on_two_cores_keeps_both() {
    let mut known = Known::default();
    let mut other = trade(5, 100);
    other.id.core = 2;
    known.insert(TradeId { core: 1, rec_id: 5 }, trade(5, 100));
    known.insert(other.id, other);
    assert_eq!(known.closed_before(101).count(), 2);
}
