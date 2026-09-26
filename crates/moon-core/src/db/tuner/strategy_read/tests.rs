//! The as-of version read, on an in-memory copy of the `strategy_versions` shape.

use super::*;

fn versions() -> Connection {
    let conn = Connection::open_in_memory().expect("memory db");
    conn.execute_batch(
        "CREATE TABLE strategy_versions (
             core_uid INTEGER, strategy_id INTEGER, valid_from INTEGER, valid_to INTEGER,
             raw_json TEXT);
         INSERT INTO strategy_versions VALUES
             (7, 42, 1000, 2000, '{\"MShotPrice\": 1.0}'),
             (7, 42, 2000, 3000, '{\"MShotPrice\": 2.0}'),
             (7, 42, 3000, NULL, '{\"MShotPrice\": 3.0}'),
             (8, 42, 1500, NULL, '{\"MShotPrice\": 8.0}');",
    )
    .expect("schema");
    conn
}

fn price(raw: Option<String>) -> Option<f64> {
    let raw = raw?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    json.get("MShotPrice")?.as_f64()
}

#[test]
fn the_version_valid_at_the_moment_is_read() {
    let conn = versions();
    assert_eq!(price(load_raw_json_at(&conn, 42, Some(7), 1000)), Some(1.0));
    assert_eq!(price(load_raw_json_at(&conn, 42, Some(7), 1999)), Some(1.0));
    assert_eq!(price(load_raw_json_at(&conn, 42, Some(7), 2000)), Some(2.0));
    assert_eq!(price(load_raw_json_at(&conn, 42, Some(7), 5000)), Some(3.0));
}

#[test]
fn a_moment_before_the_first_version_reads_the_first_version() {
    let conn = versions();
    assert_eq!(price(load_raw_json_at(&conn, 42, Some(7), 10)), Some(1.0));
}

#[test]
fn the_core_scopes_the_read_and_no_core_reads_across_cores() {
    let conn = versions();
    assert_eq!(price(load_raw_json_at(&conn, 42, Some(8), 4000)), Some(8.0));
    // Without a core, the latest-starting version valid at the moment wins, whichever core.
    assert_eq!(price(load_raw_json_at(&conn, 42, None, 2500)), Some(2.0));
    assert_eq!(price(load_raw_json_at(&conn, 42, None, 1600)), Some(8.0));
    assert_eq!(load_raw_json_at(&conn, 99, None, 2500), None);
}

#[test]
fn flatten_values_spells_booleans_and_lists_in_strategy_format() {
    let raw = r#"{"MShotPrice": 1.5, "MShotMinusSatoshi": true, "CoinsBlackList": ["A", "B"]}"#;
    let keys = [
        "MShotPrice",
        "MShotMinusSatoshi",
        "CoinsBlackList",
        "Missing",
    ]
    .map(String::from)
    .to_vec();
    let out = flatten_values(raw, &keys).expect("object");
    assert_eq!(out.get("MShotPrice").map(String::as_str), Some("1.5"));
    assert_eq!(
        out.get("MShotMinusSatoshi").map(String::as_str),
        Some("YES")
    );
    assert_eq!(out.get("CoinsBlackList").map(String::as_str), Some("A,B"));
    assert!(!out.contains_key("Missing"));
    assert_eq!(flatten_values("[]", &keys), None);
}
