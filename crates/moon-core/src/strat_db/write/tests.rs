use super::*;

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
