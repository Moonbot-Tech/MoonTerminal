//! Per-IP lane state and pending transitions.

use super::*;

/// One IP lane's serialized queue.
#[derive(Default)]
pub(super) struct Lane {
    pub(super) order: VecDeque<CoreId>,
    pub(super) active: Option<CoreId>,
    /// Set by a `NeverDropped` or `Timeout` failure. While set, the lane never pops its next
    /// entry -- see [`SessionManager::clear_stalled_lane`] for the only exit.
    ///
    /// `active` is deliberately left pointing at the core that caused the stall rather than
    /// cleared to `None`: it is what lets [`SessionManager::clear_stalled_lane`] find which lane a
    /// given failed core belongs to, and the pop condition already requires `!stalled`
    /// regardless of what `active` holds.
    pub(super) stalled: bool,
}

/// Metadata captured at enqueue time, needed later to close out the attempt's history row.
pub(super) struct AttemptMeta {
    /// When THIS attempt was enqueued, for the history record.
    pub(super) started_ms: i64,
    /// The core's display name at enqueue; see [`CoreUpdateRecord::core_name`].
    pub(super) core_name: String,
    /// The target requested at enqueue -- read back at pop time and moved into the `Sent` phase.
    pub(super) target: UpdateTarget,
    /// The core's `server_version` at enqueue, used as [`CoreUpdateRecord::from`] only for an
    /// attempt that is closed out before ever reaching `Sent` (a `Queued` core abandoned at
    /// quit). Every other closure captures a fresher baseline at the moment it actually matters.
    pub(super) from: Option<u32>,
    /// `CoreData::report_traces_epoch` at SEND time. That counter advances only on
    /// `FeedMsg::RunStateForgotten` -- a DIFFERENT MoonBot process answers the connection -- so a
    /// value past this snapshot is the one proof this attempt's core process actually restarted,
    /// as opposed to an ordinary network reconnect to the same process. See [`verified_outcome`].
    pub(super) restarts0: u64,
}

/// Per-IP update queue and its retained history, owned by [`SessionManager`].
#[derive(Default)]
pub(crate) struct CoreUpdateQueue {
    pub(super) lanes: HashMap<IpAddr, Lane>,
    pub(super) phases: HashMap<CoreId, CoreUpdatePhase>,
    pub(super) attempts: HashMap<CoreId, AttemptMeta>,
    pub(super) history: VecDeque<CoreUpdateRecord>,
    /// Cores whose `Verifying` phase requested a respawn, pending pickup by the coordinator's
    /// existing `reconnect_request` drain (`boot.rs`). This queue owns no `AppConfig`, which is
    /// precisely why the request must leave as data rather than as a direct call; see
    /// [`SessionManager::take_update_respawn_requests`].
    pub(super) respawn_requests: Vec<CoreId>,
    pub(super) rev: u64,
    pub(super) history_rev: u64,
    /// Highest `now_ms` this queue has ever ticked at, used to keep the clock this queue reads
    /// non-decreasing; see [`SessionManager::tick_core_updates`].
    pub(super) last_now_ms: i64,
}

impl CoreUpdateQueue {
    pub(super) fn bump_rev(&mut self) {
        self.rev = self.rev.wrapping_add(1);
    }

    /// Clamp `now_ms` to the highest value this queue has ever seen, and raise the floor to the
    /// result. The ONE non-decreasing sequence every timestamp this queue records is drawn from --
    /// see `last_now_ms` and every call site -- so a `started_ms` and an `ended_ms` can never be
    /// drawn from different clocks, whichever public entry point produced each of them.
    pub(super) fn clamped_now(&mut self, now_ms: i64) -> i64 {
        let v = now_ms.max(self.last_now_ms);
        self.last_now_ms = v;
        v
    }
}

/// One pending state-machine transition, computed from a read-only pass over `phases` and the
/// store, applied in a second pass. Split in two because computing a transition reads the store
/// while applying one mutates `core_updates`, and the borrow checker will not let a single pass
/// hold both a shared borrow of `self.store` and a mutable one of `self.core_updates` at once --
/// this also happens to make the "read everything, then decide" shape explicit.
pub(super) enum Transition {
    ToWaiting {
        target: UpdateTarget,
        from: Option<u32>,
        epoch0: u64,
        sent_at_ms: i64,
        left_at_ms: i64,
    },
    ToVerifying {
        target: UpdateTarget,
        from: Option<u32>,
        epoch1: u64,
        sent_at_ms: i64,
        left_at_ms: i64,
        verify_at_ms: i64,
    },
    Done {
        lane_addr: IpAddr,
        from: Option<u32>,
        outcome: CoreUpdateOutcome,
        /// Whether the lane should stall (`true`) or advance (`false`).
        stall: bool,
    },
}
