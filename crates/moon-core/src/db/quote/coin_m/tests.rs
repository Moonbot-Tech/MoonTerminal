use std::collections::HashSet;

use rusqlite::Connection;

use super::super::learn_coin_m_cores;
use super::*;

/// Columns the COIN-M rule needs, plus the row id that bounds an examined span.
fn columns(bounded: bool) -> HashSet<String> {
    let mut out: HashSet<String> = ["core_uid", "fname", "coin"]
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    if bounded {
        out.insert("newrecid".to_string());
    }
    out
}

/// A replica holding nothing yet, with the primary key production seeks through.
fn replica() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER, newrecid INTEGER, fname TEXT, coin TEXT,
             PRIMARY KEY (core_uid, newrecid))",
    )
    .unwrap();
    conn
}

/// Insert one row for a core, COIN-M or ordinary.
///
/// The COIN-M spelling is the live one: `USD-` names the inverse market that a USD-M core writes
/// as `USDT-`, and the contract shape carries the settlement date.
fn insert(conn: &Connection, core: i64, newrecid: i64, coin_m: bool) {
    let (fname, coin) = if coin_m {
        ("Pump_USD-ETH_0926_1m", "ETH_0926")
    } else {
        ("Pump_USDT-BTC_1m", "BTCUSDT")
    };
    conn.execute(
        "INSERT INTO orders_rep (core_uid, newrecid, fname, coin) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![core, newrecid, fname, coin],
    )
    .unwrap();
}

/// Run the production entry point over one replica.
fn learn_from(conn: &Connection, bounded: bool) {
    learn_coin_m_cores(
        conn,
        &[crate::db::ReadSource {
            table: "orders_rep",
            cols: columns(bounded),
            legacy: false,
        }],
    );
}

/// The reason the scan can be skipped at all: knowledge is per core, so a core that appears later
/// is examined when it appears — not on a timer, and not by re-reading everyone else.
#[test]
fn a_core_appearing_later_is_examined_when_it_appears() {
    reset();
    let conn = replica();
    insert(&conn, 1, 1, false);
    learn_from(&conn, true);
    assert!(cores().is_empty(), "an ordinary core is not COIN-M");

    insert(&conn, 2, 1, true);
    learn_from(&conn, true);
    assert_eq!(cores(), BTreeSet::from([2]));
}

/// The span, not the core, is what gets marked examined. A core whose FIRST COIN-M row arrives
/// after it was already examined must still be recognized — otherwise the money of a core that
/// switched markets stays off by the price of a bitcoin for the rest of the session.
#[test]
fn rows_arriving_above_an_examined_span_are_examined() {
    reset();
    let conn = replica();
    insert(&conn, 1, 1, false);
    insert(&conn, 1, 2, false);
    learn_from(&conn, true);
    assert!(cores().is_empty());

    insert(&conn, 1, 3, true);
    learn_from(&conn, true);
    assert_eq!(cores(), BTreeSet::from([1]));
}

/// Both ends of the span are examined, because a resumed history download fills in BELOW it —
/// the replica's own known hole (`Resume(MAX+1)`) writes ids under the ones already stored.
#[test]
fn rows_arriving_below_an_examined_span_are_examined() {
    reset();
    let conn = replica();
    insert(&conn, 1, 10, false);
    insert(&conn, 1, 11, false);
    learn_from(&conn, true);
    assert!(cores().is_empty());

    insert(&conn, 1, 4, true);
    learn_from(&conn, true);
    assert_eq!(cores(), BTreeSet::from([1]));
}

/// The steady state reads nothing: a row REWRITTEN inside an examined span is not seen again.
///
/// This pins the trade the module makes deliberately, and the span alone cannot close it — a
/// catch-up page and a partial live upsert both write between ids already stored. What closes it
/// is the writer saying so on commit, which the next two tests exercise; re-reading every examined
/// row on every discovery is the two-second scan this module exists to remove.
#[test]
fn a_row_rewritten_inside_an_examined_span_is_not_re_examined() {
    reset();
    let conn = replica();
    insert(&conn, 1, 1, false);
    learn_from(&conn, true);
    assert!(cores().is_empty());

    conn.execute(
        "UPDATE orders_rep SET fname = 'Pump_USD-ETH_0926_1m', coin = 'ETH_0926'
          WHERE core_uid = 1 AND newrecid = 1",
        [],
    )
    .unwrap();
    learn_from(&conn, true);
    assert!(cores().is_empty(), "an examined span is not read twice");
}

/// A catch-up fills ids UNDER the local maximum (`ReportStart::Checkpoint` re-fetches exactly
/// those pages), so the writer's post-commit signal is what makes the module look again. Without
/// it the core keeps the wrong money for the rest of the session.
#[test]
fn a_settled_catch_up_makes_the_module_look_inside_the_span_again() {
    reset();
    let conn = replica();
    insert(&conn, 1, 1, false);
    insert(&conn, 1, 9, false);
    learn_from(&conn, true);
    assert!(cores().is_empty());

    insert(&conn, 1, 5, true); // Strictly inside the examined span, as a caught-up page lands.
    learn_from(&conn, true);
    assert!(cores().is_empty(), "the span's ends did not move");

    reexamine_core(1);
    learn_from(&conn, true);
    assert_eq!(cores(), BTreeSet::from([1]));
}

/// A recreated replica serves a DIFFERENT database, so the verdict it produced goes with it —
/// otherwise a core that no longer trades COIN-M would keep rewriting nameless liquidations by
/// their entry price.
#[test]
fn a_recreated_replica_forgets_the_verdict() {
    reset();
    let conn = replica();
    insert(&conn, 1, 1, true);
    learn_from(&conn, true);
    assert_eq!(cores(), BTreeSet::from([1]));

    conn.execute("DELETE FROM orders_rep", []).unwrap();
    forget_core(1);
    insert(&conn, 1, 1, false);
    learn_from(&conn, true);
    assert!(cores().is_empty(), "the wiped core is judged on its new rows");
}

/// A proven core stays proven. Its history keeps the rows that proved it however the live table
/// changes, and re-proving it would put the expensive scan back on the hot path.
#[test]
fn a_proven_core_is_never_re_examined() {
    reset();
    let conn = replica();
    insert(&conn, 1, 1, true);
    learn_from(&conn, true);
    assert_eq!(cores(), BTreeSet::from([1]));

    conn.execute("DELETE FROM orders_rep", []).unwrap();
    learn_from(&conn, true);
    assert_eq!(cores(), BTreeSet::from([1]));
}

/// Without a row id there is no span to bound anything with, so the core is read end to end once
/// and never again — the same conservative direction as before: it corrects what it proved.
#[test]
fn a_source_without_row_ids_is_read_once() {
    reset();
    let conn = replica();
    insert(&conn, 1, 1, false);
    learn_from(&conn, false);
    assert!(cores().is_empty());

    insert(&conn, 1, 2, true);
    learn_from(&conn, false);
    assert!(cores().is_empty(), "an unbounded source cannot say what is new");

    // A core that never appeared is still examined, since only the examined ones are skipped.
    insert(&conn, 2, 1, true);
    learn_from(&conn, false);
    assert_eq!(cores(), BTreeSet::from([2]));
}

/// The legacy table carries no row id and is only ever purged from, so it is swept exactly once.
#[test]
fn the_legacy_source_is_swept_once() {
    reset();
    let conn = replica();
    conn.execute_batch(
        "CREATE TABLE closed_sell_reports (core_uid INTEGER, fname TEXT, coin TEXT)",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO closed_sell_reports (core_uid, fname, coin)
         VALUES (5, 'Pump_USD-ETH_0926_1m', 'ETH_0926')",
        [],
    )
    .unwrap();
    let sources = [crate::db::ReadSource {
        table: "closed_sell_reports",
        cols: columns(false),
        legacy: true,
    }];
    learn_coin_m_cores(&conn, &sources);
    assert_eq!(cores(), BTreeSet::from([5]));

    conn.execute(
        "INSERT INTO closed_sell_reports (core_uid, fname, coin)
         VALUES (6, 'Pump_USD-BNB_0926_1m', 'BNB_0926')",
        [],
    )
    .unwrap();
    learn_coin_m_cores(&conn, &sources);
    assert_eq!(cores(), BTreeSet::from([5]), "the sweep does not run twice");
}

/// A wipe drops the verdict, and a core still on the legacy table was PROVEN by that table — so
/// the one-shot sweep has to re-arm with it. Latched shut, such a core would keep the wrong money
/// for the session with no path back.
#[test]
fn a_wipe_re_arms_the_legacy_sweep() {
    reset();
    let conn = replica();
    conn.execute_batch(
        "CREATE TABLE closed_sell_reports (core_uid INTEGER, fname TEXT, coin TEXT)",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO closed_sell_reports (core_uid, fname, coin)
         VALUES (5, 'Pump_USD-ETH_0926_1m', 'ETH_0926')",
        [],
    )
    .unwrap();
    let sources = [crate::db::ReadSource {
        table: "closed_sell_reports",
        cols: columns(false),
        legacy: true,
    }];
    learn_coin_m_cores(&conn, &sources);
    assert_eq!(cores(), BTreeSet::from([5]));

    forget_core(5);
    learn_coin_m_cores(&conn, &sources);
    assert_eq!(cores(), BTreeSet::from([5]), "the legacy rows prove it again");
}
