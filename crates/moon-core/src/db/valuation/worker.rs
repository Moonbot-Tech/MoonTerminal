//! Dedicated report reconciliation and historical-rate worker.
//!
//! # Which `closedate` reads pass through the time axis, and which never may
//!
//! A report row's `closedate` is CORE-LOCAL wall clock ([`crate::db::report_axis`]), while the
//! spot-rate series this worker keys into is genuinely UTC. Every derivation of a RATE MINUTE
//! therefore goes through the one seam [`valuation_minute`], against the single [`ReportAxis`]
//! bound in [`run_worker`]. There is no second derivation: a new one is a new bug on a skewed
//! core, which is why the funnel is a named function rather than a convention.
//!
//! The opposite half is just as load-bearing. Reads that use `closedate` as IDENTITY or ORDERING
//! stay on the RAW column permanently and must never be routed through the axis:
//!
//! - [`reconciliation_batch`]'s keyset query and its `(closedate, core_uid, row_id)` descending
//!   cursor. Its total monotonicity is what makes the walk terminate exactly once; correcting the
//!   column would reorder rows under a cursor already past them, so a batch would be re-visited or
//!   skipped outright — and a better offset estimate later would do it again to all of history.
//! - The `v.closedate = r.closedate` coverage join and the `trade_values` upsert key
//!   (`super::coverage_sql`, `super::store_trade_value`). Both sides of that comparison are the
//!   stored value; converting one of them matches nothing.
//! - [`trade_key`], the in-memory deferred-row identity.
//!
//! `moon-core/tests/valuation_never_routed_contract.rs` anchors on the exact source text of all
//! four, so routing one of them through the axis reddens rather than silently costing a
//! reconciliation pass. It is a source-text test because all four sit in private items an
//! integration test cannot reach; the funnel's own behaviour — [`valuation_minute`] converting
//! before it floors — is asserted properly in `worker/tests.rs`, which can see it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use rusqlite::Connection;

use super::health::{
    self, FailureKind, FaultCause, ValuationFault, ValuationStage, ValuationStatus,
};
use super::provider::{FetchFailure, resolve_latest_rate, resolve_rate_batch};
use super::resolver::resolve_historical_rate;
use super::{
    HttpSpotRateSource, OutboxAction, OutboxEvent, SpotRateSource, TradeInput, TradeSource,
};
use crate::db::{DbMsg, FailKind, ReadFail, ReadResult, ReportAxis, ReportTx};
use crate::util::now_unix_ms_i64;

/// Number of report rows reconciled before publishing progress and yielding to durable outbox work.
const RECONCILE_BATCH: usize = 256;

/// Number of durable report changes handled in one ordered prefix.
const OUTBOX_BATCH: usize = 512;

/// Maximum deferred rows processed before yielding to reconciliation and durable outbox work.
const DEFERRED_BATCH: usize = 64;

mod handle;
pub use handle::ValuationHandle;
pub(in crate::db::valuation) use handle::wake_for_recovery;
use handle::*;

mod stage;
pub use stage::spawn_worker;
use stage::*;

mod schedule;
use schedule::*;

mod reconcile;
use reconcile::*;

mod trades;
use trades::*;

mod live_rates;
use live_rates::*;

mod deferred;
use deferred::*;

mod keys;
use keys::*;

/// Refresh current rates, reconcile historical rows, consume durable changes, and park until the
/// next due retry, freshness, stall, report, or minute boundary.
///
/// Args:
///     report_tx: Sole report-writer sink used for outbox acknowledgements.
///     source: Closed-candle boundary used by historical and current-rate valuation.
///     generation: Monotonic valuation publication counter.
///     dirty: Coalescing UI wake edge.
///     current_wanted: Whether the application-wide current-rate mode is enabled.
///     sink: Channel publishing worker health to the UI.
///     initial_store: Startup-validated cache, or `None` when background recovery must retry.
fn run_worker(
    report_tx: ReportTx,
    source: Arc<dyn SpotRateSource>,
    generation: Arc<AtomicU64>,
    dirty: Arc<AtomicBool>,
    current_wanted: Arc<AtomicBool>,
    mut sink: StatusSink,
    initial_store: Option<Connection>,
) {
    let mut store = initial_store;
    // The one axis every rate-minute derivation in this worker resolves against.
    //
    // Seeded as the identity and REFRESHED by the two stages that open a report reader of their
    // own — see `refresh_axis`. It is not loaded once here because a measured offset arrives while
    // this worker is already running: the writer that stores a segment also stages a rescan for
    // that core, so the rows arrive at a stage that must already be on the new axis to value them
    // at the right minute.
    let mut axis = ReportAxis::identity_core_local();
    let mut deferred: BTreeMap<(i64, i64, i64), TradeInput> = BTreeMap::new();
    let mut reconciliation = Some(ReconcileState::new());
    let mut pending_ack = None;
    let mut status = ValuationStatus::default();
    let mut current = CurrentRateState::default();
    'worker: loop {
        let now_ms = now_unix_ms_i64();
        let mut retry_after = None;
        // BEFORE the cache-recovery gate on purpose. Current rates live in memory and need no
        // derived cache, so an unopenable `valuation.sqlite` — an entirely unrelated failure — must
        // not also take the mode that does not depend on it. Everything below this point requires
        // an open store; this stage does not.
        if current_wanted.load(Ordering::Acquire) {
            // OUTSIDE `attempt`, and therefore outside the stage's provider backoff. Retiring an
            // expired rate is not a provider operation: gating it on the retry gate would mean a
            // failing provider — the very case that lets a rate expire — also decides when the
            // expiry is noticed, and a 300 s backoff would keep an expired figure on screen for
            // five minutes past the cutoff it is supposed to enforce.
            if current.expire_stale(now_ms) {
                current.publish_snapshot(&generation, &dirty, now_ms);
            }
            // Only `Ran { more: true }` short-circuits, and it must: the remaining currencies
            // would otherwise be resolved a minute apart. Everything else falls through,
            // `Attempt::CacheLost` included — a provider failure while the derived cache is
            // also unhealthy belongs to the recovery stage below, whose own fault is the one
            // that matters in that window.
            if let Attempt::Done(StageTurn::Ran { more: true }) = attempt(
                &mut status,
                &mut sink,
                ValuationStage::CurrentRates,
                now_ms,
                &mut retry_after,
                || refresh_current_rates(source.as_ref(), &generation, &dirty, &mut current),
            ) {
                continue;
            }
        } else {
            // Current-rate mode is disabled, so this stage issues no provider request at all — the
            // whole point of the demand flag. Progress is still recorded so a run left failing when
            // the mode was switched off cannot report a stall for an idle feature.
            current.stand_down();
            record_progress(&mut status, &mut sink, ValuationStage::CurrentRates, now_ms);
        }
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
                    // Capped by the freshness deadline too: an unopenable derived cache must not
                    // decide how long an expired current rate — which needs no cache — stays up.
                    //
                    // `deferred_due` is `false` here: the store was just dropped above, so no
                    // deferred row can be processed on the next turn and the minute boundary is
                    // not a deadline yet. Capping this backoff to it would turn the cache-recovery
                    // wait into a one-minute retry loop.
                    let settled = now_unix_ms_i64();
                    park_worker(&status, &current, &current_wanted, false, delay, settled);
                    continue;
                }
            }
        }
        let store_ref = store.as_ref().expect("valuation store recovered above");
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
                        &mut axis,
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
                    &mut axis,
                    &report_tx,
                    &generation,
                    &dirty,
                    &mut deferred,
                    &mut pending_ack,
                    &mut reconciliation,
                )
            },
        ) {
            Attempt::CacheLost => continue 'worker,
            // Deliberately NOT conditioned on `retry_after`: every stage guards its own backoff, so
            // draining a full outbox batch cannot re-run a stage that is resting. Gating this on
            // another stage's wait would throttle live report changes to one 512-event batch per
            // backoff, up to five minutes apart. The reconciliation re-arm itself lives inside
            // `consume_outbox` (see `outbox_activity_should_rearm_reconciliation`), keyed on
            // whether the drain saw a partition-invalidating event this turn — not on any event,
            // and not on `more`/`batch_was_full`.
            Attempt::Done(StageTurn::Ran { more: true }) => continue,
            _ => {}
        }
        if current_minute_closed_any(store_ref, &axis, &deferred) {
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
                        &axis,
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
        // The freshness deadline caps the park exactly as the stall deadline does. Expiry is
        // evaluated when the loop turns, so a worker asleep on a five-minute provider backoff would
        // otherwise notice the cutoff only when that backoff ended — and health-only revisions
        // deliberately requery nothing, so the expired figure would stay on screen meanwhile.
        park_worker(
            &status,
            &current,
            &current_wanted,
            !deferred.is_empty(),
            delay,
            settled,
        );
    }
}

#[cfg(test)]
mod tests;
