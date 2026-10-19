use rusqlite::Connection;

use super::*;
use crate::db::analytics::{Query, unified_from};
use crate::db::tuner::{FIELDS, FieldClass, StratFilters};

/// Evaluate the projection against one comment, the way the unified source runs it.
fn parse(comment: Option<&str>) -> Option<f64> {
    let conn = Connection::open_in_memory().expect("memory db");
    let cols: HashSet<String> = ["comment".to_string()].into();
    let expr = derived_sql(DROPS_PRICE_DELTA, "r", &cols).expect("derived field");
    conn.query_row(
        &format!("SELECT {expr} FROM (SELECT ?1 AS comment) r"),
        [comment],
        |row| row.get(0),
    )
    .expect("projection runs")
}

#[test]
fn a_drops_comment_yields_its_price_delta() {
    let comment = " DropsDetection: <UNIT_01> USDT-XALPHA  DailyVol: 27m PriceIsLow: false \
                   xPriceDelta: 3.1 \r\nBTC(5min, 1sec) = 0.00%  MAvg(30sec, 1sec) = -0.07%";
    assert_eq!(parse(Some(comment)), Some(3.1));
    // A line break straight after the number ends the token as a space does.
    let tight = " DropsDetection: <UNIT_01> USDT-XALPHA  PriceIsLow: true xPriceDelta: 12\r\nBTC";
    assert_eq!(parse(Some(tight)), Some(12.0));
    let last = " DropsDetection: <UNIT_01> USDT-XALPHA xPriceDelta: 0.5";
    assert_eq!(parse(Some(last)), Some(0.5));
}

#[test]
fn other_rows_and_malformed_tokens_yield_null() {
    for comment in [
        // Another detect carrying the same token is not a Drops deal.
        " PumpDetection: <UNIT_01> USDT-XALPHA xPriceDelta: 3.0 ",
        // A Drops deal whose comment lacks the token.
        " DropsDetection: <UNIT_01> USDT-XALPHA  DailyVol: 27m PriceIsLow: false \r\nBTC",
        // Malformed numbers.
        " DropsDetection: <UNIT_01> USDT-XALPHA xPriceDelta: 3.0.1 ",
        " DropsDetection: <UNIT_01> USDT-XALPHA xPriceDelta: abc ",
        " DropsDetection: <UNIT_01> USDT-XALPHA xPriceDelta: 3,0 ",
        " DropsDetection: <UNIT_01> USDT-XALPHA xPriceDelta: . ",
        " DropsDetection: <UNIT_01> USDT-XALPHA xPriceDelta:  3.0",
        "",
    ] {
        assert_eq!(parse(Some(comment)), None, "{comment:?}");
    }
    assert_eq!(parse(None), None);
}

#[test]
fn a_source_without_a_comment_column_projects_null() {
    assert_eq!(
        derived_sql(DROPS_PRICE_DELTA, "r", &HashSet::new()).as_deref(),
        Some("NULL")
    );
    let cols: HashSet<String> = ["comment".to_string()].into();
    assert_eq!(derived_sql("dvol", "r", &cols), None);
}

/// The field sits in the By-filter field set, saves its Min only, and no Ignore flag hides it.
#[test]
fn the_price_delta_is_a_by_filter_field_without_a_max() {
    let spec = FIELDS
        .iter()
        .find(|spec| spec.col == DROPS_PRICE_DELTA)
        .expect("xPriceDelta is a tuner field");
    assert_eq!(spec.p_min, Some("DropsPriceDelta"));
    assert_eq!(spec.p_max, None);
    assert_eq!(spec.class, FieldClass::Detect);
    assert!(spec.mapped());
    let ignoring = StratFilters {
        ignore_filters: true,
        ..StratFilters::default()
    };
    assert!(!ignoring.class_ignored(FieldClass::Detect));
}

/// The unified source every tuner read goes through carries the parsed value per row.
#[test]
fn the_unified_source_projects_the_parsed_value() {
    let conn = Connection::open_in_memory().expect("memory db");
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER, core_name TEXT, coin TEXT,
             isshort INTEGER, buydate INTEGER, closedate INTEGER, profitbtc REAL,
             strategyid INTEGER, emulator INTEGER, spentbtc REAL, basecurrency INTEGER,
             comment TEXT);
         INSERT INTO orders_rep VALUES
             (1, 'CORE_A', 'XALPHA', 0, 900, 1000, 1.0, 7, 0, 10.0, 1,
              ' DropsDetection: <UNIT_01> USDT-XALPHA  DailyVol: 27m PriceIsLow: false xPriceDelta: 2.9 \r\nBTC'),
             (1, 'CORE_A', 'XALPHA', 0, 900, 1001, 1.0, 8, 0, 10.0, 1, ' MoonShot: <UNIT_01>');",
    )
    .expect("schema");
    let q = Query {
        to: i64::MAX,
        ..Query::default()
    };
    let src = unified_from(&conn, &q)
        .expect("source")
        .expect("the replica exists");
    let values = conn
        .prepare(&format!(
            "SELECT o.\"{DROPS_PRICE_DELTA}\" FROM {src} ORDER BY o.closedate"
        ))
        .expect("prepare")
        .query_map(rusqlite::params![0, i64::MAX], |row| {
            row.get::<_, Option<f64>>(0)
        })
        .expect("query")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("rows");
    assert_eq!(values, vec![Some(2.9), None]);
}
