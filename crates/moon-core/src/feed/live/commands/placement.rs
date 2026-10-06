//! Strategy revision stamps, placement plans and queued placement guards.

use super::*;

/// The `strategy_ver` a strategy created or restored on this core is sent with: the revision the
/// core itself stamps, read off the strategies it already holds.
///
/// On the wire that number is Moonbot's strategy FORMAT version, not an edit counter — every live
/// strategy of a core carries the same one (12 on every core seen so far), and a snapshot sent
/// with `0` comes back stamped with it. That echo then differs from the desired revision, so
/// moonproto's resolution reads it as `Superseded` — "a newer revision won" — and never compares
/// the fields; the panel shows "another change sent first overwrote this one" for a create the
/// core actually accepted, and a create whose fields the core replaced with defaults is reported
/// the same way. Sent with the stamp, the echo matches the revision and resolves to `Confirmed`
/// or `Adjusted` on the fields, which is the message the user can act on.
///
/// The maximum is taken so a core holding strategies written by a newer Moonbot is not sent one
/// it would have to migrate; a core with no strategies yet gets `0`, the old behaviour.
pub(super) fn revision_stamp(full: &[StrategySnapshot]) -> i32 {
    full.iter().map(|s| s.strategy_ver).max().unwrap_or(0)
}

/// Resolve a spec's placement anchor for the core it is actually being applied to.
///
/// THE one place a foreign anchor is dropped. Strategy ids are small per-core sequences, so an
/// id borrowed from another core almost certainly EXISTS here — it just belongs to an unrelated
/// strategy, and the copy would land silently beside that one instead of appending.
pub(super) fn anchor_on_core(insert_after: Option<(u64, u64)>, core: u64) -> Option<u64> {
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
pub(super) fn plan_insert_positions(ids: &[u64], anchors: &[Option<u64>]) -> Vec<usize> {
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
pub(super) fn strategy_placements_unchanged(
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
pub(in crate::feed::live) struct StrategyPlacementGuard {
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
    pub(in crate::feed::live) fn new() -> Self {
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
    pub(super) fn folder_base(
        &mut self,
        confirmed: Vec<String>,
        last_modified: i64,
    ) -> Vec<String> {
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
    pub(super) fn note_queued_folders(&mut self, paths: Vec<String>, base_modified: i64) {
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
    pub(super) fn note_queued_sync(
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
    pub(super) fn pending_order(&mut self, last_modified: u64) -> Option<&[u64]> {
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
    pub(super) fn retire_order(&mut self) {
        self.queued_order = None;
    }

    /// Return whether live and still-pending placement views both match the caller's snapshot.
    ///
    /// Once the live snapshot catches up exactly, the redundant queued shadow is discarded. A
    /// later external mutation is then checked solely against the new live snapshot.
    pub(super) fn allows_snapshot(
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
    pub(super) fn allows(&mut self, client: &MoonClient, expected: Vec<(u64, String)>) -> bool {
        self.allows_snapshot(snapshot_strategy_placements(client), expected)
    }
}

/// Clone only strategy ids and raw paths from MoonProto's current public snapshot.
pub(super) fn snapshot_strategy_placements(client: &MoonClient) -> Option<Vec<(u64, String)>> {
    Some(
        client
            .snapshot()?
            .strats()
            .snapshots()
            .map(|strategy| (strategy.strategy_id, strategy.path.to_string()))
            .collect(),
    )
}
