//! Drains coordinator commands: `CoreCmd::SetMarket` carries the complete desired market role,
//! while strategy, trading, asset, and settings commands are deltas or actions.

use std::sync::mpsc::{Receiver, TryRecvError};

use moonproto::state::StratsState;
use moonproto::{MoonClient, StrategyFields, StrategyKind, StrategySchema, StrategySnapshot};

use super::account_reconciliation::BALANCE_TRACE_LEVEL;
use super::client_settings::{ClientSettingsSequence, ManualOrder};
use super::market_role::MarketRoleState;
use super::shared_config::SharedConfigSequence;
use crate::config::ServerConfig;
use crate::feed::assets::to_exchange_kind;
use crate::feed::strategies::{fv_from_str, strat_kind_name};
use crate::feed::{
    CoreCmd, CoreConfigEditEvent, LatestMarketRole, MarketRoleAssignment, UpdateTarget, order_edit,
    trade,
};
use crate::util::now_unix_ms as now_ms;

#[cfg(test)]
mod tests;

/// Maximum commands processed before a coalesced market role is applied and control is yielded.
const MAX_COMMANDS_PER_DRAIN: usize = 256;

/// Resolve a spec's placement anchor for the core it is actually being applied to.
///
/// THE one place a foreign anchor is dropped. Strategy ids are small per-core sequences, so an
/// id borrowed from another core almost certainly EXISTS here — it just belongs to an unrelated
/// strategy, and the copy would land silently beside that one instead of appending.
fn anchor_on_core(insert_after: Option<(u64, u64)>, core: u64) -> Option<u64> {
    insert_after
        .filter(|(anchor_core, _)| *anchor_core == core)
        .map(|(_, id)| id)
}

/// One slot of the core's strategy list while a create batch is being applied.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// A strategy the core already has, identified by its id.
    Existing(u64),
    /// A strategy this batch adds, carrying the id it asked to sit after.
    Added(Option<u64>),
}

/// Plans where each new strategy lands in the core's list, honouring "put it after this one".
///
/// Resolved against a MIRROR of the list as it will look while the batch is applied, not against
/// one snapshot of the ids: every insertion shifts everything after it, so a batch with two
/// different anchors resolved from stale indexes drops the later one in front of its own anchor.
/// Specs sharing an anchor keep the order they were given, each landing after the sibling placed
/// before it rather than reversing the batch.
///
/// An anchor this core does not have — a stale id or a cross-core paste — appends, preserving the
/// safe fallback for callers without a valid placement request.
///
/// Args:
///     ids: Strategy ids currently in the core's list, in list order.
///     anchors: Per-spec `insert_after`, in the order the specs will be inserted.
///
/// Returns:
///     One index per spec, to be used with `Vec::insert` in that same order.
fn plan_insert_positions(ids: &[u64], anchors: &[Option<u64>]) -> Vec<usize> {
    let mut live: Vec<Slot> = ids.iter().map(|id| Slot::Existing(*id)).collect();
    let mut out = Vec::with_capacity(anchors.len());
    for anchor in anchors.iter().copied() {
        let at = match anchor.and_then(|a| live.iter().position(|s| *s == Slot::Existing(a))) {
            Some(pos) => {
                let mut at = pos + 1;
                while live.get(at) == Some(&Slot::Added(anchor)) {
                    at += 1;
                }
                at
            }
            None => live.len(),
        };
        live.insert(at, Slot::Added(anchor));
        out.push(at);
    }
    out
}

/// Compare complete strategy placements without depending on snapshot list order.
///
/// Args:
///     current: Placements in the feed thread's latest MoonProto snapshot.
///     expected: Placements captured by the caller before a conditional destructive command.
///
/// Returns:
///     `true` only when both snapshots contain the same strategy ids at the same raw paths.
fn strategy_placements_unchanged(
    mut current: Vec<(u64, String)>,
    mut expected: Vec<(u64, String)>,
) -> bool {
    current.sort_unstable();
    expected.sort_unstable();
    current == expected
}

/// Tracks the newest full strategy list accepted by MoonProto's asynchronous runtime queue.
///
/// `MoonClient::snapshot()` changes only when that runtime later handles the queued batch. A guard
/// that reads only the public snapshot can therefore miss an earlier create or move from this same
/// feed thread. Keeping the queued placements closes that window without predicting server-side
/// changes: a conditional delete is allowed only when both views match the caller's evidence.
pub(super) struct StrategyPlacementGuard {
    queued_sync: Option<Vec<(u64, String)>>,
}

impl StrategyPlacementGuard {
    /// Create an empty guard before the feed thread has queued any full-list synchronization.
    pub(super) fn new() -> Self {
        Self { queued_sync: None }
    }

    /// Remember the placements in a full-list synchronization accepted by MoonProto's queue.
    fn note_queued_sync(&mut self, placements: Vec<(u64, String)>) {
        self.queued_sync = Some(placements);
    }

    /// Return whether live and still-pending placement views both match the caller's snapshot.
    ///
    /// Once the live snapshot catches up exactly, the redundant queued shadow is discarded. A
    /// later external mutation is then checked solely against the new live snapshot.
    fn allows_snapshot(
        &mut self,
        live: Option<Vec<(u64, String)>>,
        expected: Vec<(u64, String)>,
    ) -> bool {
        let Some(live) = live else {
            return false;
        };
        if self
            .queued_sync
            .as_ref()
            .is_some_and(|queued| strategy_placements_unchanged(live.clone(), queued.clone()))
        {
            self.queued_sync = None;
        }
        strategy_placements_unchanged(live, expected.clone())
            && self
                .queued_sync
                .as_ref()
                .is_none_or(|queued| strategy_placements_unchanged(queued.clone(), expected))
    }

    /// Read MoonProto's current placements and apply [`Self::allows_snapshot`].
    fn allows(&mut self, client: &MoonClient, expected: Vec<(u64, String)>) -> bool {
        self.allows_snapshot(snapshot_strategy_placements(client), expected)
    }
}

/// Clone only strategy ids and raw paths from MoonProto's current public snapshot.
fn snapshot_strategy_placements(client: &MoonClient) -> Option<Vec<(u64, String)>> {
    Some(
        client
            .snapshot()?
            .strats()
            .snapshots()
            .map(|strategy| (strategy.strategy_id, strategy.path.to_string()))
            .collect(),
    )
}

/// Result of one bounded command-drain pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CommandDrain {
    Disconnected,
    QueueEmpty,
    BudgetExhausted,
}

impl CommandDrain {
    /// Returns whether the live loop may block waiting for its next wake signal.
    pub(super) fn may_wait(self) -> bool {
        matches!(self, Self::QueueEmpty)
    }
}

/// Adopts the latest successfully queued market role while holding its publication lock.
///
/// The returned guard must remain alive until MoonProto receives the adopted role. This prevents a
/// concurrent sender from publishing account-only state between adoption and an older provider
/// apply.
pub(super) fn lock_and_adopt_latest_market_role<'a>(
    latest_market_role: &'a LatestMarketRole,
    market_role: &mut MarketRoleState,
    force_market_sample: &mut bool,
) -> std::sync::MutexGuard<'a, Option<MarketRoleAssignment>> {
    let latest = latest_market_role.lock();
    if let Some(assignment) = latest.as_ref() {
        *force_market_sample |= market_role.update(
            assignment.provider,
            assignment.markets.clone(),
            assignment.orderbook_markets.clone(),
        );
    }
    latest
}

/// Applies the authoritative market-role snapshot to the current MoonProto client.
fn apply_latest_market_role(
    latest_market_role: &LatestMarketRole,
    market_role: &mut MarketRoleState,
    force_market_sample: &mut bool,
    client: &MoonClient,
    server_id: u64,
) {
    let _latest =
        lock_and_adopt_latest_market_role(latest_market_role, market_role, force_market_sample);
    market_role.apply_if_needed(client, server_id);
}

/// Maps a `SignalType` field value to a strategy-kind (`StrategyKind`) ordinal. In Moonbot, a
/// strategy's type (kind) is its SignalType, but the snapshot stores the kind in a separate `kind`
/// byte rather than a field. Editing the field alone therefore does not change the kind, so map
/// the string to an ordinal and rebuild the snapshot consistently. First match the authoritative
/// kind names from the core schema, then fall back to our hard-coded names. No match means `None`
/// (leave the kind unchanged).
fn signaltype_to_kind_ordinal(schema: Option<&StrategySchema>, value: &str) -> Option<u8> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    if let Some(s) = schema {
        if let Some(k) = s.kinds.iter().find(|k| k.name.eq_ignore_ascii_case(v)) {
            return Some(k.ordinal());
        }
    }
    (0u8..=23).find(|o| strat_kind_name(*o).eq_ignore_ascii_case(v))
}

/// Tracks local strategy-command timestamps in a `HashMap` plus a wildcard so strat_db can
/// heuristically mark snapshot versions `origin=local`. The wildcard covers commands without a
/// known id (creation assigns the id inside `rebuild_sync`). The 30-second TTL can misclassify a
/// recent remote change as local or a delayed local echo as remote.
pub(super) struct LocalStratEdits {
    ids: std::collections::HashMap<u64, std::time::Instant>,
    wildcard: Option<std::time::Instant>,
}

const LOCAL_EDIT_TTL: std::time::Duration = std::time::Duration::from_secs(30);

impl LocalStratEdits {
    pub(super) fn new() -> Self {
        Self {
            ids: std::collections::HashMap::new(),
            wildcard: None,
        }
    }

    fn mark(&mut self, id: u64) {
        self.ids.insert(id, std::time::Instant::now());
    }

    fn mark_all(&mut self) {
        self.wildcard = Some(std::time::Instant::now());
    }

    /// Returns the 30-second local-origin heuristic for this id or the wildcard.
    pub(super) fn is_local(&self, id: u64) -> bool {
        let fresh = |t: &std::time::Instant| t.elapsed() < LOCAL_EDIT_TTL;
        self.ids.get(&id).map(fresh).unwrap_or(false)
            || self.wildcard.as_ref().map(fresh).unwrap_or(false)
    }

    pub(super) fn prune(&mut self) {
        self.ids.retain(|_, t| t.elapsed() < LOCAL_EDIT_TTL);
        if self
            .wildcard
            .map(|t| t.elapsed() >= LOCAL_EDIT_TTL)
            .unwrap_or(false)
        {
            self.wildcard = None;
        }
    }
}

/// Start the outgoing full-list sync from CONFIRMED state with every still-open edit's
/// DESIRED snapshot laid back over it, and every still-open create/restore appended.
///
/// Since MoonProto 9c7b3d73 `stage_local_strategies_owned` no longer overwrites local state,
/// `strats.snapshots()` is strictly core-confirmed. This guards against TWO distinct failure
/// modes, and a reader who has only seen the first must not "simplify" the second step away:
///
/// - **Reverted field.** Rebuilding from confirmed state alone would re-send the PRE-EDIT value
///   for every strategy whose previous edit has not yet been echoed, silently reverting the
///   user's change on the wire.
/// - **Vanished create/restore.** A still-open create or restore has, by construction, no
///   confirmed counterpart yet — it exists only as an entry in `strategy_edits()` — so it is
///   absent from `strats.snapshots()` entirely. Omitting step 2 below would drop it from the
///   NEXT unrelated outgoing sync altogether, since `stage_local_snapshot_batch` rebuilds its
///   whole `strategy_edits` map from whatever list this sends: a strategy missing from that list
///   is not merely reverted, it stops existing.
///
/// Applies to a `Pending` edit and a `TimedOut` one alike, in both steps: a timeout is
/// explicitly not a rejection in the upstream contract (a late core echo still confirms), so
/// dropping a timed-out desired value here would convert a lost echo into a real revert or a
/// real disappearance.
///
/// Appended entries are sorted by `(submitted_at, strategy_id)` because `strategy_edits()` is a
/// `HashMap` iterator with no stable order — an unsorted append would make the outgoing list
/// order vary between runs.
///
/// One accepted side effect: re-staging resets `submitted_at` and `deadline` for every still-
/// open edit, so an unrelated edit EXTENDS another's 45 s confirmation window. It can only ever
/// extend, never cause a false `TimedOut`. It is not fixed here — the fix belongs upstream.
fn overlay_pending_edits(strats: &StratsState) -> Vec<StrategySnapshot> {
    let mut full: Vec<StrategySnapshot> = strats
        .snapshots()
        .map(
            |confirmed| match strats.strategy_edit(confirmed.strategy_id) {
                Some(edit) => edit.desired().clone(),
                None => confirmed.clone(),
            },
        )
        .collect();

    let mut unconfirmed: Vec<_> = strats
        .strategy_edits()
        .filter(|(id, _)| strats.snapshot(*id).is_none())
        .map(|(_, edit)| (edit.submitted_at(), edit.desired().clone()))
        .collect();
    unconfirmed.sort_by_key(|(submitted_at, snapshot)| (*submitted_at, snapshot.strategy_id));
    full.extend(unconfirmed.into_iter().map(|(_, snapshot)| snapshot));

    full
}

/// Shared strategy-sync path: load the COMPLETE current set, let `build` edit it (patch fields,
/// change paths, or add entries), and send ONE `sync_local_strategies` plus a log entry if
/// anything changed. `build` returns the number of affected entries and increments `last_date`
/// (the Delphi rollback guard) on changed snapshots itself.
fn rebuild_sync(
    client: &MoonClient,
    server_id: u64,
    action: &str,
    strategy_placements: &mut StrategyPlacementGuard,
    build: impl FnOnce(&mut Vec<StrategySnapshot>, Option<&StrategySchema>, u64) -> usize,
) {
    if let Some(snap) = client.snapshot() {
        let strats = snap.strats();
        let schema = strats.strategy_schema();
        let now = now_ms() as u64;
        let mut full: Vec<StrategySnapshot> = overlay_pending_edits(strats);
        let changed = build(&mut full, schema, now);
        if changed > 0 {
            let placements = full
                .iter()
                .map(|strategy| (strategy.strategy_id, strategy.path.to_string()))
                .collect();
            match client.strategies().sync_local_strategies(full) {
                Ok(()) => {
                    strategy_placements.note_queued_sync(placements);
                    log::info!(
                        "core {} {action} {changed} strategies",
                        crate::feed::core_label(server_id)
                    );
                }
                Err(error) => log::warn!(
                    "core {} {action} strategies failed: {error}",
                    crate::feed::core_label(server_id)
                ),
            }
        }
    }
}

/// Drains one bounded coordinator-command batch while applying the latest market role separately.
///
/// `SetMarket` queue entries are wake/order markers; their payloads can be stale behind an action
/// backlog, so the shared authoritative snapshot is adopted before and after the batch. The return
/// value tells the live loop whether it disconnected, emptied the queue, or must poll again without
/// blocking. `core_config_events` collects any shared-config edit lifecycle events a queue-drain
/// send produced; the caller sends them as `FeedMsg::CoreConfigEdit` and stamps their clock, the
/// same as the events an event-batch-driven `SharedConfigSequence::drive` produces.
pub(super) fn drain_commands(
    cmd_rx: &Receiver<CoreCmd>,
    client: &MoonClient,
    server: &ServerConfig,
    latest_market_role: &LatestMarketRole,
    market_role: &mut MarketRoleState,
    force_market_sample: &mut bool,
    orders_mutated: &mut bool,
    local_strat_edits: &mut LocalStratEdits,
    strategy_placements: &mut StrategyPlacementGuard,
    client_settings_sequence: &mut ClientSettingsSequence,
    shared_config_sequence: &mut SharedConfigSequence,
    core_config_events: &mut Vec<CoreConfigEditEvent>,
) -> CommandDrain {
    apply_latest_market_role(
        latest_market_role,
        market_role,
        force_market_sample,
        client,
        server.id,
    );
    let mut drained = 0usize;
    loop {
        match cmd_rx.try_recv() {
            Ok(CoreCmd::SetMarket { .. }) => {}
            Ok(CoreCmd::StrategiesAction { checks, start_stop }) => {
                // 1. Synchronize checkboxes: update local `checked` on changed entries and send
                //    the delta (CheckedSync) to the server.
                for (id, checked) in &checks {
                    if let Err(error) = client.strategies().set_checked(*id, *checked) {
                        log::warn!(
                            "core {} set strategy {id} checked={checked} failed: {error}",
                            crate::feed::core_label(server.id)
                        );
                    }
                }
                if !checks.is_empty() {
                    if let Err(error) = client.strategies().send_checked_delta() {
                        log::warn!(
                            "core {} send checked delta failed: {error}",
                            crate::feed::core_label(server.id)
                        );
                    }
                }
                // 2. Start or stop checked strategies (a separate engine command).
                match start_stop {
                    Some(true) => {
                        if let Err(error) = client.strategies().start() {
                            log::warn!(
                                "core {} start strategies failed: {error}",
                                crate::feed::core_label(server.id)
                            );
                        }
                    }
                    Some(false) => {
                        if let Err(error) = client.strategies().stop() {
                            log::warn!(
                                "core {} stop strategies failed: {error}",
                                crate::feed::core_label(server.id)
                            );
                        }
                    }
                    None => {}
                }
                log::info!(
                    "core {} strategies action: checks={} start_stop={:?}",
                    crate::feed::core_label(server.id),
                    checks.len(),
                    start_stop
                );
            }
            Ok(CoreCmd::EditStrategyFields { edits }) => {
                for (id, _) in &edits {
                    local_strat_edits.mark(*id);
                }
                // `sync_local_strategies` SYNCHRONIZES THE ENTIRE local set (moonproto calls
                // replace_with_snapshots). Patch EVERY entry listed in `edits` in one pass and
                // issue one sync; separate commands for one core's strategies would overwrite
                // each other.
                rebuild_sync(
                    client,
                    server.id,
                    "edit",
                    strategy_placements,
                    |full, schema, now| {
                        let mut edited = 0usize;
                        for sc in full.iter_mut() {
                            let Some((_, changes)) =
                                edits.iter().find(|(id, _)| *id == sc.strategy_id)
                            else {
                                continue;
                            };
                            for (name, val) in changes {
                                let existing = sc.fields.get(name).cloned();
                                let stype = schema.and_then(|s| s.field(name)).map(|f| f.type_id);
                                sc.fields.insert(
                                    name.as_str(),
                                    fv_from_str(existing.as_ref(), stype, val),
                                );
                            }
                            // Changing SignalType changes the strategy kind. The snapshot stores the
                            // kind in a separate `pub(crate)` byte rather than a field, so rebuild it
                            // with the new `kind`; otherwise the tree's kind badge stays stale.
                            if let Some((_, sig)) = changes
                                .iter()
                                .find(|(n, _)| n.eq_ignore_ascii_case("SignalType"))
                            {
                                if let Some(ord) = signaltype_to_kind_ordinal(schema, sig) {
                                    if ord != sc.kind().ordinal() {
                                        *sc = StrategySnapshot::new(
                                            sc.strategy_id,
                                            sc.strategy_ver,
                                            sc.last_date,
                                            sc.checked,
                                            StrategyKind::from_ordinal(ord),
                                            sc.path.clone(),
                                            sc.fields.clone(),
                                        );
                                    }
                                }
                            }
                            // Deliberately not bumping `strategy_ver`: moonproto's `same_revision`
                            // compares `last_date` AND `strategy_ver`. We send back the last
                            // core-confirmed `strategy_ver` untouched, so if the core preserves it
                            // the echo matches and the edit resolves to `Confirmed`. Bumping it
                            // locally would be strictly worse — if the core does not preserve it,
                            // the echo matches neither `same_revision` nor
                            // `revision_strictly_dominates`, the edit resolves to nothing, and every
                            // successful edit would sit `Pending` until it reported `TimedOut` at
                            // 45 s. The Delphi rollback guard is `>=` on both fields, so bumping
                            // `last_date` alone already wins it.
                            sc.last_date = now.max(sc.last_date + 1);
                            edited += 1;
                        }
                        edited
                    },
                );
            }
            Ok(CoreCmd::DeleteStrategy { id }) => {
                // `TStratDelete(strategy_id=id, folder_path="")` deletes one strategy.
                // The UI enforces the "unchecked only" rule before sending the command.
                if let Err(error) = client.strategies().delete(id, "") {
                    log::warn!(
                        "core {} delete strategy {id} failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
                log::info!(
                    "core {} delete strategy {id}",
                    crate::feed::core_label(server.id)
                );
            }
            Ok(CoreCmd::DeleteStrategyIfUnchanged {
                id,
                expected_placements,
            }) => {
                if strategy_placements.allows(client, expected_placements) {
                    if let Err(error) = client.strategies().delete(id, "") {
                        log::warn!(
                            "core {} delete unchanged strategy {id} failed: {error}",
                            crate::feed::core_label(server.id)
                        );
                    } else {
                        log::info!(
                            "core {} delete unchanged strategy {id}",
                            crate::feed::core_label(server.id)
                        );
                    }
                } else {
                    log::warn!(
                        "core {} delete unchanged strategy {id} skipped: live or queued placements changed",
                        crate::feed::core_label(server.id)
                    );
                }
            }
            Ok(CoreCmd::DeleteFolder { path }) => {
                // `TStratDelete(strategy_id=0, folder_path=path)` deletes an entire folder.
                if let Err(error) = client.strategies().delete(0, path.as_str()) {
                    log::warn!(
                        "core {} delete folder {path} failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
                log::info!(
                    "core {} delete folder {path}",
                    crate::feed::core_label(server.id)
                );
            }
            Ok(CoreCmd::DeleteEmptyFolder {
                path,
                expected_placements,
            }) => {
                if strategy_placements.allows(client, expected_placements) {
                    // MoonProto has no atomic delete-if-empty precondition. This last local guard
                    // covers both its live snapshot and queued full-list syncs; an external client
                    // can still change the folder after this check.
                    if let Err(error) = client.strategies().delete(0, path.as_str()) {
                        log::warn!(
                            "core {} delete empty folder {path} failed: {error}",
                            crate::feed::core_label(server.id)
                        );
                    } else {
                        log::info!(
                            "core {} delete empty folder {path}",
                            crate::feed::core_label(server.id)
                        );
                    }
                } else {
                    log::warn!(
                        "core {} delete empty folder {path} skipped: live or queued placements changed",
                        crate::feed::core_label(server.id)
                    );
                }
            }
            Ok(CoreCmd::CreateStrategies { specs }) => {
                // New ids are assigned inside `rebuild_sync` (max + 1), so mark an edit to any id.
                local_strat_edits.mark_all();
                // Add new snapshots to the complete set. The id is max + 1 for the TARGET core,
                // which is safe for cross-core paste. Parse fields from strings according to the
                // schema type, as `fv_from_str` does for edits, with `existing=None`.
                rebuild_sync(
                    client,
                    server.id,
                    "create",
                    strategy_placements,
                    |full, schema, now| {
                        let mut next_id = full.iter().map(|s| s.strategy_id).max().unwrap_or(0) + 1;
                        // Plan the whole batch before insertion because each insertion shifts every
                        // later position; id assignment still scans the complete vector.
                        let ids: Vec<u64> = full.iter().map(|s| s.strategy_id).collect();
                        // The drain knows the destination core authoritatively, so it is the safe
                        // boundary for rejecting a foreign placement anchor.
                        let anchors: Vec<Option<u64>> = specs
                            .iter()
                            .map(|spec| anchor_on_core(spec.insert_after, server.id))
                            .collect();
                        let positions = plan_insert_positions(&ids, &anchors);
                        for (spec, at) in specs.iter().zip(positions) {
                            let id = next_id;
                            next_id += 1;
                            let mut fields = StrategyFields::new();
                            for (name, val) in &spec.fields {
                                let stype = schema.and_then(|s| s.field(name)).map(|f| f.type_id);
                                fields.insert(name.as_str(), fv_from_str(None, stype, val));
                            }
                            full.insert(
                                at,
                                StrategySnapshot::new(
                                    id,
                                    0,
                                    now,
                                    false,
                                    StrategyKind::from_ordinal(spec.kind_ordinal),
                                    spec.folder_path.clone(),
                                    fields,
                                ),
                            );
                        }
                        specs.len()
                    },
                );
            }
            Ok(CoreCmd::RestoreStrategy {
                id,
                kind_ordinal,
                folder_path,
                fields,
            }) => {
                local_strat_edits.mark(id);
                rebuild_sync(
                    client,
                    server.id,
                    "restore",
                    strategy_placements,
                    |full, schema, now| {
                        // It is already live (double-click in the menu or an echo), so do not duplicate it.
                        if full.iter().any(|s| s.strategy_id == id) {
                            return 0;
                        }
                        let mut f = StrategyFields::new();
                        for (name, val) in &fields {
                            let stype = schema.and_then(|s| s.field(name)).map(|x| x.type_id);
                            f.insert(name.as_str(), fv_from_str(None, stype, val));
                        }
                        full.push(StrategySnapshot::new(
                            id,
                            0,
                            now,
                            false, // A restored strategy is always UNCHECKED and must be enabled deliberately.
                            StrategyKind::from_ordinal(kind_ordinal),
                            folder_path.clone(),
                            f,
                        ));
                        1
                    },
                );
            }
            Ok(CoreCmd::MoveStrategies { moves }) => {
                // Change `path` and increment `last_date` for the selected strategies in one sync.
                rebuild_sync(
                    client,
                    server.id,
                    "move",
                    strategy_placements,
                    |full, _schema, now| {
                        let mut changed = 0usize;
                        for sc in full.iter_mut() {
                            if let Some((_, new_path)) =
                                moves.iter().find(|(id, _)| *id == sc.strategy_id)
                            {
                                sc.path = new_path.as_str().into();
                                sc.last_date = now.max(sc.last_date + 1);
                                changed += 1;
                            }
                        }
                        changed
                    },
                );
            }
            Ok(CoreCmd::TransferAsset {
                asset,
                qty,
                from,
                to,
            }) => {
                // Transfer strictly within THIS core because the client belongs to one core.
                if let Err(error) = client.balances().transfer_asset(
                    &asset,
                    qty,
                    to_exchange_kind(from),
                    to_exchange_kind(to),
                ) {
                    log::warn!(
                        "core {} transfer {qty} {asset} {from:?}->{to:?} failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
                // Request a fresh list after the transfer so the UI sees the new balances.
                if let Err(error) = client.balances().refresh_transfer_assets() {
                    log::warn!(
                        "core {} refresh transfer assets failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
                log::info!(
                    "core {} transfer {qty} {asset} {from:?}->{to:?}",
                    crate::feed::core_label(server.id)
                );
            }
            Ok(CoreCmd::RefreshTransferAssets) => {
                if let Err(error) = client.balances().refresh_transfer_assets() {
                    log::warn!(
                        "core {} refresh transfer assets failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
                // Also request a fresh balance snapshot as a manual nudge against phantom Assets
                // entries (a sold coin that remains stuck). Clicking the core in the window is the
                // user's request to reread balances. This is cheap: it queries the core, not the
                // exchange.
                if let Err(error) = client.balances().refresh() {
                    log::warn!(
                        "core {} balance refresh request failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                } else {
                    // Same level as the automatic path, and for a sharper reason than "a click is
                    // rare": the Assets panel re-requests for every scoped core on every cache
                    // rebuild while `transfer_rev == 0` (`panels/assets/cache.rs`), which measured
                    // 5250 of these lines a day with whole groups of cores stamped in the same
                    // millisecond. That re-request is a defect of its own; this keeps it out of the
                    // log until it is fixed — and the message says "refresh" rather than "click"
                    // because a click is not what usually produces it.
                    log::log!(
                        BALANCE_TRACE_LEVEL,
                        "core {} balance refresh requested (assets refresh)",
                        crate::feed::core_label(server.id)
                    );
                }
            }
            Ok(CoreCmd::ConvertDust) => {
                // Convert small balances to BNB through the Engine API; this is irreversible.
                if let Err(error) = client.balances().convert_dust_bnb() {
                    log::warn!(
                        "core {} convert dust failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
                if let Err(error) = client.balances().refresh_transfer_assets() {
                    log::warn!(
                        "core {} refresh transfer assets failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
                log::info!("core {} convert dust", crate::feed::core_label(server.id));
            }
            Ok(CoreCmd::ChartAlertUpsert {
                market,
                obj_uid,
                blob,
            }) => {
                if let Err(error) = client.chart_alerts().upsert(market.clone(), obj_uid, blob) {
                    log::warn!(
                        "core {} chart alert upsert {market} uid={obj_uid} failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                } else {
                    log::info!(
                        "core {} chart alert upsert {market} uid={obj_uid}",
                        crate::feed::core_label(server.id)
                    );
                }
            }
            Ok(CoreCmd::ChartAlertDelete { market, obj_uid }) => {
                if let Err(error) = client.chart_alerts().delete(market.clone(), obj_uid) {
                    log::warn!(
                        "core {} chart alert delete {market} uid={obj_uid} failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                } else {
                    log::info!(
                        "core {} chart alert delete {market} uid={obj_uid}",
                        crate::feed::core_label(server.id)
                    );
                }
            }
            // Order commands have different local visibility. Some edits of retained orders update
            // the local model synchronously, while new/join/split/close/sell commands are enqueued
            // without inserting their result locally. For flagged commands, `orders_mutated` asks
            // the feed loop for an immediate best-effort snapshot publish; that snapshot may still
            // precede an asynchronous mutation or omit a newly created row.
            Ok(CoreCmd::PlaceOrder {
                market,
                short,
                price,
                size,
                strategy_id,
                exit,
                planned_sell,
                sync_exit,
            }) => {
                client_settings_sequence.enqueue_order(ManualOrder {
                    market,
                    short,
                    price,
                    size,
                    strategy_id,
                    exit,
                    planned_sell,
                    sync_exit,
                });
            }
            Ok(CoreCmd::MoveOrder { uid, new_price }) => {
                trade::move_order(client, server.id, uid, new_price);
                *orders_mutated = true;
            }
            Ok(CoreCmd::CancelOrder { uid }) => {
                trade::cancel_order(client, server.id, uid);
                *orders_mutated = true;
            }
            Ok(CoreCmd::SetOrderStop { uid, kind, on }) => {
                trade::set_order_stop(client, server.id, uid, kind, on);
                *orders_mutated = true;
            }
            Ok(CoreCmd::MoveOrderStopPrice { uid, kind, price }) => {
                trade::move_order_stop_price(client, server.id, uid, kind, price);
                *orders_mutated = true;
            }
            Ok(CoreCmd::UpdateOrderStopsForm { uid, form }) => {
                order_edit::update_order_stops_form(client, server.id, uid, form);
                *orders_mutated = true;
            }
            Ok(CoreCmd::SetReportRowsDeleted {
                deleted,
                ranges,
                singles,
            }) => {
                // Soft-delete/restore intent. The core commits it and echoes
                // `ReportEvent::RowsDeleted`, which flips the local `deleted` flag; nothing is
                // written locally here. Not an order mutation, so no snapshot is forced.
                match client
                    .reports()
                    .set_rows_deleted(deleted, &ranges, &singles)
                {
                    Ok(n) => log::info!(
                        "core {} set report rows deleted={deleted} -> {n} батч(ей)",
                        crate::feed::core_label(server.id)
                    ),
                    Err(error) => {
                        log::warn!(
                            "core {} set_rows_deleted failed: {error}",
                            crate::feed::core_label(server.id)
                        )
                    }
                }
            }
            Ok(CoreCmd::PanicSellMarket { market, on }) => {
                trade::panic_sell_market(client, server.id, market, on);
                *orders_mutated = true;
            }
            Ok(CoreCmd::TurnOrderPanicSell { uid, on }) => {
                trade::turn_order_panic_sell(client, server.id, uid, on);
                *orders_mutated = true;
            }
            Ok(CoreCmd::MarketSellPosition { market }) => {
                trade::market_sell_position(client, server.id, market);
            }
            Ok(CoreCmd::MarketSellToken { market, size }) => {
                trade::market_sell_token(client, server.id, market, size);
            }
            Ok(CoreCmd::CancelMarketBuys { market }) => {
                trade::cancel_market_buys(client, server.id, &market);
            }
            Ok(CoreCmd::JoinSells { market, short }) => {
                trade::join_sells(client, server.id, market, short);
            }
            Ok(CoreCmd::SplitOrder { uid, parts }) => {
                trade::split_order(client, server.id, uid, parts);
            }
            Ok(CoreCmd::SplitOrderForMarket { market, parts }) => {
                trade::split_order_for_market(client, server.id, market, parts);
            }
            Ok(CoreCmd::ShiftOrdersPercent {
                market,
                sell,
                percent,
            }) => {
                trade::shift_orders_percent(client, server.id, market, sell, percent);
            }
            Ok(CoreCmd::MoveOrdersToPrice {
                market,
                sell,
                kind,
                price,
                side,
            }) => {
                trade::move_orders_to_price(client, server.id, market, sell, kind, price, side);
            }
            Ok(CoreCmd::SellsToZone {
                market,
                min_price,
                max_price,
                short,
            }) => {
                trade::sells_to_zone(client, server.id, market, min_price, max_price, short);
            }
            Ok(CoreCmd::EditClientSettings(edit)) => {
                client_settings_sequence.enqueue_edit(edit);
            }
            Ok(CoreCmd::EditCoreConfig { config, touched }) => {
                shared_config_sequence.enqueue(config, touched);
            }
            Ok(CoreCmd::RefreshSharedConfig) => {
                if let Err(error) = client.settings().refresh_shared_config() {
                    log::warn!(
                        "core {} refresh shared config failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
            }
            Ok(CoreCmd::SyncGroupExit(exit)) => {
                client_settings_sequence.enqueue_group_exit(exit);
            }
            Ok(CoreCmd::SetHedgeMode(on)) => {
                // This performs a REAL exchange action through the Engine API. Ignore the ticket;
                // its outcome arrives as an `Event::EngineAction`, NOT as a HedgeModeUpdated —
                // only `refresh_hedge_mode` produces that one. The event loop therefore re-reads
                // the mode when the action reports success; see the `Event::EngineAction` arm.
                match client.account().set_hedge_mode(on) {
                    Ok(_ticket) => log::info!(
                        "core {} set hedge mode -> {on}",
                        crate::feed::core_label(server.id)
                    ),
                    Err(error) => {
                        log::warn!(
                            "core {} set hedge mode -> {on} failed: {error}",
                            crate::feed::core_label(server.id)
                        )
                    }
                }
            }
            Ok(CoreCmd::SetLeverage { market, leverage }) => {
                // This performs a REAL exchange action through the Engine API. Ignore the ticket;
                // the new leverage arrives in a market balance push (`leverage_x`) and updates
                // the leverage map in Assets.
                match client.account().set_leverage(&market, leverage) {
                    Ok(_ticket) => {
                        log::info!(
                            "core {} set leverage {market} -> {leverage}x",
                            crate::feed::core_label(server.id)
                        )
                    }
                    Err(error) => log::warn!(
                        "core {} set leverage {market} -> {leverage}x failed: {error}",
                        crate::feed::core_label(server.id)
                    ),
                }
            }
            Ok(CoreCmd::RestartNow) => {
                // Start or restart the runtime; the result reaches the store via RuntimeStateUpdated.
                if let Err(error) = client.settings().restart_now() {
                    log::warn!(
                        "core {} restart_now failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                } else {
                    log::info!(
                        "core {} restart_now sent",
                        crate::feed::core_label(server.id)
                    );
                }
            }
            Ok(CoreCmd::UpdateVersion { target }) => {
                // Fire-and-forget: no ack, and MoonProto expects the link to drop. Completion is
                // observed only as a version change through the store; see `CoreCmd::UpdateVersion`.
                let result = match target {
                    UpdateTarget::Release => client.settings().request_release_update(),
                    UpdateTarget::Named(n) => client.settings().request_version_update(n),
                };
                if let Err(error) = result {
                    log::warn!(
                        "core {} update_version failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                } else {
                    log::info!(
                        "core {} update_version sent",
                        crate::feed::core_label(server.id)
                    );
                }
            }
            Ok(CoreCmd::SetAutoDetect(on)) => {
                // Passive mode off/on; the new value reaches the store via RuntimeStateUpdated,
                // the same command that carries `is_started`.
                if let Err(error) = client.settings().set_auto_detect_active(on) {
                    log::warn!(
                        "core {} set_auto_detect_active({on}) failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                } else {
                    log::info!(
                        "core {} set_auto_detect_active({on}) sent",
                        crate::feed::core_label(server.id)
                    );
                }
            }
            Ok(CoreCmd::ResetProfit(kind)) => {
                let proto_kind = match kind {
                    crate::feed::ResetProfitKind::Session => {
                        moonproto::ResetProfitKind::CurrentProfit
                    }
                    crate::feed::ResetProfitKind::All => moonproto::ResetProfitKind::AllProfit,
                };
                if let Err(error) = client.settings().reset_profit(proto_kind) {
                    log::warn!(
                        "core {} reset_profit({kind:?}) failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                } else {
                    log::info!(
                        "core {} reset_profit({kind:?}) sent",
                        crate::feed::core_label(server.id)
                    );
                }
            }
            Ok(CoreCmd::CancelAllOrders) => {
                // This performs a REAL exchange action. Ignore the ticket; the result arrives in
                // an order snapshot.
                match client.account().cancel_all_orders() {
                    Ok(_ticket) => log::info!(
                        "core {} cancel_all_orders sent",
                        crate::feed::core_label(server.id)
                    ),
                    Err(error) => {
                        log::warn!(
                            "core {} cancel_all_orders failed: {error}",
                            crate::feed::core_label(server.id)
                        )
                    }
                }
            }
            Ok(CoreCmd::SetBlacklist { on, text }) => {
                client_settings_sequence.enqueue_blacklist(on, text);
            }
            Ok(CoreCmd::SetExcludeBlacklistedDelta(on)) => {
                if let Err(error) = client
                    .settings()
                    .set_exclude_blacklisted_markets_from_exchange_delta(on)
                {
                    log::warn!(
                        "core {} set exclude blacklisted delta failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
            }
            Err(TryRecvError::Empty) => {
                apply_latest_market_role(
                    latest_market_role,
                    market_role,
                    force_market_sample,
                    client,
                    server.id,
                );
                *orders_mutated |= client_settings_sequence.drive(client, server.id);
                // Held back while a compact settings write is unechoed: a full-config packet
                // built on the stale retained snapshot would revert it. See
                // `ClientSettingsSequence::is_idle`.
                if client_settings_sequence.is_idle() {
                    shared_config_sequence.drive(client, server.id, core_config_events);
                } else {
                    shared_config_sequence.note_gated(server.id);
                }
                return CommandDrain::QueueEmpty;
            }
            Err(TryRecvError::Disconnected) => {
                let _ = client.disconnect();
                return CommandDrain::Disconnected;
            }
        }
        drained += 1;
        if drained >= MAX_COMMANDS_PER_DRAIN {
            apply_latest_market_role(
                latest_market_role,
                market_role,
                force_market_sample,
                client,
                server.id,
            );
            *orders_mutated |= client_settings_sequence.drive(client, server.id);
            if client_settings_sequence.is_idle() {
                shared_config_sequence.drive(client, server.id, core_config_events);
            } else {
                shared_config_sequence.note_gated(server.id);
            }
            return CommandDrain::BudgetExhausted;
        }
    }
}
