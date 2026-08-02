//! Dedicated report reconciliation and historical-rate worker.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use super::provider::{resolve_rate, resolve_rate_batch, FetchFailure};
use super::{
    CachedRate, HttpSpotRateSource, OutboxAction, OutboxEvent, SpotRateSource, TradeInput,
    TradeSource,
};
use crate::db::{DbMsg, ReadFail, ReadResult, ReportTx};

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
    thread: std::thread::Thread,
}

impl ValuationHandle {
    /// Wake the worker after a committed report outbox change.
    pub fn wake(&self) {
        self.thread.unpark();
    }
}

/// Result of preparing one current report row.
enum PrepareResult {
    /// The row is durably reflected, permanently unavailable, or no longer eligible.
    Complete { changed: bool },
    /// The row belongs to the still-open current UTC minute.
    Deferred,
    /// A transient provider failure requires a later retry without acknowledging outbox work.
    Retry(String),
}

/// Transient prefetch failure carrying any cache progress committed before the failure.
#[derive(Debug)]
struct PrefetchError {
    /// Failure description retained for retry logging.
    message: String,
    /// Whether an earlier operation in the same prefetch batch changed durable coverage.
    changed: bool,
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
    let thread_generation = generation.clone();
    let thread_dirty = commit_dirty.clone();
    let join = std::thread::Builder::new()
        .name("report-valuation".to_string())
        .spawn(move || {
            run_worker(
                report_tx,
                source,
                thread_generation,
                thread_dirty,
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
///     initial_store: Startup-validated cache, or `None` when background recovery must retry.
fn run_worker(
    report_tx: ReportTx,
    source: Arc<dyn SpotRateSource>,
    generation: Arc<AtomicU64>,
    dirty: Arc<AtomicBool>,
    initial_store: Option<Connection>,
) {
    let mut store = initial_store;
    let mut deferred: BTreeMap<(i64, i64, i64), TradeInput> = BTreeMap::new();
    let mut reconciliation = Some(ReconcileState::new());
    let mut pending_ack = None;
    'worker: loop {
        if store.is_none() || !super::cache_is_healthy() {
            drop(store.take());
            match super::open_canonical_store() {
                Ok(recovered) => {
                    store = Some(recovered);
                    reset_after_recovery(&mut deferred, &mut reconciliation, &mut pending_ack);
                    publish(&generation, &dirty);
                    log::info!("valuation: derived cache is healthy; full reconciliation resumed");
                }
                Err(error) => {
                    log::warn!("valuation: cache recovery paused: {error}");
                    std::thread::park_timeout(Duration::from_secs(30));
                    continue;
                }
            }
        }
        let store_ref = store.as_ref().expect("valuation store recovered above");
        let mut retry_after = None;
        if let Some(state) = &mut reconciliation {
            match reconcile_step(
                store_ref,
                source.as_ref(),
                &generation,
                &dirty,
                &mut deferred,
                state,
            ) {
                Ok(true) => reconciliation = None,
                Ok(false) => {}
                Err(error) => {
                    log::warn!("valuation: startup reconciliation paused: {error}");
                    if !super::cache_is_healthy() {
                        continue 'worker;
                    }
                    retry_after = Some(Duration::from_secs(30));
                }
            }
        }
        match consume_outbox(
            store_ref,
            source.as_ref(),
            &report_tx,
            &generation,
            &dirty,
            &mut deferred,
            &mut pending_ack,
        ) {
            Ok(more) if more && retry_after.is_none() => continue,
            Ok(_) => {}
            Err(error) => {
                log::warn!("valuation: durable outbox paused: {error}");
                if !super::cache_is_healthy() {
                    continue 'worker;
                }
                retry_after = Some(Duration::from_secs(30));
            }
        }
        if current_minute_closed_any(&deferred) {
            match process_deferred(
                store_ref,
                source.as_ref(),
                &generation,
                &dirty,
                &mut deferred,
            ) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => {
                    log::warn!("valuation: deferred minute paused: {error}");
                    if !super::cache_is_healthy() {
                        continue 'worker;
                    }
                    retry_after = Some(Duration::from_secs(30));
                }
            }
        }
        let delay = retry_after.unwrap_or_else(|| {
            if reconciliation.is_some() {
                Duration::from_millis(25)
            } else if pending_ack.is_some() {
                Duration::from_millis(25)
            } else {
                delay_to_next_minute()
            }
        });
        std::thread::park_timeout(delay);
    }
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
///     Whether both physical report sources reached their keyset tails.
fn reconcile_step(
    store: &Connection,
    source: &dyn SpotRateSource,
    generation: &AtomicU64,
    dirty: &AtomicBool,
    deferred: &mut BTreeMap<(i64, i64, i64), TradeInput>,
    state: &mut ReconcileState,
) -> Result<bool, String> {
    let sources = [TradeSource::Typed, TradeSource::Legacy];
    while state.source_index < sources.len() {
        let trade_source = sources[state.source_index];
        let conn = crate::db::open_reader().map_err(|error| error.to_string())?;
        let inputs = reconciliation_batch(&conn, trade_source, state.after, RECONCILE_BATCH)
            .map_err(|error| error.to_string())?;
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
        return Ok(false);
    }
    Ok(true)
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
///     Whether another full outbox batch may already be waiting.
fn consume_outbox(
    store: &Connection,
    source: &dyn SpotRateSource,
    report_tx: &ReportTx,
    generation: &AtomicU64,
    dirty: &AtomicBool,
    deferred: &mut BTreeMap<(i64, i64, i64), TradeInput>,
    pending_ack: &mut Option<i64>,
) -> Result<bool, String> {
    let conn = match crate::db::open_reader() {
        Ok(conn) => conn,
        Err(ReadFail::NotReady) => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    let batch = super::read_outbox(&conn, OUTBOX_BATCH).map_err(|error| error.to_string())?;
    let batch_was_full = batch.len() == OUTBOX_BATCH;
    let events = unacknowledged_events(&batch, pending_ack);
    if events.is_empty() {
        return Ok(false);
    }
    let mut row_inputs = Vec::new();
    for event in events {
        if event.action == OutboxAction::Row {
            if let Some(input) = load_trade(&conn, event.source, event.core_uid, event.row_id)
                .map_err(|error| error.to_string())?
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
    Ok(batch_was_full)
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
                Err(error) => PrepareResult::Retry(error.to_string()),
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
                    if rate.candle_close_ms >= now_unix_ms() {
                        return PrepareResult::Deferred;
                    }
                    if let Err(error) = super::store_rate(store, &rate, now_unix_ms()) {
                        return PrepareResult::Retry(super::store_error(error));
                    }
                    rate
                }
                Err(FetchFailure::Missing) => {
                    let rate_changed = match super::store_permanent_missing(
                        store,
                        input.quote_ordinal,
                        minute_utc,
                        now_unix_ms(),
                    ) {
                        Ok(changed) => changed > 0,
                        Err(error) => return PrepareResult::Retry(super::store_error(error)),
                    };
                    return merge_change(
                        delete_trade(store, input.source, input.core_uid, input.row_id),
                        rate_changed,
                    );
                }
                Err(FetchFailure::Transient(error)) => return PrepareResult::Retry(error),
            }
        }
        Err(error) => return PrepareResult::Retry(super::store_error(error)),
    };
    match super::store_trade_value(store, input, &rate, now_unix_ms()) {
        Ok(changed) => PrepareResult::Complete {
            changed: changed > 0,
        },
        Err(error) => PrepareResult::Retry(super::store_error(error)),
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
                    message: super::store_error(error),
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
            let fetched_at = now_unix_ms();
            for rate in &batch.ready {
                if rate.candle_close_ms >= fetched_at {
                    return Err(PrefetchError {
                        message: format!(
                            "{} {} returned an unclosed minute {}",
                            rate.provider, rate.symbol, rate.minute_utc
                        ),
                        changed,
                    });
                }
                match super::store_rate(store, rate, fetched_at) {
                    Ok(stored) => changed |= stored > 0,
                    Err(error) => {
                        return Err(PrefetchError {
                            message: super::store_error(error),
                            changed,
                        });
                    }
                }
            }
            if let Some(error) = batch.transient {
                return Err(PrefetchError {
                    message: error,
                    changed,
                });
            }
            for minute in batch.missing {
                match super::store_permanent_missing(store, quote_ordinal, minute, fetched_at) {
                    Ok(stored) => changed |= stored > 0,
                    Err(error) => {
                        return Err(PrefetchError {
                            message: super::store_error(error),
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
///     Completed change flag, or the retry description after publishing earlier progress.
fn settle_prefetch(
    result: Result<bool, PrefetchError>,
    generation: &AtomicU64,
    dirty: &AtomicBool,
) -> Result<bool, String> {
    match result {
        Ok(changed) => Ok(changed),
        Err(error) => {
            if error.changed {
                publish(generation, dirty);
            }
            Err(error.message)
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
        Err(error) => PrepareResult::Retry(super::store_error(error)),
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
        Err(error) => PrepareResult::Retry(super::store_error(error)),
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
///     Whether any eligible row was processed, or a transient failure.
fn process_deferred(
    store: &Connection,
    source: &dyn SpotRateSource,
    generation: &AtomicU64,
    dirty: &AtomicBool,
    deferred: &mut BTreeMap<(i64, i64, i64), TradeInput>,
) -> Result<bool, String> {
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
    Ok(!keys.is_empty())
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
    now_unix_ms().div_euclid(60_000) * 60
}

/// Current wall-clock time in Unix milliseconds.
///
/// Returns:
///     Saturating signed timestamp.
fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

/// Delay anchored to the next UTC minute boundary plus a small close-publication margin.
///
/// Returns:
///     Positive wait duration that does not drift from process start.
fn delay_to_next_minute() -> Duration {
    let now = now_unix_ms();
    let next = (now.div_euclid(60_000) + 1) * 60_000 + 250;
    Duration::from_millis(next.saturating_sub(now).max(1) as u64)
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests;
