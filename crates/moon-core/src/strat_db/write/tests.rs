use super::*;

fn cfg() -> StrategiesStoreCfg {
    StrategiesStoreCfg::default()
}

fn dump(id: i64, name: &str, tp: i64, comment: &str) -> StratDump {
    let mut fields = Map::new();
    fields.insert("StrategyName".into(), Value::from(name));
    fields.insert("TakeProfit".into(), Value::from(tp));
    fields.insert("Comment".into(), Value::from(comment)); // косметика (игнор)
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

/// Миграция индексов на БД старой версии: дублирующий idx_sv_lookup
/// сносится, idx_strat_sid появляется и реально используется планировщиком
/// для лукапов по strategy_id без core_uid (strategy_cores и т.п.).
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
    // Создание → версия created.
    apply_full_set(&conn, &mut st, 7, "core", true, &[dump(1, "A", 5, "x")]).unwrap();
    assert_eq!(versions(&conn, 1).len(), 1);
    assert_eq!(versions(&conn, 1)[0].0, "created");
    // Косметика (Comment) — версия НЕ создаётся.
    apply_full_set(&conn, &mut st, 7, "core", false, &[dump(1, "A", 5, "y")]).unwrap();
    assert_eq!(versions(&conn, 1).len(), 1);
    // Реальная правка TakeProfit → версия params, прошлая закрыта valid_to.
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
    // StrategyName в игноре: переименование — только head, без версии.
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
    // Удалили (пропала из набора) → версия закрыта, head.deleted=1.
    apply_full_set(&conn, &mut st, 7, "core", false, &[dump(2, "B", 3, "x")]).unwrap();
    assert!(versions(&conn, 1)[0].2.is_some(), "закрыта при удалении");
    // Вернулась С ТЕМ ЖЕ контентом → НЕ новая версия, старая переоткрыта.
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
    // Стратегия 2 пропала из полного набора → deleted, версия закрыта.
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
    // Вернулась С ИЗМЕНЕНИЕМ (TP 3→9) → новая версия restored.
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
    // «Рестарт writer'а»: state перечитывается с диска, дедуп не ломается.
    let mut st2 = init(&conn, &cfg()).unwrap();
    apply_full_set(&conn, &mut st2, 7, "core", false, &[dump(1, "A", 5, "x")]).unwrap();
    assert_eq!(
        versions(&conn, 1).len(),
        1,
        "эхо после рестарта не плодит версию"
    );
}
