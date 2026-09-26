// Explicit imports, never `use super::*`: the crate's views import `gpui::*`, whose own
// `test` shadows the built-in attribute and makes `#[test]` expand recursively.
use std::collections::HashSet;

use super::super::super::state::TapeStatus;
use super::{
    ClusterKey, FlightSnap, KeptFlight, MAX_CONTINUATIONS, MAX_IN_FLIGHT, ReturnedRow, RowOrigin,
    TaggedRow, adopt_booked_row, adopt_origin, cancel_autoload_rows, continues, pick_cluster,
    pick_dispatchable, release_booked_uid, requeued_origin, retain_flight, retry_wait,
    returned_row, rows_leaving_total, rows_still_dropping, split_by_key,
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
    let key = |exchange_key, market, open_ms, close_ms| ClusterKey {
        exchange_key,
        market,
        open_ms,
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

fn tagged(uid: i64, origin: RowOrigin) -> TaggedRow {
    TaggedRow { uid, origin }
}

fn snap(id: u64, rows: Vec<TaggedRow>) -> FlightSnap {
    FlightSnap { id, rows }
}

/// Switching the autoload off drops its pending and deferred rows and cancels a walk that
/// serves only those rows. A user row in the same batch stays. A walk that already took a
/// user row keeps running — cancelling it would drop the user's ask — but the autoload rows
/// on that walk are dropped, so the return does not spend another walk on them.
///
/// Leaving those autoload rows unmarked in `dropped` turns this red: the switch-off then
/// keeps paying exchange weight for autoload continuations after the switch is off.
#[test]
fn switching_autoload_off_drops_only_autoload_rows() {
    let user = RowOrigin::User;
    let auto = RowOrigin::Autoload;
    let pending = [tagged(1, auto), tagged(2, user), tagged(3, auto)];
    let deferred = [tagged(4, auto), tagged(5, user)];
    let in_flight = [
        snap(1, vec![tagged(6, auto), tagged(7, auto)]),
        snap(2, vec![tagged(8, auto), tagged(9, user)]),
        snap(3, vec![tagged(10, user)]),
    ];
    let plan = cancel_autoload_rows(&pending, &deferred, &in_flight);
    assert_eq!(plan.pending, vec![tagged(2, user)]);
    assert_eq!(plan.deferred, vec![tagged(5, user)]);
    assert_eq!(
        plan.in_flight,
        vec![
            KeptFlight {
                id: 2,
                rows: vec![tagged(8, auto), tagged(9, user)],
                dropped: vec![8],
            },
            KeptFlight {
                id: 3,
                rows: vec![tagged(10, user)],
                dropped: vec![],
            },
        ]
    );
    assert_eq!(plan.unmark, vec![6, 7, 8]);
    assert_eq!(
        plan.removed, 6,
        "three queued, the two-row walk, and the mixed row"
    );
    let untouched =
        cancel_autoload_rows(&[tagged(2, user)], &[], &[snap(4, vec![tagged(10, user)])]);
    assert_eq!(untouched.removed, 0);
    assert!(untouched.unmark.is_empty());
    assert!(untouched.in_flight[0].dropped.is_empty());
}

/// A dropped autoload row is not deferred or continued when the walk comes back short.
/// A row the walk already covered is still filed. A user row on the same walk is unchanged,
/// including a stop, which files it instead of putting it back.
///
/// Treating a dropped row as an ordinary return turns the first two assertions red, and the
/// autoload keeps walking after the switch is off. Filing a user row as dropped turns the
/// user assertions red and the user's ask disappears.
#[test]
fn a_dropped_autoload_row_is_not_put_back_on_the_queue() {
    assert_eq!(
        returned_row(true, true, false, false),
        ReturnedRow::Dropped { file: false },
        "rate-limited"
    );
    assert_eq!(
        returned_row(true, false, true, false),
        ReturnedRow::Dropped { file: false },
        "budget-short"
    );
    assert_eq!(
        returned_row(true, false, false, false),
        ReturnedRow::Dropped { file: true },
        "covered"
    );
    assert_eq!(
        returned_row(true, true, false, true),
        ReturnedRow::Dropped { file: false },
        "a stop does not revive a dropped autoload row"
    );
    assert_eq!(returned_row(false, true, false, false), ReturnedRow::Defer);
    assert_eq!(
        returned_row(false, false, true, false),
        ReturnedRow::Continue
    );
    assert_eq!(returned_row(false, false, false, false), ReturnedRow::File);
    assert_eq!(
        returned_row(false, true, false, true),
        ReturnedRow::File,
        "a stop files a user row instead of waiting"
    );
}

/// A user ask for a row the autoload dropped on a live walk clears that drop and adopts
/// the row. The id stays on this flight, so the return keeps the user's continuation.
///
/// Leaving the drop set turns the assertion red: the user's press is swallowed and the
/// walk's return discards the row.
#[test]
fn a_user_ask_revives_a_dropped_autoload_row() {
    let mut origins = [RowOrigin::Autoload, RowOrigin::User];
    let mut dropped = [true, false];
    assert!(adopt_booked_row(&[8, 9], &mut origins, &mut dropped, 8));
    assert_eq!(origins[0], RowOrigin::User);
    assert!(!dropped[0]);
    assert!(!adopt_booked_row(&[8, 9], &mut origins, &mut dropped, 8));
    assert!(!adopt_booked_row(&[8, 9], &mut origins, &mut dropped, 9));
}

/// A second switch-off must not offer an autoload row that the first one already
/// dropped. Offering it again subtracts the same id from the batch total, and the
/// user's row then finishes against a total of zero.
#[test]
fn a_second_autoload_cancel_does_not_drop_the_same_row_again() {
    let auto = RowOrigin::Autoload;
    let user = RowOrigin::User;
    let rows = [tagged(8, auto), tagged(9, user)];
    let first = rows_still_dropping(&rows, &[false, false]);
    let plan = cancel_autoload_rows(&[], &[], &[snap(2, first)]);
    assert_eq!(plan.unmark, vec![8]);
    assert_eq!(plan.removed, 1);
    let second = rows_still_dropping(&rows, &[true, false]);
    assert_eq!(second, vec![tagged(9, user)]);
    let again = cancel_autoload_rows(&[], &[], &[snap(2, second)]);
    assert!(again.unmark.is_empty());
    assert_eq!(again.removed, 0);
    assert!(again.in_flight[0].dropped.is_empty());
}

/// An uncovered dropped row leaves the flight when the walk passes it. A Fetch
/// trades press can then queue that id. Leaving it booked adopts the press and
/// throws the ask away, because the cluster loop does not visit the row again.
#[test]
fn a_passed_dropped_row_leaves_the_flight() {
    let mut uids = vec![8, 9];
    let mut origins = vec![RowOrigin::Autoload, RowOrigin::User];
    let mut dropped = vec![true, false];
    assert!(release_booked_uid(&mut uids, &mut origins, &mut dropped, 8));
    assert_eq!(uids, vec![9]);
    assert_eq!(origins, vec![RowOrigin::User]);
    assert_eq!(dropped, vec![false]);
    assert!(!release_booked_uid(
        &mut uids,
        &mut origins,
        &mut dropped,
        8
    ));
}

/// A cancelled walk's return removes its own flight id. A newer flight for the same row
/// ids, started after the cancel took the old flight off the books, stays booked.
///
/// Matching the row ids instead turns this red: the old thread removes the user's new
/// flight, and the caption goes idle while that walk is still out.
#[test]
fn a_returning_walk_removes_only_its_own_flight() {
    let mut booked = vec![2];
    assert!(
        !retain_flight(&mut booked, 1),
        "the cancelled flight is already gone"
    );
    assert_eq!(booked, vec![2]);
    assert!(retain_flight(&mut booked, 2));
    assert!(booked.is_empty());
}

/// A user ask for a row the autoload already queued makes it the user's. An autoload ask does
/// not take a user row back.
#[test]
fn a_user_ask_adopts_an_autoload_row() {
    assert_eq!(
        adopt_origin(RowOrigin::Autoload, RowOrigin::User),
        RowOrigin::User
    );
    assert_eq!(
        adopt_origin(RowOrigin::User, RowOrigin::Autoload),
        RowOrigin::User
    );
    assert_eq!(
        adopt_origin(RowOrigin::Autoload, RowOrigin::Autoload),
        RowOrigin::Autoload
    );
    assert_eq!(
        adopt_origin(RowOrigin::User, RowOrigin::User),
        RowOrigin::User
    );
}

/// A deferral or a continuation must keep a user adoption made while the walk was out.
/// The origin the walk carried from dispatch is autoload; the in-flight record is what moved.
#[test]
fn a_requeued_row_keeps_the_adoption_made_during_the_walk() {
    assert_eq!(
        requeued_origin(RowOrigin::Autoload, Some(RowOrigin::User)),
        RowOrigin::User
    );
    assert_eq!(
        requeued_origin(RowOrigin::Autoload, Some(RowOrigin::Autoload)),
        RowOrigin::Autoload
    );
    assert_eq!(
        requeued_origin(RowOrigin::User, None),
        RowOrigin::User,
        "a flight already gone leaves the origin the walk carried"
    );
}

/// An id already filed, or named twice across the dropped lists, leaves the batch total once.
#[test]
fn a_filed_row_is_not_subtracted_from_the_total_again() {
    let done = HashSet::from([1]);
    assert_eq!(rows_leaving_total(&[1, 2, 2, 3], &done), 2);
    assert_eq!(rows_leaving_total(&[1], &done), 0);
    assert_eq!(rows_leaving_total(&[], &done), 0);
}
