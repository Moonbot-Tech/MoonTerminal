use super::*;

/// Индексы реплики создаются и для БД, где колонки появились ДО кода
/// индексов: создание только в ветке успешного ALTER теряло индекс
/// навсегда (колонка уже есть → цикл схемы её скипает).
#[test]
fn rep_indexes_created_for_preexisting_columns() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    // Скелет реплики с колонками, но БЕЗ индексов — как БД старой версии.
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL,
            core_name TEXT NOT NULL, newrecid INTEGER NOT NULL,
            PRIMARY KEY (core_uid, newrecid));
         ALTER TABLE orders_rep ADD COLUMN closedate INTEGER;
         ALTER TABLE orders_rep ADD COLUMN strategyid INTEGER;
         ALTER TABLE orders_rep ADD COLUMN buydate INTEGER;",
    )
    .unwrap();
    let cursors = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let open_rows = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    rep::init(&conn, cursors, open_rows).unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index'
             AND name IN ('idx_rep_closedate','idx_rep_strat')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 2, "оба индекса должны существовать после init");
    // Планировщик реально ходит по индексу для дефолтного фильтра периода.
    let plan: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT * FROM orders_rep
             WHERE closedate >= 1 AND closedate < 2 AND closedate > 0",
            [],
            |r| r.get(3),
        )
        .unwrap();
    assert!(
        plan.contains("idx_rep_closedate"),
        "план без индекса: {plan}"
    );
}

/// Переходный читатель: строки из ЛЕГАСИ-таблицы и typed-реплики видны вместе
/// (UNION ALL), db_id легаси отдаётся как `id`; после SyncComplete последнего
/// легаси-ядра таблица сносится и читатель живёт на одной реплике.
#[test]
fn union_reader_and_legacy_drop() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();

    // Легаси-таблица + строка ядра 1. `init_db` её больше НЕ создаёт (на переходном
    // периоде она уже есть у пользователя); воссоздаём минимальную схему ДО `rep::init`,
    // чтобы он увидел `legacy_exists=true` и умел её снести на SyncComplete.
    conn.execute_batch(
        "CREATE TABLE closed_sell_reports (
            core_uid INTEGER NOT NULL, core_name TEXT NOT NULL, db_id INTEGER NOT NULL,
            coin TEXT, profitbtc REAL, closedate INTEGER,
            created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
            PRIMARY KEY (core_uid, db_id));
         INSERT INTO closed_sell_reports
            (core_uid, core_name, db_id, coin, profitbtc, closedate, created_ms, updated_ms)
         VALUES (1, 'BB1', 42, 'BTCUSDT', 0.5, 1780000000, 0, 0);",
    )
    .unwrap();

    let cursors = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let open_rows = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut st = rep::init(&conn, cursors, open_rows).unwrap();

    // Строка реплики ядра 2 (колонки — как их дорастила бы схема ядра).
    for ddl in [
        "ALTER TABLE orders_rep ADD COLUMN coin TEXT",
        "ALTER TABLE orders_rep ADD COLUMN profitbtc REAL",
        "ALTER TABLE orders_rep ADD COLUMN closedate INTEGER",
        "ALTER TABLE orders_rep ADD COLUMN id INTEGER",
    ] {
        conn.execute(ddl, []).unwrap();
    }
    conn.execute(
        "INSERT INTO orders_rep (core_uid, core_name, newrecid, coin, profitbtc, closedate, id)
         VALUES (2, 'Rep', 7, 'ETHUSDT', 1.5, 1780000100, 99)",
        [],
    )
    .unwrap();

    let cols = display_columns(&conn).expect("схема читается");
    assert!(cols.iter().any(|c| c == "id"), "cols: {cols:?}");
    assert!(!cols.iter().any(|c| c == "db_id" || c == "newrecid"));

    let t = query_reports(&conn, &ReportFilter::default(), "closedate", true, 10)
        .expect("выборка читается");
    assert_eq!(t.rows.len(), 2);
    assert!(t.core_uids.contains(&1) && t.core_uids.contains(&2));
    // Легаси db_id виден в колонке id.
    let id_ix = t.cols.iter().position(|c| c == "id").unwrap();
    let legacy_row = t.core_uids.iter().position(|u| *u == 1).unwrap();
    assert_eq!(t.rows[legacy_row][id_ix], Value::Integer(42));

    let (profit, count) = query_totals(&conn, &ReportFilter::default()).expect("итоги читаются");
    assert_eq!(count, 2);
    assert!((profit - 2.0).abs() < 1e-9);

    // SyncComplete ядра 1 → его легаси-строки вычищены; таблица опустела → DROP.
    let done = moonproto::ReportSyncComplete {
        ticket: moonproto::ReportSyncTicket { sync_id: 1 },
        page_count: 0,
        total_rows: 0,
        max_rec_id: 10,
        next_from_rec_id: 11,
    };
    rep::apply_sync_complete(&conn, &mut st, 1, &done);
    let legacy_left = table_columns_res(&conn).expect("схема читается");
    assert!(legacy_left.is_empty(), "легаси-таблица должна быть снесена");
    let t = query_reports(&conn, &ReportFilter::default(), "closedate", true, 10)
        .expect("выборка читается после сноса легаси");
    assert_eq!(t.rows.len(), 1);
    assert_eq!(t.core_uids, vec![2]);
    // init_db больше не воскрешает легаси (маркер legacy_dropped).
    init_db(&conn).unwrap();
    assert!(table_columns_res(&conn).expect("схема читается").is_empty());
}

/// Index-page damage fails the report query instead of returning a table
/// that the panel or file export could mistake for a complete result.
#[test]
fn corrupt_replica_fails_report_query_instead_of_truncating() {
    let path = test_support::temp_db("report-rows");
    let day = 1_780_000_000i64 / 86_400 * 86_400;
    let conn = test_support::build_replica(&path, &test_support::spread_rows(day, 2000));

    let filter = ReportFilter {
        date_from: Some(day),
        date_to: Some(day + 2000 * 60),
        emulator: Some(false),
        ..Default::default()
    };
    // The fixture is healthy first, so the assertions below cannot pass for
    // a trivial reason (an empty or unreadable database).
    let before =
        query_reports(&conn, &filter, "closedate", true, 5_000).expect("до порчи выборка читается");
    assert_eq!(before.rows.len(), 2000);

    // Pin the plan: if the planner stops using this index, the test must fail
    // loudly rather than quietly stop exercising the damaged page.
    let plan: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT core_uid FROM orders_rep
             WHERE closedate IS NOT NULL AND closedate >= 1 AND closedate <= 2
             ORDER BY closedate DESC",
            [],
            |r| r.get(3),
        )
        .unwrap();
    assert!(
        plan.contains("idx_rep_closedate"),
        "план без индекса: {plan}"
    );

    test_support::corrupt_leaf_page(conn, &path, "idx_rep_closedate");
    let conn = Connection::open(&path).expect("битая БД всё ещё открывается");

    let res = query_reports(&conn, &filter, "closedate", true, 5_000);
    assert!(
        !matches!(res, Ok(_)),
        "сбой чтения обязан вернуть ошибку, а не частичную/пустую таблицу: \
         такой файл экспорта неотличим от полного"
    );
    assert!(
        matches!(
            res,
            Err(ReadFail::Failed {
                kind: FailKind::Corrupt,
                ..
            })
        ),
        "порча должна классифицироваться как Corrupt"
    );

    drop(conn);
    test_support::remove_db(&path);
}

/// Aggregate corruption returns an error rather than the zero totals of a
/// healthy empty period.
#[test]
fn corrupt_replica_fails_report_totals_instead_of_zeroing() {
    let path = test_support::temp_db("report-totals");
    let day = 1_780_000_000i64 / 86_400 * 86_400;
    let conn = test_support::build_replica(&path, &test_support::spread_rows(day, 2000));

    // No date bounds: the aggregate must read the table itself, so damaging a
    // TABLE leaf (not an index) is what this scan is guaranteed to hit.
    let filter = ReportFilter {
        emulator: Some(false),
        ..Default::default()
    };
    let (profit, count) = query_totals(&conn, &filter).expect("до порчи итоги читаются");
    assert_eq!(count, 2000);
    assert!(profit.is_finite());

    test_support::corrupt_leaf_page(conn, &path, "orders_rep");
    let conn = Connection::open(&path).expect("битая БД всё ещё открывается");

    let res = query_totals(&conn, &filter);
    assert!(
        !matches!(res, Ok(_)),
        "сбой агрегата обязан вернуть ошибку, а не (0.0, 0)"
    );

    drop(conn);
    test_support::remove_db(&path);
}

/// Protects `max_core_uid`: it must fold BOTH report schemas, and a source whose table does not
/// exist yet must read as "nothing here" rather than failing the probe.
///
/// The plausible edit: dropping the `core_uid` column guard and querying every source
/// `read_sources_res` reports. That function always reports the modern table, which does not
/// exist until `rep::init` has run, so a replica that has only ever held legacy rows would turn
/// a healthy "no rows" into a hard read failure.
///
/// Consequence: the caller treats a failure as "this store contributes nothing", which is
/// precisely the state in which a deleted core's uid gets handed to a new core along with its
/// trades and P&L.
#[test]
fn max_core_uid_folds_both_report_schemas() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();

    // Neither the legacy table (init_db no longer creates it) nor `orders_rep` exists yet;
    // an empty store is not a failure.
    assert!(
        matches!(max_core_uid(&conn), Ok(None)),
        "an empty replica must read as no rows, never as a read failure"
    );

    // Legacy rows still exist on the transition period; seed the table here (init_db no
    // longer creates it) to exercise the cross-schema uid fold.
    conn.execute_batch(
        "CREATE TABLE closed_sell_reports (
            core_uid INTEGER NOT NULL, core_name TEXT NOT NULL, db_id INTEGER NOT NULL,
            created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
            PRIMARY KEY (core_uid, db_id));
         INSERT INTO closed_sell_reports (core_uid, core_name, db_id, created_ms, updated_ms)
         VALUES (12, 'legacy', 1, 0, 0);",
    )
    .unwrap();
    assert!(
        matches!(max_core_uid(&conn), Ok(Some(12))),
        "a uid living only in the legacy schema still has to raise the mark"
    );

    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL,
            core_name TEXT NOT NULL, newrecid INTEGER NOT NULL,
            PRIMARY KEY (core_uid, newrecid));
         INSERT INTO orders_rep VALUES (7, 'modern', 1);",
    )
    .unwrap();
    assert!(
        matches!(max_core_uid(&conn), Ok(Some(12))),
        "the mark is the maximum ACROSS schemas, not the last one queried"
    );

    conn.execute("INSERT INTO orders_rep VALUES (30, 'modern', 2)", [])
        .unwrap();
    assert!(matches!(max_core_uid(&conn), Ok(Some(30))));
}
