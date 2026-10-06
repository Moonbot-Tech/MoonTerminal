use super::*;

/// Frozen pre-optimization strip and hash encoding, preserving insertion order and clone behavior.
fn old_hash_of(m: &Map<String, Value>, ignore: &HashSet<String>) -> i64 {
    let stripped: Map<String, Value> = m
        .iter()
        .filter(|(k, _)| !k.starts_with("__") && !ignore.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let s = serde_json::to_string(&Value::Object(stripped.clone())).unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish() as i64
}

/// Changing the hash encoder's order or filtering must not rewrite persisted strategy history.
#[test]
fn content_hash_matches_previous_encoding() {
    let mut fields = Map::new();
    fields.insert(
        "zeta".into(),
        Value::from("\u{041f}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442} \u{1f680}"),
    );
    fields.insert(
        "alpha".into(),
        serde_json::json!({"z": [null, true, {"nested": "\u{00e9}"}], "a": 1}),
    );
    fields.insert("float".into(), Value::from(1.23456789));
    fields.insert("negative_zero".into(), Value::from(-0.0));
    fields.insert("tiny".into(), Value::from(1e-30));
    fields.insert("huge".into(), Value::from(1e30));
    fields.insert(
        "__metadata".into(),
        serde_json::json!(["ignored", {"x": 2}]),
    );
    fields.insert("Comment".into(), Value::from("cosmetic"));
    fields.insert("StrategyName".into(), Value::from("presentation"));
    let reversed = fields
        .iter()
        .rev()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let ignored_only = Map::from_iter([
        ("__x".into(), Value::from(1)),
        ("Comment".into(), Value::from("ignored")),
    ]);
    let cases = [Map::new(), fields, reversed, ignored_only];
    for ignore in [
        HashSet::new(),
        HashSet::from(["Comment".into(), "StrategyName".into()]),
    ] {
        for fields in &cases {
            assert_eq!(
                old_hash_of(fields, &ignore),
                hash_of(&strip(fields, &ignore))
            );
            assert_eq!(
                serde_json::to_string(fields).unwrap(),
                serde_json::to_string(&Value::Object(fields.clone())).unwrap()
            );
        }
    }
}

/// Measure the echoed full-set cost separately from first-set insertion and fixture creation.
#[test]
#[ignore = "synthetic release benchmark"]
fn bench_full_set_unchanged() {
    let (conn, mut st) = setup();
    let dumps = benchmark_dumps();
    assert_eq!(
        apply_full_set(&conn, &mut st, 7, "core", true, &dumps).unwrap(),
        2_000
    );
    let mut timings = Vec::with_capacity(5);
    for _ in 0..5 {
        let start = std::time::Instant::now();
        let changed = apply_full_set(&conn, &mut st, 7, "core", false, &dumps).unwrap();
        timings.push(start.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(changed, 0, "an echoed full set must not create history");
    }
    let versions: i64 = conn
        .query_row("SELECT COUNT(*) FROM strategy_versions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(versions, 2_000);
    timings.sort_by(f64::total_cmp);
    println!("[BENCH] full_set_unchanged median_ms={:.3}", timings[2]);
}

/// The common 2,000-strategy, 300-field workload for echoed and changed full sets.
fn benchmark_dumps() -> Vec<StratDump> {
    (1..=2_000)
        .map(|id| {
            let mut d = dump(id, "synthetic", 5, "cosmetic");
            d.fields = (0..300)
                .map(|field| (format!("Field{field:03}"), Value::from(id * 300 + field)))
                .collect();
            d
        })
        .collect()
}

/// Measure version creation when one field changes in 200 of the 2,000 strategies.
#[test]
#[ignore = "synthetic release benchmark"]
fn bench_full_set_one_changed() {
    let mut timings = Vec::with_capacity(5);
    for _ in 0..5 {
        let (conn, mut st) = setup();
        let mut dumps = benchmark_dumps();
        assert_eq!(
            apply_full_set(&conn, &mut st, 7, "core", true, &dumps).unwrap(),
            2_000
        );
        for d in &mut dumps[..200] {
            d.fields.insert("Field000".into(), Value::from(-1));
        }
        let start = std::time::Instant::now();
        let changed = apply_full_set(&conn, &mut st, 7, "core", false, &dumps).unwrap();
        timings.push(start.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(changed, 200);
        let versions: i64 = conn
            .query_row("SELECT COUNT(*) FROM strategy_versions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(versions, 2_200);
        let changed_fields: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM strategy_versions WHERE change_kind='params' AND n_changed=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(changed_fields, 200);
    }
    timings.sort_by(f64::total_cmp);
    println!("[BENCH] full_set_one_changed median_ms={:.3}", timings[2]);
}

fn cfg() -> StrategiesStoreCfg {
    StrategiesStoreCfg::default()
}

fn dump(id: i64, name: &str, tp: i64, comment: &str) -> StratDump {
    let mut fields = Map::new();
    fields.insert("StrategyName".into(), Value::from(name));
    fields.insert("TakeProfit".into(), Value::from(tp));
    fields.insert("Comment".into(), Value::from(comment)); // Cosmetic field (ignored).
    StratDump {
        strategy_id: id,
        name: name.into(),
        kind: "Drops".into(),
        kind_ordinal: 2,
        folder_path: "f".into(),
        is_short: false,
        checked: false,
        server_ver: 1,
        server_ms: 1000,
        fields,
        local_edit: false,
    }
}

fn versions(conn: &Connection, id: i64) -> Vec<(String, i64, Option<i64>)> {
    let mut stmt = conn
        .prepare(
            "SELECT change_kind, n_changed, valid_to FROM strategy_versions
             WHERE strategy_id=?1 ORDER BY valid_from",
        )
        .unwrap();
    stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn setup() -> (Connection, State) {
    let conn = Connection::open_in_memory().unwrap();
    let st = init(&conn, &cfg()).unwrap();
    (conn, st)
}

/// Regression target: deleting `idx_strat_sid` creation or legacy `idx_sv_lookup` cleanup from
/// `write.rs:init` breaks the `has(...)` or query-plan assertion and would make upgraded databases
/// retain extra version-write overhead or lose indexed tuner lookups by strategy ID.
#[test]
fn init_migrates_indexes() {
    let (conn, _) = setup();
    conn.execute(
        "CREATE INDEX idx_sv_lookup
         ON strategy_versions(core_uid, strategy_id, valid_from DESC)",
        [],
    )
    .unwrap();
    let _ = init(&conn, &cfg()).unwrap();
    let has = |name: &str| -> bool {
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            > 0
    };
    assert!(
        !has("idx_sv_lookup"),
        "дубль UNIQUE-индекса должен сноситься"
    );
    assert!(has("idx_strat_sid"));
    let plan: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN
             SELECT core_uid FROM strategies WHERE strategy_id=1 AND deleted=0",
            [],
            |r| r.get(3),
        )
        .unwrap();
    assert!(plan.contains("idx_strat_sid"), "план без индекса: {plan}");
}

#[test]
fn create_then_cosmetic_then_param() {
    let (conn, mut st) = setup();
    // Creation produces a `created` version.
    apply_full_set(&conn, &mut st, 7, "core", true, &[dump(1, "A", 5, "x")]).unwrap();
    assert_eq!(versions(&conn, 1).len(), 1);
    assert_eq!(versions(&conn, 1)[0].0, "created");
    // Changing the cosmetic `Comment` field does not create a version.
    apply_full_set(&conn, &mut st, 7, "core", false, &[dump(1, "A", 5, "y")]).unwrap();
    assert_eq!(versions(&conn, 1).len(), 1);
    // A real `TakeProfit` edit creates a `params` version and closes the previous one.
    apply_full_set(&conn, &mut st, 7, "core", false, &[dump(1, "A", 7, "y")]).unwrap();
    let v = versions(&conn, 1);
    assert_eq!(v.len(), 2);
    assert_eq!(v[1].0, "params");
    assert_eq!(v[1].1, 1, "изменено одно поле");
    assert!(v[0].2.is_some(), "первая версия закрыта");
    assert!(v[1].2.is_none(), "текущая открыта");
}

#[test]
fn rename_does_not_version_but_updates_head() {
    let (conn, mut st) = setup();
    apply_full_set(&conn, &mut st, 7, "core", true, &[dump(1, "A", 5, "x")]).unwrap();
    // `StrategyName` is ignored, so renaming updates only the head without a new version.
    apply_full_set(&conn, &mut st, 7, "core", false, &[dump(1, "B", 5, "x")]).unwrap();
    assert_eq!(versions(&conn, 1).len(), 1);
    let name: String = conn
        .query_row("SELECT name FROM strategies WHERE strategy_id=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(name, "B");
}

#[test]
fn restore_same_content_reopens_version() {
    let (conn, mut st) = setup();
    apply_full_set(&conn, &mut st, 7, "core", true, &[dump(1, "A", 5, "x")]).unwrap();
    // Removing the strategy from the set closes its version and sets `head.deleted=1`.
    apply_full_set(&conn, &mut st, 7, "core", false, &[dump(2, "B", 3, "x")]).unwrap();
    assert!(versions(&conn, 1)[0].2.is_some(), "закрыта при удалении");
    // Restoring the same content reopens the old version instead of creating a new one.
    apply_full_set(
        &conn,
        &mut st,
        7,
        "core",
        false,
        &[dump(1, "A", 5, "x"), dump(2, "B", 3, "x")],
    )
    .unwrap();
    let v = versions(&conn, 1);
    assert_eq!(v.len(), 1, "restored-версия не создана");
    assert!(v[0].2.is_none(), "версия переоткрыта (valid_to=NULL)");
    let del: i64 = conn
        .query_row(
            "SELECT deleted FROM strategies WHERE strategy_id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(del, 0);
}

#[test]
fn missing_marks_deleted_and_reappear_restores() {
    let (conn, mut st) = setup();
    apply_full_set(
        &conn,
        &mut st,
        7,
        "core",
        true,
        &[dump(1, "A", 5, "x"), dump(2, "B", 3, "x")],
    )
    .unwrap();
    // Strategy 2 disappears from the complete set, so it is deleted and its version is closed.
    apply_full_set(&conn, &mut st, 7, "core", false, &[dump(1, "A", 5, "x")]).unwrap();
    let del: i64 = conn
        .query_row(
            "SELECT deleted FROM strategies WHERE strategy_id=2",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(del, 1);
    assert!(
        versions(&conn, 2)[0].2.is_some(),
        "версия удалённой закрыта"
    );
    // Restoring it with a change (`TakeProfit` 3 to 9) creates a new `restored` version.
    apply_full_set(
        &conn,
        &mut st,
        7,
        "core",
        false,
        &[dump(1, "A", 5, "x"), dump(2, "B", 9, "x")],
    )
    .unwrap();
    let v = versions(&conn, 2);
    assert_eq!(v.last().unwrap().0, "restored");
}

#[test]
fn empty_set_does_not_mass_delete() {
    let (conn, mut st) = setup();
    apply_full_set(&conn, &mut st, 7, "core", true, &[dump(1, "A", 5, "x")]).unwrap();
    apply_full_set(&conn, &mut st, 7, "core", false, &[]).unwrap();
    let del: i64 = conn
        .query_row(
            "SELECT deleted FROM strategies WHERE strategy_id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(del, 0, "пустой набор не считается «удалили всё»");
}

#[test]
fn state_cache_survives_reload() {
    let (conn, mut st) = setup();
    apply_full_set(&conn, &mut st, 7, "core", true, &[dump(1, "A", 5, "x")]).unwrap();
    // Simulate a writer restart: reload state from disk and preserve deduplication.
    let mut st2 = init(&conn, &cfg()).unwrap();
    apply_full_set(&conn, &mut st2, 7, "core", false, &[dump(1, "A", 5, "x")]).unwrap();
    assert_eq!(
        versions(&conn, 1).len(),
        1,
        "эхо после рестарта не плодит версию"
    );
}

/// Dropping `write.rs:forget`'s `st.heads.remove(&(uid, id))` eviction lets a later FullSet
/// resurrect a forgotten strategy as `restored`; removing `AND deleted=1` instead purges a live
/// head. Either edit silently restores historyless data or destroys a live strategy.
#[test]
fn forget_evicts_deleted_heads_and_preserves_live_heads() {
    let (conn, mut st) = setup();
    apply_full_set(
        &conn,
        &mut st,
        7,
        "core",
        true,
        &[dump(1, "deleted", 5, "x")],
    )
    .unwrap();
    apply_full_set(
        &conn,
        &mut st,
        7,
        "core",
        false,
        &[dump(2, "other", 3, "x")],
    )
    .unwrap();

    forget(&conn, &mut st, 7, &[1]).unwrap();
    apply_full_set(
        &conn,
        &mut st,
        7,
        "core",
        false,
        &[dump(1, "deleted", 5, "x")],
    )
    .unwrap();
    assert_eq!(
        versions(&conn, 1),
        vec![("created".to_string(), 0, None)],
        "a forgotten id must be a new created history, never a restored stale head"
    );

    forget(&conn, &mut st, 7, &[1]).unwrap();
    let live: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM strategies WHERE core_uid=7 AND strategy_id=1 AND deleted=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(live, 1, "forget must not purge a live head");
}
