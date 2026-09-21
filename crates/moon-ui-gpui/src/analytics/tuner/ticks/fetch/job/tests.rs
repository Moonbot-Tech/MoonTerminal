// Explicit imports, never `use super::*`: the crate's views import `gpui::*`, whose own
// `test` shadows the built-in attribute and makes `#[test]` expand recursively.
use std::collections::HashSet;

use super::super::super::state::TapeStatus;
use super::{
    ClusterKey, MAX_CONTINUATIONS, MAX_IN_FLIGHT, continues, pick_cluster, pick_dispatchable,
    retry_wait, split_by_key,
};
use moon_core::market::trade_replay::TickStatus;

/// Only a gate refusal on a still-uncovered row is asked again, with the gate's own number;
/// a row the tiles covered anyway, and every other status, is final here (a venue's own
/// refusal gets its one retry in the thread, not in this rule).
#[test]
fn only_a_refused_uncovered_row_waits_the_gate_out() {
    let refused = TickStatus::RateLimited { retry_in_s: 27 };
    assert_eq!(retry_wait(refused, TapeStatus::Missing), Some(27));
    assert_eq!(retry_wait(refused, TapeStatus::Covered), None);
    assert_eq!(retry_wait(TickStatus::Failed, TapeStatus::Missing), None);
    assert_eq!(retry_wait(TickStatus::NoRoute, TapeStatus::Missing), None);
    assert_eq!(retry_wait(TickStatus::Served, TapeStatus::Missing), None);
}

/// One request per exchange key at a time, the oldest row first, and none past the ceiling:
/// the dispatcher skips the rows of a busy key for the next key's oldest row. A row the user is
/// looking at goes before its venue's older rows, but never onto a busy venue.
#[test]
fn the_dispatcher_takes_the_oldest_row_of_a_free_key_marked_rows_first() {
    // Queue order is newest-first; the end is the oldest.
    let keys = ["gate", "binance", "okx", "binance"];
    let plain =
        |busy: &HashSet<&str>, out| pick_dispatchable(keys.iter().map(|k| (*k, false)), busy, out);
    let none: HashSet<&str> = HashSet::new();
    assert_eq!(plain(&none, 0), Some(3), "nothing out: the oldest row goes");
    let binance_busy: HashSet<&str> = ["binance"].into_iter().collect();
    assert_eq!(
        plain(&binance_busy, 1),
        Some(2),
        "binance out: okx's oldest goes, binance's second row waits"
    );
    let all_busy: HashSet<&str> = ["binance", "okx", "gate"].into_iter().collect();
    assert_eq!(plain(&all_busy, 3), None);
    assert_eq!(
        plain(&none, MAX_IN_FLIGHT),
        None,
        "the ceiling holds whatever the keys"
    );
    // The newest binance row (index 1) and the gate row (index 0) are what the user sees.
    let marked = [
        ("gate", true),
        ("binance", true),
        ("okx", false),
        ("binance", false),
    ];
    assert_eq!(
        pick_dispatchable(marked.iter().copied(), &none, 0),
        Some(1),
        "the oldest MARKED row goes before the venue's older unmarked one"
    );
    assert_eq!(
        pick_dispatchable(marked.iter().copied(), &binance_busy, 1),
        Some(0),
        "binance busy: the marked gate row, not the unmarked okx one"
    );
    let gate_and_binance_busy: HashSet<&str> = ["binance", "gate"].into_iter().collect();
    assert_eq!(
        pick_dispatchable(marked.iter().copied(), &gate_and_binance_busy, 2),
        Some(2),
        "no marked row on a free venue: the oldest unmarked one goes"
    );
}

/// A row goes back for more tape only when the walk ran, stopped short of the focus, gained
/// on the previous walk, left the row missing, and has continuations left; a complete walk,
/// a walk without gain, a covered row, a refusal and the ceiling all end it.
#[test]
fn a_row_continues_while_a_short_walk_gains_tape() {
    let served = TickStatus::Served;
    assert!(continues(served, TapeStatus::Missing, true, 60_000, 0, 0));
    assert!(continues(
        served,
        TapeStatus::Missing,
        true,
        120_000,
        60_000,
        3
    ));
    assert!(continues(
        TickStatus::Streaming,
        TapeStatus::Missing,
        true,
        1,
        0,
        0
    ));
    assert!(
        !continues(served, TapeStatus::Missing, false, 60_000, 0, 0),
        "a walk that reached the whole focus has nothing more to fetch"
    );
    assert!(
        !continues(served, TapeStatus::Missing, true, 60_000, 60_000, 1),
        "no gain: the venue serves nothing for the stretch"
    );
    assert!(
        !continues(served, TapeStatus::Covered, true, 60_000, 0, 0),
        "a covered row is done whatever the walk did"
    );
    assert!(
        !continues(
            served,
            TapeStatus::Missing,
            true,
            60_000,
            0,
            MAX_CONTINUATIONS
        ),
        "the ceiling"
    );
    for status in [
        TickStatus::Failed,
        TickStatus::NoRoute,
        TickStatus::NoTrades,
        TickStatus::RateLimited { retry_in_s: 30 },
        TickStatus::OutOfRetention { retention_ms: 1 },
    ] {
        assert!(
            !continues(status, TapeStatus::Missing, true, 60_000, 0, 0),
            "{status:?} is the venue's word, not a budget stop"
        );
    }
}

/// A cluster is the seed plus every row of the same market whose margined window overlaps the
/// hull — through a bridge row — never another market or exchange, and never past
/// the long-position threshold from the first entry to the last exit.
#[test]
fn a_cluster_takes_the_overlapping_rows_of_one_market_within_a_long_position() {
    const SEC: i64 = 1_000;
    /// The threshold as this test hands it in: the default five minutes.
    const LONG_POSITION_MS: i64 = 5 * 60 * SEC;
    let key = |exchange_key, market, buy_ms, close_ms| ClusterKey {
        exchange_key,
        market,
        buy_ms,
        close_ms,
        margin_ms: 30 * SEC,
    };
    let base = 1_000_000 * SEC;
    let rows = [
        // 0: another market, same minute — never joins.
        key("binance", "BTCUSDT", base, base + 10 * SEC),
        // 1: the seed's market, 50 s after the seed's close: joins through the margins.
        key("binance", "AKEUSDT", base + 70 * SEC, base + 80 * SEC),
        // 2: joins only through row 1 (140 s after the seed, 50 after row 1).
        key("binance", "AKEUSDT", base + 130 * SEC, base + 140 * SEC),
        // 3: the seed.
        key("binance", "AKEUSDT", base, base + 20 * SEC),
        // 4: same market, but four minutes after row 2 — no overlap, stays.
        key("binance", "AKEUSDT", base + 400 * SEC, base + 410 * SEC),
        // 5: same market name on another exchange — never joins.
        key("gate", "AKEUSDT", base, base + 10 * SEC),
    ];
    const _: () = assert!(
        140 * SEC + 30 * SEC < LONG_POSITION_MS,
        "the cluster stays short"
    );
    assert_eq!(pick_cluster(&rows, 3, LONG_POSITION_MS), vec![1, 2, 3]);
    assert_eq!(
        pick_cluster(&rows, 0, LONG_POSITION_MS),
        vec![0],
        "a lone row is its own cluster"
    );
    // Overlapping rows whose hull would pass a long position's length: the hull stops growing.
    let long = [
        key(
            "okx",
            "ONE-USDT-SWAP",
            base,
            base + LONG_POSITION_MS - 30 * SEC,
        ),
        key(
            "okx",
            "ONE-USDT-SWAP",
            base + LONG_POSITION_MS - 20 * SEC,
            base + LONG_POSITION_MS + 60 * SEC,
        ),
    ];
    assert_eq!(pick_cluster(&long, 0, LONG_POSITION_MS), vec![0]);
    // A wider threshold takes the same two rows as one cluster.
    assert_eq!(pick_cluster(&long, 0, 2 * LONG_POSITION_MS), vec![0, 1]);
}

/// A deferral takes every queued row of the refused venue, in queue order, and leaves the
/// other venues' rows in theirs.
#[test]
fn a_deferral_takes_the_venues_rows_and_keeps_the_rest_in_order() {
    let rows = vec![
        (1, "binance"),
        (2, "gate"),
        (3, "binance"),
        (4, "okx"),
        (5, "gate"),
    ];
    let (same, other) = split_by_key(rows, "gate", |r| r.1);
    assert_eq!(same, vec![(2, "gate"), (5, "gate")]);
    assert_eq!(other, vec![(1, "binance"), (3, "binance"), (4, "okx")]);
    let (none, all) = split_by_key(vec![(1, "binance")], "", |r| r.1);
    assert!(none.is_empty(), "a row of no venue never joins a wait");
    assert_eq!(all, vec![(1, "binance")]);
}
