//! What one closed trade carried beside its prices, which row answers for it, and what survives a
//! source that lost columns.

use rusqlite::Connection;

use super::{detect_text, query_trade_meta, ChartTradeRecord};

/// The comment shape the core actually writes: one detect line, then its own health.
///
/// Transcribed from a live replica (a `Hook` strategy, 2026-08), CRLF included, because the tail is
/// what this module exists to drop and a hand-written approximation would not have it.
const REAL_COMMENT: &str = concat!(
    " <HookTest1>Hook Short Depth: 2.47% R: 120% d: 2.35%  (High: 0.019788  Min: 0.020276",
    "  Max: 0.019692  [AbsHigh: 0.019687 Drop: 20.80%] VolK: 19.47) InitialPrice: 0.020301\r\n",
    " CPU: Bot 3 (Avg: 2) Sys: 6  AppLatency: 0.0 sec  API Req: 3 / 600   API Orders: 0 / 5000\r\n",
    "Latency: 46 / 57  Ping: 404 / 113",
);

/// One trade as the chart history hands it out; only the fields the lookup matches on matter.
fn record(core_uid: u64, record_id: i64, coin: &str, buy: i64, close: i64) -> ChartTradeRecord {
    ChartTradeRecord {
        record_id,
        core_uid,
        coin: coin.to_string(),
        buy_date: buy,
        close_date: close,
        buy_price: 1.0,
        sell_price: 2.0,
        quantity: 3.0,
        is_short: false,
        emulator: false,
        profit: None,
        quote: None,
        profit_pct: None,
    }
}

/// Seed a typed replica holding one trade with metadata and one without.
fn fixture() -> Connection {
    let conn = Connection::open_in_memory().expect("open trade-meta fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL,
             newrecid INTEGER NOT NULL,
             coin TEXT,
             buydate INTEGER,
             closedate INTEGER,
             comment TEXT,
             strategyid INTEGER,
             sellreason TEXT
         );",
    )
    .expect("create trade-meta fixture");
    conn.execute(
        "INSERT INTO orders_rep VALUES (7, 11, 'BTC', 100, 200, ?1, -8797559844610818221,
             'Auto Price Down')",
        [REAL_COMMENT],
    )
    .expect("seed detected trade");
    conn.execute_batch(
        "INSERT INTO orders_rep VALUES (7, 12, 'ETH', 300, 400, '', 0, '');
         INSERT INTO orders_rep VALUES (8, 11, 'BTC', 100, 200, 'other core', 5, 'Sell Price');",
    )
    .expect("seed bare trades");
    conn
}

/// The health tail the core appends is not part of the detect line.
#[test]
fn the_diagnostic_tail_is_dropped() {
    let line = detect_text(REAL_COMMENT);
    assert!(line.starts_with("<HookTest1>Hook Short"), "kept: {line}");
    assert!(line.ends_with("InitialPrice: 0.020301"), "kept: {line}");
    assert!(!line.contains("CPU:"), "tail survived: {line}");
    assert!(!line.contains("Latency:"), "tail survived: {line}");
    assert!(!line.contains("Ping:"), "tail survived: {line}");
}

/// A detect written over two lines keeps BOTH, joined into one caption.
///
/// The reason this is not "the first line": a handful of strategies state their conditions over two
/// lines, and halving them silently is exactly the failure the caption cannot show.
#[test]
fn a_two_line_detect_survives_whole() {
    let line = detect_text("Depth: 2%\r\nBuffer: [1..3]\r\n CPU: Bot 3 Sys: 6");
    assert_eq!(line, "Depth: 2% Buffer: [1..3]");
}

/// A comment holding nothing but the core's own health reduces to nothing.
#[test]
fn a_diagnostics_only_comment_is_empty() {
    assert_eq!(detect_text(" CPU: Bot 3 Sys: 6\r\nLatency: 46 / 57"), "");
    assert_eq!(detect_text(""), "");
}

/// Only `CPU:` and `Latency:` are dropped: a detect line beginning with another of the core's
/// diagnostic WORDS is still the detect line.
#[test]
fn a_detect_line_starting_with_a_diagnostic_word_survives() {
    assert_eq!(
        detect_text("API burst detected: 12/s\r\n CPU: Bot 3 Sys: 6"),
        "API burst detected: 12/s"
    );
    assert_eq!(detect_text("Ping spike 400ms"), "Ping spike 400ms");
}

/// The row is addressed by its exact core AND record id.
#[test]
fn the_exact_row_answers_and_a_foreign_core_does_not() {
    let conn = fixture();
    let meta = query_trade_meta(&conn, &record(7, 11, "BTC", 100, 200))
        .expect("read trade meta")
        .expect("row 11 exists");
    assert!(meta.detect.starts_with("<HookTest1>Hook Short"));
    assert_eq!(meta.strategy_id, Some(-8797559844610818221));
    assert_eq!(meta.sell_reason, "Auto Price Down");

    let other = query_trade_meta(&conn, &record(8, 11, "BTC", 100, 200))
        .expect("read trade meta")
        .expect("row 11 of core 8 exists");
    assert_eq!(other.strategy_id, Some(5), "read the other core's row");

    assert_eq!(
        query_trade_meta(&conn, &record(9, 11, "BTC", 100, 200)).expect("read trade meta"),
        None,
        "a core with no such row answers nothing"
    );
}

/// The trade's OWN facts are matched beside its id, so a row that merely shares the number is not
/// mistaken for it.
///
/// This is what a record id alone cannot do: the two report sources count their ids independently,
/// so while a core is mid-migration the same number names two different trades.
#[test]
fn a_row_that_only_shares_the_number_is_refused() {
    let conn = fixture();
    // Right id, right core, WRONG trade — a different coin and different stamps.
    assert_eq!(
        query_trade_meta(&conn, &record(7, 11, "DOGE", 999, 1000)).expect("read trade meta"),
        None
    );
    // Right coin, wrong stamps: still refused.
    assert_eq!(
        query_trade_meta(&conn, &record(7, 11, "BTC", 101, 200)).expect("read trade meta"),
        None
    );
    // The coin is matched case-insensitively, the way every other coin predicate here is.
    assert!(query_trade_meta(&conn, &record(7, 11, "btc", 100, 200))
        .expect("read trade meta")
        .is_some());
}

/// A trade the core wrote nothing about reads as nothing to print.
///
/// The row IS there — the search simply has nothing to hand back, and the caller cannot tell that
/// apart from an absent row anyway: both mean the captions print nothing.
#[test]
fn a_row_the_core_wrote_nothing_about_has_nothing_to_print() {
    let conn = fixture();
    assert_eq!(
        query_trade_meta(&conn, &record(7, 12, "ETH", 300, 400)).expect("read trade meta"),
        None
    );
}

/// Strategy `0` is no strategy: the replica stores it for "none", and a caption naming strategy
/// zero would be a number matching nothing in the reader's list.
#[test]
fn strategy_zero_is_no_strategy() {
    let conn = Connection::open_in_memory().expect("open zero-strategy fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL, newrecid INTEGER NOT NULL, coin TEXT,
             buydate INTEGER, closedate INTEGER, comment TEXT, strategyid INTEGER);
         INSERT INTO orders_rep VALUES (7, 11, 'BTC', 100, 200, 'PumpDetection: 4%', 0);",
    )
    .expect("seed zero-strategy fixture");
    let meta = query_trade_meta(&conn, &record(7, 11, "BTC", 100, 200))
        .expect("read trade meta")
        .expect("row 11 exists");
    assert_eq!(meta.detect, "PumpDetection: 4%");
    assert_eq!(meta.strategy_id, None);
}

/// A replica predating these columns answers without failing — it simply has nothing to state.
///
/// The direction that matters: an old profile still OPENS the window, with empty captions, rather
/// than erroring out of a read the window's whole point does not depend on.
#[test]
fn a_source_without_the_columns_answers_nothing() {
    let conn = Connection::open_in_memory().expect("open legacy fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL, newrecid INTEGER NOT NULL, coin TEXT,
             buydate INTEGER, closedate INTEGER);
         INSERT INTO orders_rep VALUES (7, 11, 'BTC', 100, 200);",
    )
    .expect("seed legacy fixture");
    assert_eq!(
        query_trade_meta(&conn, &record(7, 11, "BTC", 100, 200)).expect("read trade meta"),
        None
    );
}

/// A typed row whose `newrecid` is still zero is handed out under its `id`, and is found by it.
///
/// The identity is shared with the read that MINTED it — `COALESCE(NULLIF(newrecid,0), id, 0)` —
/// so a reader that asked `newrecid` alone would find nothing here, or worse, another trade.
#[test]
fn a_typed_row_without_a_rec_id_is_found_by_its_own_id() {
    let conn = Connection::open_in_memory().expect("open id-fallback fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL, newrecid INTEGER NOT NULL, id INTEGER,
             coin TEXT, buydate INTEGER, closedate INTEGER, comment TEXT);
         INSERT INTO orders_rep VALUES (7, 0, 42, 'BTC', 100, 200, 'DropsDetection: -3.1%');",
    )
    .expect("seed id-fallback fixture");
    let meta = query_trade_meta(&conn, &record(7, 42, "BTC", 100, 200))
        .expect("read trade meta")
        .expect("row 42 exists");
    assert_eq!(meta.detect, "DropsDetection: -3.1%");
}

/// Values are decoded by their STORAGE CLASS, not by the column's declared type.
///
/// The replica is written by an untyped upsert, so a column can hold whatever the core sent. A
/// typed decode would fail the whole row and lose the two fields that were perfectly readable.
#[test]
fn an_oddly_stored_value_costs_only_itself() {
    let conn = Connection::open_in_memory().expect("open odd-storage fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL, newrecid INTEGER NOT NULL, coin TEXT,
             buydate INTEGER, closedate INTEGER, comment TEXT, strategyid INTEGER,
             sellreason TEXT);
         INSERT INTO orders_rep VALUES (7, 11, 'BTC', 100, 200, 'PumpDetection: 4%',
             '-8797559844610818221', 'Sell Price');",
    )
    .expect("seed odd-storage fixture");
    let meta = query_trade_meta(&conn, &record(7, 11, "BTC", 100, 200))
        .expect("read trade meta")
        .expect("row 11 exists");
    assert_eq!(meta.detect, "PumpDetection: 4%");
    assert_eq!(meta.sell_reason, "Sell Price");
    assert_eq!(
        meta.strategy_id,
        Some(-8797559844610818221),
        "a text-stored id is parsed rather than losing the whole row"
    );
}

/// Record id `0` is the projection for "this source cannot address rows", so it is never asked
/// about: a query for it would match whichever rows happen to carry it.
#[test]
fn record_id_zero_is_never_looked_up() {
    let conn = Connection::open_in_memory().expect("open zero-id fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL, newrecid INTEGER NOT NULL, coin TEXT,
             buydate INTEGER, closedate INTEGER, comment TEXT);
         INSERT INTO orders_rep VALUES (7, 0, 'BTC', 100, 200, 'should not be read');",
    )
    .expect("seed zero-id fixture");
    assert_eq!(
        query_trade_meta(&conn, &record(7, 0, "BTC", 100, 200)).expect("read trade meta"),
        None
    );
}
