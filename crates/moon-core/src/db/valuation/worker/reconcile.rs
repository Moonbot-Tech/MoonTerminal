use super::*;

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
///     axis: Per-core time axis rate minutes are resolved against.
///     generation: Monotonic valuation publication counter.
///     dirty: Coalescing UI wake edge.
///     deferred: Current-minute rows retained until their candle closes.
///     state: Source and keyset cursor advanced only after a complete batch.
///
/// Returns:
///     Whether the sources drained, advanced, or are still waiting for the report replica or its
///     schema.
pub(in crate::db::valuation::worker) fn reconcile_step(
    store: &Connection,
    source: &dyn SpotRateSource,
    axis: &mut ReportAxis,
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
        refresh_axis(&conn, axis)?;
        // `None` means this source's schema has not finished delivering the columns this walk
        // needs (a matched-but-incomplete layout, or a table not yet carrying `closedate`/
        // `basecurrency`/`profitbtc`) — a healthy startup state, never "this source is drained".
        // Treating it as an empty batch would advance `state.source_index` and permanently retire
        // the walk once both sources report it, which is exactly the collapse this stage must not
        // perform on a read failure. `AwaitingInputs` leaves `state.source_index`/`state.after`
        // untouched so the same source is retried once its schema catches up.
        //
        // A genuinely ABSENT source (no layout at all, e.g. a fully-migrated user's missing
        // legacy table) is NOT this case: `reconciliation_batch` reports that as a real, empty
        // `Some(Vec::new())`, which the `is_empty()` branch below correctly advances past.
        //
        // A cache-attachment failure is a THIRD, different fact, and is never routed through
        // `StageTurn` here at all — `reconciliation_batch` reports it as a genuine `Err`, which
        // the `?` below sends through the normal failure/cache-recovery path instead.
        let Some(inputs) = reconciliation_batch(&conn, trade_source, state.after, RECONCILE_BATCH)
            .map_err(report_fault)?
        else {
            return Ok(StageTurn::AwaitingInputs);
        };
        if inputs.is_empty() {
            state.source_index += 1;
            state.after = None;
            continue;
        }
        let turn = commit_batch(store, || {
            let mut turn = BatchTurn::default();
            let prefetched = match prefetch_rates(store, source, axis, &inputs) {
                Ok(prefetched) => prefetched,
                Err(error) => {
                    turn.changed = error.changed;
                    turn.fault = Some(error.fault);
                    return turn;
                }
            };
            turn.changed = prefetched.changed;
            let mut provider_fault = prefetched.provider_fault;
            for input in &inputs {
                let minute = valuation_minute(axis, input);
                match prepare_trade(
                    store,
                    source,
                    axis,
                    input,
                    prefetched
                        .canonical_exact_missing
                        .contains(&(input.quote_ordinal, minute)),
                ) {
                    PrepareResult::Complete {
                        changed: input_changed,
                    } => turn.changed |= input_changed,
                    PrepareResult::Deferred {
                        changed: input_changed,
                    } => {
                        turn.changed |= input_changed;
                        deferred.insert(trade_key(input), input.clone());
                    }
                    PrepareResult::Retry(error) if error.kind == FailureKind::Provider => {
                        match defer_provider_trade(store, axis, input) {
                            Ok(input_changed) => turn.changed |= input_changed,
                            Err(error) => {
                                turn.fault = Some(error);
                                return turn;
                            }
                        }
                        deferred.insert(trade_key(input), input.clone());
                        provider_fault.get_or_insert(error);
                    }
                    PrepareResult::Retry(error) => {
                        turn.fault = Some(error);
                        return turn;
                    }
                }
            }
            turn.provider_fault = provider_fault;
            turn
        })?;
        if turn.changed {
            publish(generation, dirty);
        }
        // A batch that ended early leaves the cursor where it is, so the walk retries it.
        if let Some(error) = turn.fault {
            return Err(error);
        }
        let provider_fault = turn.provider_fault;
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
        if let Some(error) = provider_fault {
            return Err(error);
        }
        return Ok(StageTurn::Ran { more: true });
    }
    Ok(StageTurn::Drained)
}

/// Whether a drained reconciliation walk should resume because the outbox drain this turn
/// observed a PARTITION-INVALIDATING event — one that wipes a whole core/source of prepared
/// values with no per-row outbox event left behind to re-value them individually.
///
/// Deliberately NOT keyed on the outbox batch size (`OUTBOX_BATCH`/a `batch_was_full` flag): that
/// couples a tuning constant to worker lifecycle and never re-arms at all for a cold start smaller
/// than one full batch. Deliberately NOT keyed on "any event this turn" either: on a live terminal
/// almost every turn drains at least one `Row`/`Delete` event, and each re-arm's "drained" outcome
/// is not cheap — `reconciliation_batch`'s antijoin (`LEFT JOIN ... AND v.row_id IS NULL`) cannot
/// short-circuit when nothing matches, so proving there is nothing left to reconcile costs a full
/// closedate-range index walk plus a per-row probe and a temp B-tree for the tie-break columns, for
/// BOTH sources, synchronously, before this turn's own outbox drain even runs. Re-arming on every
/// turn would turn a one-time backfill cost into a permanent per-turn full-history rescan competing
/// with the live outbox drain on the money-path thread — the very thing this goal exists to speed
/// up.
///
/// The only two facts that matter: whether reconciliation has anything left to do on its own, and
/// whether this turn's outbox activity actually left rows unvalued with no per-row event able to
/// catch them. Only `OutboxAction::RescanCore`/`PurgeLegacy` do that — both call
/// `delete_partition`, wiping `trade_values` for a whole core/source, so a walk is the only thing
/// that can recover the now-unvalued rows. `Row` values its own row directly in the same batch;
/// `Delete` removes a row that needs no revaluing; neither leaves anything for a walk to find. A
/// cold start needs no re-arm at all — `run_worker` already seeds `reconciliation = Some(..)`.
///
/// **Named residual, not silently fixed**: the walk this re-arms still drains `[Typed, Legacy]`
/// sequentially (see [`reconcile_step`]'s own doc), so "newest-first" holds PER SOURCE, not
/// globally — a core with a large, still-undrained legacy partition has its newest LEGACY rows
/// wait behind the entire typed backlog. Interleaving the two sources needs a merged two-cursor
/// walk, which would reshape the cursor expression `valuation_never_routed_contract.rs:156`
/// anchors verbatim; that is a materially bigger change than this goal and is not made here.
///
/// Args:
///     reconciliation: Current startup/backfill reconciliation cursor, or `None` once drained.
///     events: This turn's unacknowledged outbox events, already durability-filtered by the
///         caller.
///
/// Returns:
///     `true` when a drained walk should be re-armed: reconciliation is idle AND at least one
///     event this turn invalidated a whole partition.
pub(in crate::db::valuation::worker) fn outbox_activity_should_rearm_reconciliation(
    reconciliation: &Option<ReconcileState>,
    events: &[OutboxEvent],
) -> bool {
    reconciliation.is_none()
        && events.iter().any(|event| {
            matches!(
                event.action,
                OutboxAction::RescanCore | OutboxAction::PurgeLegacy
            )
        })
}

/// Consume one contiguous report outbox prefix and acknowledge it only after valuation commits.
///
/// A partition-invalidating event observed this turn also re-arms a drained reconciliation walk —
/// see [`outbox_activity_should_rearm_reconciliation`] for why only that event kind qualifies, and
/// for the per-source residual it does not fix.
///
/// Args:
///     store: Open valuation writer connection.
///     source: Historical closed-candle boundary.
///     axis: Per-core time axis rate minutes are resolved against.
///     report_tx: Sole report writer used for acknowledgement.
///     generation: Monotonic valuation publication counter.
///     dirty: Coalescing UI wake edge.
///     deferred: Current-minute rows retained until their candle closes.
///     pending_ack: Highest sequence sent to the report writer but not yet observed deleted.
///     reconciliation: Startup/backfill reconciliation cursor, re-armed here once drained if this
///         turn's durable activity makes that worthwhile.
///
/// Returns:
///     Whether another full outbox batch may already be waiting, or that the replica is absent.
pub(in crate::db::valuation::worker) fn consume_outbox(
    store: &Connection,
    source: &dyn SpotRateSource,
    axis: &mut ReportAxis,
    report_tx: &ReportTx,
    generation: &AtomicU64,
    dirty: &AtomicBool,
    deferred: &mut BTreeMap<(i64, i64, i64), TradeInput>,
    pending_ack: &mut Option<i64>,
    reconciliation: &mut Option<ReconcileState>,
) -> Result<StageTurn, FaultCause> {
    let conn = match crate::db::open_reader() {
        Ok(conn) => conn,
        Err(ReadFail::NotReady) => return Ok(StageTurn::AwaitingReplica),
        Err(error) => return Err(report_fault(error)),
    };
    refresh_axis(&conn, axis)?;
    let batch = crate::db::valuation::read_outbox(&conn, OUTBOX_BATCH).map_err(report_fault)?;
    let batch_was_full = batch.len() == OUTBOX_BATCH;
    let events = unacknowledged_events(&batch, pending_ack);
    if events.is_empty() {
        return Ok(StageTurn::Ran { more: false });
    }
    if outbox_activity_should_rearm_reconciliation(reconciliation, events) {
        *reconciliation = Some(ReconcileState::new());
    }
    let mut row_inputs = Vec::with_capacity(events.len());
    // One slot per event in `events`, index-aligned rather than keyed by identity. A duplicate
    // `Row` event for one identity therefore keeps its OWN observation instead of collapsing into
    // a shared map entry: a later `None` correctly overrides an earlier `Some` in pass 2 rather
    // than being lost behind it. It also lets pass 2 take ownership of each `TradeInput` by
    // value, so inserting one into `deferred` needs no clone.
    let mut loaded: Vec<Option<TradeInput>> = Vec::with_capacity(events.len());
    let mut loader = TradeLoader::default();
    for event in events {
        let input = if event.action == OutboxAction::Row {
            loader
                .load(&conn, event.source, event.core_uid, event.row_id)
                .map_err(report_fault)?
        } else {
            None
        };
        if let Some(input) = &input {
            row_inputs.push(input.clone());
        }
        loaded.push(input);
    }
    let turn = commit_batch(store, || {
        let mut turn = BatchTurn::default();
        let prefetched = match prefetch_rates(store, source, axis, &row_inputs) {
            Ok(prefetched) => prefetched,
            Err(error) => {
                turn.changed = error.changed;
                turn.fault = Some(error.fault);
                return turn;
            }
        };
        turn.changed = prefetched.changed;
        let mut provider_fault = prefetched.provider_fault;
        for (event, input) in events.iter().zip(loaded) {
            match process_event(
                store,
                source,
                axis,
                *event,
                input.clone(),
                deferred,
                &prefetched.canonical_exact_missing,
            ) {
                PrepareResult::Complete {
                    changed: event_changed,
                }
                | PrepareResult::Deferred {
                    changed: event_changed,
                } => {
                    turn.changed |= event_changed;
                    turn.acknowledged = Some(event.seq);
                }
                PrepareResult::Retry(error)
                    if error.kind == FailureKind::Provider && input.is_some() =>
                {
                    let input = input.as_ref().expect("provider row failure has input");
                    match defer_provider_trade(store, axis, input) {
                        Ok(event_changed) => turn.changed |= event_changed,
                        Err(error) => {
                            turn.fault = Some(error);
                            return turn;
                        }
                    }
                    deferred.insert(trade_key(input), input.clone());
                    provider_fault.get_or_insert(error);
                    turn.acknowledged = Some(event.seq);
                }
                PrepareResult::Retry(error) => {
                    turn.fault = Some(error);
                    return turn;
                }
            }
        }
        turn.provider_fault = provider_fault;
        turn
    })?;
    // Acknowledge and publish only what the batch transaction made durable.
    if let Some(through_seq) = turn.acknowledged {
        send_ack(report_tx, pending_ack, through_seq);
    }
    if turn.changed {
        publish(generation, dirty);
    }
    if let Some(error) = turn.fault.or(turn.provider_fault) {
        return Err(error);
    }
    Ok(StageTurn::Ran {
        more: batch_was_full,
    })
}

/// What one batch transaction did, settled by the caller only after it commits.
#[derive(Default)]
struct BatchTurn {
    /// Highest outbox sequence whose effect the batch wrote.
    acknowledged: Option<i64>,
    /// Whether any prepared value or cached rate changed.
    changed: bool,
    /// Failure that ended the batch early.
    fault: Option<FaultCause>,
    /// Provider outage met by rows the batch deferred; surfaced after the batch settles.
    provider_fault: Option<FaultCause>,
}

/// Run one batch of valuation-store writes as a single transaction.
///
/// The store is a rebuildable cache with this worker as its only writer, so one commit per batch
/// replaces one disk-synced autocommit per statement. The work done before an early failure is
/// committed as well, exactly as the per-statement autocommits persisted it; a crash before the
/// commit loses the whole batch, which the unacknowledged outbox or the unadvanced
/// reconciliation cursor re-derives.
///
/// Args:
///     store: Open valuation writer connection, outside any transaction.
///     body: The batch's writes; its outcome is returned once they are durable.
///
/// Returns:
///     The body's outcome, or the store fault when the transaction cannot begin or commit.
pub(in crate::db::valuation::worker) fn commit_batch<T>(
    store: &Connection,
    body: impl FnOnce() -> T,
) -> Result<T, FaultCause> {
    let transaction = store
        .unchecked_transaction()
        .map_err(crate::db::valuation::store_fault)?;
    let outcome = body();
    transaction
        .commit()
        .map_err(crate::db::valuation::store_fault)?;
    Ok(outcome)
}

/// Apply one durable report event to the prepared valuation store.
///
/// The row arm reuses the `TradeInput` the caller's pass-1 prefetch loop already loaded for THIS
/// event's own slot, rather than reading the report replica a second time. Under a quiescent
/// writer the two reads are provably identical; under a concurrent writer commit they could
/// already differ before this change — and today's second read silently disagreed with the
/// `prefetch_rates` batch computed from the first, which is strictly less coherent than reusing
/// one read for both. It cannot lose a newer value either way: the writer that mutated a row also
/// calls `stage_row` in the same transaction, producing a strictly higher outbox `seq`
/// (AUTOINCREMENT). `send_ack` only ever acknowledges a `seq` this worker actually saw, and
/// `ack_outbox` deletes the acknowledged prefix, so a newer write always survives as its own later
/// event, and `store_trade_value`'s upsert lets that later write win.
///
/// `loaded` is this event's own slot, not a shared lookup: a duplicate `Row` event for the same
/// identity earlier in the same batch is a DIFFERENT slot with its own independently-observed
/// value (`Some` or `None`), so a later `None` correctly deletes even when an earlier event in the
/// same batch saw `Some`.
///
/// Args:
///     store: Open valuation writer connection.
///     source: Historical closed-candle boundary.
///     axis: Per-core time axis rate minutes are resolved against.
///     event: Ordered durable outbox event.
///     loaded: This event's own pass-1 load outcome, `Some` for a `Row` event whose trade was
///         found, `None` otherwise.
///     deferred: Current-minute rows retained until their candle closes.
///     canonical_exact_missing: Keys already proven absent on canonical exact routes.
///
/// Returns:
///     Completed, deferred, or transient-retry result.
pub(in crate::db::valuation::worker) fn process_event(
    store: &Connection,
    source: &dyn SpotRateSource,
    axis: &ReportAxis,
    event: OutboxEvent,
    loaded: Option<TradeInput>,
    deferred: &mut BTreeMap<(i64, i64, i64), TradeInput>,
    canonical_exact_missing: &BTreeSet<(i64, i64)>,
) -> PrepareResult {
    match event.action {
        OutboxAction::Row => {
            let key = (event.source.code(), event.core_uid, event.row_id);
            deferred.remove(&key);
            match loaded {
                Some(input) => {
                    let minute = valuation_minute(axis, &input);
                    match prepare_trade(
                        store,
                        source,
                        axis,
                        &input,
                        canonical_exact_missing.contains(&(input.quote_ordinal, minute)),
                    ) {
                        PrepareResult::Deferred { changed } => {
                            deferred.insert(trade_key(&input), input);
                            PrepareResult::Deferred { changed }
                        }
                        result => result,
                    }
                }
                None => delete_trade(store, event.source, event.core_uid, event.row_id),
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
