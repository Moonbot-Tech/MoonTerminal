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

fn price(raw: Option<(i64, String)>) -> Option<f64> {
    let (_, raw) = raw?;
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

/// The pass reader answers what the one-shot read answers, at every boundary of three
/// versions, for a core, another core and no core — twice over, the second time off its memo.
#[test]
fn the_pass_reader_reads_what_the_one_shot_read_reads() {
    let keys = vec!["MShotPrice".to_string(), "Missing".to_string()];
    let reader = StrategyValuesReader::over(versions());
    let conn = versions();
    for _ in 0..2 {
        for core in [Some(7), Some(8), None] {
            for at in [10, 999, 1000, 1500, 1999, 2000, 2999, 3000, 5000] {
                let one_shot = load_raw_json_at(&conn, 42, core, at)
                    .and_then(|(_, raw)| flatten_values(&raw, &keys));
                assert_eq!(
                    reader.values_at(42, core, at, &keys),
                    one_shot,
                    "{core:?} at {at}"
                );
            }
        }
        assert_eq!(reader.values_at(99, Some(7), 1000, &keys), None);
    }
    let parsed = reader.parsed.lock().unwrap().len();
    assert_eq!(parsed, 4, "each version parsed once");
}

/// A version deleted and replaced by another that takes over its row id is read anew: the
/// pass reader answers the new values, as the one-shot read does.
#[test]
fn a_reused_row_id_reads_the_new_version() {
    let keys = vec!["MShotPrice".to_string()];
    let conn = Connection::open_in_memory().expect("memory db");
    conn.execute_batch(
        "CREATE TABLE strategy_versions (
             core_uid INTEGER, strategy_id INTEGER, valid_from INTEGER, valid_to INTEGER,
             raw_json TEXT);
         INSERT INTO strategy_versions VALUES (7, 42, 1000, NULL, '{\"MShotPrice\": 1.0}');",
    )
    .expect("schema");
    let reader = StrategyValuesReader::over(conn);
    let old = reader.values_at(42, Some(7), 1500, &keys);
    assert!(old.is_some(), "first version");
    let rewrite = |sql: &str| {
        let idle = reader.idle.lock().unwrap();
        idle[0].execute_batch(sql).expect("rewrite");
    };
    rewrite(
        "DELETE FROM strategy_versions;
         INSERT INTO strategy_versions VALUES (7, 42, 1000, NULL, '{\"MShotPrice\": 25.0}');",
    );
    let rowid: i64 = {
        let idle = reader.idle.lock().unwrap();
        idle[0]
            .query_row("SELECT rowid FROM strategy_versions", [], |r| r.get(0))
            .expect("row")
    };
    assert_eq!(rowid, 1, "the new version reuses the deleted row id");
    let one_shot = {
        let idle = reader.idle.lock().unwrap();
        load_raw_json_at(&idle[0], 42, Some(7), 1500)
            .and_then(|(_, raw)| flatten_values(&raw, &keys))
    };
    let new = reader.values_at(42, Some(7), 1500, &keys);
    assert_eq!(new, one_shot);
    assert_ne!(new, old, "the old parse was not reused");
}
