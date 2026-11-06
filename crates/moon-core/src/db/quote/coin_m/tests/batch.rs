//! Each case runs alone with the actual process-wide knowledge used by production readers.
use super::*;
use crate::db::{ReportFilter, RowScope, query_totals, read_snapshot};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);
static START: OnceLock<mpsc::Sender<()>> = OnceLock::new();
static DONE: OnceLock<Mutex<mpsc::Receiver<()>>> = OnceLock::new();
static TRIGGERED: AtomicBool = AtomicBool::new(false);

/// Exclude the named proof row from money while keeping it available to source discovery.
fn filter() -> ReportFilter {
    ReportFilter {
        core_uids: vec![7],
        rows: RowScope::Closed,
        exact_coins: Some(vec!["BNB_0927".into()]),
        ..Default::default()
    }
}

/// Check the final public result, not the generated SQL or the cache's own stored fields.
fn assert_money(conn: &Connection, ordinal: u8, amount: f64) {
    let totals = query_totals(conn, &filter()).unwrap();
    assert_eq!(totals.quotes.orders, 2);
    assert_eq!(
        totals.quotes.totals.len(),
        1,
        "one batch must not mix money interpretations"
    );
    let total = totals.quotes.totals[0];
    assert_eq!(total.currency.ordinal(), ordinal);
    assert!((total.profit - amount).abs() < 1e-12, "{total:?}");
}

/// Two physically separate, financially identical liquidations expose a mixed interpretation.
fn seed(path: &std::path::Path, proof: bool) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL;
        CREATE TABLE orders_rep(core_uid INTEGER, newrecid INTEGER, closedate INTEGER,
        profitbtc REAL, spentbtc REAL, basecurrency INTEGER, coin TEXT, fname TEXT,
        sellreason TEXT, buyprice REAL, PRIMARY KEY(core_uid,newrecid));
        CREATE TABLE closed_sell_reports(core_uid INTEGER, db_id INTEGER, closedate INTEGER,
        profitbtc REAL, spentbtc REAL, basecurrency INTEGER, coin TEXT, fname TEXT,
        sellreason TEXT, buyprice REAL);
        INSERT INTO orders_rep VALUES(7,1,1800000000,-0.01,0.01,1,'BNB_0927','','LIQUIDATION',100);
        INSERT INTO closed_sell_reports VALUES(7,2,1800000060,-0.01,0.01,1,'BNB_0927','','LIQUIDATION',100);").unwrap();
    if proof {
        add_proof(&conn);
    }
    conn
}

/// Supply the named evidence that distinguishes COIN-M from ordinary dated USD-M liquidation.
fn add_proof(conn: &Connection) {
    conn.execute_batch("INSERT INTO orders_rep VALUES(7,3,1800000120,0,0,1,'ETH_0927','Pump_USD-ETH_0927_x','Sell Price',100);").unwrap();
}

/// Schedule the competing reader after one source's money SQL has already been built.
fn interleave(sql: &str) {
    if sql.starts_with("SELECT ")
        && sql.contains("SUM(")
        && sql.contains(" FROM orders_rep r")
        && !TRIGGERED.swap(true, Ordering::SeqCst)
    {
        START.get().unwrap().send(()).unwrap();
        DONE.get()
            .unwrap()
            .lock()
            .unwrap()
            .recv_timeout(WAIT)
            .unwrap();
    }
}

/// A later proof or replica wipe cannot change the second half of an already running batch.
fn grow(path: std::path::PathBuf, forget_running: bool) {
    let writer = seed(&path, forget_running);
    let mut old = Connection::open(&path).unwrap();
    #[allow(deprecated)]
    old.trace(Some(interleave));
    let snapshot = read_snapshot(&old).unwrap();
    if !forget_running {
        add_proof(&writer);
    } else {
        assert_eq!(
            snapshot
                .query_row(
                    "SELECT COUNT(*) FROM orders_rep WHERE fname LIKE '%USD-%'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1,
            "the old WAL itself carries complete COIN-M evidence"
        );
    }
    let (start, started) = mpsc::channel();
    let (done, completed) = mpsc::channel();
    START.set(start).unwrap();
    DONE.set(Mutex::new(completed)).unwrap();
    let worker_path = path.clone();
    let worker = std::thread::spawn(move || {
        started.recv_timeout(WAIT).unwrap();
        let fresh = Connection::open(worker_path).unwrap();
        if forget_running {
            let publication =
                crate::db::report_writer::commit_stateful_batch(&writer, &mut (), |tx, _| {
                    tx.execute_batch("DELETE FROM orders_rep; DELETE FROM closed_sell_reports;")
                        .unwrap();
                    let publication = publication_guard();
                    Ok(publication)
                })
                .unwrap();
            forget_core(7);
            drop(publication);
            assert_eq!(query_totals(&fresh, &filter()).unwrap().quotes.orders, 0);
        } else {
            assert_money(&fresh, 0, -2.0);
        }
        done.send(()).unwrap();
    });
    let (ordinal, amount) = if forget_running {
        (0, -2.0)
    } else {
        (1, -0.02)
    };
    assert_money(&snapshot, ordinal, amount);
    worker.join().unwrap();
    // Nested public reads inherit the original interpretation even after another reader learns.
    assert_money(&snapshot, ordinal, amount);
    if !forget_running {
        let nested_conn = Connection::open(&path).unwrap();
        let nested = read_snapshot(&nested_conn).unwrap();
        assert_money(&nested, ordinal, amount);
        drop(nested);
        assert_money(&snapshot, ordinal, amount);
    }
    drop(snapshot);
    if forget_running {
        assert_eq!(query_totals(&old, &filter()).unwrap().quotes.orders, 0);
    } else {
        assert_money(&old, 0, -2.0);
    }
    assert!(TRIGGERED.load(Ordering::SeqCst));
}

/// An old snapshot cannot restore obsolete proof or suppress discovery inside unchanged id bounds.
fn old_wal(path: std::path::PathBuf, forget: bool, raw_transaction: bool) {
    let writer = seed(&path, forget);
    if !forget {
        writer.execute_batch("INSERT INTO orders_rep VALUES(7,3,1800000120,0,0,1,'ETH_0927','Pump_USDT-ETH_0927_x','Sell Price',100);").unwrap();
    }
    assert_money(
        &writer,
        if forget { 0 } else { 1 },
        if forget { -2.0 } else { -0.02 },
    );
    let old = Connection::open(&path).unwrap();
    // No money query initializes this pin before the invalidation: it must learn its OLD WAL
    // locally afterwards, rather than restoring old proof or examined spans into the live cache.
    let snapshot = if raw_transaction {
        old.execute_batch("BEGIN; SELECT COUNT(*) FROM orders_rep;")
            .unwrap();
        None
    } else {
        Some(read_snapshot(&old).unwrap())
    };
    let worker = std::thread::spawn(move || {
        let mut state = ();
        let publication =
            crate::db::report_writer::commit_stateful_batch(&writer, &mut state, |tx, _| {
                if forget {
                    tx.execute("DELETE FROM orders_rep WHERE newrecid=3", [])
                        .unwrap();
                } else {
                    tx.execute(
                        "UPDATE orders_rep SET fname='Pump_USD-ETH_0927_x' WHERE newrecid=3",
                        [],
                    )
                    .unwrap();
                }
                let publication = publication_guard();
                Ok(publication)
            })
            .unwrap();
        if forget {
            forget_core(7);
        } else {
            reexamine_core(7);
        }
        drop(publication);
    });
    worker.join().unwrap();
    assert_money(
        snapshot
            .as_deref()
            .map(|transaction| &**transaction)
            .unwrap_or(&old),
        if forget { 0 } else { 1 },
        if forget { -2.0 } else { -0.02 },
    );
    assert_eq!(
        knowledge_revision(),
        None,
        "an old WAL cannot certify the invalidated live cache"
    );
    drop(snapshot);
    if raw_transaction {
        old.execute_batch("ROLLBACK").unwrap();
    }
    assert_money(
        &old,
        if forget { 1 } else { 0 },
        if forget { -0.02 } else { -2.0 },
    );
}

/// A new WAL cannot pair committed replacement rows with the previous invalidation revision.
fn commit_boundary(path: std::path::PathBuf) {
    let writer = seed(&path, true);
    assert_money(&writer, 0, -2.0);
    let (committed, commit_done) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut state = ();
        let publication =
            crate::db::report_writer::commit_stateful_batch(&writer, &mut state, |tx, _| {
                tx.execute("DELETE FROM orders_rep WHERE newrecid=3", [])
                    .unwrap();
                let publication = publication_guard();
                Ok(publication)
            })
            .unwrap();
        committed.send(()).unwrap();
        released.recv_timeout(WAIT).unwrap();
        forget_core(7);
        drop(publication);
    });
    commit_done.recv_timeout(WAIT).unwrap();
    let (entered, entry) = mpsc::channel();
    let (finished, finish) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let conn = Connection::open(path).unwrap();
        entered.send(()).unwrap();
        let snapshot = read_snapshot(&conn).unwrap();
        assert_money(&snapshot, 1, -0.02);
        finished.send(()).unwrap();
    });
    entry.recv_timeout(WAIT).unwrap();
    assert!(
        matches!(
            finish.recv_timeout(Duration::from_millis(30)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "new WAL must wait until the commit/invalidation publication boundary is released"
    );
    release.send(()).unwrap();
    finish.recv_timeout(WAIT).unwrap();
    worker.join().unwrap();
    reader.join().unwrap();
}

/// A late valid proof remains applicable across another core's wipe or a proof-preserving catch-up.
fn preserved_proof(path: std::path::PathBuf, reexamine: bool) {
    let writer = seed(&path, false);
    writer.execute_batch("INSERT INTO orders_rep VALUES(9,1,1800000000,1,1,1,'BTCUSDT','Pump_USDT-BTCUSDT_x','Sell Price',100);").unwrap();
    let old = Connection::open(&path).unwrap();
    let snapshot = read_snapshot(&old).unwrap();
    add_proof(&writer);
    std::thread::spawn(move || {
        assert_money(&writer, 0, -2.0);
        let publication =
            crate::db::report_writer::commit_stateful_batch(&writer, &mut (), |tx, _| {
                if !reexamine {
                    tx.execute("DELETE FROM orders_rep WHERE core_uid=9", [])
                        .unwrap();
                }
                Ok(publication_guard())
            })
            .unwrap();
        if reexamine {
            reexamine_core(7);
        } else {
            forget_core(9);
        }
        drop(publication);
        assert_money(&writer, 0, -2.0);
    })
    .join()
    .unwrap();
    assert_money(&snapshot, 0, -2.0);
}

#[test]
/// Exercise production's shared state in isolated processes instead of test-local knowledge.
fn money_batches_keep_one_interpretation_across_readers_and_replica_resets() {
    let Ok(case) = std::env::var("MOON_COIN_M_BATCH_CASE") else {
        for case in [
            "grow",
            "forget-running",
            "forget",
            "reexamine",
            "raw-forget",
            "commit",
            "unrelated-forget",
            "proof-reexamine",
        ] {
            let root =
                std::env::temp_dir().join(format!("moon-c101-{}-{case}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "db::quote::coin_m::tests::batch::money_batches_keep_one_interpretation_across_readers_and_replica_resets", "--nocapture"])
                .env("MOON_COIN_M_SHARED_TEST", "1").env("MOON_COIN_M_BATCH_CASE", case)
                .env("MOON_COIN_M_BATCH_ROOT", &root).output().unwrap();
            assert!(
                output.status.success(),
                "{case}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            std::fs::remove_dir_all(root).unwrap();
        }
        return;
    };
    let path = std::path::PathBuf::from(std::env::var_os("MOON_COIN_M_BATCH_ROOT").unwrap())
        .join("reports.sqlite");
    match case.as_str() {
        "grow" => grow(path, false),
        "forget-running" => grow(path, true),
        "forget" => old_wal(path, true, false),
        "reexamine" => old_wal(path, false, false),
        "raw-forget" => old_wal(path, true, true),
        "commit" => commit_boundary(path),
        "unrelated-forget" => preserved_proof(path, false),
        "proof-reexamine" => preserved_proof(path, true),
        _ => panic!("unknown isolated case"),
    }
}
