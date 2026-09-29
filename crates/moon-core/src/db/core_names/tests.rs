use super::*;

fn names() -> CoreNames {
    CoreNames::from_pairs([(1, "core-a-renamed"), (3, "  "), (4, "o'core")])
}

/// Evaluate `CoreNames::sql` over one synthetic row.
fn displayed(names: &CoreNames, uid: i64, stored: &str) -> String {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE t (core_uid INTEGER, core_name TEXT)")
        .unwrap();
    conn.execute(
        "INSERT INTO t VALUES (?1, ?2)",
        rusqlite::params![uid, stored],
    )
    .unwrap();
    conn.query_row(&format!("SELECT {} FROM t r", names.sql("r")), [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn a_configured_core_shows_its_current_name_in_sql_and_rust() {
    let names = names();
    assert_eq!(displayed(&names, 1, "core-a-old"), "core-a-renamed");
    assert_eq!(names.resolve(1, "core-a-old"), "core-a-renamed");
}

#[test]
fn an_unconfigured_or_blank_named_core_keeps_its_stored_name() {
    let names = names();
    assert_eq!(displayed(&names, 2, "core-b-gone"), "core-b-gone");
    assert_eq!(displayed(&names, 3, "core-c"), "core-c");
    assert_eq!(names.resolve(2, "core-b-gone"), "core-b-gone");
    assert_eq!(
        displayed(&CoreNames::default(), 1, "core-a-old"),
        "core-a-old"
    );
}

#[test]
fn a_quote_in_a_configured_name_is_escaped() {
    assert_eq!(displayed(&names(), 4, "x"), "o'core");
}
