//! Per-IP serialized core-update queue, with a retained history.
//!
//! Cores are enqueued, at most ONE core per IP lane is ever in flight, different IPs proceed in
//! parallel, and each in-flight core is carried to a decided outcome by a state machine driven
//! entirely by an injected clock (`now_ms`) plus what the store already reports. Modeled on
//! `session/run_state.rs`: pure typed values plus one `impl SessionManager` block, so there is
//! exactly one owner of the rules.
//!
//! `moon-core` structurally cannot localize: every value here is typed so the UI can route it
//! through `t!` itself, never a `String` meant for display.
//!
//! The UI coordinator calls [`SessionManager::tick_core_updates`] from its regular tick with a
//! real clock; this module keeps the state machine deterministic by accepting that clock as an
//! argument alongside the store reads it needs.
//!
//! Lanes key on [`std::net::IpAddr`] directly. An earlier draft of this module had a
//! `UpdateLane::Unknown(CoreId)` variant for a core with no known address, but [`eligible`]
//! already rejects `endpoint.is_none()` -- so that variant could only ever hold the exact unsafe
//! state the eligibility gate exists to forbid: a private lane per IP-less core, letting two cores
//! on one physical machine update at the same time. A type that can only ever hold the forbidden
//! value should not exist, so there is no lane type at all here, only the address.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use super::SessionManager;
use super::store::CoreId;
use crate::feed::{ConnStatus, CoreStartupState, UpdateTarget};

/// How long a `Sent` command may sit without the core ever leaving `Ready`, before this gives up
/// and assumes nothing happened (`Failed(NeverDropped)`, which stalls the lane).
///
/// HYPOTHESIS: nobody has measured a real MoonBot update-and-restart yet. What would settle it:
/// one live campaign's `ended_ms - started_ms`, read from its resulting history record.
const SEND_TO_DROP_TIMEOUT_MS: i64 = 180_000;

/// How long a `Waiting` core may stay away before this gives up (`Failed(Timeout)`, which stalls
/// the lane). Measured from `sent_at_ms`, never from `left_at_ms` -- a core that takes its time
/// leaving `Ready` must not effectively earn extra time to come back.
///
/// Same hypothesis and check as [`SEND_TO_DROP_TIMEOUT_MS`].
const DROP_TO_READY_TIMEOUT_MS: i64 = 900_000;

/// Retained history ring size -- about ten full campaigns on a 200-core fleet.
const HISTORY_CAP: usize = 2_000;

/// Phase of one core's update attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreUpdatePhase {
    /// Waiting for its lane to free up. `held` mirrors the lane's `stalled` flag, so a row can
    /// draw itself without a second lookup.
    Queued {
        lane: IpAddr,
        held: bool,
        /// The instant this core was FIRST popped and found not Ready, or `None` if it has
        /// never failed that check. Distinct from `AttemptMeta::started_ms` (when this attempt
        /// was ENQUEUED): a fleet routinely holds a core queued far longer than
        /// [`SEND_TO_DROP_TIMEOUT_MS`] behind a busy or stalled lane, and measuring the bound
        /// from enqueue would defeat the not-Ready grace window for exactly the deep-queue case
        /// it exists for. Cleared back to `None` the moment the core is seen Ready at a pop, so
        /// two blips hours apart never accumulate toward one bound.
        not_ready_since: Option<i64>,
    },
    /// The update command was sent; the core has not yet been observed leaving `Ready`.
    Sent {
        target: UpdateTarget,
        /// Baseline build reported before this attempt, or `None` if the core had never reported
        /// one -- captured fresh at the moment this command was sent, never at enqueue: a core can
        /// sit queued for a while, and the send is the instant this value actually describes.
        from: Option<u32>,
        /// `CoreData::conn_epoch` read at the same moment as `from`, above. The completion
        /// predicate proves the core actually departed by comparing against THIS baseline, not
        /// against whatever epoch happened to be current when the core was merely enqueued -- a
        /// core can leave and return for reasons that have nothing to do with this update while
        /// queued, and a baseline taken then would misread that unrelated departure as the
        /// update's.
        epoch0: u64,
        sent_at_ms: i64,
    },
    /// The core has been observed leaving `Ready` (`conn_epoch` moved past `epoch0`) and has not
    /// yet been observed settled again.
    Waiting {
        target: UpdateTarget,
        from: Option<u32>,
        epoch0: u64,
        sent_at_ms: i64,
        left_at_ms: i64,
    },
    /// The attempt reached a terminal outcome. A core in this phase is eligible to be enqueued
    /// again; [`eligible`] treats `Done` as though nothing were tracked for it.
    Done(CoreUpdateOutcome),
}

/// How one update attempt ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoreUpdateOutcome {
    /// The core came back on a different build than it left on.
    Succeeded { from: Option<u32>, to: u32 },
    /// The core came back on the SAME build it left on. This is a success for the queue --
    /// nothing is in flight on that IP once the core is back -- but a NEUTRAL outcome for the row,
    /// never rendered as a failure. It is also a deliberate, recorded deviation from a literal
    /// reading of "never two simultaneous updates on one IP": the invariant bought is exactly
    /// that, and a core that provably departed and returned has nothing in flight, regardless of
    /// whether the build changed. Stalling here would kill every bulk campaign at its first
    /// already-current core, and a fleet-relative "behind" predicate ([`SessionManager::cores_behind`])
    /// cannot avoid selecting those.
    Unchanged { version: u32 },
    /// The attempt failed; see [`UpdateFailure`] for which way.
    Failed(UpdateFailure),
}

/// Why one update attempt failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateFailure {
    /// `send_core_cmd` returned `Err`: nothing was sent, so nothing is in flight on that IP. The
    /// lane advances.
    NotSent,
    /// The core never left `Ready` within [`SEND_TO_DROP_TIMEOUT_MS`] of the command being sent.
    /// This cannot prove the core did not start an update this simply failed to observe, and
    /// assuming otherwise is how two updates land on one IP -- so the lane STALLS.
    NeverDropped,
    /// The core was never sent anything: it was not `Ready` at pop time and stayed that way for
    /// at least [`SEND_TO_DROP_TIMEOUT_MS`], measured from ENQUEUE rather than from a send, since
    /// none was ever made. Distinct from [`Gone`](Self::Gone) (vanished from the configuration)
    /// and from [`NeverDropped`](Self::NeverDropped) (the command WAS sent and the core never
    /// departed) -- the audit log must not conflate three different stories. The lane STALLS.
    NotReady,
    /// The core left `Ready` and never came back settled within [`DROP_TO_READY_TIMEOUT_MS`] of
    /// the send. The lane STALLS.
    Timeout,
    /// The core's endpoint became unknown (`None`) by the time its turn came up, or the core
    /// vanished from configuration entirely while an update was in flight for it. The lane
    /// advances -- nothing is in flight for a core that no longer has an address.
    Gone,
    /// The application quit gracefully while this attempt was still in flight; see
    /// [`SessionManager::abandon_core_updates`].
    Abandoned,
}

/// One closed row of update history.
///
/// Kept independent of any live core or session: [`UpdateFailure::Gone`] is an explicitly
/// supported outcome, so a record can outlive the core it describes, and every field a later
/// reader needs is snapshotted here rather than resolved from current state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreUpdateRecord {
    #[serde(default)]
    pub core: CoreId,
    /// The core's configured display name, snapshotted at ENQUEUE and never re-resolved at read
    /// time: by the time a record is written the core may be gone from configuration (a
    /// `Failed(Gone)` row, or one written after the core was removed mid-campaign), and an audit
    /// row that cannot name its own core is not an audit row. This is user DATA carried through,
    /// not a UI string -- `moon-core` still produces no localized text here.
    #[serde(default)]
    pub core_name: String,
    /// Address of the lane this attempt ran on. Deliberately NOT an `Option`: a record only ever
    /// exists for a core that passed [`eligible`], and an eligible core has a known endpoint. Do
    /// not widen this back to `Option<IpAddr>` -- there is no code path that produces a record for
    /// a core without one.
    pub lane_addr: IpAddr,
    /// Baseline build the core reported before this attempt, independent of `outcome` -- a failed
    /// row still needs to say "from which version" it was failing to move. For an attempt that
    /// never reached `Sent`, this is the core's `server_version` at enqueue rather than at send,
    /// since there was no send to capture it at.
    #[serde(default)]
    pub from: Option<u32>,
    #[serde(default)]
    pub started_ms: i64,
    #[serde(default)]
    pub ended_ms: i64,
    pub target: UpdateTarget,
    pub outcome: CoreUpdateOutcome,
}

/// Typed result of a bulk enqueue, so the UI can word the outcome without re-deriving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UpdateEnqueueReport {
    pub accepted: usize,
    pub skipped_offline: usize,
    pub skipped_already: usize,
}

/// Fleet-wide totals over every tracked update attempt, for a footer that must be readable
/// without expanding a single server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UpdateFleetSummary {
    pub updating: usize,
    pub queued: usize,
    pub failed: usize,
    pub done: usize,
    pub lanes_stalled: usize,
}

/// One IP lane's serialized queue.
#[derive(Default)]
struct Lane {
    order: VecDeque<CoreId>,
    active: Option<CoreId>,
    /// Set by a `NeverDropped` or `Timeout` failure. While set, the lane never pops its next
    /// entry -- see [`SessionManager::clear_stalled_lane`] for the only exit.
    ///
    /// `active` is deliberately left pointing at the core that caused the stall rather than
    /// cleared to `None`: it is what lets [`SessionManager::clear_stalled_lane`] find which lane a
    /// given failed core belongs to, and the pop condition already requires `!stalled`
    /// regardless of what `active` holds.
    stalled: bool,
}

/// Metadata captured at enqueue time, needed later to close out the attempt's history row.
struct AttemptMeta {
    /// When THIS attempt was enqueued, for the history record.
    started_ms: i64,
    /// The core's display name at enqueue; see [`CoreUpdateRecord::core_name`].
    core_name: String,
    /// The target requested at enqueue -- read back at pop time and moved into the `Sent` phase.
    target: UpdateTarget,
    /// The core's `server_version` at enqueue, used as [`CoreUpdateRecord::from`] only for an
    /// attempt that is closed out before ever reaching `Sent` (a `Queued` core abandoned at
    /// quit). Every other closure captures a fresher baseline at the moment it actually matters.
    from: Option<u32>,
}

/// Per-IP update queue and its retained history, owned by [`SessionManager`].
#[derive(Default)]
pub(crate) struct CoreUpdateQueue {
    lanes: HashMap<IpAddr, Lane>,
    phases: HashMap<CoreId, CoreUpdatePhase>,
    attempts: HashMap<CoreId, AttemptMeta>,
    history: VecDeque<CoreUpdateRecord>,
    rev: u64,
    history_rev: u64,
    /// Highest `now_ms` this queue has ever ticked at, used to keep the clock this queue reads
    /// non-decreasing; see [`SessionManager::tick_core_updates`].
    last_now_ms: i64,
}

impl CoreUpdateQueue {
    fn bump_rev(&mut self) {
        self.rev = self.rev.wrapping_add(1);
    }

    /// Clamp `now_ms` to the highest value this queue has ever seen, and raise the floor to the
    /// result. The ONE non-decreasing sequence every timestamp this queue records is drawn from --
    /// see `last_now_ms` and every call site -- so a `started_ms` and an `ended_ms` can never be
    /// drawn from different clocks, whichever public entry point produced each of them.
    fn clamped_now(&mut self, now_ms: i64) -> i64 {
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
enum Transition {
    ToWaiting {
        target: UpdateTarget,
        from: Option<u32>,
        epoch0: u64,
        sent_at_ms: i64,
        left_at_ms: i64,
    },
    Done {
        lane_addr: IpAddr,
        from: Option<u32>,
        outcome: CoreUpdateOutcome,
        /// Whether the lane should stall (`true`) or advance (`false`).
        stall: bool,
    },
}

impl SessionManager {
    /// Enqueue one core for an update, pinning its lane to its CURRENT endpoint.
    ///
    /// Args:
    ///     core: Core to enqueue.
    ///     target: Release or a named build.
    ///     now_ms: Injected clock, recorded as the attempt's `started_ms`.
    ///
    /// Returns:
    ///     Whether the core was accepted. `false` means [`eligible`] rejected it or it is the
    ///     failed core that still owns a stalled lane and must be retried instead.
    pub fn enqueue_core_update(&mut self, core: CoreId, target: UpdateTarget, now_ms: i64) -> bool {
        let now_ms = self.core_updates.clamped_now(now_ms);
        let Some(addr) = self.eligible(core) else {
            return false;
        };
        // `eligible` treats a `Done` phase as idle, but a core whose OWN failure stalled its
        // lane is not simply idle: `active` still points at it (see `Lane::stalled`), and only
        // `clear_stalled_lane` may release that lane. An ordinary enqueue reaching this core
        // must skip it rather than silently overwriting the `Done` phase and stranding the lane
        // -- and every sibling still queued behind it -- with no way back. This check
        // deliberately lives here rather than inside `eligible`: `retry_core_update` calls
        // `eligible` BEFORE `clear_stalled_lane`, and at that moment this core IS its lane's
        // stalled `active` by
        // construction -- putting the same check in `eligible` would make every retry attempt
        // fail eligibility and never reach the clear. Placed here instead, retry is unaffected:
        // it reaches this function only AFTER `clear_stalled_lane` has already run.
        if self
            .core_updates
            .lanes
            .get(&addr)
            .is_some_and(|lane| lane.stalled && lane.active == Some(core))
        {
            return false;
        }
        let core_name = self
            .sessions
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let from = self.store.core(core).and_then(|d| d.server_version);
        let held = self
            .core_updates
            .lanes
            .get(&addr)
            .is_some_and(|lane| lane.stalled);
        self.core_updates.phases.insert(
            core,
            CoreUpdatePhase::Queued {
                lane: addr,
                held,
                not_ready_since: None,
            },
        );
        self.core_updates.attempts.insert(
            core,
            AttemptMeta {
                started_ms: now_ms,
                core_name,
                target,
                from,
            },
        );
        self.core_updates
            .lanes
            .entry(addr)
            .or_default()
            .order
            .push_back(core);
        self.core_updates.bump_rev();
        true
    }

    /// Enqueue many cores at once, filling the same queue a single enqueue would -- bulk actions
    /// never send commands directly.
    ///
    /// Args:
    ///     cores: Cores to enqueue.
    ///     target: Release or a named build, applied to every core in the batch.
    ///     now_ms: Injected clock.
    ///
    /// Returns:
    ///     Typed counts so the UI can word the outcome ("47 queued, 3 already updating").
    pub fn enqueue_core_updates(
        &mut self,
        cores: &[CoreId],
        target: UpdateTarget,
        now_ms: i64,
    ) -> UpdateEnqueueReport {
        let now_ms = self.core_updates.clamped_now(now_ms);
        let mut report = UpdateEnqueueReport::default();
        for &core in cores {
            if self.is_in_flight(core) {
                report.skipped_already += 1;
                continue;
            }
            if self.enqueue_core_update(core, target.clone(), now_ms) {
                report.accepted += 1;
            } else {
                report.skipped_offline += 1;
            }
        }
        report
    }

    /// Run one pass of the whole state machine: reconcile cores that vanished from configuration,
    /// advance every in-flight core, pop whatever lane is free, then refresh the `held` flag on
    /// every queued entry.
    ///
    /// Args:
    ///     now_ms: Injected clock. The only time source this queue ever reads.
    ///
    /// Returns:
    ///     Whether any phase, lane, or history entry changed -- a caller uses this to skip a
    ///     pointless repaint.
    pub fn tick_core_updates(&mut self, now_ms: i64) -> bool {
        // Clamp to non-decreasing: `now_ms` is `SystemTime`-backed and can step backward on a
        // manual clock change or a stepped NTP correction. A forward jump fails safe into a
        // visible stall (the existing timeouts fire early); a backward jump fails open into an
        // invisible wedge (a timeout difference that goes negative never reaches its threshold),
        // so only the backward direction needs clamping here. Shared with every other public
        // entry point that stamps a timestamp into a tracked record (`clamped_now`), precisely so
        // a `started_ms` and an `ended_ms` can never be drawn from different clocks.
        let now_ms = self.core_updates.clamped_now(now_ms);
        let mut changed = false;
        changed |= self.reconcile_vanished_updates(now_ms);
        changed |= self.advance_in_flight_updates(now_ms);
        changed |= self.pop_ready_lanes(now_ms);
        changed |= self.refresh_held_flags();
        if changed {
            self.core_updates.bump_rev();
        }
        changed
    }

    /// Read one core's current update phase, or `None` if it has never been enqueued (or its
    /// tracked state was reconciled away after vanishing while merely `Queued`).
    pub fn core_update_phase(&self, core: CoreId) -> Option<&CoreUpdatePhase> {
        self.core_updates.phases.get(&core)
    }

    /// Change token covering every phase, lane, and pop this queue has made. A cached surface
    /// compares this instead of re-deriving and diffing every row's state itself.
    pub fn core_update_rev(&self) -> u64 {
        self.core_updates.rev
    }

    /// Fold every tracked core into fleet-wide totals.
    pub fn core_update_summary(&self) -> UpdateFleetSummary {
        let mut summary = UpdateFleetSummary::default();
        for phase in self.core_updates.phases.values() {
            match phase {
                CoreUpdatePhase::Queued { .. } => summary.queued += 1,
                CoreUpdatePhase::Sent { .. } | CoreUpdatePhase::Waiting { .. } => {
                    summary.updating += 1;
                }
                CoreUpdatePhase::Done(CoreUpdateOutcome::Failed(_)) => summary.failed += 1,
                CoreUpdatePhase::Done(_) => summary.done += 1,
            }
        }
        summary.lanes_stalled = self
            .core_updates
            .lanes
            .values()
            .filter(|l| l.stalled)
            .count();
        summary
    }

    /// Every core whose reported build is older than the newest build anywhere in the fleet.
    ///
    /// Computes [`Self::fleet_newest_version`] internally rather than taking a caller-supplied
    /// basis: an earlier draft took `newest` as a parameter, which let a caller hand this
    /// selection a different comparison than the one the panel prints, defeating the whole point
    /// of centralizing the calculation. No caller may select on any basis but this one.
    pub fn cores_behind(&self) -> Vec<CoreId> {
        let Some(newest) = self.fleet_newest_version() else {
            return Vec::new();
        };
        self.store
            .cores()
            .filter_map(|(id, core)| core.server_version.map(|v| (id, v)))
            .filter(|(_, v)| *v < newest)
            .map(|(id, _)| id)
            .collect()
    }

    /// The newest MoonBot build reported anywhere in the fleet, from the STORE and never from a
    /// scoped row set.
    ///
    /// A panel scoped to a subset of the fleet must not compute a lower maximum and silently flag
    /// nothing, and two Core Status panels with different scopes must not disagree about which
    /// cores are stale -- so this is the one comparison basis, computed once, that both the queue
    /// and every panel read. The terminal has no release feed and never learns what "latest"
    /// actually is; this is only the newest build MoonBot itself has reported running anywhere.
    pub fn fleet_newest_version(&self) -> Option<u32> {
        self.store
            .cores()
            .filter_map(|(_, core)| core.server_version)
            .max()
    }

    /// Clear the stall flag on the lane this core belongs to, releasing its held siblings.
    ///
    /// Args:
    ///     core: The core whose failure stalled a lane -- found by locating the lane whose
    ///         `active` still points at it, since a stall deliberately never clears `active`.
    ///
    /// Returns:
    ///     Whether that lane was actually stalled. `false` is a legal, silent no-op: a caller may
    ///     offer this control from a row the queue has already moved past.
    pub fn clear_stalled_lane(&mut self, core: CoreId) -> bool {
        let Some(addr) = self
            .core_updates
            .lanes
            .iter()
            .find(|(_, lane)| lane.stalled && lane.active == Some(core))
            .map(|(addr, _)| *addr)
        else {
            return false;
        };
        if let Some(lane) = self.core_updates.lanes.get_mut(&addr) {
            lane.stalled = false;
            lane.active = None;
        }
        self.refresh_held_for_lane(addr);
        self.core_updates.bump_rev();
        true
    }

    /// Drop this core's `Done` phase and enqueue it again, clearing its lane's stall if this core
    /// is the one that set it.
    ///
    /// Args:
    ///     core: Core to retry. Must currently be in a `Done` phase.
    ///     target: Release or a named build for the new attempt.
    ///     now_ms: Injected clock.
    ///
    /// Returns:
    ///     Whether re-enqueueing succeeded. `false` means the core is not in a `Done` phase or
    ///     [`eligible`] rejects it now; in either case its existing phase is left untouched.
    pub fn retry_core_update(&mut self, core: CoreId, target: UpdateTarget, now_ms: i64) -> bool {
        let now_ms = self.core_updates.clamped_now(now_ms);
        if !matches!(
            self.core_updates.phases.get(&core),
            Some(CoreUpdatePhase::Done(_))
        ) {
            return false;
        }
        // Decide eligibility BEFORE touching the lane: clearing the stall first and then finding
        // the core ineligible would release a sibling on this IP even though the stall existed
        // precisely because the previous attempt could not be proven finished.
        if self.eligible(core).is_none() {
            return false;
        }
        self.clear_stalled_lane(core);
        self.enqueue_core_update(core, target, now_ms)
    }

    /// The target retained for this core's current or most recently completed attempt.
    ///
    /// Args:
    ///     core: Core whose retained update target to read.
    ///
    /// Returns:
    ///     A cloned target while the queue still retains metadata for the core.
    pub fn last_update_target(&self, core: CoreId) -> Option<UpdateTarget> {
        self.core_updates
            .attempts
            .get(&core)
            .map(|meta| meta.target.clone())
    }

    /// Non-draining borrow of the retained history. `SessionManager` is the single owner; reading
    /// never removes a record. Persistence hooks read this and decide whether to save from
    /// [`Self::core_update_history_rev`].
    pub fn core_update_history(&self) -> &VecDeque<CoreUpdateRecord> {
        &self.core_updates.history
    }

    /// Change token that advances on every appended history record.
    pub fn core_update_history_rev(&self) -> u64 {
        self.core_updates.history_rev
    }

    /// Seed the retained history from persisted storage at boot.
    ///
    /// Called exactly ONCE, before any campaign runs, into a queue whose history is empty by
    /// construction -- nothing else ever writes a record this call did not itself later append,
    /// so there is no duplicate to guard against here and no dedup key to invent. If a future
    /// caller ever needs to call this more than once, that is a new invariant to design for, not
    /// a reason to silently make this idempotent now. Deliberately does not bump
    /// [`Self::core_update_history_rev`]: a persistence layer that just loaded this data must not
    /// immediately think it needs to save it back.
    pub fn seed_core_update_history(&mut self, records: Vec<CoreUpdateRecord>) {
        let mut history: VecDeque<CoreUpdateRecord> = records.into();
        // Same trim `finish_core` applies on every push: keep the last `HISTORY_CAP` entries
        // (newest kept) rather than letting a persisted file longer than the cap stay over it.
        while history.len() > HISTORY_CAP {
            history.pop_front();
        }
        self.core_updates.history = history;
    }

    /// Close out every non-`Done` attempt as `Failed(Abandoned)`, for a graceful quit.
    ///
    /// KNOWN LIMIT, stated here because this is the guarantee's only owner: this covers a
    /// graceful quit alone. `on_app_quit` is skipped by `std::process::exit`
    /// (`crates/moon-ui-gpui/src/startup/boot.rs:432-436`, and the static contract at
    /// `crates/moon-ui-gpui/tests/theme_contract/windowing.rs:539-555` repeats it), so a crash, a
    /// forced termination, or a power loss drops the in-flight queue with no `Abandoned` record --
    /// the honest account after such an exit is a history that simply stops. No per-transition
    /// durable marker closes this gap: the cost would be a disk write on every transition for a
    /// bounded, minutes-long campaign, which is not worth paying for a crash-only edge case.
    ///
    /// Args:
    ///     now_ms: Injected clock, recorded as `ended_ms` on every abandoned record.
    ///
    /// Returns:
    ///     How many attempts were abandoned.
    pub fn abandon_core_updates(&mut self, now_ms: i64) -> usize {
        let now_ms = self.core_updates.clamped_now(now_ms);
        let pending: Vec<CoreId> = self
            .core_updates
            .phases
            .iter()
            .filter(|(_, phase)| !matches!(phase, CoreUpdatePhase::Done(_)))
            .map(|(core, _)| *core)
            .collect();
        for &core in &pending {
            let phase = self.core_updates.phases.get(&core);
            let was_queued = matches!(phase, Some(CoreUpdatePhase::Queued { .. }));
            let from = match phase {
                Some(
                    CoreUpdatePhase::Sent { from, .. } | CoreUpdatePhase::Waiting { from, .. },
                ) => *from,
                _ => self.core_updates.attempts.get(&core).and_then(|m| m.from),
            };
            let Some(lane_addr) = self.current_lane(core) else {
                // Invariant violation: a tracked, non-`Done` phase with no lane this core could be
                // resolved from. Log and skip rather than losing the core silently or panicking
                // the whole terminal over a quit-path bookkeeping bug.
                log::error!(
                    "core {core}: abandon skipped, no lane resolvable for its update phase"
                );
                continue;
            };
            self.finish_core(
                core,
                lane_addr,
                from,
                CoreUpdateOutcome::Failed(UpdateFailure::Abandoned),
                now_ms,
            );
            // Mirror every sibling closure path (`reconcile_vanished_updates`,
            // `advance_in_flight_updates`): an abandoned lane must not keep pointing at a core
            // that just closed out, or it can never pop again and can never be cleared.
            if let Some(lane) = self.core_updates.lanes.get_mut(&lane_addr) {
                if was_queued {
                    lane.order.retain(|id| *id != core);
                } else {
                    lane.active = None;
                }
            }
        }
        if !pending.is_empty() {
            self.core_updates.bump_rev();
        }
        pending.len()
    }

    /// Eligibility gate, shared by every enqueue path.
    ///
    /// Rejects: no live session for this core; `status != Ready`; no baseline build
    /// (`server_version.is_none()`); no address to serialize on (`endpoint.is_none()`); or the
    /// core is already tracked in a non-`Done` phase. The last two reasons are the same reason
    /// twice: a core that has never fully come up has neither a build to move from nor an address
    /// to serialize on.
    ///
    /// Args:
    ///     core: Core to check.
    ///
    /// Returns:
    ///     The address to pin its lane to, or `None` if any rejection applies.
    fn eligible(&self, core: CoreId) -> Option<IpAddr> {
        if !self.sessions.iter().any(|s| s.id == core) {
            return None;
        }
        let data = self.store.core(core)?;
        if data.status != ConnStatus::Ready {
            return None;
        }
        data.server_version?;
        let endpoint = data.endpoint?;
        if self.is_in_flight(core) {
            return None;
        }
        Some(endpoint.address)
    }

    /// Whether `core` has a live (non-`Done`) update attempt tracked. The single definition of
    /// "this core has a live update attempt", shared by every caller that must not double-track
    /// a core.
    fn is_in_flight(&self, core: CoreId) -> bool {
        matches!(
            self.core_updates.phases.get(&core),
            Some(phase) if !matches!(phase, CoreUpdatePhase::Done(_))
        )
    }

    /// Resolve which lane a tracked core currently belongs to.
    ///
    /// A `Queued` phase carries its lane directly; a `Sent` or `Waiting` phase does not, so this
    /// searches for the lane whose `active` still points at the core -- which a stall deliberately
    /// leaves in place, see [`Lane::stalled`].
    fn current_lane(&self, core: CoreId) -> Option<IpAddr> {
        match self.core_updates.phases.get(&core) {
            Some(CoreUpdatePhase::Queued { lane, .. }) => Some(*lane),
            Some(CoreUpdatePhase::Sent { .. } | CoreUpdatePhase::Waiting { .. }) => self
                .core_updates
                .lanes
                .iter()
                .find(|(_, lane)| lane.active == Some(core))
                .map(|(addr, _)| *addr),
            _ => None,
        }
    }

    /// Resolve a core's lane, logging the shared "no lane resolvable" error under `phase_name`
    /// when it cannot be found. Callers pattern-match the `None` to `continue` past the core.
    fn lane_or_skip(&self, core: CoreId, phase_name: &str) -> Option<IpAddr> {
        let lane = self.current_lane(core);
        if lane.is_none() {
            log::error!(
                "core {core}: no lane resolvable for its {phase_name} update phase, skipping"
            );
        }
        lane
    }

    /// Close one core's in-flight attempt: append a history record (when an attempt is on
    /// record) and set its phase to `Done(outcome)`.
    ///
    /// Args:
    ///     core: Core whose attempt is closing.
    ///     lane_addr: Address of the lane this attempt ran on -- recorded even for `Failed(Gone)`,
    ///         since a history row must still say which server was affected.
    ///     from: Baseline build the core reported before this attempt; see
    ///         [`CoreUpdateRecord::from`].
    ///     outcome: How the attempt ended.
    ///     now_ms: Injected clock, used as `ended_ms`.
    fn finish_core(
        &mut self,
        core: CoreId,
        lane_addr: IpAddr,
        from: Option<u32>,
        outcome: CoreUpdateOutcome,
        now_ms: i64,
    ) {
        // Deliberately retained: a failed core keeps its `Done` phase and its retry
        // affordance indefinitely, so the attempt's TARGET must stay reachable for at least as
        // long as that -- see `last_update_target`, the one reader. Already overwritten by the
        // next `enqueue_core_update` for this core and already dropped when the core vanishes
        // (`reconcile_vanished_updates` clears both maps), so this costs at most one small entry
        // per core that has ever been updated, bounded by the fleet.
        if let Some(meta) = self.core_updates.attempts.get(&core) {
            let record = CoreUpdateRecord {
                core,
                core_name: meta.core_name.clone(),
                lane_addr,
                from,
                started_ms: meta.started_ms,
                ended_ms: now_ms,
                target: meta.target.clone(),
                outcome: outcome.clone(),
            };
            self.core_updates.history.push_back(record);
            while self.core_updates.history.len() > HISTORY_CAP {
                self.core_updates.history.pop_front();
            }
            self.core_updates.history_rev = self.core_updates.history_rev.wrapping_add(1);
        }
        self.core_updates
            .phases
            .insert(core, CoreUpdatePhase::Done(outcome));
    }

    /// Step 1 of [`Self::tick_core_updates`]: reconcile cores whose session no longer exists
    /// (removed from configuration), rather than in `lifecycle::drop_core` -- one owner of the
    /// rule, so `drop_core` never grows a dependency on this queue.
    fn reconcile_vanished_updates(&mut self, now_ms: i64) -> bool {
        let vanished_active: Vec<(CoreId, IpAddr, Option<u32>)> = self
            .core_updates
            .phases
            .iter()
            .filter_map(|(&core, phase)| {
                if self.sessions.iter().any(|s| s.id == core) {
                    return None;
                }
                match phase {
                    CoreUpdatePhase::Sent { from, .. } | CoreUpdatePhase::Waiting { from, .. } => {
                        self.current_lane(core).map(|addr| (core, addr, *from))
                    }
                    _ => None,
                }
            })
            .collect();
        let vanished_queued: Vec<(CoreId, IpAddr)> = self
            .core_updates
            .phases
            .iter()
            .filter_map(|(&core, phase)| {
                if self.sessions.iter().any(|s| s.id == core) {
                    return None;
                }
                match phase {
                    CoreUpdatePhase::Queued { lane, .. } => Some((core, *lane)),
                    _ => None,
                }
            })
            .collect();
        // A core removed from configuration keeps its terminal `Done` phase around forever
        // otherwise -- `CoreId`s are never reused, so every remove-then-re-add cycle over a long
        // session would leave one more dead entry. No history or lane side effects are owed: the
        // record is already written and the lane was already settled when the phase became
        // `Done`.
        let vanished_done: Vec<CoreId> = self
            .core_updates
            .phases
            .iter()
            .filter_map(|(&core, phase)| {
                if self.sessions.iter().any(|s| s.id == core) {
                    return None;
                }
                matches!(phase, CoreUpdatePhase::Done(_)).then_some(core)
            })
            .collect();

        let changed =
            !vanished_active.is_empty() || !vanished_queued.is_empty() || !vanished_done.is_empty();

        for (core, lane_addr, from) in vanished_active {
            self.finish_core(
                core,
                lane_addr,
                from,
                CoreUpdateOutcome::Failed(UpdateFailure::Gone),
                now_ms,
            );
            if let Some(lane) = self.core_updates.lanes.get_mut(&lane_addr) {
                lane.active = None;
            }
        }
        // Merely `Queued` entries are dropped silently, with NO history record: they never
        // started anything, so there is nothing to audit.
        for (core, lane_addr) in vanished_queued {
            self.core_updates.phases.remove(&core);
            self.core_updates.attempts.remove(&core);
            if let Some(lane) = self.core_updates.lanes.get_mut(&lane_addr) {
                lane.order.retain(|id| *id != core);
            }
        }
        for core in vanished_done {
            self.core_updates.phases.remove(&core);
            self.core_updates.attempts.remove(&core);
        }
        changed
    }

    /// Step 2 of [`Self::tick_core_updates`]: advance every `Sent` or `Waiting` core, per the
    /// transition table.
    fn advance_in_flight_updates(&mut self, now_ms: i64) -> bool {
        let mut transitions: Vec<(CoreId, Transition)> = Vec::new();
        for (&core, phase) in &self.core_updates.phases {
            match phase {
                CoreUpdatePhase::Sent {
                    target,
                    from,
                    epoch0,
                    sent_at_ms,
                } => {
                    let Some(data) = self.store.core(core) else {
                        // Reconciled on the next tick's step 1; skip for now rather than acting on
                        // stale data.
                        continue;
                    };
                    if data.conn_epoch > *epoch0 {
                        transitions.push((
                            core,
                            Transition::ToWaiting {
                                target: target.clone(),
                                from: *from,
                                epoch0: *epoch0,
                                sent_at_ms: *sent_at_ms,
                                left_at_ms: now_ms,
                            },
                        ));
                    } else if now_ms - sent_at_ms >= SEND_TO_DROP_TIMEOUT_MS {
                        let Some(lane_addr) = self.lane_or_skip(core, "Sent") else {
                            continue;
                        };
                        transitions.push((
                            core,
                            Transition::Done {
                                lane_addr,
                                from: *from,
                                outcome: CoreUpdateOutcome::Failed(UpdateFailure::NeverDropped),
                                stall: true,
                            },
                        ));
                    }
                }
                CoreUpdatePhase::Waiting {
                    from, sent_at_ms, ..
                } => {
                    let Some(data) = self.store.core(core) else {
                        continue;
                    };
                    // Gated on the SNAPSHOT `CoreStartupStatus.state`, never on a lifecycle event:
                    // MoonProto's per-core Ready/Connected events do not arrive in a fixed order,
                    // and `startup` is observed through `startup_rev`, which advances only on
                    // changed PROGRESS. This is the single place this feature could hang forever.
                    let settled = data.status == ConnStatus::Ready
                        && data.startup.state == CoreStartupState::Ready
                        && data.server_version.is_some();
                    if settled {
                        let v = data.server_version.expect("settled implies Some above");
                        let outcome = if Some(v) != *from {
                            CoreUpdateOutcome::Succeeded { from: *from, to: v }
                        } else {
                            CoreUpdateOutcome::Unchanged { version: v }
                        };
                        let Some(lane_addr) = self.lane_or_skip(core, "Waiting") else {
                            continue;
                        };
                        transitions.push((
                            core,
                            Transition::Done {
                                lane_addr,
                                from: *from,
                                outcome,
                                stall: false,
                            },
                        ));
                    } else if now_ms - sent_at_ms >= DROP_TO_READY_TIMEOUT_MS {
                        let Some(lane_addr) = self.lane_or_skip(core, "Waiting") else {
                            continue;
                        };
                        transitions.push((
                            core,
                            Transition::Done {
                                lane_addr,
                                from: *from,
                                outcome: CoreUpdateOutcome::Failed(UpdateFailure::Timeout),
                                stall: true,
                            },
                        ));
                    }
                }
                _ => {}
            }
        }

        let changed = !transitions.is_empty();
        for (core, transition) in transitions {
            match transition {
                Transition::ToWaiting {
                    target,
                    from,
                    epoch0,
                    sent_at_ms,
                    left_at_ms,
                } => {
                    self.core_updates.phases.insert(
                        core,
                        CoreUpdatePhase::Waiting {
                            target,
                            from,
                            epoch0,
                            sent_at_ms,
                            left_at_ms,
                        },
                    );
                }
                Transition::Done {
                    lane_addr,
                    from,
                    outcome,
                    stall,
                } => {
                    self.finish_core(core, lane_addr, from, outcome, now_ms);
                    if let Some(lane) = self.core_updates.lanes.get_mut(&lane_addr) {
                        if stall {
                            lane.stalled = true;
                        } else {
                            lane.active = None;
                        }
                    }
                }
            }
        }
        changed
    }

    /// Step 3 of [`Self::tick_core_updates`]: pop the front of every lane that is free, not
    /// stalled, and non-empty.
    ///
    /// Revalidates the endpoint at pop time (not only at enqueue): a core pinned to lane `addr`
    /// can have moved to a different address, or lost its address entirely, while it sat queued --
    /// `reconcile` respawns the session whenever the connection signature changes, clearing the
    /// old endpoint before the replacement feed publishes a new one, so the window between those
    /// two events is real, not theoretical.
    fn pop_ready_lanes(&mut self, now_ms: i64) -> bool {
        let ready_lanes: Vec<IpAddr> = self
            .core_updates
            .lanes
            .iter()
            .filter(|(_, lane)| lane.active.is_none() && !lane.stalled && !lane.order.is_empty())
            .map(|(addr, _)| *addr)
            .collect();

        let mut changed = false;
        for addr in ready_lanes {
            let Some(core) = self
                .core_updates
                .lanes
                .get_mut(&addr)
                .and_then(|lane| lane.order.pop_front())
            else {
                continue;
            };
            changed = true;

            let Some(data) = self.store.core(core) else {
                // Reconciled on the next tick's step 1. Drop the phase and attempt now rather than
                // leaving a ghost entry that points at a lane it is no longer queued in.
                log::error!("core {core}: popped with no live store entry, dropping");
                self.core_updates.phases.remove(&core);
                self.core_updates.attempts.remove(&core);
                continue;
            };
            let current_endpoint = data.endpoint;
            let from = data.server_version;
            let epoch0 = data.conn_epoch;

            match current_endpoint {
                Some(ep) if ep.address == addr => {
                    if data.status != ConnStatus::Ready {
                        // The core dropped out of Ready while it sat queued behind a same-IP
                        // sibling. Its endpoint survives an ordinary disconnect, so "the address
                        // still matches" is not evidence it is reachable -- sending here would
                        // land in the long-lived command channel and replay much later against a
                        // completely different attempt. Re-queue at the front; a later tick picks
                        // it up once it is back -- but only while it is still within the bound: an
                        // unbounded re-queue here would pop, check and push back the same core
                        // forever, invisibly, without ever setting `stalled` the way `NeverDropped`
                        // / `Timeout` do for the phases that follow this one.
                        //
                        // Measured from `not_ready_since` -- the first time THIS attempt failed a
                        // pop-time Ready check -- never from `AttemptMeta::started_ms` (when it was
                        // ENQUEUED). A fleet routinely holds a core queued behind a busy or stalled
                        // lane far longer than `SEND_TO_DROP_TIMEOUT_MS`, and the first pop of such
                        // a core -- an ordinary reconnect blip, exactly what this check exists to
                        // absorb -- would already be past an enqueue-measured bound, defeating the
                        // grace window in the deep-queue case it exists for.
                        let not_ready_since = match self.core_updates.phases.get(&core) {
                            Some(CoreUpdatePhase::Queued {
                                not_ready_since, ..
                            }) => *not_ready_since,
                            _ => None,
                        };
                        let stalled_out = matches!(
                            not_ready_since,
                            Some(since) if now_ms - since >= SEND_TO_DROP_TIMEOUT_MS
                        );
                        if stalled_out {
                            self.finish_core(
                                core,
                                addr,
                                from,
                                CoreUpdateOutcome::Failed(UpdateFailure::NotReady),
                                now_ms,
                            );
                            if let Some(lane) = self.core_updates.lanes.get_mut(&addr) {
                                // `active` is set here (it was `None`, this lane was ready to
                                // pop), mirroring every other stall: `clear_stalled_lane` finds a
                                // stalled lane by `active == Some(core)`, and this core is the one
                                // this stall is about.
                                lane.active = Some(core);
                                lane.stalled = true;
                            }
                            log::warn!(
                                "core {core}: never became Ready at pop time within the bound on {addr}, stalling lane"
                            );
                            continue;
                        }
                        if let Some(lane) = self.core_updates.lanes.get_mut(&addr) {
                            lane.order.push_front(core);
                        }
                        let held = self
                            .core_updates
                            .lanes
                            .get(&addr)
                            .is_some_and(|lane| lane.stalled);
                        self.core_updates.phases.insert(
                            core,
                            CoreUpdatePhase::Queued {
                                lane: addr,
                                held,
                                not_ready_since: Some(not_ready_since.unwrap_or(now_ms)),
                            },
                        );
                        log::info!(
                            "core update: core {core} not Ready at pop time on {addr}, re-queued"
                        );
                        continue;
                    }
                    // Closes the one remaining hole in the one-per-IP invariant: nothing
                    // revalidates the endpoint for an ACTIVE core the way the branch above does
                    // for a queued one. A core mid-flight whose config moved its address onto
                    // `addr` keeps its attempt anchored to the lane it left, so `addr` never gets
                    // marked busy for it -- letting a second core popped on `addr` run
                    // concurrently with it. At most a handful of attempts are ever in flight, so
                    // this scan is free at fleet scale.
                    let other_in_flight_here =
                        self.core_updates.phases.iter().any(|(&other, phase)| {
                            other != core
                                && matches!(
                                    phase,
                                    CoreUpdatePhase::Sent { .. } | CoreUpdatePhase::Waiting { .. }
                                )
                                && self
                                    .store
                                    .core(other)
                                    .and_then(|d| d.endpoint)
                                    .is_some_and(|other_ep| other_ep.address == addr)
                        });
                    if other_in_flight_here {
                        if let Some(lane) = self.core_updates.lanes.get_mut(&addr) {
                            lane.order.push_front(core);
                        }
                        // This core WAS just observed Ready at this pop, even though it is
                        // deferred here for a lane conflict rather than sent -- clear any
                        // not-Ready streak so a later, unrelated blip gets its own fresh grace
                        // window instead of inheriting this one's age.
                        let held = self
                            .core_updates
                            .lanes
                            .get(&addr)
                            .is_some_and(|lane| lane.stalled);
                        self.core_updates.phases.insert(
                            core,
                            CoreUpdatePhase::Queued {
                                lane: addr,
                                held,
                                not_ready_since: None,
                            },
                        );
                        log::info!(
                            "core update: core {core} deferred, another attempt is already live on {addr}"
                        );
                        continue;
                    }
                    let Some(target) = self
                        .core_updates
                        .attempts
                        .get(&core)
                        .map(|meta| meta.target.clone())
                    else {
                        log::error!("core {core}: popped with no recorded update target, dropping");
                        self.core_updates.phases.remove(&core);
                        continue;
                    };
                    match self.update_core_version(core, target.clone()) {
                        Ok(()) => {
                            if let Some(lane) = self.core_updates.lanes.get_mut(&addr) {
                                lane.active = Some(core);
                            }
                            self.core_updates.phases.insert(
                                core,
                                CoreUpdatePhase::Sent {
                                    target,
                                    from,
                                    epoch0,
                                    sent_at_ms: now_ms,
                                },
                            );
                        }
                        Err(_err) => {
                            log::warn!(
                                "core {core}: update command not sent (core unreachable or command channel closed)"
                            );
                            self.finish_core(
                                core,
                                addr,
                                from,
                                CoreUpdateOutcome::Failed(UpdateFailure::NotSent),
                                now_ms,
                            );
                        }
                    }
                }
                Some(ep) => {
                    // The endpoint moved: re-bucket to the FRONT of the new address's lane, at
                    // its original ordering priority, and leave it `Queued`. The normal pop rule
                    // picks it up once that lane is free -- never send here, or two updates could
                    // run concurrently on the address it just left or the one it just joined.
                    let new_addr = ep.address;
                    // Carried over, not cleared and not reset to now: a lane move is neither a
                    // Ready nor a not-Ready observation for this attempt, so it must not touch
                    // the not-Ready grace window either way.
                    let not_ready_since = match self.core_updates.phases.get(&core) {
                        Some(CoreUpdatePhase::Queued {
                            not_ready_since, ..
                        }) => *not_ready_since,
                        _ => None,
                    };
                    self.core_updates
                        .lanes
                        .entry(new_addr)
                        .or_default()
                        .order
                        .push_front(core);
                    let held = self
                        .core_updates
                        .lanes
                        .get(&new_addr)
                        .is_some_and(|lane| lane.stalled);
                    self.core_updates.phases.insert(
                        core,
                        CoreUpdatePhase::Queued {
                            lane: new_addr,
                            held,
                            not_ready_since,
                        },
                    );
                    log::info!(
                        "core update: core {core} lane moved {addr} -> {new_addr}, re-queued"
                    );
                }
                None => {
                    // No address at all: not eligible, nothing sent, nothing in flight on the old
                    // lane.
                    self.finish_core(
                        core,
                        addr,
                        from,
                        CoreUpdateOutcome::Failed(UpdateFailure::Gone),
                        now_ms,
                    );
                }
            }
        }
        changed
    }

    /// Step 4 of [`Self::tick_core_updates`]: recompute `held` on every `Queued` entry from its
    /// lane's `stalled` flag.
    fn refresh_held_flags(&mut self) -> bool {
        let addrs: Vec<IpAddr> = self.core_updates.lanes.keys().copied().collect();
        let mut changed = false;
        for addr in addrs {
            changed |= self.refresh_held_for_lane(addr);
        }
        changed
    }

    /// Recompute `held` on every `Queued` entry in one lane, and report whether any changed.
    fn refresh_held_for_lane(&mut self, addr: IpAddr) -> bool {
        let Some(stalled) = self.core_updates.lanes.get(&addr).map(|lane| lane.stalled) else {
            return false;
        };
        let cores: Vec<CoreId> = self
            .core_updates
            .lanes
            .get(&addr)
            .map(|lane| lane.order.iter().copied().collect())
            .unwrap_or_default();
        let mut changed = false;
        for core in cores {
            if let Some(CoreUpdatePhase::Queued { held, .. }) =
                self.core_updates.phases.get_mut(&core)
            {
                if *held != stalled {
                    *held = stalled;
                    changed = true;
                }
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests;
