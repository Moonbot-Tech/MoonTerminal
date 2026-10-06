use super::*;

/// Result of preparing one current report row.
pub(in crate::db::valuation::worker) enum PrepareResult {
    /// The row is durably reflected, scheduled for retry, or no longer eligible.
    Complete { changed: bool },
    /// The row awaits either its minute close or a persisted successor-search retry boundary.
    Deferred { changed: bool },
    /// A classified provider, report-read, or cache failure requires a later retry.
    Retry(FaultCause),
}

/// Classified prefetch failure carrying any cache progress committed before the failure.
#[derive(Debug)]
pub(in crate::db::valuation::worker) struct PrefetchError {
    /// Classified failure retained for retry logging and publication.
    pub(in crate::db::valuation::worker) fault: FaultCause,
    /// Whether an earlier operation in the same prefetch batch changed durable coverage.
    pub(in crate::db::valuation::worker) changed: bool,
}

/// Successful exact-rate prefetch state consumed by the following per-row preparation pass.
#[derive(Debug, PartialEq)]
pub(in crate::db::valuation::worker) struct PrefetchOutcome {
    /// Provider fault retained after independent rows have been processed.
    pub(in crate::db::valuation::worker) provider_fault: Option<FaultCause>,
    /// Whether prefetch changed durable rate coverage.
    pub(in crate::db::valuation::worker) changed: bool,
    /// Quote/minute keys whose canonical Binance/Bybit exact routes were all absent.
    pub(in crate::db::valuation::worker) canonical_exact_missing: BTreeSet<(i64, i64)>,
}

/// Wait between polls while the report replica has not been created yet.
pub(in crate::db::valuation::worker) const REPLICA_POLL: Duration = Duration::from_secs(5);

/// What one stage turn accomplished.
///
/// Shared by all three work-stage attempts so an unavailable report replica has one outcome
/// wherever a stage reads it: healthy startup state, but not progress by that stage. Treating it as
/// progress would clear an unresolved failing run — a provider outage would be retracted by the
/// replica going missing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::db::valuation::worker) enum StageTurn {
    /// The stage did its work; `more` requests another worker-loop turn without parking.
    Ran { more: bool },
    /// Reconciliation reached the keyset tail of both physical sources.
    Drained,
    /// The report replica does not exist yet, which is a healthy startup state, not a failure.
    AwaitingReplica,
    /// The report replica exists, but a source's schema has not finished delivering the columns
    /// this stage needs — a healthy startup state, distinct from `AwaitingReplica` (whose subject
    /// is the replica FILE, not its schema). A cache-attachment failure is a THIRD, different
    /// fact and is never reported through `StageTurn` at all: it surfaces as a genuine `Err` and
    /// flows through `attempt`'s existing cache-recovery detection instead.
    AwaitingInputs,
}

impl StageTurn {
    /// Whether this turn counts as the stage making progress.
    ///
    /// Returns:
    ///     `false` while the stage could not access the report replica, or while it could but the
    ///     source's schema had not finished delivering the columns this stage needs.
    pub(in crate::db::valuation::worker) const fn is_progress(self) -> bool {
        !matches!(self, Self::AwaitingReplica | Self::AwaitingInputs)
    }
}

/// Classify one report-replica read failure.
///
/// Args:
///     error: Classified read failure from the report reader.
///
/// Returns:
///     Stage-less cause carrying the diagnostic text.
pub(in crate::db::valuation::worker) fn report_fault(error: ReadFail) -> FaultCause {
    FaultCause::new(FailureKind::ReportRead, error.to_string())
}

/// Exclusive descending reconciliation cursor: `(closedate, core_uid, row_id)`.
///
/// Named because the `deferred` map next door keys on a structurally identical `(i64, i64, i64)`
/// meaning `(source_code, core_uid, row_id)`. A Rust alias is transparent, so this does NOT stop
/// one being passed for the other — it only names the field order at each site so the swap is
/// visible while reading. Make it a newtype if that stops being enough.
pub(in crate::db::valuation::worker) type ReconcileCursor = (i64, i64, i64);

/// Incremental startup reconciliation cursor shared across worker-loop turns.
pub(in crate::db::valuation::worker) struct ReconcileState {
    pub(in crate::db::valuation::worker) source_index: usize,
    pub(in crate::db::valuation::worker) after: Option<ReconcileCursor>,
}

impl ReconcileState {
    /// Start at the typed source above its newest key.
    ///
    /// Returns:
    ///     Fresh incremental reconciliation cursor.
    pub(in crate::db::valuation::worker) const fn new() -> Self {
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
pub(in crate::db::valuation::worker) fn reset_after_recovery(
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
pub(in crate::db::valuation::worker) fn unacknowledged_events<'a>(
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
pub(in crate::db::valuation::worker) fn send_ack(
    report_tx: &ReportTx,
    pending_ack: &mut Option<i64>,
    through_seq: i64,
) {
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

/// Start a valuation worker with a caller-supplied deterministic or production spot-rate source.
///
/// Args:
///     report_tx: Sole report-writer sink used for outbox acknowledgements.
///     source: Closed-candle boundary used by historical and current-rate valuation.
///
/// Returns:
///     Worker handle, or `None` only when the background thread cannot be created. Storage
///     initialization failures remain retryable inside that thread.
pub(in crate::db::valuation::worker) fn spawn_worker_with_source(
    report_tx: ReportTx,
    source: Arc<dyn SpotRateSource>,
) -> Option<ValuationHandle> {
    let initial_store = match crate::db::valuation::open_canonical_store() {
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
    let current_wanted = Arc::new(AtomicBool::new(false));
    let thread_generation = generation.clone();
    let thread_dirty = commit_dirty.clone();
    let thread_current_wanted = current_wanted.clone();
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
                thread_current_wanted,
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
        current_wanted,
        thread,
    })
}
