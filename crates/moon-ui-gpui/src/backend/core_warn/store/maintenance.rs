//! Background warning writer and its one-time index maintenance.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use rusqlite::{Connection, Transaction, TransactionBehavior};

/// SQLite stores this exact definition without an IF NOT EXISTS clause or trailing semicolon.
pub(super) const SERIES_INDEX_SQL: &str =
    "CREATE UNIQUE INDEX ix_warn_series_unique ON core_warning_series(episode_id, badge, subject)";

/// Ordered warning mutations accepted by the dedicated writer thread.
pub(super) enum WarnWrite {
    /// A closed episode and its initial topology capture in one transaction.
    Episode {
        id: i64,
        episode: super::WarnEpisode,
        slices: Vec<super::WarnSlice>,
    },
    /// A later topology recapture in one transaction.
    Capture {
        episode_id: i64,
        slices: Vec<super::WarnSlice>,
    },
    /// Retention ordered after earlier captures and before later ones.
    Prune { cutoff_ms: i64 },
}

impl WarnWrite {
    /// Identify writes that would overwrite another badge through the legacy constraint.
    fn requires_series_index(&self) -> bool {
        match self {
            Self::Episode { slices, .. } | Self::Capture { slices, .. } => !slices.is_empty(),
            Self::Prune { .. } => false,
        }
    }

    /// Apply one ordered mutation and return the rows changed, or the underlying SQLite error.
    pub(super) fn execute(
        self,
        conn: &Connection,
        revision: &AtomicU64,
    ) -> rusqlite::Result<usize> {
        match self {
            Self::Episode {
                id,
                episode,
                slices,
            } => {
                let tx = conn.unchecked_transaction()?;
                let changed = super::insert_episode_row(&tx, id, &episode)?;
                for slice in &slices {
                    super::insert_slice_row(&tx, id, slice)?;
                }
                tx.commit()?;
                revision.fetch_add(1, Ordering::Release);
                Ok(changed)
            }
            Self::Capture { episode_id, slices } => {
                let tx = conn.unchecked_transaction()?;
                let mut changed = 0;
                for slice in &slices {
                    changed += super::insert_slice_row(&tx, episode_id, slice)?;
                }
                tx.commit()?;
                Ok(changed)
            }
            Self::Prune { cutoff_ms } => conn.execute(
                "DELETE FROM core_warning_series WHERE episode_id IN \
                 (SELECT id FROM core_warnings WHERE start_ms < ?1)",
                [cutoff_ms],
            ),
        }
    }
}

/// A cloneable writer failure that can be reported to submitters and flush callers.
#[derive(Default)]
struct WriterFailure {
    /// A worker-connection failure rejects every mutation.
    connection: Option<String>,
    /// A mandatory series-index failure rejects captures but not episodes or pruning.
    series: Option<String>,
}

/// Requests accepted by the writer thread; flushes make explicit deletion observe committed writes.
enum WorkerMessage {
    /// One FIFO mutation.
    Write(WarnWrite),
    /// Reply after every earlier message has been processed.
    Flush(mpsc::SyncSender<Result<(), String>>),
}

/// Dedicated background connection for every warning mutation and optional index upgrade.
pub(super) struct WarnWriter {
    /// FIFO to the writer thread; taken on drop so the thread drains and exits.
    sender: Option<mpsc::Sender<WorkerMessage>>,
    /// Joined on drop after the channel closes, releasing the database file deterministically.
    task: Option<std::thread::JoinHandle<()>>,
    /// Failures shared back to nonblocking submitters and explicit flush callers.
    failure: Arc<Mutex<WriterFailure>>,
}

impl WarnWriter {
    /// Start the writer, performing requested index work before draining accepted mutations.
    pub(super) fn start(
        path: PathBuf,
        rebuild_series: bool,
        build_read: bool,
        revision: Arc<AtomicU64>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let failure = Arc::new(Mutex::new(WriterFailure::default()));
        let worker_failure = Arc::clone(&failure);
        let task = std::thread::spawn(move || {
            let (connection, connection_error) = match open_writer(&path) {
                Ok(connection) => (Some(connection), None),
                Err(error) => (None, Some(error.to_string())),
            };
            let series_error = match (&connection, rebuild_series) {
                (Some(connection), true) => rebuild_series_index(connection)
                    .err()
                    .map(|error| error.to_string()),
                _ => None,
            };
            if let Some(error) = &series_error {
                log::warn!("core warnings series index build failed: {error}");
            }
            if build_read
                && let Some(connection) = &connection
                && let Err(error) = build_read_indexes(connection)
            {
                // Read indexes only accelerate correct reads: their failure must not discard
                // accepted episodes, captures or retention.
                log::warn!("core warnings read index build failed: {error}");
            }
            {
                let mut state = worker_failure
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.connection = connection_error.clone();
                state.series = series_error.clone();
            }

            while let Ok(message) = receiver.recv() {
                match message {
                    WorkerMessage::Write(write) => {
                        let Some(connection) = &connection else {
                            log::warn!(
                                "core warnings write failed: {}",
                                connection_error.as_deref().unwrap_or("writer unavailable")
                            );
                            continue;
                        };
                        let write = match (write.requires_series_index(), &series_error) {
                            (true, Some(error)) => {
                                log::warn!(
                                    "core warnings capture failed; preserving any episode row: {error}"
                                );
                                match write {
                                    WarnWrite::Episode { id, episode, .. } => WarnWrite::Episode {
                                        id,
                                        episode,
                                        slices: Vec::new(),
                                    },
                                    _ => continue,
                                }
                            }
                            _ => write,
                        };
                        if let Err(error) = write.execute(connection, &revision) {
                            log::warn!("core warnings write failed: {error}");
                        }
                    }
                    WorkerMessage::Flush(reply) => {
                        let _ = reply.send(
                            connection_error
                                .clone()
                                .or_else(|| series_error.clone())
                                .map_or(Ok(()), Err),
                        );
                    }
                }
            }
        });
        Self {
            sender: Some(sender),
            task: Some(task),
            failure,
        }
    }

    /// Queue a mutation without waiting for SQLite, or reject it from an already-known failure.
    pub(super) fn submit(&self, write: WarnWrite) -> rusqlite::Result<()> {
        {
            let state = self
                .failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(error) = &state.connection {
                return Err(writer_error(error.clone()));
            }
            if matches!(write, WarnWrite::Capture { .. })
                && let Some(error) = &state.series
            {
                return Err(writer_error(error.clone()));
            }
        }
        self.sender
            .as_ref()
            .expect("writer sender remains present until drop")
            .send(WorkerMessage::Write(write))
            .map_err(|error| writer_error(error.to_string()))
    }

    /// Wait until every earlier mutation commits or logs its failure, then report writer health.
    pub(super) fn flush(&self) -> rusqlite::Result<()> {
        let (reply, result) = mpsc::sync_channel(0);
        self.sender
            .as_ref()
            .expect("writer sender remains present until drop")
            .send(WorkerMessage::Flush(reply))
            .map_err(|error| writer_error(error.to_string()))?;
        result
            .recv_timeout(Duration::from_secs(30))
            .map_err(|error| writer_error(error.to_string()))?
            .map_err(writer_error)
    }
}

impl Drop for WarnWriter {
    /// Close the FIFO and join after accepted writes drain, releasing SQLite before file cleanup.
    fn drop(&mut self) {
        self.sender.take();
        if let Some(task) = self.task.take()
            && task.join().is_err()
        {
            log::warn!("core warnings writer thread panicked");
        }
    }
}

/// Open and configure the connection confined to the writer thread.
fn open_writer(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    moon_core::db::wal::enable(&conn)?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(conn)
}

/// Preserve a stored writer error without exposing internal failure state in the store API.
fn writer_error(message: String) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
        Some(message),
    )
}

/// Acquire the writer slot before preparing optional DDL and release it atomically on failure.
fn build_read_indexes(conn: &Connection) -> rusqlite::Result<()> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    tx.execute_batch(super::READ_INDEX_SCHEMA)?;
    tx.commit()
}

/// Replace the legacy constraint atomically, preserving it if building the new index fails.
pub(super) fn rebuild_series_index(conn: &Connection) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DROP INDEX IF EXISTS ix_warn_series_unique", [])?;
    tx.execute_batch(SERIES_INDEX_SQL)?;
    tx.commit()
}
