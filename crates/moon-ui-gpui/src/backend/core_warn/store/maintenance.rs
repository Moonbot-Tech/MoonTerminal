//! One-time warning-index maintenance and warning writes ordered behind it.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use rusqlite::{Connection, Transaction, TransactionBehavior, params};

/// SQLite stores this exact definition without an IF NOT EXISTS clause or trailing semicolon.
pub(super) const SERIES_INDEX_SQL: &str =
    "CREATE UNIQUE INDEX ix_warn_series_unique ON core_warning_series(episode_id, badge, subject)";

/// Warning writes ordered behind the index upgrade; only slices require the new unique index.
pub(super) enum WarnWrite {
    /// A closed episode, with an id already returned to its subsequent capture calls.
    Episode {
        id: i64,
        episode: super::WarnEpisode,
    },
    /// A captured CPU/memory or ping slice, already encoded in its persisted format.
    Slice {
        episode_id: i64,
        badge: i64,
        subject: String,
        blob: Vec<u8>,
    },
    /// Startup retention, ordered after earlier captures and before later ones.
    Prune { cutoff_ms: i64 },
}

impl WarnWrite {
    /// Identify writes that would overwrite another badge through the legacy constraint.
    fn requires_series_index(&self) -> bool {
        matches!(self, Self::Slice { .. })
    }

    /// Apply one ordered mutation and return the rows changed, or the underlying SQLite error.
    pub(super) fn execute(
        self,
        conn: &Connection,
        revision: &AtomicU64,
    ) -> rusqlite::Result<usize> {
        match self {
            Self::Episode { id, episode } => {
                let changed = super::insert_episode_row(conn, id, &episode)?;
                revision.fetch_add(1, Ordering::Release);
                Ok(changed)
            }
            Self::Slice {
                episode_id,
                badge,
                subject,
                blob,
            } => conn.execute(
                "INSERT OR REPLACE INTO core_warning_series (episode_id, badge, subject, blob) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![episode_id, badge, subject, blob],
            ),
            Self::Prune { cutoff_ms } => conn.execute(
                "DELETE FROM core_warning_series WHERE episode_id IN \
                 (SELECT id FROM core_warnings WHERE start_ms < ?1)",
                [cutoff_ms],
            ),
        }
    }
}

/// Completion and ordered writes for missing or incompatible indexes; absent on ordinary opens.
pub(super) struct IndexMaintenance {
    /// Captures and pruning remain nonblocking while migration owns the writer lock.
    pending: Arc<Mutex<MigrationQueue>>,
    /// Joined only after completion, or before an explicit Storage deletion.
    task: Option<JoinHandle<rusqlite::Result<()>>>,
    /// Keep a failed migration failed rather than writing through the legacy constraint later.
    failure: Option<String>,
}

/// The queue closes under the same lock used by submitters, so no accepted write can be lost.
enum MigrationQueue {
    /// Captures/pruning waiting for the new uniqueness constraint.
    Pending(VecDeque<WarnWrite>),
    /// Completed work, or a retained error that prevents further series mutations.
    Finished(Result<(), String>),
}

impl IndexMaintenance {
    /// Build the requested indexes on one worker connection, then drain accepted slice writes.
    pub(super) fn start(
        path: PathBuf,
        rebuild_series: bool,
        build_recent: bool,
        revision: Arc<AtomicU64>,
    ) -> Self {
        let pending = Arc::new(Mutex::new(MigrationQueue::Pending(VecDeque::new())));
        let worker_queue = Arc::clone(&pending);
        let task = std::thread::spawn(move || {
            let connection = (|| -> rusqlite::Result<Connection> {
                let conn = Connection::open(path)?;
                moon_core::db::wal::enable(&conn)?;
                conn.pragma_update(None, "synchronous", "NORMAL")?;
                Ok(conn)
            })();
            let result: rusqlite::Result<()> = (|| {
                let conn = match connection {
                    Ok(conn) => conn,
                    Err(error) => {
                        let message = error.to_string();
                        drain_writes(
                            &worker_queue,
                            Err(&message),
                            rebuild_series.then_some(message.as_str()),
                            &revision,
                        )?;
                        return Err(error);
                    }
                };
                let series_error = if rebuild_series {
                    rebuild_series_index(&conn)
                        .err()
                        .map(|error| error.to_string())
                } else {
                    None
                };
                if let Some(error) = &series_error {
                    log::warn!("core warnings series index build failed: {error}");
                }
                if build_recent && let Err(error) = build_recent_indexes(&conn) {
                    // These indexes only accelerate correct reads: their failure must not
                    // discard accepted episodes, captures or the startup prune.
                    log::warn!("core warnings read index build failed: {error}");
                }
                drain_writes(&worker_queue, Ok(&conn), series_error.as_deref(), &revision)?;
                match series_error {
                    Some(error) => Err(index_error(error)),
                    None => Ok(()),
                }
            })();
            if let Err(error) = &result {
                log::warn!("core warnings index maintenance/write failed: {error}");
            }
            result
        });
        Self {
            pending,
            task: Some(task),
            failure: None,
        }
    }

    /// Queue a mutation, or return it after the worker has atomically completed its drain.
    pub(super) fn submit(&self, write: WarnWrite) -> rusqlite::Result<Option<WarnWrite>> {
        let mut state = self
            .pending
            .lock()
            .map_err(|_| rusqlite::Error::UnwindingPanic)?;
        match &mut *state {
            MigrationQueue::Pending(queue) => {
                queue.push_back(write);
                Ok(None)
            }
            MigrationQueue::Finished(Ok(())) => Ok(Some(write)),
            MigrationQueue::Finished(Err(error)) if write.requires_series_index() => {
                Err(index_error(error.clone()))
            }
            MigrationQueue::Finished(Err(_)) => Ok(Some(write)),
        }
    }

    /// Join a completed migration, or await it for an explicit Storage deletion.
    /// Any failure remains available on every later call, preventing unsafe writes.
    pub(super) fn finish(&mut self) -> rusqlite::Result<()> {
        if let Some(task) = self.task.take() {
            self.failure = task
                .join()
                .unwrap_or(Err(rusqlite::Error::UnwindingPanic))
                .err()
                .map(|error| error.to_string());
        }
        match &self.failure {
            Some(error) => Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
                Some(error.clone()),
            )),
            None => Ok(()),
        }
    }
}

/// Close the handoff only after every accepted write commits or logs its own failure.
/// A failed mandatory series index rejects unsafe captures, but episodes and pruning stay usable.
fn drain_writes(
    queue: &Mutex<MigrationQueue>,
    conn: Result<&Connection, &str>,
    series_error: Option<&str>,
    revision: &AtomicU64,
) -> rusqlite::Result<()> {
    loop {
        let write = {
            let mut state = queue.lock().map_err(|_| rusqlite::Error::UnwindingPanic)?;
            let MigrationQueue::Pending(pending) = &mut *state else {
                unreachable!()
            };
            match pending.pop_front() {
                Some(write) => Some(write),
                None => {
                    *state = MigrationQueue::Finished(
                        series_error.map_or(Ok(()), |error| Err(error.to_owned())),
                    );
                    None
                }
            }
        };
        let Some(write) = write else { break };
        if write.requires_series_index()
            && let Some(error) = series_error
        {
            log::warn!("core warnings queued capture failed: {error}");
            continue;
        }
        match conn {
            Ok(conn) => {
                if let Err(error) = write.execute(conn, revision) {
                    log::warn!("core warnings queued write failed: {error}");
                }
            }
            Err(error) => log::warn!("core warnings queued write failed: {error}"),
        }
    }
    Ok(())
}

/// Preserve a stored index error for unsafe captures without poisoning independent mutations.
fn index_error(message: String) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
        Some(message),
    )
}

/// Acquire the writer slot before preparing optional DDL and release it atomically on failure.
fn build_recent_indexes(conn: &Connection) -> rusqlite::Result<()> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    tx.execute_batch(super::RECENT_INDEX_SCHEMA)?;
    tx.commit()
}

/// Replace the legacy constraint atomically, preserving it if building the new index fails.
pub(super) fn rebuild_series_index(conn: &Connection) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DROP INDEX IF EXISTS ix_warn_series_unique", [])?;
    tx.execute_batch(SERIES_INDEX_SQL)?;
    tx.commit()
}
