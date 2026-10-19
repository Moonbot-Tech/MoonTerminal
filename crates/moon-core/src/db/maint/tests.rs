//! Daily compaction slot, retry and low-disk regression tests using isolated files.

use super::*;
use crate::backups::{DAY_MS, DueOutcome, NOON_MS};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};

/// Distinguish parallel fixture directories in one test process.
static SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// Treating a still-running VACUUM as failed would enqueue it again in the same daily slot.
#[test]
fn timed_out_trades_compaction_finishes_the_daily_slot() {
    let fixture = Fixture::new();
    let dbs = fixture.databases();
    let marker = fixture.0.join("marker");
    let (reply, rx) = std::sync::mpsc::channel();
    let result = rx.recv_timeout(Duration::ZERO);
    compact_due_into(
        &marker,
        &dbs[2..3],
        NOON_MS,
        |_| true,
        |_| compact_daily_trades_result(result),
    )
    .unwrap();
    assert!(reply.send(Ok(())).is_ok());
    assert_eq!(rx.recv().unwrap(), Ok(()));
    assert!(matches!(
        compact_due_into(
            &marker,
            &dbs[2..3],
            NOON_MS + 1,
            |_| true,
            |_| panic!("long VACUUM retried")
        ),
        Ok(DueOutcome::Current(_))
    ));
    assert!(
        compact_daily_trades_result(Err(std::sync::mpsc::RecvTimeoutError::Disconnected)).is_err()
    );
}

/// Own only a uniquely created temporary directory, including cleanup after assertions.
struct Fixture(PathBuf);

impl Fixture {
    /// Create a root without touching any production path.
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "moon-compaction-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    /// Create three present database placeholders and one absent source.
    fn databases(&self) -> Vec<PathBuf> {
        let paths: Vec<_> = ["reports", "strategies", "trades", "absent"]
            .iter()
            .map(|name| self.0.join(name))
            .collect();
        for path in &paths[..3] {
            std::fs::write(path, []).unwrap();
        }
        paths
    }
}

impl Drop for Fixture {
    /// Remove only the owned fixture directory.
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Daily tape maintenance must use a cache worker and tolerate an unavailable handle.
#[test]
fn daily_trades_compaction_uses_the_worker_or_skips() {
    use crate::market::trade_replay::trade_cache::TradeCache;
    assert!(compact_daily_trades(None).is_ok());
    let cache = TradeCache::open_with_ceiling(":memory:".into(), || None).unwrap();
    compact_daily_trades(Some(cache)).unwrap();
}

/// Omitting either the temporary copy or WAL write-back would admit VACUUM below its budget.
#[test]
fn vacuum_room_counts_two_main_files_and_wal() {
    let fixture = Fixture::new();
    let db = fixture.0.join("sized.sqlite");
    std::fs::write(&db, [0; 100]).unwrap();
    std::fs::write(fixture.0.join("sized.sqlite-wal"), [0; 30]).unwrap();
    let total = 20 * 1024_u64.pow(3);
    let needed = 2 * 1024_u64.pow(3) + 230;
    assert!(!crate::disk_reserve::vacuum_has_room_with(&db, |_| Some((
        needed - 1,
        total
    ))));
    assert!(crate::disk_reserve::vacuum_has_room_with(&db, |_| Some((
        needed, total
    ))));
}

/// A refused reports lease must not prevent peer maintenance or leave the slot overdue.
#[test]
fn refused_reports_access_skips_only_reports_and_advances_slot() {
    let fixture = Fixture::new();
    let dbs = fixture.databases();
    let marker = fixture.0.join("marker");
    let maintenance = std::sync::Mutex::new(());
    let calls = RefCell::new(Vec::new());
    compact_due_into(
        &marker,
        &dbs,
        NOON_MS,
        |_| true,
        |db| {
            compact_daily_db(
                db,
                &dbs[0],
                &dbs[1],
                &maintenance,
                || anyhow::bail!("injected lease refusal"),
                |path| {
                    calls.borrow_mut().push(path.to_path_buf());
                    Ok(())
                },
            )
        },
    )
    .unwrap();
    assert_eq!(*calls.borrow(), vec![dbs[1].clone(), dbs[2].clone()]);
    assert_eq!(
        std::fs::read_to_string(marker).unwrap(),
        NOON_MS.to_string()
    );
}

/// Extending the strategy exclusion across peer VACUUMs would unnecessarily delay snapshots.
#[test]
fn strategy_lock_is_held_only_during_strategy_compaction() {
    let fixture = Fixture::new();
    let dbs = fixture.databases();
    let maintenance = std::sync::Mutex::new(());
    for db in &dbs[..3] {
        compact_daily_db(
            db,
            &dbs[0],
            &dbs[1],
            &maintenance,
            || Ok(()),
            |path| {
                assert_eq!(maintenance.try_lock().is_err(), path == dbs[1]);
                Ok(())
            },
        )
        .unwrap();
        assert!(maintenance.try_lock().is_ok());
    }
    let _guard = maintenance.lock().unwrap();
    for db in [&dbs[0], &dbs[2]] {
        compact_daily_db(db, &dbs[0], &dbs[1], &maintenance, || Ok(()), |_| Ok(())).unwrap();
    }
    assert!(
        compact_daily_db(
            &dbs[1],
            &dbs[0],
            &dbs[1],
            &maintenance,
            || Ok(()),
            |_| panic!("strategy compacted while snapshot lock held")
        )
        .is_err()
    );
}

/// Re-running a current slot or missing exact noon would duplicate or postpone daily VACUUM.
#[test]
fn runs_once_per_noon_slot_and_skips_missing_sources() {
    let fixture = Fixture::new();
    let dbs = fixture.databases();
    let marker = fixture.0.join("marker");
    let noon = 20_000 * DAY_MS + NOON_MS;
    let calls = RefCell::new(Vec::new());
    let compact = |path: &Path| {
        calls.borrow_mut().push(path.to_path_buf());
        Ok(())
    };
    assert!(matches!(
        compact_due_into(&marker, &dbs, noon - 1, |_| true, compact).unwrap(),
        DueOutcome::Created(_)
    ));
    assert_eq!(*calls.borrow(), dbs[..3]);
    compact_due_into(&marker, &dbs, noon, |_| true, compact).unwrap();
    assert_eq!(calls.borrow().len(), 6);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), noon.to_string());
    assert!(matches!(
        compact_due_into(
            &marker,
            &dbs,
            noon + 1,
            |_| true,
            |_| panic!("current slot compacted twice")
        )
        .unwrap(),
        DueOutcome::Current(_)
    ));
    compact_due_into(&marker, &dbs, noon + DAY_MS, |_| true, compact).unwrap();
    assert_eq!(calls.borrow().len(), 9);
}

/// A busy strategy database must not repeat reports VACUUM or starve tape maintenance.
#[test]
fn busy_strategies_retry_without_repeating_finished_peers() {
    let fixture = Fixture::new();
    let dbs = fixture.databases();
    let marker = fixture.0.join("marker");
    let calls = RefCell::new(Vec::new());
    let result = compact_due_into(
        &marker,
        &dbs,
        NOON_MS,
        |_| true,
        |path| {
            anyhow::ensure!(path != dbs[1], "injected failure");
            calls.borrow_mut().push(path.to_path_buf());
            Ok(())
        },
    );
    assert!(result.is_err());
    assert_eq!(*calls.borrow(), vec![dbs[0].clone(), dbs[2].clone()]);
    assert!(std::fs::read_to_string(&marker).unwrap().contains(':'));
    calls.borrow_mut().clear();
    compact_due_into(
        &marker,
        &dbs,
        NOON_MS,
        |_| true,
        |path| {
            calls.borrow_mut().push(path.to_path_buf());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(*calls.borrow(), vec![dbs[1].clone()]);
    assert_eq!(
        std::fs::read_to_string(marker).unwrap(),
        NOON_MS.to_string()
    );
}

/// A low-disk source must be skipped without withholding the slot from successful peers.
#[test]
fn insufficient_room_skips_database_but_advances_marker() {
    let fixture = Fixture::new();
    let dbs = fixture.databases();
    let marker = fixture.0.join("marker");
    let calls = RefCell::new(Vec::new());
    compact_due_into(
        &marker,
        &dbs,
        NOON_MS,
        |path| path != dbs[1],
        |path| {
            calls.borrow_mut().push(path.to_path_buf());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(*calls.borrow(), vec![dbs[0].clone(), dbs[2].clone()]);
    assert_eq!(
        std::fs::read_to_string(marker).unwrap(),
        NOON_MS.to_string()
    );
}
