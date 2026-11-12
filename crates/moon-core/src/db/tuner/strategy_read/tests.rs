//! The as-of version read, on an in-memory copy of the `strategy_versions` shape.

use super::*;

/// A missing file is empty, but lenient strategy reads must never hide a broken retention schema.
#[test]
fn checked_kinds_distinguishes_absent_and_broken_database() {
    const CHILD: &str = "MOON_STRATEGY_KINDS_TEST_CHILD";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = std::path::PathBuf::from(root);
        assert!(crate::config::paths::set_data_dir_override(root.clone()));
        let path = crate::config::paths::strategies_db_path();
        assert!(path.starts_with(&root));
        assert!(!path.exists());
        assert!(strategy_kinds_checked(&[(42, 7)]).unwrap().is_empty());
        assert!(!path.exists(), "an absent database must not be created");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE strategy_versions (core_uid INTEGER, strategy_id INTEGER,
             valid_from INTEGER, valid_to INTEGER);",
        )
        .unwrap();
        assert!(strategy_kinds_checked(&[(42, 7)]).is_err());
        assert!(strategy_kinds(&[(42, 7)]).is_empty());
        drop(conn);
        std::fs::write(&path, b"corrupt sqlite file").unwrap();
        assert!(strategy_kinds_checked(&[(42, 7)]).is_err());
        return;
    }
    let fixture = crate::db::test_support::temp_db("strategy-kinds-checked");
    let root = fixture.parent().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "db::tuner::strategy_read::tests::checked_kinds_distinguishes_absent_and_broken_database", "--nocapture"])
        .env(CHILD, root)
        .output().unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Schema defaults of the parameters the cards below read, lowercase as the tuner keeps them.
fn defaults() -> std::collections::HashMap<String, f64> {
    [
        ("minvolume", 100.0),
        ("maxvolume", 100_000.0),
        ("minhourlyvolume", 10.0),
        ("maxhourlyvolume", 10_000.0),
        ("maxhourlyvolfast", 0.0),
        ("delta_3h_min", 0.0),
        ("delta_3h_max", 20.0),
        ("delta2_min", 0.0),
        ("delta2_max", 20.0),
        ("maxleverage", 0.0),
        ("minleverage", 1.0),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect()
}

/// The corridor a strategy is born with is a bound Moonbot applies, so it must reach the card
/// (#1072: the live daily volume 100..100000 came back as no bound at all); it is only marked.
/// A side the core reads as "off" at zero stays out, default or not.
#[test]
fn a_factory_corridor_is_a_marked_bound_and_a_zero_off_side_is_none() {
    let raw = r#"{
        "IgnoreVolume": "NO",
        "MinVolume": 100, "MaxVolume": 100000,
        "MinHourlyVolume": 25, "MaxHourlyVolume": 10000,
        "MinHourlyVolFast": 0, "MaxHourlyVolFast": 0,
        "Delta_3h_Min": 0, "Delta_3h_Max": 20,
        "MinLeverage": 1, "MaxLeverage": 0
    }"#;
    let card = filters_of(raw, &defaults()).expect("a card");
    assert!(card.found && !card.ignore_volume);
    assert_eq!(
        card.bounds.get("dvol"),
        Some(&(Some(100.0), Some(100_000.0)))
    );
    assert!(card.factory.contains("dvol"));
    // One typed side makes the field the user's, the factory side still a bound.
    assert_eq!(card.bounds.get("hvol"), Some(&(Some(25.0), Some(10_000.0))));
    assert!(!card.factory.contains("hvol"));
    // Zero is a real lower bound of a delta, and the factory one.
    assert_eq!(card.bounds.get("d3h"), Some(&(Some(0.0), Some(20.0))));
    assert!(card.factory.contains("d3h"));
    // The fast hourly volume ignores zero on both sides; the leverage cap ignores it too.
    assert_eq!(card.bounds.get("hvolf"), None);
    assert_eq!(card.bounds.get("lev"), Some(&(Some(1.0), None)));
}

/// A slot's thresholds are bounds at their factory values too, and a slot of a type the report
/// has no column for is a live filter the save must not treat as free.
#[test]
fn slot_thresholds_and_foreign_slots_count_at_factory_values() {
    let raw = r#"{
        "Delta2_Type": "1h", "Delta2_Min": 0, "Delta2_Max": 20,
        "Delta3_Type": "2h", "Delta3_Min": 0, "Delta3_Max": 1000
    }"#;
    let card = filters_of(raw, &defaults()).expect("a card");
    assert_eq!(card.slot_of("d1h"), Some((2, Some(0.0), Some(20.0))));
    assert_eq!(card.foreign_slots, vec![(3, "2h".to_string())]);
    assert!(
        card.factory.contains("d1h"),
        "a slot's factory thresholds are marked"
    );
    // One typed threshold makes the slot the user's.
    let typed = r#"{"Delta2_Type": "1h", "Delta2_Min": 0, "Delta2_Max": 7}"#;
    let card = filters_of(typed, &defaults()).expect("a card");
    assert_eq!(card.slot_of("d1h"), Some((2, Some(0.0), Some(7.0))));
    assert!(!card.factory.contains("d1h"));
}

/// The limits outside the grid are listed only where the core checks them: a zero price step
/// blocks every coin and only 100 or more switches it off, a ping cap needs a latency cap, the
/// other pings are off at zero.
#[test]
fn extras_list_the_limits_the_core_checks_at_their_stored_values() {
    let listed = |raw: &str| -> Vec<(&'static str, f64)> {
        filters_of(raw, &defaults())
            .expect("a card")
            .extras
            .iter()
            .map(|(extra, value)| (extra.key, *value))
            .collect()
    };
    assert_eq!(
        listed(r#"{"PenaltyTime": 300, "PriceStepMax": "0,5", "MinPing": 0, "MaxPing": 40}"#),
        vec![("PenaltyTime", 300.0), ("PriceStepMax", 0.5)],
        "a ping cap without a latency cap is not checked"
    );
    assert_eq!(
        listed(r#"{"PenaltyTime": 0, "PriceStepMax": 0, "MaxLatency": 150, "MaxPing": 40}"#),
        vec![
            ("PriceStepMax", 0.0),
            ("MaxLatency", 150.0),
            ("MaxPing", 40.0)
        ],
        "a zero price step is a live limit"
    );
    assert!(listed(r#"{"PriceStepMax": 100}"#).is_empty());
    assert!(
        listed(r#"{"PriceStepMax": 250}"#).is_empty(),
        "anything from 100 up is off"
    );
    // The rest of the Time, Price and Delta groups, in the table's order: a pause or a cap is
    // off at zero, a session or funding limit means something below it.
    assert_eq!(
        listed(
            r#"{"TradePenaltyTime": 60, "FundingBefore": -30, "FundingAfter": 0,
                "MaxBalance": 0, "MaxPosition": 500, "SessionProfitMin": -20,
                "SessionProfitMax": 0, "TotalLoss": 0, "WorkingPriceMin": 0.5,
                "WorkingPriceMax": 0, "PriceStepMin": 0.01, "PriceStepMax": 100,
                "GlobalFilterPenalty": 120, "FilterMin": 0, "FilterMax": -1.5}"#
        ),
        vec![
            ("TradePenaltyTime", 60.0),
            ("FundingBefore", -30.0),
            ("MaxPosition", 500.0),
            ("SessionProfitMin", -20.0),
            ("WorkingPriceMin", 0.5),
            ("PriceStepMin", 0.01),
            ("GlobalFilterPenalty", 120.0),
            ("FilterMax", -1.5),
        ]
    );
    // Each one sleeps under its own group's flag as well as the master one.
    let gates = |key: &str| {
        WAKE_EXTRAS
            .iter()
            .find(|extra| extra.key == key)
            .map(|extra| extra.gates.to_vec())
    };
    assert_eq!(
        gates("TradePenaltyTime"),
        Some(vec!["IgnoreFilters", "IgnoreTime"])
    );
    assert_eq!(
        gates("PriceStepMin"),
        Some(vec!["IgnoreFilters", "IgnorePrice"])
    );
    assert_eq!(
        gates("FilterMax"),
        Some(vec!["IgnoreFilters", "IgnoreDelta"])
    );
    let card = filters_of(r#"{"IgnoreTime": "YES", "IgnorePing": true}"#, &defaults()).unwrap();
    assert!(card.ignores("IgnoreTime") && card.ignores("IgnorePing"));
    assert!(!card.ignores("IgnorePrice") && !card.ignores("IgnoreFilters"));
    assert!(filters_of("[]", &defaults()).is_none());
}

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
