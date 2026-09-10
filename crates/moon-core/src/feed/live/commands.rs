//! Drains coordinator commands: `CoreCmd::SetMarket` carries the complete desired market role,
//! while strategy, trading, asset, and settings commands are deltas or actions.

use std::sync::mpsc::{Receiver, TryRecvError};

use moonproto::state::StratsState;
use moonproto::{MoonClient, StrategyKind, StrategySchema, StrategySnapshot};

use super::account_reconciliation::BALANCE_TRACE_LEVEL;
use super::client_settings::{ClientSettingsSequence, ManualOrder, ManualOrderKind};
use super::market_role::MarketRoleState;
use super::shared_config::SharedConfigSequence;
use crate::config::ServerConfig;
use crate::feed::assets::to_exchange_kind;
use crate::feed::strategies::{fields_from_text, fv_from_str, strat_kind_name};
use crate::feed::{
    CoreCmd, CoreConfigEditEvent, LatestMarketRole, MarketRoleAssignment, UpdateTarget, order_edit,
    trade,
};
use crate::util::now_unix_ms as now_ms;

#[cfg(test)]
mod tests;

/// Maximum commands processed before a coalesced market role is applied and control is yielded.
const MAX_COMMANDS_PER_DRAIN: usize = 256;

/// Last requested strategy-filter overlay market, re-sent when MoonClient is replaced.
///
/// Lives across [`super::run`] retries in the feed spawn loop: `applied` is per client, the
/// wanted market is the coordinator's. Resetting both on each `run` would clear a still-wanted
/// overlay and the UI would not re-issue an unchanged request.
#[derive(Default)]
pub(in crate::feed) struct ChartTextWanted {
    market: String,
    need_filters: bool,
    applied: bool,
}

impl ChartTextWanted {
    /// Forget the applied flag so a replacement client is told again.
    ///
    /// The wanted market is kept: it is what the coordinator last asked for, independent of which
    /// MoonClient is connected.
    pub(in crate::feed) fn begin_client(&mut self) {
        self.applied = false;
    }

    fn send(&mut self, client: &MoonClient, server_id: u64) {
        let result = if self.need_filters && !self.market.is_empty() {
            client
                .chart_text()
                .set_visible_market(&self.market, true, false)
        } else {
            client.chart_text().clear_visible_market()
        };
        match result {
            Ok(()) => self.applied = true,
            Err(error) => {
                self.applied = false;
                log::debug!(
                    "core {} set chart text market={} filters={} failed: {error}",
                    crate::feed::core_label(server_id),
                    self.market,
                    self.need_filters
                );
            }
        }
    }
}

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
    queued_order: Option<QueuedOrder>,
    queued_folders: Option<QueuedFolders>,
}

/// The strategy sequence the last accepted sync carried, and the confirmed order it was built on.
///
/// Kept because `strats.snapshots()` is the CORE-CONFIRMED order and moonproto rewrites it only
/// from the core's own Full echo. Between a reorder and that echo, every other strategy command —
/// a checkbox, a field edit, a move — rebuilds the outgoing list from the confirmed order and would
/// hand the core back the arrangement the operator had just replaced. So the queued sequence is
/// applied to every outgoing list until the core has answered.
/// The folder tree the last accepted folder edit carried, and the confirmed tree it was built on.
///
/// The same shape as [`QueuedOrder`] and for the same reason: `folder_paths()` is what the CORE has
/// confirmed, and between an edit and its echo a second edit built on that list would drop the
/// first. The version is the folder tree's own — moonproto advances it only when the core publishes
/// one — so once it moves, the core has spoken and this is retired.
struct QueuedFolders {
    /// Paths in the tree last sent.
    paths: Vec<String>,
    /// `StratsState::folders_last_modified` at that moment.
    base_modified: i64,
}

struct QueuedOrder {
    /// Strategy ids in the order last sent.
    ids: Vec<u64>,
    /// `StratsState::last_modified` at that moment: the version moonproto advances ONLY in
    /// `apply_server_order` — that is, only when the core publishes a full snapshot. Once it moves,
    /// the core has ruled, whether it accepted this order or overruled it, and re-asserting ours
    /// past that point would be an argument with no end.
    ///
    /// KNOWN LIMIT: that version belongs to the snapshot, not to the order, and moonproto exposes
    /// no order-specific one. So an unrelated full snapshot racing the send retires the sequence
    /// before the core has applied it, and the reorder is lost — the window goes on drawing it
    /// until its own confirmation window closes. The alternative, ignoring the version, is the
    /// endless argument above.
    base_modified: u64,
}

impl StrategyPlacementGuard {
    /// Create an empty guard before the feed thread has queued any full-list synchronization.
    pub(super) fn new() -> Self {
        Self {
            queued_sync: None,
            queued_order: None,
            queued_folders: None,
        }
    }

    /// The newest folder tree this terminal knows: the one it last sent, or the core's own.
    ///
    /// Args:
    ///     confirmed: The core's confirmed tree.
    ///     last_modified: That tree's version.
    ///
    /// Returns:
    ///     The base a folder edit must be applied to.
    fn folder_base(&mut self, confirmed: Vec<String>, last_modified: i64) -> Vec<String> {
        if self
            .queued_folders
            .as_ref()
            .is_some_and(|queued| queued.base_modified != last_modified)
        {
            self.queued_folders = None;
        }
        match &self.queued_folders {
            Some(queued) => queued.paths.clone(),
            None => confirmed,
        }
    }

    /// Remember a folder tree accepted by MoonProto's queue.
    fn note_queued_folders(&mut self, paths: Vec<String>, base_modified: i64) {
        self.queued_folders = Some(QueuedFolders {
            paths,
            base_modified,
        });
    }

    /// Remember what a full-list synchronization accepted by MoonProto's queue carried.
    ///
    /// Args:
    ///     placements: `(strategy id, raw folder path)` for every row in that list.
    ///     order: The same rows' ids, in the sequence they were sent.
    ///     base_modified: The confirmed order's version at the moment of sending.
    fn note_queued_sync(
        &mut self,
        placements: Vec<(u64, String)>,
        order: Vec<u64>,
        base_modified: u64,
    ) {
        self.queued_sync = Some(placements);
        self.queued_order = Some(QueuedOrder {
            ids: order,
            base_modified,
        });
    }

    /// The sequence still owed to the core, or `None` once the core has published its own.
    ///
    /// Args:
    ///     last_modified: The confirmed order's current version.
    ///
    /// Returns:
    ///     Ids in the order last sent, while that send is still the newest word on the subject.
    fn pending_order(&mut self, last_modified: u64) -> Option<&[u64]> {
        if self
            .queued_order
            .as_ref()
            .is_some_and(|queued| queued.base_modified != last_modified)
        {
            self.queued_order = None;
        }
        self.queued_order
            .as_ref()
            .map(|queued| queued.ids.as_slice())
    }

    /// Drop the queued sequence, once something has established that the core no longer owes it.
    fn retire_order(&mut self) {
        self.queued_order = None;
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
/// Returns the list, and whether the queued order it was given has been satisfied and can be
/// retired.
///
/// Appended entries are sorted by `(submitted_at, strategy_id)` because `strategy_edits()` is a
/// `HashMap` iterator with no stable order — an unsorted append would make the outgoing list
/// order vary between runs.
///
/// One accepted side effect: re-staging resets `submitted_at` and `deadline` for every still-
/// open edit, so an unrelated edit EXTENDS another's 45 s confirmation window. It can only ever
/// extend, never cause a false `TimedOut`. It is not fixed here — the fix belongs upstream.
fn overlay_pending_edits(
    strats: &StratsState,
    order: Option<&[u64]>,
) -> (Vec<StrategySnapshot>, bool) {
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

    // The order this terminal last sent and the core has not answered yet. Without it every command
    // here would rebuild the list in the CONFIRMED order and quietly undo a reorder still in
    // flight — see [`QueuedOrder`].
    if let Some(order) = order {
        let ranks: std::collections::HashMap<u64, usize> = order
            .iter()
            .enumerate()
            .map(|(rank, id)| (*id, rank))
            .collect();
        if crate::feed::strategy_order::resequence(&mut full, |sc| {
            ranks.get(&sc.strategy_id).copied()
        }) == 0
        {
            // The confirmed list already holds this sequence, so there is nothing left to owe. The
            // second retirement rule, and the one that covers a core whose Full carries no order
            // version at all: `last_modified` then never moves, and the version test alone would
            // keep re-asserting a sequence the core had already applied.
            return (full, true);
        }
    }

    (full, false)
}

/// Apply one folder-tree edit and send it, choosing the base and refusing what cannot be sent.
///
/// The base is the newest tree this terminal knows: the one it last sent while the core has not
/// answered, otherwise the core's confirmed one. That is the whole reason folder edits arrive here
/// as intents — a tree assembled upstream is assembled from a snapshot that may already be stale,
/// and the wire form deletes every folder it omits.
///
/// Refuses to send a tree moonproto would reject rather than discovering it as an error: some real
/// MoonBot folder names cannot survive its validator at all (see [`crate::feed::CoreFolders`]), and
/// on such a core a submission would be refused whole.
///
/// Args:
///     client: The core's client.
///     server_id: Core id, for the log.
///     action: Log label.
///     strategy_placements: Guard holding the tree this terminal last sent.
///     edit: Rewrites the base into the desired tree.
///
/// Returns:
///     Nothing; every refusal is logged where it happens.
fn folder_edit(
    client: &MoonClient,
    server_id: u64,
    action: &str,
    subject: &str,
    strategy_placements: &mut StrategyPlacementGuard,
    edit: impl FnOnce(&[String]) -> Vec<String>,
) {
    let Some(snap) = client.snapshot() else {
        log::warn!(
            "core {} {action} folder {subject:?} skipped: strategy state is not ready",
            crate::feed::core_label(server_id)
        );
        return;
    };
    let strats = snap.strats();
    let last_modified = strats.folders_last_modified();
    if last_modified == 0 {
        log::info!(
            "core {} {action} folder {subject:?} skipped: this core keeps no folder tree",
            crate::feed::core_label(server_id)
        );
        return;
    }
    let confirmed: Vec<String> = strats.folder_paths().map(str::to_string).collect();
    let base = strategy_placements.folder_base(confirmed, last_modified);
    let desired = edit(&base);
    // Validated the way moonproto validates it: a folder submission carries the tree, and the
    // library checks every current strategy path alongside it.
    let rows = strats.snapshots().map(|sc| sc.path.as_ref());
    if !crate::feed::folder_tree::sendable(desired.iter().map(String::as_str).chain(rows)) {
        log::warn!(
            "core {} {action} folder {subject:?} skipped: this core's tree holds a path              MoonProto refuses",
            crate::feed::core_label(server_id)
        );
        return;
    }
    let count = desired.len();
    match client.strategies().sync_local_folders(desired.clone()) {
        Ok(()) => {
            strategy_placements.note_queued_folders(desired, last_modified);
            log::info!(
                "core {} {action} folder {subject:?}, {count} in the tree",
                crate::feed::core_label(server_id)
            );
        }
        Err(error) => log::warn!(
            "core {} {action} folder {subject:?} failed: {error}",
            crate::feed::core_label(server_id)
        ),
    }
}

/// Join every relocated row to the run its destination folder ALREADY occupies.
///
/// Called after a move has rewritten `path`, so a folder the operator dropped rows into stays one
/// contiguous group rather than two — the core asks for that (moonproto `docs/strats.md`, "Strategy
/// Order"), and the tree places a folder where its first strategy appears, so a row left at its old
/// index can drag a whole folder to a new place from a gesture that named neither.
///
/// Three rules keep it from moving anything it was not asked to:
///
///   * Only rows whose path actually CHANGED are considered. A drag that includes rows already in
///     the destination leaves those exactly where they are.
///   * The anchor is a row that was NOT part of this move. So a folder RENAME — where every row
///     carrying the new name is one of the renamed ones — relocates nothing at all, and neither
///     does a move into a folder that does not exist yet.
///   * Rows joining the same run are placed in the order they were given, one after another.
///
/// Built as one rebuilding pass rather than a sequence of `remove`/`insert` calls. That is not a
/// matter of cost: every removal shifts every later index, so a plan expressed in positions goes
/// stale the moment two destinations interleave — which `ops::move_folder` and `ops::rename_folder`
/// both produce — and the rows then land one slot early, splitting the very runs this repairs.
/// Positions here are only ever read from the ORIGINAL list, and each row is emitted exactly once.
///
/// Args:
///     full: The complete strategy set, already carrying the new paths.
///     relocated: `(strategy id, new folder path)` for the rows whose folder actually changed.
///
/// Returns:
///     Nothing; a row with no existing destination run to join is left untouched.
fn regroup_moved(full: &mut Vec<StrategySnapshot>, relocated: &[(u64, String)]) {
    let moved: std::collections::HashSet<u64> = relocated.iter().map(|(id, _)| *id).collect();
    let index_of: std::collections::HashMap<u64, usize> = full
        .iter()
        .enumerate()
        .map(|(at, sc)| (sc.strategy_id, at))
        .collect();

    // Per anchor row, the ids that follow it. The anchor is the LAST row of that destination this
    // move did not touch; without one there is no run to join and the row is left alone.
    let mut following: std::collections::HashMap<usize, Vec<u64>> =
        std::collections::HashMap::new();
    let mut joining: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for (id, path) in relocated {
        if !index_of.contains_key(id) {
            continue;
        }
        let anchor = full
            .iter()
            .rposition(|sc| sc.path.as_ref() == path && !moved.contains(&sc.strategy_id));
        if let Some(anchor) = anchor {
            following.entry(anchor).or_default().push(*id);
            joining.insert(*id);
        }
    }
    if joining.is_empty() {
        return;
    }

    let mut slots: Vec<Option<StrategySnapshot>> =
        std::mem::take(full).into_iter().map(Some).collect();
    let mut rebuilt: Vec<StrategySnapshot> = Vec::with_capacity(slots.len());
    for at in 0..slots.len() {
        let Some(id) = slots[at].as_ref().map(|sc| sc.strategy_id) else {
            continue;
        };
        if joining.contains(&id) {
            // Emitted behind its anchor instead, wherever that sits.
            continue;
        }
        let Some(row) = slots[at].take() else {
            continue;
        };
        rebuilt.push(row);
        let Some(ids) = following.get(&at) else {
            continue;
        };
        for id in ids {
            if let Some(row) = index_of.get(id).and_then(|from| slots[*from].take()) {
                rebuilt.push(row);
            }
        }
    }
    *full = rebuilt;
}

/// Shared strategy-sync path: load the COMPLETE current set, let `build` edit it (patch fields,
/// change paths, or add entries), and send ONE `sync_local_strategies` plus a log entry if
/// anything changed. `build` returns the number of affected entries and increments `last_date`
/// (the Delphi rollback guard) on changed snapshots itself.
/// Returns whether the sync was actually ACCEPTED by the client queue: a caller that records the
/// change somewhere else — `local_strat_edits`, say — must not claim it happened when the send
/// failed or when `build` changed nothing.
#[must_use]
fn rebuild_sync(
    client: &MoonClient,
    server_id: u64,
    action: &str,
    strategy_placements: &mut StrategyPlacementGuard,
    folders: Option<Vec<String>>,
    build: impl FnOnce(&mut Vec<StrategySnapshot>, Option<&StrategySchema>, u64) -> usize,
) -> bool {
    if let Some(snap) = client.snapshot() {
        let strats = snap.strats();
        let schema = strats.strategy_schema();
        let now = now_ms() as u64;
        // Read before the list is built: it is both the baseline the queued order is judged against
        // and the one recorded with the next send.
        let last_modified = strats.last_modified();
        let (mut full, order_satisfied) =
            overlay_pending_edits(strats, strategy_placements.pending_order(last_modified));
        if order_satisfied {
            strategy_placements.retire_order();
        }
        let changed = build(&mut full, schema, now);
        // A folder tree is worth a snapshot on its own: a rename whose rows all vanished between
        // queueing and here still has to take the emptied folder with it.
        if changed > 0 || folders.is_some() {
            let placements = full
                .iter()
                .map(|strategy| (strategy.strategy_id, strategy.path.to_string()))
                .collect();
            let sequence: Vec<u64> = full.iter().map(|strategy| strategy.strategy_id).collect();
            // One snapshot for both when a folder tree comes along: the core applies the
            // strategy changes first and the newer tree second, which is what removes a folder the
            // strategies have just left. Sent as two commands they could arrive the other way
            // round, and the core would then refuse to drop a folder that still held rows.
            let queued = match folders {
                Some(paths) => client
                    .strategies()
                    .sync_local_strategies_with_folders(full, paths),
                None => client.strategies().sync_local_strategies(full),
            };
            match queued {
                Ok(()) => {
                    strategy_placements.note_queued_sync(placements, sequence, last_modified);
                    log::info!(
                        "core {} {action} {changed} strategies",
                        crate::feed::core_label(server_id)
                    );
                    return true;
                }
                Err(error) => log::warn!(
                    "core {} {action} strategies failed: {error}",
                    crate::feed::core_label(server_id)
                ),
            }
        }
    }
    false
}

/// Drains one bounded coordinator-command batch while applying the latest market role separately.
///
/// `SetMarket` queue entries are wake/order markers; their payloads can be stale behind an action
/// backlog, so the shared authoritative snapshot is adopted before and after the batch. The return
/// value tells the live loop whether it disconnected, emptied the queue, or must poll again without
/// blocking. `problems_relist` is raised by an operator-requested diagnostics re-read and consumed
/// by the caller's own publish block, so a burst of presses costs one snapshot read rather than
/// one per press. `core_config_events` collects any shared-config edit lifecycle events a queue-drain
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
    problems_relist: &mut bool,
    local_strat_edits: &mut LocalStratEdits,
    strategy_placements: &mut StrategyPlacementGuard,
    client_settings_sequence: &mut ClientSettingsSequence,
    shared_config_sequence: &mut SharedConfigSequence,
    core_config_events: &mut Vec<CoreConfigEditEvent>,
    chart_text: &mut ChartTextWanted,
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
                // Which strategies this command actually changed, filled inside the rebuild below.
                // Claiming an id as locally edited before knowing that would hand `strat_db` a
                // 30 s window in which a genuinely EXTERNAL change is stamped `origin = "local"`.
                let mut edited_ids: Vec<u64> = Vec::new();
                // `sync_local_strategies` SYNCHRONIZES THE ENTIRE local set (moonproto calls
                // replace_with_snapshots). Patch EVERY entry listed in `edits` in one pass and
                // issue one sync; separate commands for one core's strategies would overwrite
                // each other.
                let queued = rebuild_sync(
                    client,
                    server.id,
                    "edit",
                    strategy_placements,
                    // No folder tree: these edit rows, never the set of folders.
                    None,
                    |full, schema, now| {
                        let mut edited = 0usize;
                        for sc in full.iter_mut() {
                            let Some((_, changes)) =
                                edits.iter().find(|(id, _)| *id == sc.strategy_id)
                            else {
                                continue;
                            };
                            let mut applied = 0usize;
                            for (name, val) in changes {
                                let existing = sc.fields.get(name).cloned();
                                let stype = schema.and_then(|s| s.field(name)).map(|f| f.type_id);
                                let Some(value) = fv_from_str(existing.as_ref(), stype, val) else {
                                    // Leave the field at its current value. Inserting a fallback
                                    // here would send a number the user never typed, and the core
                                    // would answer with its own default for it.
                                    log::warn!(
                                        "core {} edit strategy {}: field {name} kept, {val:?} is not a value of its type",
                                        crate::feed::core_label(server.id),
                                        sc.strategy_id
                                    );
                                    continue;
                                };
                                sc.fields.insert(name.as_str(), value);
                                applied += 1;
                            }
                            // Every field was rejected: this strategy is untouched, so it must not
                            // claim a new `last_date` and must not make the batch look changed.
                            if applied == 0 {
                                continue;
                            }
                            edited_ids.push(sc.strategy_id);
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
                if queued {
                    for id in edited_ids {
                        local_strat_edits.mark(id);
                    }
                }
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
                //
                // The answer is ignored deliberately: `mark_all` above cannot be narrowed to ids
                // that do not exist yet, so there is nothing here to withhold on a failed send.
                let _ = rebuild_sync(
                    client,
                    server.id,
                    "create",
                    strategy_placements,
                    // No folder tree: these edit rows, never the set of folders.
                    None,
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
                            let fields = fields_from_text(
                                schema,
                                &spec.fields,
                                server.id,
                                &format!("create strategy {id}"),
                            );
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
                let queued = rebuild_sync(
                    client,
                    server.id,
                    "restore",
                    strategy_placements,
                    // No folder tree: these edit rows, never the set of folders.
                    None,
                    |full, schema, now| {
                        // It is already live (double-click in the menu or an echo), so do not duplicate it.
                        if full.iter().any(|s| s.strategy_id == id) {
                            return 0;
                        }
                        let f = fields_from_text(
                            schema,
                            &fields,
                            server.id,
                            &format!("restore strategy {id}"),
                        );
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
                if queued {
                    local_strat_edits.mark(id);
                }
            }
            Ok(CoreCmd::MoveStrategies { moves, rebase }) => {
                // The folder half, built HERE from the newest tree this terminal knows. Declined
                // whole — leaving the strategies to travel alone — when the result is something
                // MoonProto would refuse, because it validates the bundle as one: a tree it will
                // not take would otherwise turn a working rename into a refusal that moves nothing.
                let planned = rebase.and_then(|(old_key, new_key)| {
                    let snap = client.snapshot()?;
                    let strats = snap.strats();
                    let last_modified = strats.folders_last_modified();
                    if last_modified == 0 {
                        return None;
                    }
                    let confirmed: Vec<String> =
                        strats.folder_paths().map(str::to_string).collect();
                    let base = strategy_placements.folder_base(confirmed, last_modified);
                    let desired = crate::feed::folder_tree::rebase(&base, &old_key, &new_key);
                    // Every path the submission carries goes through the same validator as the
                    // tree — the folders, the rows as they stand, and the paths this move is about
                    // to give them, which is where a name the operator just typed shows up.
                    let rows = strats.snapshots().map(|sc| sc.path.as_ref());
                    let targets = moves.iter().map(|(_, path)| path.as_str());
                    let sendable = crate::feed::folder_tree::sendable(
                        desired
                            .iter()
                            .map(String::as_str)
                            .chain(rows)
                            .chain(targets),
                    );
                    if !sendable {
                        log::warn!(
                            "core {} move {old_key:?} -> {new_key:?}: folder tree left out, a                              path MoonProto refuses",
                            crate::feed::core_label(server.id)
                        );
                        return None;
                    }
                    Some((desired, last_modified))
                });
                let folders = planned.as_ref().map(|(desired, _)| desired.clone());
                // Change `path` and increment `last_date` for the selected strategies in one sync.
                let queued = rebuild_sync(
                    client,
                    server.id,
                    "move",
                    strategy_placements,
                    folders,
                    |full, _schema, now| {
                        let mut changed = 0usize;
                        let mut relocated: Vec<(u64, String)> = Vec::new();
                        for sc in full.iter_mut() {
                            if let Some((_, new_path)) =
                                moves.iter().find(|(id, _)| *id == sc.strategy_id)
                            {
                                if sc.path.as_ref() != new_path.as_str() {
                                    relocated.push((sc.strategy_id, new_path.clone()));
                                }
                                sc.path = new_path.as_str().into();
                                sc.last_date = now.max(sc.last_date + 1);
                                changed += 1;
                            }
                        }
                        regroup_moved(full, &relocated);
                        changed
                    },
                );
                // Recorded only once the queue has taken it. A tree noted before the send would
                // become the base of the NEXT folder edit while the core never received it.
                if let (true, Some((desired, base))) = (queued, planned) {
                    strategy_placements.note_queued_folders(desired, base);
                }
            }
            Ok(CoreCmd::AddFolder { path }) => {
                folder_edit(
                    client,
                    server.id,
                    "add",
                    &path,
                    strategy_placements,
                    |base| crate::feed::folder_tree::with_added(base, &path),
                );
            }
            Ok(CoreCmd::RemoveFolder { path }) => {
                folder_edit(
                    client,
                    server.id,
                    "remove",
                    &path,
                    strategy_placements,
                    |base| crate::feed::folder_tree::without(base, &path),
                );
            }
            Ok(CoreCmd::ReorderStrategies { order }) => {
                // The new SEQUENCE is the whole edit: no field is patched and no `last_date` moves,
                // because moonproto versions strategy order separately from per-strategy edit
                // dates and reads the order off the row sequence of the Full snapshot this sends.
                let _ = rebuild_sync(
                    client,
                    server.id,
                    "reorder",
                    strategy_placements,
                    None,
                    |full, _schema, _now| {
                        let ranks: std::collections::HashMap<u64, usize> = order
                            .iter()
                            .enumerate()
                            .map(|(rank, id)| (*id, rank))
                            .collect();
                        // Counted against the list as this terminal last left it — `full`
                        // arrives already carrying any order still owed to the core — so pressing
                        // Down and then Up inside one round trip is seen for what it is: a real
                        // change back, rather than a no-op against a confirmed order the core is
                        // no longer holding.
                        crate::feed::strategy_order::resequence(full, |sc| {
                            ranks.get(&sc.strategy_id).copied()
                        })
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
            Ok(CoreCmd::SetChartText {
                market,
                need_filters,
            }) => {
                chart_text.market = market;
                chart_text.need_filters = need_filters;
                chart_text.send(client, server.id);
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
                    kind: ManualOrderKind::Immediate { planned_sell },
                    sync_exit,
                });
            }
            Ok(CoreCmd::PlacePendingOrder {
                market,
                short,
                trigger_price,
                size,
                strategy_id,
                exit,
                sync_exit,
            }) => {
                // Behind the SAME barrier as an immediate order: a bare pending is priced from the
                // core's own exit generation when it fires, so releasing it ahead of that generation
                // would create it under the TP/SL the trader just replaced.
                client_settings_sequence.enqueue_order(ManualOrder {
                    market,
                    short,
                    price: trigger_price,
                    size,
                    strategy_id,
                    exit,
                    kind: ManualOrderKind::Pending,
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
            Ok(CoreCmd::MarketSellToken { market, qty, price }) => {
                trade::market_sell_token(client, server.id, market, qty, price);
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
            Ok(CoreCmd::SetFavMarket { market, on }) => {
                shared_config_sequence.enqueue_fav_market(market, on);
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
            Ok(CoreCmd::TestProblem { text }) => {
                // The reply is a diagnostic row arriving through the normal problems events, not a
                // response to this call — the library promises no request-specific answer. So the
                // log names only that it was sent, and the panel is where success is read.
                if let Err(error) = client.settings().test_problem(text.as_str()) {
                    log::warn!(
                        "core {} test_problem failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                } else {
                    log::info!(
                        "core {} test_problem sent: {text:?}",
                        crate::feed::core_label(server.id)
                    );
                }
            }
            Ok(CoreCmd::ClearProblems) => {
                // Logged at WARN even when it succeeds, unlike its neighbours: this drops confirmed
                // findings for every terminal watching the core and they cannot be restored, so the
                // log has to carry a trace of who asked for it.
                match client.settings().clear_problems() {
                    Err(error) => log::warn!(
                        "core {} clear_problems failed: {error}",
                        crate::feed::core_label(server.id)
                    ),
                    Ok(()) => log::warn!(
                        "core {} clear_problems sent — every confirmed diagnostic on this core is \
                         dropped for all terminals",
                        crate::feed::core_label(server.id)
                    ),
                }
            }
            Ok(CoreCmd::RefreshProblems) => {
                // Deliberately sends NOTHING: see `CoreCmd::RefreshProblems` for why no request
                // exists. The flag makes the live loop republish from the retained snapshot once
                // this batch is drained.
                *problems_relist = true;
                log::debug!(
                    "core {} problems re-read requested",
                    crate::feed::core_label(server.id)
                );
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
            Ok(CoreCmd::SetTempBlacklist { adds, removes }) => {
                client_settings_sequence.enqueue_temp_blacklist(adds, removes);
            }
            Ok(CoreCmd::SetDeltasByTrades(on)) => {
                if let Err(error) = client.streams().set_deltas_by_trades(on) {
                    log::warn!(
                        "core {} set deltas by trades failed: {error}",
                        crate::feed::core_label(server.id)
                    );
                }
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
                if !chart_text.applied {
                    chart_text.send(client, server.id);
                }
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
