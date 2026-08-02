//! Dedicated report reconciliation and historical-rate worker.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use rusqlite::Connection;

use super::health::{
    self, FailureKind, FaultCause, ValuationFault, ValuationStage, ValuationStatus,
};
use super::provider::{resolve_rate, resolve_rate_batch, FetchFailure};
use super::{
    CachedRate, HttpSpotRateSource, OutboxAction, OutboxEvent, SpotRateSource, TradeInput,
    TradeSource,
};
use crate::db::{DbMsg, ReadFail, ReadResult, ReportTx};
use crate::util::now_unix_ms_i64;

/// Number of report rows reconciled before publishing progress and yielding to durable outbox work.
const RECONCILE_BATCH: usize = 256;

/// Number of durable report changes handled in one ordered prefix.
const OUTBOX_BATCH: usize = 512;

/// Registered valuation thread used to interrupt a long park when corruption is detected.
static WORKER_THREAD: OnceLock<Mutex<Option<std::thread::Thread>>> = OnceLock::new();

/// Wake the active valuation worker so cache recovery starts without waiting for its timer.
pub(super) fn wake_for_recovery() {
    let slot = WORKER_THREAD.get_or_init(|| Mutex::new(None));
    let worker = match slot.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    if let Some(worker) = worker {
        worker.unpark();
    }
}

/// Publish the active worker thread for corruption-triggered recovery wakes.
///
/// Args:
///     worker: Newly spawned valuation worker thread.
fn register_worker(worker: std::thread::Thread) {
    let slot = WORKER_THREAD.get_or_init(|| Mutex::new(None));
    match slot.lock() {
        Ok(mut guard) => *guard = Some(worker),
        Err(poisoned) => *poisoned.into_inner() = Some(worker),
    }
}

/// Background valuation publication and wake handle.
pub struct ValuationHandle {
    /// Monotonic generation incremented after prepared values or coverage state commit.
    pub generation: Arc<AtomicU64>,
    /// Coalescing UI wake edge set after prepared values or coverage state commit.
    pub commit_dirty: Arc<AtomicBool>,
    /// Monotonic counter bumped only when published worker health changes shape.
    ///
    /// Separate from `generation` because a stalled worker commits no data: a surface polling the
    /// data generation alone can never learn that nothing is coming.
    pub status_revision: Arc<AtomicU64>,
    /// Coalescing UI wake edge set beside every `status_revision` bump.
    pub status_dirty: Arc<AtomicBool>,
    /// Latest health snapshot published before the corresponding revision bump.
    status: Arc<RwLock<ValuationStatus>>,
    thread: std::thread::Thread,
}

impl ValuationHandle {
    /// Wake the worker after a committed report outbox change.
    pub fn wake(&self) {
        self.thread.unpark();
    }

    /// Take the first health snapshot together with the revision it belongs to.
    ///
    /// The two reads are ordered revision-then-snapshot, mirroring the worker's
    /// snapshot-then-revision publish order. Reversed, a publication landing between them would
    /// pair the OLD health with the NEW revision, and every later poll would see matching counters
    /// and keep serving the stale value. That ordering is why callers seed through this method
    /// instead of reading the two fields themselves.
    ///
    /// Returns:
    ///     Revision to poll against, and the health published at or before it.
    pub fn seed_status(&self) -> (u64, ValuationStatus) {
        let revision = self.status_revision.load(Ordering::Relaxed);
        (revision, self.read_status())
    }

    /// Take a fresh snapshot only when the published revision moved.
    ///
    /// Reading the snapshot takes a lock, so callers poll the counter — the same
    /// poll-a-revision-counter contract every panel in this application follows — and pay for the
    /// lock only on a real transition, never per frame.
    ///
    /// Args:
    ///     last: Revision this caller has already applied; updated in place when it moves.
    ///
    /// Returns:
    ///     New health, or `None` while nothing changed.
    pub fn status_if_changed(&self, last: &mut u64) -> Option<ValuationStatus> {
        let revision = self.status_revision.load(Ordering::Relaxed);
        if revision == *last {
            return None;
        }
        *last = revision;
        Some(self.read_status())
    }

    /// Clone the published health.
    ///
    /// Returns:
    ///     Current health, recovered from the lock even if another holder panicked.
    fn read_status(&self) -> ValuationStatus {
        match self.status.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

/// Channel the worker publishes health through.
struct StatusSink {
    /// Latest published health.
    status: Arc<RwLock<ValuationStatus>>,
    /// Monotonic counter observed by UI polls.
    revision: Arc<AtomicU64>,
    /// Coalescing UI wake edge.
    dirty: Arc<AtomicBool>,
    /// Signature of the last publication, so unchanged turns publish nothing.
    published: u64,
}

impl StatusSink {
    /// Publish health when the facts the UI renders changed.
    ///
    /// Args:
    ///     status: Current worker health.
    ///     now_ms: Current wall-clock time in Unix milliseconds.
    fn publish(&mut self, status: &ValuationStatus, now_ms: i64) {
        let signature = status.signature(now_ms);
        if signature == self.published {
            return;
        }
        self.published = signature;
        match self.status.write() {
            Ok(mut guard) => *guard = status.clone(),
            Err(poisoned) => *poisoned.into_inner() = status.clone(),
        }
        self.revision.fetch_add(1, Ordering::AcqRel);
        self.dirty.store(true, Ordering::Release);
    }
}

/// Result of preparing one current report row.
enum PrepareResult {
    /// The row is durably reflected, permanently unavailable, or no longer eligible.
    Complete { changed: bool },
    /// The row belongs to the still-open current UTC minute.
    Deferred,
    /// A classified provider, report-read, or cache failure requires a later retry.
    Retry(FaultCause),
}

/// Classified prefetch failure carrying any cache progress committed before the failure.
#[derive(Debug)]
struct PrefetchError {
    /// Classified failure retained for retry logging and publication.
    fault: FaultCause,
    /// Whether an earlier operation in the same prefetch batch changed durable coverage.
    changed: bool,
}

/// Wait between polls while the report replica has not been created yet.
const REPLICA_POLL: Duration = Duration::from_secs(5);

/// What one stage turn accomplished.
///
/// Shared by all three work-stage attempts so an unavailable report replica has one outcome
/// wherever a stage reads it: healthy startup state, but not progress by that stage. Treating it as
/// progress would clear an unresolved failing run — a provider outage would be retracted by the
/// replica going missing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StageTurn {
    /// The stage did its work; `more` requests another worker-loop turn without parking.
    Ran { more: bool },
    /// Reconciliation reached the keyset tail of both physical sources.
    Drained,
    /// The report replica does not exist yet, which is a healthy startup state, not a failure.
    AwaitingReplica,
}

impl StageTurn {
    /// Whether this turn counts as the stage making progress.
    ///
    /// Returns:
    ///     `false` only while the stage could not access the report replica.
    const fn is_progress(self) -> bool {
        !matches!(self, Self::AwaitingReplica)
    }
}

/// Classify one report-replica read failure.
///
/// Args:
///     error: Classified read failure from the report reader.
///
/// Returns:
///     Stage-less cause carrying the diagnostic text.
fn report_fault(error: ReadFail) -> FaultCause {
    FaultCause::new(FailureKind::ReportRead, error.to_string())
}

/// Exclusive descending reconciliation cursor: `(closedate, core_uid, row_id)`.
///
/// Named because the `deferred` map next door keys on a structurally identical `(i64, i64, i64)`
/// meaning `(source_code, core_uid, row_id)`. A Rust alias is transparent, so this does NOT stop
/// one being passed for the other — it only names the field order at each site so the swap is
/// visible while reading. Make it a newtype if that stops being enough.
type ReconcileCursor = (i64, i64, i64);

/// Incremental startup reconciliation cursor shared across worker-loop turns.
struct ReconcileState {
    source_index: usize,
    after: Option<ReconcileCursor>,
}

impl ReconcileState {
    /// Start at the typed source above its newest key.
    ///
    /// Returns:
    ///     Fresh incremental reconciliation cursor.
    const fn new() -> Self {
        Self {
            source_index: 0,
            after: None,
        }
    }
}

/// Reset volatile worker state after the derived store is replaced.
///
/// Args:
///     deferred: Current-minute inputs tied to the retired cache.
///     reconciliation: Startup cursor that must restart from the first physical source.
///     pending_ack: In-flight outbox boundary tied to the retired worker state.
fn reset_after_recovery(
    deferred: &mut BTreeMap<(i64, i64, i64), TradeInput>,
    reconciliation: &mut Option<ReconcileState>,
    pending_ack: &mut Option<i64>,
) {
    deferred.clear();
    *reconciliation = Some(ReconcileState::new());
    *pending_ack = None;
}

/// Exclude the locally acknowledged prefix until the report writer deletes it durably.
///
/// Args:
///     events: Current ordered outbox batch.
///     pending_ack: Highest sequence already sent to the report writer.
///
/// Returns:
///     Events not yet reflected by a sent acknowledgement.
fn unacknowledged_events<'a>(
    events: &'a [OutboxEvent],
    pending_ack: &mut Option<i64>,
) -> &'a [OutboxEvent] {
    let Some(through_seq) = *pending_ack else {
        return events;
    };
    if events.is_empty() || events[0].seq > through_seq {
        *pending_ack = None;
        return events;
    }
    let first = events.partition_point(|event| event.seq <= through_seq);
    &events[first..]
}

/// Enqueue one cumulative outbox acknowledgement and remember its in-flight boundary.
///
/// Args:
///     report_tx: Sole report writer sink.
///     pending_ack: Highest sequence already sent but not yet observed deleted.
///     through_seq: New safely reflected contiguous sequence.
fn send_ack(report_tx: &ReportTx, pending_ack: &mut Option<i64>, through_seq: i64) {
    report_tx.send(DbMsg::ValuationAck { through_seq });
    *pending_ack = Some(pending_ack.map_or(through_seq, |pending| pending.max(through_seq)));
}

/// Start the production valuation worker.
///
/// Args:
///     report_tx: Sole report-writer sink used to acknowledge durable outbox prefixes.
///
/// Returns:
///     Worker handle, or `None` only when the background thread cannot be created. Storage
///     initialization failures remain retryable inside that thread.
pub fn spawn_worker(report_tx: ReportTx) -> Option<ValuationHandle> {
    spawn_worker_with_source(report_tx, Arc::new(HttpSpotRateSource::new()))
}

/// Start a valuation worker with a caller-supplied deterministic or production rate source.
///
/// Args:
///     report_tx: Sole report-writer sink used for outbox acknowledgements.
///     source: Historical closed-candle boundary.
///
/// Returns:
///     Worker handle, or `None` only when the background thread cannot be created. Storage
///     initialization failures remain retryable inside that thread.
fn spawn_worker_with_source(
    report_tx: ReportTx,
    source: Arc<dyn SpotRateSource>,
) -> Option<ValuationHandle> {
    let initial_store = match super::open_canonical_store() {
        Ok(store) => Some(store),
        Err(error) => {
            log::error!("valuation: initial cache recovery failed: {error}");
            None
        }
    };
    let generation = Arc::new(AtomicU64::new(0));
    let commit_dirty = Arc::new(AtomicBool::new(false));
    let status = Arc::new(RwLock::new(ValuationStatus::default()));
    let status_revision = Arc::new(AtomicU64::new(0));
    let status_dirty = Arc::new(AtomicBool::new(false));
    let thread_generation = generation.clone();
    let thread_dirty = commit_dirty.clone();
    let sink = StatusSink {
        status: status.clone(),
        revision: status_revision.clone(),
        dirty: status_dirty.clone(),
        published: ValuationStatus::default().signature(0),
    };
    let join = std::thread::Builder::new()
        .name("report-valuation".to_string())
        .spawn(move || {
            run_worker(
                report_tx,
                source,
                thread_generation,
                thread_dirty,
                sink,
                initial_store,
            );
        });
    let join = match join {
        Ok(join) => join,
        Err(error) => {
            log::error!("valuation: failed to start worker thread: {error}");
            return None;
        }
    };
    let thread = join.thread().clone();
    register_worker(thread.clone());
    drop(join);
    Some(ValuationHandle {
        generation,
        commit_dirty,
        status_revision,
        status_dirty,
        status,
        thread,
    })
}

/// Reconcile historical rows, consume durable changes, and sleep on report/minute boundaries.
///
/// Args:
///     report_tx: Sole report-writer sink used for outbox acknowledgements.
///     source: Historical closed-candle boundary.
///     generation: Monotonic valuation publication counter.
///     dirty: Coalescing UI wake edge.
///     sink: Channel publishing worker health to the UI.
///     initial_store: Startup-validated cache, or `None` when background recovery must retry.
fn run_worker(
    report_tx: ReportTx,
    source: Arc<dyn SpotRateSource>,
    generation: Arc<AtomicU64>,
    dirty: Arc<AtomicBool>,
    mut sink: StatusSink,
    initial_store: Option<Connection>,
) {
    let mut store = initial_store;
    let mut deferred: BTreeMap<(i64, i64, i64), TradeInput> = BTreeMap::new();
    let mut reconciliation = Some(ReconcileState::new());
    let mut pending_ack = None;
    let mut status = ValuationStatus::default();
    'worker: loop {
        let now_ms = now_unix_ms_i64();
        if store.is_none() || !super::cache_is_healthy() {
            drop(store.take());
            // Deliberately NOT gated by `ValuationStatus::wait_for` through `attempt`.
            // `wake_for_recovery` unparks this thread when corruption is detected; refusing the
            // recovery attempt would make that wake wait out a backoff of up to five minutes. The
            // growing park below paces a cache that keeps failing to open.
            match super::open_canonical_store() {
                Ok(recovered) => {
                    store = Some(recovered);
                    reset_after_recovery(&mut deferred, &mut reconciliation, &mut pending_ack);
                    // Clear only cache-caused runs; provider and report-read failures remain valid
                    // after the derived cache is replaced.
                    status.record_recovery();
                    sink.publish(&status, now_ms);
                    publish(&generation, &dirty);
                    log::info!("valuation: derived cache is healthy; full reconciliation resumed");
                }
                Err(error) => {
                    let delay = note_failure(
                        &mut status,
                        &mut sink,
                        FaultCause::new(FailureKind::CacheUnhealthy, error.to_string())
                            .at(ValuationStage::CacheRecovery),
                        now_ms,
                    );
                    std::thread::park_timeout(until_stall(&status, delay, now_unix_ms_i64()));
                    continue;
                }
            }
        }
        let store_ref = store.as_ref().expect("valuation store recovered above");
        let mut retry_after = None;
        let reconciled = match &mut reconciliation {
            Some(state) => attempt(
                &mut status,
                &mut sink,
                ValuationStage::Reconcile,
                now_ms,
                &mut retry_after,
                || {
                    reconcile_step(
                        store_ref,
                        source.as_ref(),
                        &generation,
                        &dirty,
                        &mut deferred,
                        state,
                    )
                },
            ),
            None => Attempt::Resting,
        };
        match reconciled {
            Attempt::CacheLost => continue 'worker,
            Attempt::Done(StageTurn::Drained) => reconciliation = None,
            _ => {}
        }
        match attempt(
            &mut status,
            &mut sink,
            ValuationStage::Outbox,
            now_ms,
            &mut retry_after,
            || {
                consume_outbox(
                    store_ref,
                    source.as_ref(),
                    &report_tx,
                    &generation,
                    &dirty,
                    &mut deferred,
                    &mut pending_ack,
                )
            },
        ) {
            Attempt::CacheLost => continue 'worker,
            // Deliberately NOT conditioned on `retry_after`: every stage guards its own backoff, so
            // draining a full outbox batch cannot re-run a stage that is resting. Gating this on
            // another stage's wait would throttle live report changes to one 512-event batch per
            // backoff, up to five minutes apart.
            Attempt::Done(StageTurn::Ran { more: true }) => continue,
            _ => {}
        }
        if current_minute_closed_any(&deferred) {
            match attempt(
                &mut status,
                &mut sink,
                ValuationStage::DeferredMinute,
                now_ms,
                &mut retry_after,
                || {
                    process_deferred(
                        store_ref,
                        source.as_ref(),
                        &generation,
                        &dirty,
                        &mut deferred,
                    )
                },
            ) {
                Attempt::CacheLost => continue 'worker,
                Attempt::Done(StageTurn::Ran { more: true }) => continue,
                _ => {}
            }
        } else {
            // Nothing is due for this stage. An outbox delete, core rescan or legacy purge can drop
            // exactly the rows a failing run was retrying, and a run kept open for work that no
            // longer exists would report a stall that nothing could ever clear.
            record_progress(
                &mut status,
                &mut sink,
                ValuationStage::DeferredMinute,
                now_ms,
            );
        }
        let settled = now_unix_ms_i64();
        sink.publish(&status, settled);
        let delay = retry_after.unwrap_or_else(|| {
            if reconciliation.is_some() || pending_ack.is_some() {
                Duration::from_millis(25)
            } else {
                delay_to_next_minute()
            }
        });
        std::thread::park_timeout(until_stall(&status, delay, settled));
    }
}

/// What one gated stage attempt did.
enum Attempt {
    /// The stage is inside its backoff and was not run.
    Resting,
    /// The stage was attempted and reported its own outcome.
    Done(StageTurn),
    /// The stage failed; its run is recorded and its backoff selected.
    Failed,
    /// The failure proved the derived cache damaged, so recovery owns it and this stage does not.
    CacheLost,
}

/// Run one stage under its own backoff, recording its health and selecting the next wait.
///
/// The three work stages share this scaffold so the backoff gate, unavailable-replica handling,
/// progress/failure publication, cache-health check before fault recording, and selection of the
/// shortest wait are written once. Stage-specific work and reactions to completed outcomes stay at
/// the call sites.
///
/// Args:
///     status: Worker health being tracked.
///     sink: Channel publishing health to the UI.
///     stage: Stage to attempt.
///     now_ms: Current wall-clock time in Unix milliseconds.
///     retry_after: Earliest wait selected so far this turn, narrowed in place.
///     run: The stage's own work.
///
/// Returns:
///     Whether the stage rested, completed with its own outcome, failed, or lost the cache.
fn attempt(
    status: &mut ValuationStatus,
    sink: &mut StatusSink,
    stage: ValuationStage,
    now_ms: i64,
    retry_after: &mut Option<Duration>,
    run: impl FnOnce() -> Result<StageTurn, FaultCause>,
) -> Attempt {
    let wait = status.wait_for(stage, now_ms);
    if !wait.is_zero() {
        *retry_after = shorter(*retry_after, wait);
        return Attempt::Resting;
    }
    match run() {
        Ok(turn) => {
            // A turn the stage could not act on is not progress: clearing an unresolved provider
            // outage because the report replica went missing would retract a reported stall.
            if turn.is_progress() {
                record_progress(status, sink, stage, now_ms);
            } else {
                *retry_after = shorter(*retry_after, REPLICA_POLL);
            }
            Attempt::Done(turn)
        }
        Err(cause) => {
            // Order matters: a read against the attached cache can PROVE the cache damaged, and
            // that failure belongs to the recovery stage. Recording it here first would open a run
            // whose kind names the wrong subsystem and which then outlives the repair as the
            // oldest stall.
            if !super::cache_is_healthy() {
                return Attempt::CacheLost;
            }
            let delay = note_failure(status, sink, cause.at(stage), now_ms);
            *retry_after = shorter(*retry_after, delay);
            Attempt::Failed
        }
    }
}

/// Shorten a park so a run that is only waiting out the clock still publishes when it stalls.
///
/// A backoff can extend past the pending stall deadline, so without this the worker would sleep
/// straight through the moment its own definition of "stalled" became true and the surfaces would
/// keep saying "retrying" after that stopped being the honest word.
///
/// Args:
///     status: Current worker health.
///     delay: Park the loop selected on its own.
///     now_ms: Current wall-clock time in Unix milliseconds.
///
/// Returns:
///     The shorter of the requested park and the wait to the next stall deadline.
fn until_stall(status: &ValuationStatus, delay: Duration, now_ms: i64) -> Duration {
    match status.next_stall_ms(now_ms) {
        Some(deadline) => delay.min(Duration::from_millis(
            deadline.saturating_sub(now_ms).max(1) as u64,
        )),
        None => delay,
    }
}

/// Keep the earliest of two outstanding waits so no eligible stage oversleeps.
///
/// Args:
///     current: Wait already selected this turn, if any.
///     candidate: Wait requested by another stage.
///
/// Returns:
///     The shorter of the two.
fn shorter(current: Option<Duration>, candidate: Duration) -> Option<Duration> {
    Some(current.map_or(candidate, |current| current.min(candidate)))
}

/// Record that one stage completed a turn, publishing only if that cleared a failing run.
///
/// A turn that changed nothing still counts as progress: reconciliation can find that every row is
/// already valued, and treating that quiet result as absent progress would report a healthy worker
/// as stuck.
///
/// Args:
///     status: Worker health being tracked.
///     sink: Channel publishing health to the UI.
///     stage: Stage that completed a turn or has no work due.
///     now_ms: Current wall-clock time in Unix milliseconds.
fn record_progress(
    status: &mut ValuationStatus,
    sink: &mut StatusSink,
    stage: ValuationStage,
    now_ms: i64,
) {
    if status.record_progress(stage) {
        sink.publish(status, now_ms);
    }
}

/// Record one stage failure, log it at a thinning cadence, and publish any transition.
///
/// Args:
///     status: Worker health being tracked.
///     sink: Channel publishing health to the UI.
///     fault: Classified failure carrying its stage.
///     now_ms: Current wall-clock time in Unix milliseconds.
///
/// Returns:
///     Backoff before that stage may be attempted again.
fn note_failure(
    status: &mut ValuationStatus,
    sink: &mut StatusSink,
    fault: ValuationFault,
    now_ms: i64,
) -> Duration {
    let stage = fault.stage;
    let delay = status.record_failure(fault, now_ms);
    let health = status.stage(stage);
    if let Some(fault) = &health.fault {
        health::log_fault(
            fault,
            health.consecutive_failures,
            health.failing_for_ms(now_ms),
        );
    }
    sink.publish(status, now_ms);
    delay
}

/// Reconcile one persisted report-row keyset batch without blocking durable live changes.
///
/// Each source is walked newest trade first (see [`reconciliation_batch`]), but the two sources
/// stay sequenced rather than merged by date, so newest-first holds PER SOURCE and not per user.
///
/// Known unserved case: legacy rows are purged per core on that core's `SyncComplete`
/// (`rep::purge_legacy`), so a core that has not finished its typed sync still keeps ALL of its
/// rows — today's included — in the legacy table. That core's newest trades are valued only after
/// the entire typed backlog drains, which is the same wait this ordering exists to remove. The
/// sequencing is accepted because legacy-only cores are transitional and shrinking, not because
/// the legacy table holds nothing recent.
///
/// Revisit when that stops being cheap — observably, when the legacy source still returns rows
/// whose `closedate` is recent. Merging the two sources by date is the fix and costs a merged
/// keyset over two heterogeneous key shapes, which is why it is not done here.
///
/// Args:
///     store: Open valuation writer connection.
///     source: Historical closed-candle boundary.
///     generation: Monotonic valuation publication counter.
///     dirty: Coalescing UI wake edge.
///     deferred: Current-minute rows retained until their candle closes.
///     state: Source and keyset cursor advanced only after a complete batch.
///
/// Returns:
///     Whether the sources drained, advanced, or are still waiting for the report replica.
fn reconcile_step(
    store: &Connection,
    source: &dyn SpotRateSource,
    generation: &AtomicU64,
    dirty: &AtomicBool,
    deferred: &mut BTreeMap<(i64, i64, i64), TradeInput>,
    state: &mut ReconcileState,
) -> Result<StageTurn, FaultCause> {
    let sources = [TradeSource::Typed, TradeSource::Legacy];
    while state.source_index < sources.len() {
        let trade_source = sources[state.source_index];
        let conn = match crate::db::open_reader() {
            Ok(conn) => conn,
            Err(ReadFail::NotReady) => return Ok(StageTurn::AwaitingReplica),
            Err(error) => return Err(report_fault(error)),
        };
        let inputs = reconciliation_batch(&conn, trade_source, state.after, RECONCILE_BATCH)
            .map_err(report_fault)?;
        if inputs.is_empty() {
            state.source_index += 1;
            state.after = None;
            continue;
        }
        let mut changed =
            settle_prefetch(prefetch_rates(store, source, &inputs), generation, dirty)?;
        for input in &inputs {
            match prepare_trade(store, source, input) {
                PrepareResult::Complete {
                    changed: input_changed,
                } => changed |= input_changed,
                PrepareResult::Deferred => {
                    deferred.insert(trade_key(input), input.clone());
                }
                PrepareResult::Retry(error) => {
                    if changed {
                        publish(generation, dirty);
                    }
                    return Err(error);
                }
            }
        }
        if changed {
            publish(generation, dirty);
        }
        // A short batch means this source is drained: advance to the next one and start it above
        // its newest row. Otherwise the cursor follows the batch's last (oldest) row.
        if inputs.len() < RECONCILE_BATCH {
            state.source_index += 1;
            state.after = None;
        } else {
            state.after = inputs
                .last()
                .map(|input| (input.closedate, input.core_uid, input.row_id));
        }
        return Ok(StageTurn::Ran { more: true });
    }
    Ok(StageTurn::Drained)
}

/// Consume one contiguous report outbox prefix and acknowledge it only after valuation commits.
///
/// Args:
///     store: Open valuation writer connection.
///     source: Historical closed-candle boundary.
///     report_tx: Sole report writer used for acknowledgement.
///     generation: Monotonic valuation publication counter.
///     dirty: Coalescing UI wake edge.
///     deferred: Current-minute rows retained until their candle closes.
///     pending_ack: Highest sequence sent to the report writer but not yet observed deleted.
///
/// Returns:
///     Whether another full outbox batch may already be waiting, or that the replica is absent.
fn consume_outbox(
    store: &Connection,
    source: &dyn SpotRateSource,
    report_tx: &ReportTx,
    generation: &AtomicU64,
    dirty: &AtomicBool,
    deferred: &mut BTreeMap<(i64, i64, i64), TradeInput>,
    pending_ack: &mut Option<i64>,
) -> Result<StageTurn, FaultCause> {
    let conn = match crate::db::open_reader() {
        Ok(conn) => conn,
        Err(ReadFail::NotReady) => return Ok(StageTurn::AwaitingReplica),
        Err(error) => return Err(report_fault(error)),
    };
    let batch = super::read_outbox(&conn, OUTBOX_BATCH).map_err(report_fault)?;
    let batch_was_full = batch.len() == OUTBOX_BATCH;
    let events = unacknowledged_events(&batch, pending_ack);
    if events.is_empty() {
        return Ok(StageTurn::Ran { more: false });
    }
    let mut row_inputs = Vec::new();
    for event in events {
        if event.action == OutboxAction::Row {
            if let Some(input) = load_trade(&conn, event.source, event.core_uid, event.row_id)
                .map_err(report_fault)?
            {
                row_inputs.push(input);
            }
        }
    }
    let mut changed = settle_prefetch(
        prefetch_rates(store, source, &row_inputs),
        generation,
        dirty,
    )?;
    let mut acknowledged = None;
    for event in events {
        match process_event(store, source, &conn, *event, deferred) {
            PrepareResult::Complete {
                changed: event_changed,
            } => {
                changed |= event_changed;
                acknowledged = Some(event.seq);
            }
            PrepareResult::Deferred => {
                acknowledged = Some(event.seq);
            }
            PrepareResult::Retry(error) => {
                if let Some(through_seq) = acknowledged {
                    send_ack(report_tx, pending_ack, through_seq);
                }
                if changed {
                    publish(generation, dirty);
                }
                return Err(error);
            }
        }
    }
    if let Some(through_seq) = acknowledged {
        send_ack(report_tx, pending_ack, through_seq);
    }
    if changed {
        publish(generation, dirty);
    }
    Ok(StageTurn::Ran {
        more: batch_was_full,
    })
}

/// Apply one durable report event to the prepared valuation store.
///
/// Args:
///     store: Open valuation writer connection.
///     source: Historical closed-candle boundary.
///     reports: Report reader observing the committed event.
///     event: Ordered durable outbox event.
///     deferred: Current-minute rows retained until their candle closes.
///
/// Returns:
///     Completed, deferred, or transient-retry result.
fn process_event(
    store: &Connection,
    source: &dyn SpotRateSource,
    reports: &Connection,
    event: OutboxEvent,
    deferred: &mut BTreeMap<(i64, i64, i64), TradeInput>,
) -> PrepareResult {
    match event.action {
        OutboxAction::Row => {
            match load_trade(reports, event.source, event.core_uid, event.row_id) {
                Ok(Some(input)) => match prepare_trade(store, source, &input) {
                    PrepareResult::Deferred => {
                        deferred.insert(trade_key(&input), input);
                        PrepareResult::Deferred
                    }
                    result => result,
                },
                Ok(None) => delete_trade(store, event.source, event.core_uid, event.row_id),
                Err(error) => PrepareResult::Retry(report_fault(error)),
            }
        }
        OutboxAction::Delete => {
            deferred.remove(&(event.source.code(), event.core_uid, event.row_id));
            delete_trade(store, event.source, event.core_uid, event.row_id)
        }
        OutboxAction::RescanCore | OutboxAction::PurgeLegacy => {
            deferred.retain(|(source_kind, core_uid, _), _| {
                *source_kind != event.source.code() || *core_uid != event.core_uid
            });
            delete_partition(store, event.source, event.core_uid)
        }
    }
}

/// Prepare one current committed report row from cache or the canonical spot provider.
///
/// Args:
///     store: Open valuation writer connection.
///     source: Historical closed-candle boundary.
///     input: Complete current report inputs.
///
/// Returns:
///     Completed, current-minute deferred, or transient-retry result.
fn prepare_trade(
    store: &Connection,
    source: &dyn SpotRateSource,
    input: &TradeInput,
) -> PrepareResult {
    let minute_utc = input.closedate.div_euclid(60) * 60;
    if minute_utc >= current_minute_utc() {
        return PrepareResult::Deferred;
    }
    let Some(currency) = crate::db::QuoteCurrency::from_report_ordinal(input.quote_ordinal) else {
        return delete_trade(store, input.source, input.core_uid, input.row_id);
    };
    let rate = match super::cached_rate(store, input.quote_ordinal, minute_utc) {
        Ok(Some(CachedRate::Ready(rate))) => rate,
        Ok(Some(CachedRate::PermanentMissing)) => {
            return delete_trade(store, input.source, input.core_uid, input.row_id);
        }
        Ok(None) => {
            match resolve_rate(source, input.quote_ordinal, currency.ticker(), minute_utc) {
                Ok(rate) => {
                    if rate.candle_close_ms >= now_unix_ms_i64() {
                        return PrepareResult::Deferred;
                    }
                    if let Err(error) = super::store_rate(store, &rate, now_unix_ms_i64()) {
                        return PrepareResult::Retry(super::store_fault(error));
                    }
                    rate
                }
                Err(FetchFailure::Missing) => {
                    let rate_changed = match super::store_permanent_missing(
                        store,
                        input.quote_ordinal,
                        minute_utc,
                        now_unix_ms_i64(),
                    ) {
                        Ok(changed) => changed > 0,
                        Err(error) => return PrepareResult::Retry(super::store_fault(error)),
                    };
                    return merge_change(
                        delete_trade(store, input.source, input.core_uid, input.row_id),
                        rate_changed,
                    );
                }
                Err(FetchFailure::Transient(error)) => {
                    return PrepareResult::Retry(FaultCause::new(FailureKind::Provider, error));
                }
            }
        }
        Err(error) => return PrepareResult::Retry(super::store_fault(error)),
    };
    match super::store_trade_value(store, input, &rate, now_unix_ms_i64()) {
        Ok(changed) => PrepareResult::Complete {
            changed: changed > 0,
        },
        Err(error) => PrepareResult::Retry(super::store_fault(error)),
    }
}

/// Populate every uncached closed rate needed by one report-row batch.
///
/// Requests are grouped by quote and bounded to provider windows of at most 1,000 minutes. Sparse
/// trade history therefore costs one request per time window rather than one request per order,
/// while the persistent rate table makes later reconciliation and restarts network-free.
///
/// Args:
///     store: Open valuation writer connection.
///     source: Historical closed-candle boundary.
///     inputs: Current report inputs about to be prepared.
///
/// Returns:
///     Whether rate coverage changed after ready and permanent-missing outcomes became durable,
///     or a transient reason.
fn prefetch_rates(
    store: &Connection,
    source: &dyn SpotRateSource,
    inputs: &[TradeInput],
) -> Result<bool, PrefetchError> {
    let current = current_minute_utc();
    let mut groups: BTreeMap<(i64, &'static str), BTreeSet<i64>> = BTreeMap::new();
    let mut changed = false;
    for input in inputs {
        let minute = input.closedate.div_euclid(60) * 60;
        if minute >= current {
            continue;
        }
        let Some(currency) = crate::db::QuoteCurrency::from_report_ordinal(input.quote_ordinal)
        else {
            continue;
        };
        match super::cached_rate(store, input.quote_ordinal, minute) {
            Ok(Some(_)) => continue,
            Ok(None) => {
                groups
                    .entry((input.quote_ordinal, currency.ticker()))
                    .or_default()
                    .insert(minute);
            }
            Err(error) => {
                return Err(PrefetchError {
                    fault: super::store_fault(error),
                    changed,
                });
            }
        }
    }
    for ((quote_ordinal, ticker), minutes) in groups {
        let minutes = minutes.into_iter().collect::<Vec<_>>();
        let mut start = 0;
        while start < minutes.len() {
            let first = minutes[start];
            let mut end = start + 1;
            while end < minutes.len()
                && end - start < 1_000
                && minutes[end].saturating_sub(first) <= 999 * 60
            {
                end += 1;
            }
            let batch = resolve_rate_batch(source, quote_ordinal, ticker, &minutes[start..end]);
            let fetched_at = now_unix_ms_i64();
            for rate in &batch.ready {
                if rate.candle_close_ms >= fetched_at {
                    return Err(PrefetchError {
                        fault: FaultCause::new(
                            FailureKind::Provider,
                            format!(
                                "{} {} returned an unclosed minute {}",
                                rate.provider, rate.symbol, rate.minute_utc
                            ),
                        ),
                        changed,
                    });
                }
                match super::store_rate(store, rate, fetched_at) {
                    Ok(stored) => changed |= stored > 0,
                    Err(error) => {
                        return Err(PrefetchError {
                            fault: super::store_fault(error),
                            changed,
                        });
                    }
                }
            }
            if let Some(error) = batch.transient {
                return Err(PrefetchError {
                    fault: FaultCause::new(FailureKind::Provider, error),
                    changed,
                });
            }
            for minute in batch.missing {
                match super::store_permanent_missing(store, quote_ordinal, minute, fetched_at) {
                    Ok(stored) => changed |= stored > 0,
                    Err(error) => {
                        return Err(PrefetchError {
                            fault: super::store_fault(error),
                            changed,
                        });
                    }
                }
            }
            start = end;
        }
    }
    Ok(changed)
}

/// Publish durable prefetch progress even when a later request in the batch must retry.
///
/// Args:
///     result: Completed prefetch flag or transient failure with committed progress.
///     generation: Monotonic valuation publication counter.
///     dirty: Coalescing UI wake edge.
///
/// Returns:
///     Completed change flag, or the classified cause after publishing earlier progress.
fn settle_prefetch(
    result: Result<bool, PrefetchError>,
    generation: &AtomicU64,
    dirty: &AtomicBool,
) -> Result<bool, FaultCause> {
    match result {
        Ok(changed) => Ok(changed),
        Err(error) => {
            if error.changed {
                publish(generation, dirty);
            }
            Err(error.fault)
        }
    }
}

/// Merge one already-durable cache change into a prepared-row result.
///
/// Args:
///     result: Prepared-row completion or retry result.
///     extra_changed: Whether rate coverage changed before the row operation.
///
/// Returns:
///     Completion carrying both changes, or the original retry/deferred result.
fn merge_change(result: PrepareResult, extra_changed: bool) -> PrepareResult {
    match result {
        PrepareResult::Complete { changed } => PrepareResult::Complete {
            changed: changed || extra_changed,
        },
        PrepareResult::Deferred => PrepareResult::Deferred,
        PrepareResult::Retry(error) => PrepareResult::Retry(error),
    }
}

/// Load one current eligible report row after a durable outbox event.
///
/// Args:
///     conn: Report reader observing committed source data.
///     source: Typed or legacy physical source.
///     core_uid: Runtime core identity.
///     row_id: `newrecid` or `db_id` according to `source`.
///
/// Returns:
///     Complete valuation inputs, no eligible/current row, or a classified read failure.
fn load_trade(
    conn: &Connection,
    source: TradeSource,
    core_uid: i64,
    row_id: i64,
) -> ReadResult<Option<TradeInput>> {
    let Some((table, columns, id_column)) = source_layout(conn, source)? else {
        return Ok(None);
    };
    if !super::has_required_trade_inputs(&columns) {
        return Ok(None);
    }
    let spent = if columns.contains("spentbtc") {
        "CASE WHEN typeof(spentbtc) IN ('integer','real') THEN spentbtc END"
    } else {
        "NULL"
    };
    let sql = format!(
        "SELECT core_uid, {id_column}, closedate, basecurrency, profitbtc, {spent}
         FROM {table}
         WHERE core_uid=?1 AND {id_column}=?2
           AND typeof(closedate)='integer' AND closedate>0
           AND typeof(basecurrency)='integer' AND basecurrency BETWEEN 0 AND 20
           AND typeof(profitbtc) IN ('integer','real')"
    );
    conn.query_row(&sql, rusqlite::params![core_uid, row_id], |row| {
        Ok(TradeInput {
            source,
            core_uid: row.get(0)?,
            row_id: row.get(1)?,
            closedate: row.get(2)?,
            quote_ordinal: row.get(3)?,
            profit_quote: row.get(4)?,
            spent_quote: row.get(5)?,
        })
    })
    .optional()
    .map_err(|error| super::read_fail::read_fail_on(conn, "valuation: load report row", error))
}

/// Read one keyset batch whose prepared inputs are absent or stale, newest trade first.
///
/// The walk descends `closedate` because a report window shows recent trades: valuing the newest
/// rows first covers what the user is looking at within the first batches, instead of after the
/// whole history. `closedate` is not unique, so the cursor carries `core_uid` and the row id as
/// tie-breaks — a date-only cursor would either skip the rest of a tied group or re-read it
/// forever.
///
/// Time-clustered batches also collapse provider traffic: `prefetch_rates` requests one inclusive
/// minute range per quote, so rows sharing nearby minutes need roughly one request per quote per
/// batch, where a single core's consecutive row ids spanned weeks and cost dozens.
///
/// This ordering DEPENDS on `rep::REP_INDEXES`' `idx_rep_closedate`; the legacy table uses
/// `idx_csr_closedate`. Each index lets the planner seek by `closedate` and block-sort only the
/// `core_uid`/row-id ties; without the source's index, the statement requires a full sort per
/// batch because its primary-key and core indexes do not lead with `closedate`. `ensure_indexes`
/// creates the typed index as soon as that column exists, while database initialization creates
/// the legacy index whenever the legacy table exists. A typed replica still waiting for the
/// `closedate` column has no rows eligible for this query.
///
/// Args:
///     conn: Report reader with `valuation.sqlite` attached.
///     source: Typed or legacy physical source.
///     after: Exclusive descending cursor; `None` starts above the newest row.
///     limit: Maximum mismatched rows to return.
///
/// Returns:
///     Current complete inputs requiring preparation, ordered newest first.
fn reconciliation_batch(
    conn: &Connection,
    source: TradeSource,
    after: Option<ReconcileCursor>,
    limit: usize,
) -> ReadResult<Vec<TradeInput>> {
    let Some((table, columns, id_column)) = source_layout(conn, source)? else {
        return Ok(Vec::new());
    };
    if !super::has_required_trade_inputs(&columns) || !super::is_attached(conn) {
        return Ok(Vec::new());
    }
    let spent = if columns.contains("spentbtc") {
        "CASE WHEN typeof(r.spentbtc) IN ('integer','real') THEN r.spentbtc END"
    } else {
        "NULL"
    };
    let spent_match = if columns.contains("spentbtc") {
        "v.spent_quote IS CASE WHEN typeof(r.spentbtc) IN ('integer','real') THEN r.spentbtc END"
    } else {
        "v.spent_quote IS NULL"
    };
    // Seeded above every real key so the first batch starts at the newest row. The comparison is
    // strict, so the one key it cannot admit is a row holding `i64::MAX` in ALL THREE columns at
    // once; `closedate` is a Unix second and `core_uid`/`row_id` are allocated counters, so that
    // combination cannot occur. A row at `closedate == i64::MAX` alone is still admitted, through
    // the `core_uid` and row-id comparisons — which is why narrowing this cursor to `closedate`
    // alone would drop it. The `closedate>0` guard below plays no part in any of this.
    let (after_close, after_core, after_row) = after.unwrap_or((i64::MAX, i64::MAX, i64::MAX));
    let sql = format!(
        "SELECT r.core_uid, r.{id_column}, r.closedate, r.basecurrency, r.profitbtc, {spent}
         FROM {table} r
         LEFT JOIN valuation.trade_values v
           ON v.source_kind={source_kind}
          AND v.core_uid=r.core_uid AND v.row_id=r.{id_column}
          AND v.algorithm_version={algorithm_version}
          AND v.closedate=r.closedate AND v.quote_ordinal=r.basecurrency
          AND v.profit_quote=r.profitbtc AND {spent_match}
         LEFT JOIN valuation.rates mr
           ON mr.algorithm_version={algorithm_version}
          AND mr.quote_ordinal=r.basecurrency
          AND mr.minute_utc=(r.closedate/60)*60
          AND mr.status=1
         WHERE typeof(r.core_uid)='integer' AND typeof(r.{id_column})='integer'
           AND typeof(r.closedate)='integer' AND r.closedate>0
           AND typeof(r.basecurrency)='integer' AND r.basecurrency BETWEEN 0 AND 20
           AND typeof(r.profitbtc) IN ('integer','real')
           AND (r.closedate, r.core_uid, r.{id_column}) < (?1, ?2, ?3)
           AND v.row_id IS NULL AND mr.minute_utc IS NULL
         ORDER BY r.closedate DESC, r.core_uid DESC, r.{id_column} DESC LIMIT ?4",
        source_kind = source.code(),
        algorithm_version = super::ALGORITHM_VERSION,
    );
    let mut stmt = conn.prepare(&sql).map_err(|error| {
        super::read_fail::read_fail_on(conn, "valuation: reconcile prepare", error)
    })?;
    let rows = stmt
        .query_map(
            rusqlite::params![after_close, after_core, after_row, limit as i64],
            |row| {
                Ok(TradeInput {
                    source,
                    core_uid: row.get(0)?,
                    row_id: row.get(1)?,
                    closedate: row.get(2)?,
                    quote_ordinal: row.get(3)?,
                    profit_quote: row.get(4)?,
                    spent_quote: row.get(5)?,
                })
            },
        )
        .map_err(|error| {
            super::read_fail::read_fail_on(conn, "valuation: reconcile query", error)
        })?;
    let mut inputs = Vec::new();
    for row in rows {
        inputs.push(row.map_err(|error| {
            super::read_fail::read_fail_on(conn, "valuation: reconcile row", error)
        })?);
    }
    Ok(inputs)
}

/// Resolve one source's physical table, column set, and stable row-id column.
///
/// Args:
///     conn: Open report reader.
///     source: Typed or legacy source partition.
///
/// Returns:
///     Source layout, absence, or a classified schema-probe failure.
fn source_layout(
    conn: &Connection,
    source: TradeSource,
) -> ReadResult<
    Option<(
        &'static str,
        std::collections::HashSet<String>,
        &'static str,
    )>,
> {
    for layout in super::super::read_sources_res(conn)? {
        if layout.legacy == (source == TradeSource::Legacy) {
            let id_column = if layout.legacy { "db_id" } else { "newrecid" };
            if !layout.cols.contains(id_column) || !layout.cols.contains("core_uid") {
                return Ok(None);
            }
            return Ok(Some((layout.table, layout.cols, id_column)));
        }
    }
    Ok(None)
}

/// Delete one prepared value, reporting whether storage changed.
///
/// Args:
///     store: Open valuation writer connection.
///     source: Typed or legacy source partition.
///     core_uid: Runtime core identity.
///     row_id: Physical source row identity.
///
/// Returns:
///     Completed result carrying the delete-change flag, or a retry result on SQLite failure.
fn delete_trade(
    store: &Connection,
    source: TradeSource,
    core_uid: i64,
    row_id: i64,
) -> PrepareResult {
    match store.execute(
        "DELETE FROM trade_values WHERE source_kind=?1 AND core_uid=?2 AND row_id=?3",
        rusqlite::params![source.code(), core_uid, row_id],
    ) {
        Ok(changed) => PrepareResult::Complete {
            changed: changed > 0,
        },
        Err(error) => PrepareResult::Retry(super::store_fault(error)),
    }
}

/// Delete one prepared source/core partition after reset or legacy purge.
///
/// Args:
///     store: Open valuation writer connection.
///     source: Typed or legacy source partition.
///     core_uid: Runtime core identity.
///
/// Returns:
///     Completed result carrying the delete-change flag, or a retry result on SQLite failure.
fn delete_partition(store: &Connection, source: TradeSource, core_uid: i64) -> PrepareResult {
    match store.execute(
        "DELETE FROM trade_values WHERE source_kind=?1 AND core_uid=?2",
        rusqlite::params![source.code(), core_uid],
    ) {
        Ok(changed) => PrepareResult::Complete {
            changed: changed > 0,
        },
        Err(error) => PrepareResult::Retry(super::store_fault(error)),
    }
}

/// Process every deferred row whose containing minute is now closed.
///
/// Args:
///     store: Open valuation writer connection.
///     source: Historical closed-candle boundary.
///     generation: Monotonic valuation publication counter.
///     dirty: Coalescing UI wake edge.
///     deferred: Current-minute rows retained by identity.
///
/// Returns:
///     Completed stage turn indicating whether to loop immediately, or a classified failure.
fn process_deferred(
    store: &Connection,
    source: &dyn SpotRateSource,
    generation: &AtomicU64,
    dirty: &AtomicBool,
    deferred: &mut BTreeMap<(i64, i64, i64), TradeInput>,
) -> Result<StageTurn, FaultCause> {
    let current = current_minute_utc();
    let keys = deferred
        .iter()
        .filter(|(_, input)| input.closedate.div_euclid(60) * 60 < current)
        .map(|(key, _)| *key)
        .collect::<Vec<_>>();
    let inputs = keys
        .iter()
        .filter_map(|key| deferred.get(key).cloned())
        .collect::<Vec<_>>();
    let mut changed = settle_prefetch(prefetch_rates(store, source, &inputs), generation, dirty)?;
    for key in &keys {
        let Some(input) = deferred.get(key).cloned() else {
            continue;
        };
        match prepare_trade(store, source, &input) {
            PrepareResult::Complete {
                changed: input_changed,
            } => {
                changed |= input_changed;
                deferred.remove(key);
            }
            PrepareResult::Deferred => {}
            PrepareResult::Retry(error) => {
                if changed {
                    publish(generation, dirty);
                }
                return Err(error);
            }
        }
    }
    if changed {
        publish(generation, dirty);
    }
    Ok(StageTurn::Ran {
        more: !keys.is_empty(),
    })
}

/// Whether any retained row's candle minute has closed.
///
/// Args:
///     deferred: Current-minute rows retained by identity.
///
/// Returns:
///     `true` when at least one row can now be retried.
fn current_minute_closed_any(deferred: &BTreeMap<(i64, i64, i64), TradeInput>) -> bool {
    let current = current_minute_utc();
    deferred
        .values()
        .any(|input| input.closedate.div_euclid(60) * 60 < current)
}

/// Stable in-memory identity for one deferred prepared value.
///
/// Args:
///     input: Current report inputs.
///
/// Returns:
///     Source-kind/core/row tuple.
fn trade_key(input: &TradeInput) -> (i64, i64, i64) {
    (input.source.code(), input.core_uid, input.row_id)
}

/// Publish one committed valuation generation and coalescing UI edge.
///
/// Args:
///     generation: Monotonic valuation publication counter.
///     dirty: Coalescing UI wake edge.
fn publish(generation: &AtomicU64, dirty: &AtomicBool) {
    generation.fetch_add(1, Ordering::AcqRel);
    dirty.store(true, Ordering::Release);
}

/// Current UTC minute start in Unix seconds.
///
/// Returns:
///     Wall-clock minute boundary.
fn current_minute_utc() -> i64 {
    now_unix_ms_i64().div_euclid(60_000) * 60
}

/// Delay anchored to the next UTC minute boundary plus a small close-publication margin.
///
/// Returns:
///     Positive wait duration that does not drift from process start.
fn delay_to_next_minute() -> Duration {
    let now = now_unix_ms_i64();
    let next = (now.div_euclid(60_000) + 1) * 60_000 + 250;
    Duration::from_millis(next.saturating_sub(now).max(1) as u64)
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests;
