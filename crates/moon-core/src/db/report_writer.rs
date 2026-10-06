use super::*;

/// Writer-channel capacity: backpressure instead of OOM.
///
/// About 16k messages consume tens of MB at peak. When the channel is full, the
/// core feed thread waits for the writer, naturally throttling catch-up.
const REPORT_QUEUE_CAP: usize = 16_384;

/// Maximum attempts for one owned writer batch before the writer fails closed.
pub(in crate::db) const REPORT_BATCH_ATTEMPTS: usize = 4;

/// Maximum exhausted retry rounds before a non-corruption error closes the writer.
pub(in crate::db) const REPORT_BATCH_MAX_FAILED_ROUNDS: u32 = 8;

/// Report-data publication urgency for one successfully committed writer batch.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::db) enum ReportPublication {
    /// Historical catch-up data may be coalesced before waking analytical readers.
    Background,
    /// Live or user-visible data must wake analytical readers immediately.
    Immediate,
}

/// Merge one message's publication into the class selected for its writer batch.
///
/// Args:
///     batch: Publication already selected for earlier messages in the batch.
///     next: Publication required by the newly applied message, or `None` for maintenance.
///
/// Returns:
///     The highest urgency observed across the batch.
pub(in crate::db) fn merge_report_publication(
    batch: Option<ReportPublication>,
    next: Option<ReportPublication>,
) -> Option<ReportPublication> {
    batch.max(next)
}

/// Publish one successfully committed report-changing writer batch.
///
/// Each dirty bit is a bounded one-slot signal. Publishing one class deliberately leaves the
/// opposite bit untouched because it may carry an unconsumed edge from an earlier batch.
///
/// Args:
///     generation: Monotonic report snapshot revision shared with readers.
///     immediate_dirty: Coalescing edge for live report mutations.
///     background_dirty: Coalescing edge for historical catch-up mutations.
///     publication: Urgency selected across the committed batch.
///
/// The function has no return value.
pub(in crate::db) fn publish_report_commit(
    generation: &AtomicU64,
    immediate_dirty: &AtomicBool,
    background_dirty: &AtomicBool,
    publication: ReportPublication,
) {
    publish_after_generation(generation, || match publication {
        ReportPublication::Background => background_dirty.store(true, Ordering::Release),
        ReportPublication::Immediate => immediate_dirty.store(true, Ordering::Release),
    });
}

/// Advance the authoritative revision before exposing its causal wake edge.
///
/// Release as well as acquire: a reader that sees the new value must also see the commit it
/// stands for, or a revision taken before a read could claim data that read missed.
pub(in crate::db) fn publish_after_generation(generation: &AtomicU64, publish_edge: impl FnOnce()) {
    generation.fetch_add(1, Ordering::AcqRel);
    publish_edge();
}

/// Process state a report read depends on beyond the committed rows, as a revision a caller can
/// compare later together with the report and valuation generations.
///
/// Returns:
///     `None` while a reader opened now might not read what an earlier one did with no generation
///     to say so: this process does not hold the replica's lease, integrity damage has stopped its
///     writer, the file is missing, or the COIN-M knowledge money SQL is built from is unsettled.
pub fn read_state_revision() -> Option<u64> {
    if !report_recovery::access_permitted()
        || integrity::writes_blocked()
        || !paths::reports_db_path().exists()
    {
        return None;
    }
    quote::coin_m::knowledge_revision()
}

/// Commit one stateful batch without exposing speculative in-memory mutations.
///
/// The callback receives the same owned batch indirectly on every attempt. A failed
/// begin, apply, or commit drops the transaction and candidate state, then retries
/// with bounded backoff. Exhaustion returns the last SQLite error so the caller can
/// enter recovery without acknowledging or skipping data.
pub(in crate::db) fn commit_stateful_batch<S: Clone, T>(
    conn: &Connection,
    state: &mut S,
    mut apply: impl FnMut(&Connection, &mut S) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    let mut last_error = None;
    for attempt in 0..REPORT_BATCH_ATTEMPTS {
        let transaction = match conn.unchecked_transaction() {
            Ok(transaction) => transaction,
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 < REPORT_BATCH_ATTEMPTS {
                    std::thread::sleep(Duration::from_millis(25u64 << attempt));
                }
                continue;
            }
        };
        let mut candidate = state.clone();
        match apply(&transaction, &mut candidate) {
            Ok(effects) => match transaction.commit() {
                Ok(()) => {
                    *state = candidate;
                    return Ok(effects);
                }
                Err(error) => last_error = Some(error),
            },
            Err(error) => last_error = Some(error),
        }
        if attempt + 1 < REPORT_BATCH_ATTEMPTS {
            std::thread::sleep(Duration::from_millis(25u64 << attempt));
        }
    }
    Err(last_error.unwrap_or(rusqlite::Error::InvalidQuery))
}

/// Keep one owned writer batch fail-closed until SQLite accepts it.
///
/// Each round retains the transaction-level retry policy from [`commit_stateful_batch`].
/// Exhausted transient rounds invoke `wait`, which applies production backoff without dropping the
/// batch. Confirmed damage or the bounded failed-round ceiling closes the writer without
/// acknowledging the owned batch, preventing an infinite retry from wedging the feed thread.
///
/// Args:
///     conn: Sole writer connection.
///     state: Last committed in-memory replica state.
///     apply: Rebuilds one candidate state and transaction on every attempt.
///     should_abort: Classifies a failed round that must stop immediately.
///     wait: Applies backoff after an exhausted retry round.
///
/// Returns:
///     Committed effects, or the last error after fail-closed termination.
pub(in crate::db) fn commit_stateful_batch_until_success<S: Clone, T>(
    conn: &Connection,
    state: &mut S,
    mut apply: impl FnMut(&Connection, &mut S) -> rusqlite::Result<T>,
    mut should_abort: impl FnMut(&rusqlite::Error) -> bool,
    mut wait: impl FnMut(u32, &rusqlite::Error),
) -> rusqlite::Result<T> {
    let mut failed_rounds = 0u32;
    loop {
        match commit_stateful_batch(conn, state, |transaction, candidate| {
            apply(transaction, candidate)
        }) {
            Ok(effects) => return Ok(effects),
            Err(error) => {
                if should_abort(&error) {
                    return Err(error);
                }
                failed_rounds = failed_rounds.saturating_add(1);
                if failed_rounds >= REPORT_BATCH_MAX_FAILED_ROUNDS {
                    return Err(error);
                }
                wait(failed_rounds, &error);
            }
        }
    }
}

/// Result row returned by SQLite's live-safe passive WAL checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::db) struct WalCheckpointStatus {
    pub(in crate::db) busy: i64,
    pub(in crate::db) log_frames: i64,
    pub(in crate::db) checkpointed_frames: i64,
}

/// Checkpoint every currently available WAL frame without waiting for readers.
pub(in crate::db) fn passive_wal_checkpoint(
    conn: &Connection,
) -> rusqlite::Result<WalCheckpointStatus> {
    conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
        Ok(WalCheckpointStatus {
            busy: row.get(0)?,
            log_frames: row.get(1)?,
            checkpointed_frames: row.get(2)?,
        })
    })
}

/// Start the sole report-replica writer and publish each report-changing commit.
///
/// Args:
///     permit: Private capability proving startup recovery and the process lease succeeded.
///
/// Returns:
///     The writer handle, or `None` when the database or writer thread cannot be initialized.
pub fn spawn_writer(_permit: report_recovery::ReportWritePermit) -> Option<ReportsHandle> {
    let (tx, rx): (std::sync::mpsc::SyncSender<DbMsg>, Receiver<DbMsg>) =
        std::sync::mpsc::sync_channel(REPORT_QUEUE_CAP);
    let path = paths::reports_db_path();
    let conn = match Connection::open(&path) {
        Ok(c) => c,
        Err(e) => {
            log::error!("отчёты: не удалось открыть {}: {e}", path.display());
            return None;
        }
    };
    if let Err(e) = init_db(&conn) {
        log::error!("отчёты: init схемы не удался: {e}");
        return None;
    }
    let starts = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut rep_state = match rep::init(&conn, starts.clone()) {
        Ok(st) => st,
        Err(e) => {
            log::error!("отчёты: init typed-реплики не удался: {e}");
            return None;
        }
    };
    let generation = Arc::new(AtomicU64::new(0));
    let gen_writer = generation.clone();
    let immediate_commit_dirty = Arc::new(AtomicBool::new(false));
    let writer_immediate_dirty = immediate_commit_dirty.clone();
    let background_commit_dirty = Arc::new(AtomicBool::new(false));
    let writer_background_dirty = background_commit_dirty.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("reports-db".into())
        .spawn(move || {
            log::info!("отчёты: writer запущен ({})", path.display());
            // WAL path used by the lazy checkpoint based on the ACTUAL file size below.
            let wal_path = paths::reports_db_files()[1].clone();
            // Batch messages into one transaction: catch-up emits thousands of rows,
            // and per-row autocommit (fsync) would stretch catch-up into minutes.
            let mut thr_count: u64 = 0;
            // Cores whose replica this batch wiped, to be re-declared at full history after the
            // acknowledgements. Reused across batches so the allocation is not per-batch.
            let mut recreated_resyncs: Vec<(u64, MoonReports)> = Vec::new();
            // Open-row pages this batch admitted, sent once it has committed. Reused like the above.
            let mut open_row_checks: Vec<(u64, Vec<i64>, MoonReports)> = Vec::new();
            // The page last registered per core, which tells a continuation from a stale one.
            let mut open_rows_walk = rep::OpenRowsWalk::default();
            let mut thr_started = std::time::Instant::now();
            // Backdate the value by 30 seconds so the first WAL size check becomes eligible
            // roughly 30 seconds after startup, on the next completed batch, rather than 60.
            let mut last_ckpt = std::time::Instant::now()
                .checked_sub(Duration::from_secs(30))
                .unwrap_or_else(std::time::Instant::now);
            loop {
                if integrity::writes_blocked() {
                    log::error!("reports: writer stopped because replica corruption was confirmed");
                    break;
                }
                let first = match rx.recv() {
                    Ok(m) => m,
                    Err(_) => break,
                };
                if integrity::writes_blocked() {
                    log::error!(
                        "reports: writer stopped after receiving a batch because replica \
                         corruption was confirmed"
                    );
                    break;
                }
                let mut batch = vec![first];
                while batch.len() < 512 {
                    match rx.try_recv() {
                        Ok(m) => batch.push(m),
                        Err(_) => break,
                    }
                }
                thr_count += batch.len() as u64;
                // Keep the owned batch until one whole attempt commits. ACK indices are
                // recreated per attempt and become observable only after that commit.
                let (ack_indices, publication, post_commit) =
                    match commit_stateful_batch_until_success(
                        &conn,
                        &mut rep_state,
                        |transaction, candidate| {
                            let mut ack_indices = Vec::new();
                            let mut publication = None;
                            // Ordered publications, recreated per attempt like the ACK indices
                            // and observable only after the attempt that commits.
                            let mut post_commit = Vec::new();
                            for (index, msg) in batch.iter().enumerate() {
                                let effect = apply_msg(transaction, candidate, msg)?;
                                publication =
                                    merge_report_publication(publication, effect.publication);
                                if effect.page_ack {
                                    ack_indices.push(index);
                                }
                                if let Some(action) = effect.post_commit {
                                    post_commit.push(action);
                                }
                            }
                            Ok((ack_indices, publication, post_commit))
                        },
                        integrity::writer_should_stop,
                        |failed_rounds, error| {
                            let delay =
                                Duration::from_secs(1u64 << failed_rounds.saturating_sub(1).min(5));
                            log::error!(
                            "reports: writer batch still blocked after {} attempts; retrying in \
                             {}s: {error}",
                            failed_rounds as usize * REPORT_BATCH_ATTEMPTS,
                            delay.as_secs()
                        );
                            std::thread::sleep(delay);
                        },
                    ) {
                        Ok(indices) => indices,
                        Err(error) => {
                            log::error!(
                            "reports: writer stopped after repeated or permanent replica failure; \
                             owned batch was not acknowledged: {error}"
                        );
                            break;
                        }
                    };
                let _ack_guard = integrity::writer_ack_guard();
                // The background scan can publish damage while this transaction is committing.
                // The publication barrier makes this check and every following ACK indivisible
                // from publishing a damage latch. Once damage becomes visible, no later ACK can
                // advance the core past uncertain data.
                if integrity::writes_blocked() {
                    log::error!(
                        "reports: writer stopped after commit because integrity damage was \
                         published; owned batch was not acknowledged"
                    );
                    break;
                }
                // Replayed in the order the writes went in, which `PostCommit` preserved by
                // construction. The full-history re-declarations are deferred until after the
                // acknowledgements below; everything else publishes here.
                for action in post_commit {
                    match action {
                        PostCommit::SyncComplete { core_uid, done } => {
                            rep::commit_sync_complete(core_uid, &done);
                            // A catch-up lands rows BETWEEN the ids already stored, which the
                            // COIN-M knowledge cannot notice on its own — it tracks the ends of
                            // what it examined. This commit is the authoritative "look again".
                            quote::coin_m::reexamine_core(core_uid);
                        }
                        PostCommit::ReplicaReset {
                            core_uid,
                            redeclare_history,
                        } => {
                            rep::commit_replica_reset(&rep_state, core_uid);
                            // The core serves a different report database now, so everything
                            // learned from the wiped rows — the span AND the verdict — is stale.
                            quote::coin_m::forget_core(core_uid);
                            recreated_resyncs.push((core_uid, redeclare_history));
                            // A page read before the wipe names ids of the superseded database.
                            open_row_checks.retain(|(uid, ..)| *uid != core_uid);
                            open_rows_walk.forget(core_uid);
                        }
                        PostCommit::CheckOpenRows {
                            core_uid,
                            after,
                            rec_ids,
                            reports,
                        } => {
                            if open_rows_walk.admit(core_uid, after.as_deref(), &rec_ids) {
                                open_row_checks.push((core_uid, rec_ids, reports));
                            }
                        }
                        PostCommit::CoreForgotten {
                            core_uid,
                            resync,
                            done,
                        } => {
                            rep::commit_replica_reset(&rep_state, core_uid);
                            quote::coin_m::forget_core(core_uid);
                            if let Some(reports) = resync {
                                recreated_resyncs.push((core_uid, reports));
                            }
                            open_row_checks.retain(|(uid, ..)| *uid != core_uid);
                            open_rows_walk.forget(core_uid);
                            let _ = done.try_send(true);
                        }
                        PostCommit::PageApplied {
                            core_uid,
                            last_rec_id,
                        } => rep::commit_page(&rep_state, core_uid, last_rec_id),
                        PostCommit::AliveMap {
                            core_uid,
                            checkpoint,
                            applied,
                        } => {
                            rep::commit_alive_map(&rep_state, core_uid, checkpoint, applied);
                        }
                    }
                }
                if let Some(publication) = publication {
                    publish_report_commit(
                        &gen_writer,
                        &writer_immediate_dirty,
                        &writer_background_dirty,
                        publication,
                    );
                }
                for index in ack_indices {
                    let DbMsg::Page { ack, page, .. } = &batch[index] else {
                        continue;
                    };
                    if let Err(error) = ack.page_applied(page) {
                        log::warn!("reports(rep): page_applied failed: {error:?}");
                    }
                }
                // After the acknowledgements on purpose: a page-detected recreation makes the
                // library restart catch-up on `page_applied`, and this request must land behind
                // that restart to replace its history depth rather than be replaced by it.
                for (core_uid, reports) in recreated_resyncs.drain(..) {
                    match reports.sync(moonproto::ReportSyncRequest::fresh(
                        moonproto::ReportHistoryDepth::All,
                    )) {
                        Ok(ticket) => {
                            rep::resync_declared(&mut rep_state, core_uid, Some(ticket.sync_id))
                        }
                        Err(error) => {
                            rep::resync_declared(&mut rep_state, core_uid, None);
                            log::warn!(
                                "отчёты(rep): ядро {core_uid} — полный sync после сброса реплики \
                                 не ушёл: {error:?}"
                            );
                        }
                    }
                }
                for (core_uid, rec_ids, reports) in open_row_checks.drain(..) {
                    if let Err(error) = reports.check_open_rows(&rec_ids) {
                        log::warn!(
                            "отчёты(rep): ядро {core_uid} — check_open_rows ({} шт) не ушёл: \
                             {error:?}",
                            rec_ids.len()
                        );
                    }
                }
                drop(_ack_guard);
                // If this batch dropped the legacy table, run a one-off VACUUM outside the
                // transaction to reclaim its hundreds of MB. Only the writer is blocked.
                if rep_state.vacuum_pending {
                    let t = std::time::Instant::now();
                    match wal::vacuum(&conn) {
                        Ok(_) => {
                            rep_state.vacuum_pending = false;
                            log::info!(
                                "reports: post-legacy VACUUM completed in {}s",
                                t.elapsed().as_secs()
                            );
                        }
                        Err(e) => log::warn!("отчёты: VACUUM не удался: {e}"),
                    }
                }
                // A passive checkpoint never waits for an Analytics reader. Inspect its
                // returned progress instead of mistaking a busy result row for success.
                if last_ckpt.elapsed() >= Duration::from_secs(60) {
                    last_ckpt = std::time::Instant::now();
                    let wal_big = std::fs::metadata(&wal_path)
                        .map(|m| m.len() > 32 * 1024 * 1024)
                        .unwrap_or(false);
                    if wal_big {
                        match passive_wal_checkpoint(&conn) {
                            Ok(status) if status.checkpointed_frames < status.log_frames => {
                                log::debug!(
                                    "reports: WAL reader pins {} of {} frames (busy={})",
                                    status.log_frames - status.checkpointed_frames,
                                    status.log_frames,
                                    status.busy
                                );
                            }
                            Ok(_) => {}
                            Err(error) => {
                                log::debug!("reports: wal_checkpoint failed: {error}");
                            }
                        }
                    }
                }
                // Throughput diagnostics for a dense catch-up stream show whether the writer
                // keeps pace with input. The bounded channel prevents OOM either way.
                let el = thr_started.elapsed();
                if el >= Duration::from_secs(10) {
                    if thr_count > 2_000 {
                        log::info!(
                            "отчёты: writer {} сообщений за {:.0}с (~{}/с)",
                            thr_count,
                            el.as_secs_f32(),
                            (thr_count as f32 / el.as_secs_f32()) as u64,
                        );
                    }
                    thr_count = 0;
                    thr_started = std::time::Instant::now();
                }
            }
            log::info!("отчёты: writer завершён");
        })
    {
        log::error!("отчёты: не удалось запустить writer thread: {e}");
        return None;
    }
    Some(ReportsHandle {
        tx: ReportSink {
            tx,
            starts,
            send_failed: Arc::new(AtomicBool::new(false)),
        },
        generation,
        immediate_commit_dirty,
        background_commit_dirty,
    })
}

/// One message's post-commit action, replayed in batch order after SQLite commits.
///
/// Actions that publish the IN-MEMORY half of a write can overwrite each other per core, so their
/// order has to match the SQLite write order. A batch can hold
/// an alive map followed by a `database_recreated` page for the same core; the committed database
/// then holds no checkpoint, and publishing the map's checkpoint afterwards would hand the next
/// feed an epoch SQLite has discarded, costing another wipe and a full catch-up.
pub(in crate::db) enum PostCommit {
    /// Log a finished catch-up. The checkpoint deliberately follows with its alive map.
    SyncComplete {
        core_uid: u64,
        done: ReportSyncComplete,
    },
    /// The replica was wiped: reset the core's start state and open rows, then replicate anew.
    ///
    /// `redeclare_history` re-sends this terminal's full-history policy. A page-detected
    /// recreation needs it because the library restarts catch-up itself on `page_applied` and
    /// reuses the `ServerDefault` depth `sync_from` resumed at; an alive-map-detected wipe needs
    /// it because nothing restarts on its own. Either way the request must be sent AFTER the
    /// acknowledgements, so it lands behind any restart rather than being replaced by one.
    ReplicaReset {
        core_uid: u64,
        redeclare_history: MoonReports,
    },
    /// The user deleted a core's report data: reset what the writer remembers about it, as a
    /// replica reset does, then answer the caller.
    ///
    /// `resync` re-declares the full-history download on a connected core, queued behind the
    /// acknowledgements exactly like a reset's; without it the next connection starts fresh.
    CoreForgotten {
        core_uid: u64,
        resync: Option<MoonReports>,
        done: std::sync::mpsc::SyncSender<bool>,
    },
    /// A catch-up page reached SQLite with its frontier: a reconnect later in this session resumes
    /// after it. Ordered with the replica reset, so a wipe later in the same batch wins.
    PageApplied { core_uid: u64, last_rec_id: i64 },
    /// An alive map reached SQLite: publish the checkpoint it committed with.
    AliveMap {
        core_uid: u64,
        checkpoint: ReportSyncCheckpoint,
        applied: rep::AliveMapApplied,
    },
    /// An open-row page was read inside the committed transaction: register it with the core.
    ///
    /// Ordered like the others so a wipe later in the same batch can drop a page read before it,
    /// and so a new walk admitted earlier in the batch outranks a stale continuation after it.
    CheckOpenRows {
        core_uid: u64,
        after: Option<Arc<[i64]>>,
        rec_ids: Vec<i64>,
        reports: MoonReports,
    },
}

/// Effects produced by one report-writer message.
pub(in crate::db) struct ApplyEffect {
    /// Whether a catch-up page may be acknowledged after the surrounding commit.
    pub(in crate::db) page_ack: bool,
    /// Report publication required after the surrounding commit, absent for maintenance.
    pub(in crate::db) publication: Option<ReportPublication>,
    /// Action to run once the surrounding transaction commits.
    ///
    /// Deciding it HERE, while applying, keeps publication ordered and honest: the list
    /// is replayed in the batch's own order, so a later message's effect can overwrite an
    /// earlier one exactly as SQLite applied them, and a message whose write did not happen
    /// contributes no action at all.
    pub(in crate::db) post_commit: Option<PostCommit>,
}

impl ApplyEffect {
    /// Attach the post-commit action produced by this message.
    fn publishing(mut self, action: PostCommit) -> Self {
        self.post_commit = Some(action);
        self
    }
    /// Build an immediately published report mutation.
    ///
    /// Args:
    ///     page_ack: Whether the surrounding commit may acknowledge one catch-up page.
    ///
    /// Returns:
    ///     Immediate report effect with optional page acknowledgement.
    const fn immediate(page_ack: bool) -> Self {
        Self {
            page_ack,
            publication: Some(ReportPublication::Immediate),
            post_commit: None,
        }
    }

    /// Build a background report mutation whose UI refresh may be coalesced.
    ///
    /// Args:
    ///     page_ack: Whether the surrounding commit may acknowledge one catch-up page.
    ///
    /// Returns:
    ///     Background report effect with optional page acknowledgement.
    const fn background(page_ack: bool) -> Self {
        Self {
            page_ack,
            publication: Some(ReportPublication::Background),
            post_commit: None,
        }
    }

    /// Build an internal maintenance effect that must not wake report readers.
    ///
    /// Returns:
    ///     Non-report-changing effect.
    const fn maintenance() -> Self {
        Self {
            page_ack: false,
            publication: None,
            post_commit: None,
        }
    }
}

/// Apply one typed-replica writer message and stage its durable valuation change.
///
/// Args:
///     conn: Active transaction receiving the message and valuation outbox mutation.
///     rep_state: Candidate replication state committed with the transaction.
///     msg: Typed replication or valuation-maintenance message to apply.
///
/// Returns:
///     Effect describing report publication and page acknowledgement after the surrounding
///     transaction commits.
pub(in crate::db) fn apply_msg(
    conn: &Connection,
    rep_state: &mut rep::RepState,
    msg: &DbMsg,
) -> rusqlite::Result<ApplyEffect> {
    match msg {
        DbMsg::Schema { core_uid, schema } => {
            rep::apply_schema(conn, rep_state, *core_uid, schema.clone())?;
            Ok(ApplyEffect::background(false))
        }
        DbMsg::Upsert {
            core_uid,
            core_name,
            row,
        } => {
            rep::apply_upsert(
                conn,
                rep_state,
                *core_uid,
                core_name,
                row,
                rep::RowSource::Live,
            )?;
            rep::note_live_row(conn, rep_state, *core_uid)?;
            valuation::stage_row(conn, valuation::TradeSource::Typed, *core_uid, row.rec_id)?;
            Ok(ApplyEffect::immediate(false))
        }
        DbMsg::Delete { core_uid, rec_id } => {
            rep::apply_delete(conn, *core_uid, *rec_id)?;
            valuation::stage_delete(conn, valuation::TradeSource::Typed, *core_uid, *rec_id)?;
            Ok(ApplyEffect::immediate(false))
        }
        DbMsg::CoreTimeOffset {
            core_uid,
            offset_secs,
            observed_at_utc,
            source,
        } => {
            rep::core_offset::ensure_table(conn)?;
            // A RE-CONFIRMATION IS NOT AN ADOPTION, and only the durable segment can tell the two
            // apart — `OffsetEstimator` holds its "already adopted" memory on the feed connection,
            // so every restart re-adopts an unchanged offset from scratch. Everything below is an
            // INVALIDATION, `stage_rescan_core` most of all: its worker arm deletes this core's
            // entire `trade_values` partition. Skipping is honest as well as cheap — the segment
            // it would write carries the offset the newest one already carries, so the axis is
            // unmoved. Why this lives in the writer rather than the sender: `core_offset::
            // latest_offset`.
            if rep::core_offset::latest_offset(conn, *core_uid) == Some(*offset_secs) {
                return Ok(ApplyEffect::maintenance());
            }
            // The segment STARTS at the observation, in seconds: everything the axis compares
            // against it is a Unix second, and the earliest segment of a core reaches backward
            // without bound anyway, so history before the first measurement is corrected too.
            rep::core_offset::store_segment(
                conn,
                *core_uid,
                observed_at_utc.div_euclid(1_000),
                *offset_secs,
                *observed_at_utc,
                source,
            )?;
            bump_axis_generation(conn)?;
            // The valuation cache is keyed on the RAW `closedate` and on `ALGORITHM_VERSION`,
            // neither of which moves when an offset is adopted, so nothing in `coverage_sql` can
            // notice on its own — already-valued rows would keep publishing a rate minute derived
            // from the uncorrected axis forever. Staging a rescan of THIS core is the narrow
            // lever: bumping the global algorithm version instead would re-value every honest
            // UTC core in the fleet for a measurement that says nothing about them.
            valuation::stage_rescan_core(conn, *core_uid)?;
            Ok(ApplyEffect::immediate(true))
        }
        DbMsg::SetDeleted { core_uid, change } => {
            rep::apply_set_deleted(conn, rep_state, *core_uid, change)?;
            Ok(ApplyEffect::immediate(false))
        }
        DbMsg::Page {
            core_uid,
            core_name,
            page,
            ack,
        } => {
            // A page of a download the user's delete superseded is acknowledged, never applied.
            if !rep::apply_page(conn, rep_state, *core_uid, core_name, page)? {
                return Ok(ApplyEffect {
                    page_ack: true,
                    publication: None,
                    post_commit: None,
                });
            }
            if page.database_recreated {
                valuation::stage_rescan_core(conn, *core_uid)?;
            }
            for row in page.rows.iter() {
                valuation::stage_row(conn, valuation::TradeSource::Typed, *core_uid, row.rec_id)?;
            }
            let effect = ApplyEffect::background(true);
            Ok(if page.database_recreated {
                effect.publishing(PostCommit::ReplicaReset {
                    core_uid: *core_uid,
                    redeclare_history: ack.clone(),
                })
            } else {
                effect.publishing(PostCommit::PageApplied {
                    core_uid: *core_uid,
                    last_rec_id: page.last_rec_id,
                })
            })
        }
        DbMsg::SyncComplete { core_uid, done } => {
            rep::apply_sync_complete(conn, rep_state, *core_uid, done)?;
            valuation::stage_legacy_purge(conn, *core_uid)?;
            Ok(
                ApplyEffect::background(false).publishing(PostCommit::SyncComplete {
                    core_uid: *core_uid,
                    done: done.clone(),
                }),
            )
        }
        DbMsg::AliveMap {
            core_uid,
            map,
            checkpoint,
        } => {
            if rep::alive_map_blocked(rep_state, *core_uid) {
                log::info!(
                    "отчёты(rep): ядро {core_uid} — карта живых строк отменённой догрузки пропущена"
                );
                return Ok(ApplyEffect::maintenance());
            }
            let applied = rep::apply_alive_map(
                conn,
                rep_state,
                *core_uid,
                map.covered_up_to,
                |rec_id| map.is_alive(rec_id),
                *checkpoint,
            )?;
            // `None` means the replica has no `deleted` column, so nothing was written and no
            // checkpoint may be published.
            let Some(applied) = applied else {
                return Ok(ApplyEffect::maintenance());
            };
            // A map that moved no row is the steady state after the first reconciliation: only
            // the checkpoint advanced, and waking every Report host for that would be noise.
            let effect = if applied.changed() {
                ApplyEffect::immediate(false)
            } else {
                ApplyEffect::maintenance()
            };
            Ok(effect.publishing(PostCommit::AliveMap {
                core_uid: *core_uid,
                checkpoint: *checkpoint,
                applied,
            }))
        }
        DbMsg::ReplicaRecreated { core_uid, reports } => {
            rep::apply_replica_recreated(conn, *core_uid)?;
            valuation::stage_rescan_core(conn, *core_uid)?;
            Ok(
                ApplyEffect::immediate(false).publishing(PostCommit::ReplicaReset {
                    core_uid: *core_uid,
                    redeclare_history: reports.clone(),
                }),
            )
        }
        DbMsg::ForgetCore {
            core_uid,
            resync,
            forget,
            done,
        } => {
            rep::apply_forget_core(conn, rep_state, *core_uid, resync.is_some(), *forget)?;
            // The clock-offset segments went with a forgotten core: the axis moved, exactly as
            // when a segment is stored, and every cache keyed on it must see that.
            if *forget {
                bump_axis_generation(conn)?;
            }
            valuation::stage_rescan_core(conn, *core_uid)?;
            Ok(
                ApplyEffect::immediate(false).publishing(PostCommit::CoreForgotten {
                    core_uid: *core_uid,
                    resync: resync.clone(),
                    done: done.clone(),
                }),
            )
        }
        DbMsg::ValuationAck { through_seq } => {
            valuation::ack_outbox(conn, *through_seq)?;
            Ok(ApplyEffect::maintenance())
        }
        DbMsg::RecheckOpenRows {
            core_uid,
            after,
            reports,
        } => {
            // A new walk is how a feed announces a new client or a reconnect: see
            // `rep::feed_restarted`.
            if after.is_none() {
                rep::feed_restarted(rep_state, *core_uid);
            }
            let rec_ids =
                rep::apply_recheck_open_rows(conn, rep_state, *core_uid, after.as_deref())?;
            Ok(
                ApplyEffect::maintenance().publishing(PostCommit::CheckOpenRows {
                    core_uid: *core_uid,
                    after: after.clone(),
                    rec_ids,
                    reports: reports.clone(),
                }),
            )
        }
    }
}
