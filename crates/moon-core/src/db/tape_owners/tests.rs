//! The owner scan on an in-memory replica: the time bound, the stamp resolution, the
//! tuner's own filter.

use rusqlite::Connection;

use super::*;

fn replica() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE orders_rep(
            core_uid INTEGER, strategyid INTEGER, coin TEXT,
            buydate INTEGER, closedate INTEGER, buydatems INTEGER, closedatems INTEGER,
            sellreason TEXT
         );
         INSERT INTO orders_rep VALUES
            (7, 42, 'ACE', 100, 200, 100500, 200500, 'Sell Price'),
            (7, 42, 'OLD', 110, 190, 0, NULL, 'Sell Price'),
            (7, 42, 'ACE', 160, 170, 160000, 170000, 'Funding'),
            (7, 0, 'BEN', 165, 175, 165000, 175000, 'Manual Sell'),
            (7, 42, 'FAR', 10, 20, 10000, 20000, 'Sell Price'),
            (7, 42, 'OPEN', 150, 0, 150000, 0, '');",
    )
    .expect("fixture");
    conn
}

fn coins(owners: &[TapeOwner]) -> Vec<&str> {
    let mut out: Vec<&str> = owners.iter().map(|o| o.coin.as_str()).collect();
    out.sort_unstable();
    out
}

/// Only rows that closed inside the bound, and only closed ones; the stamps take the
/// millisecond column when it is positive and the seconds column otherwise.
#[test]
fn the_scan_is_bounded_and_resolves_stamps() {
    let conn = replica();
    let owners = read_rows(&conn, 100, 1_000).expect("read");
    assert_eq!(coins(&owners), vec!["ACE", "ACE", "BEN", "OLD"]);
    let ace = owners
        .iter()
        .find(|o| o.coin == "ACE" && o.sell_reason == "Sell Price")
        .expect("the ACE trade");
    assert_eq!(ace.buy, ReportStamp::Millis(100_500));
    assert_eq!(ace.close, ReportStamp::Millis(200_500));
    assert_eq!((ace.core_uid, ace.strategy_id), (7, 42));
    let old = owners
        .iter()
        .find(|o| o.coin == "OLD")
        .expect("the OLD trade");
    assert_eq!(old.buy, ReportStamp::Seconds(110));
    assert_eq!(old.close, ReportStamp::Seconds(190));
    // Closed before the bound, or opened after it: nothing of the tape can be theirs.
    assert!(read_rows(&conn, 250, 1_000).expect("read").is_empty());
    assert_eq!(coins(&read_rows(&conn, 0, 50).expect("read")), vec!["FAR"]);
}

/// A replica without the millisecond columns still names every trade, in seconds.
#[test]
fn a_replica_without_ms_columns_reads_in_seconds() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE orders_rep(core_uid INTEGER, strategyid INTEGER, coin TEXT,
            buydate INTEGER, closedate INTEGER, sellreason TEXT);
         INSERT INTO orders_rep VALUES (1, 5, 'ACE', 100, 200, 'Sell Price');",
    )
    .expect("fixture");
    let owners = read_rows(&conn, 0, 1_000).expect("read");
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].buy, ReportStamp::Seconds(100));
    assert_eq!(owners[0].close, ReportStamp::Seconds(200));
}

/// The tuner's rule, as the row answers it: a resolved trading kind that closed by itself.
#[test]
fn is_tunable_follows_the_axis_filter() {
    let owner = |strategy_id: i64, kind: &str, sell_reason: &str| TapeOwner {
        core_uid: 1,
        coin: "ACE".into(),
        buy: ReportStamp::Seconds(1),
        close: ReportStamp::Seconds(2),
        strategy_id,
        sell_reason: sell_reason.into(),
        buy_set_ms: None,
        kind: kind.into(),
    };
    assert!(owner(42, "MoonShot", "Sell Price").is_tunable());
    assert!(!owner(42, "", "Sell Price").is_tunable(), "kind unresolved");
    assert!(
        !owner(42, "Manual", "Sell Price").is_tunable(),
        "container kind"
    );
    assert!(
        !owner(42, "MoonShot", "Manual Sell").is_tunable(),
        "manual exit"
    );
    assert!(
        !owner(42, "MoonShot", "Funding").is_tunable(),
        "service row"
    );
    assert!(
        !owner(0, "MoonShot", "Sell Price").is_tunable(),
        "no strategy"
    );
}
