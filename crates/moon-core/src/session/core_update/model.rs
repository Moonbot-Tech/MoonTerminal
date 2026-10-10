//! Update phases, outcomes, history rows, and timeout budgets.

use super::*;

/// How long a `Sent` command may sit without the core ever leaving `Ready`, before this gives up
/// and assumes nothing happened (`Failed(NeverDropped)`, which stalls the lane).
///
/// MEASURED 2026-08-31 on the author's own fleet: two cores completed a full MoonBot
/// update-and-restart in 24 s and 28 s, send to settled. Both bounds stay where they are: they
/// are outage budgets, not expectations, and 180 s / 900 s against a ~26 s reality is deliberate
/// headroom for a slow link or a large download. The figure is recorded so nobody re-opens this
/// as an unknown; it is not a reason to tighten either number.
pub(super) const SEND_TO_DROP_TIMEOUT_MS: i64 = 180_000;

/// How long a `Waiting` core may stay away before this gives up (`Failed(Timeout)`, which stalls
/// the lane). Measured from `sent_at_ms`, never from `left_at_ms` -- a core that takes its time
/// leaving `Ready` must not effectively earn extra time to come back.
///
/// Same measurement as [`SEND_TO_DROP_TIMEOUT_MS`].
pub(super) const DROP_TO_READY_TIMEOUT_MS: i64 = 900_000;

/// How long a `Verifying` core may take to come back on a FRESH client before this gives up
/// (`Unverified(RespawnTimedOut)`, which ADVANCES the lane).
///
/// Its own constant, measured from `verify_at_ms`, deliberately NOT the remaining slice of
/// `DROP_TO_READY_TIMEOUT_MS`: that budget is measured from `sent_at_ms`, so a core that took
/// fourteen minutes to update would enter `Verifying` with sixty seconds left and time out
/// instantly, converting a successful update into an unverified one for a reason that has
/// nothing to do with the respawn. What this leg measures is one local TCP connect plus one
/// MoonProto init, which the fleet does in seconds.
pub(super) const VERIFY_TO_READY_TIMEOUT_MS: i64 = 120_000;

/// Retained history ring size -- about ten full campaigns on a 200-core fleet.
pub(super) const HISTORY_CAP: usize = 2_000;

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
        /// Letter paired with `from`, captured at the same instant. `None` when the core had
        /// reported no letter. Not folded from `Some("")`.
        from_suffix: Option<String>,
        /// `CoreData::conn_epoch` read at the same moment as `from`, above. The completion
        /// predicate proves the core actually departed by comparing against THIS baseline, not
        /// against whatever epoch happened to be current when the core was merely enqueued -- a
        /// core can leave and return for reasons that have nothing to do with this update while
        /// queued, and a baseline taken then would misread that unrelated departure as the
        /// update's.
        epoch0: u64,
        sent_at_ms: i64,
        /// `CoreData::update_rejects` read at the same instant as `from` and `epoch0`, above. A
        /// rejection is attributed to THIS attempt only if the counter has moved past this
        /// baseline -- the same snapshot/compare idiom `epoch0` already uses, and for the same
        /// reason: a core can be rejected for a PRIOR attempt's target while this one sits queued
        /// behind it, and a baseline taken at send time is what keeps that from being misread as
        /// this attempt's own refusal.
        ///
        /// RESIDUAL, deliberately unfixed: this counter proves the rejection was OBSERVED after
        /// the send, never that it was CAUSED by it -- `ServerLogEvent` carries only a time and
        /// free text, no command id and no target identifier (MoonProto `events/types.rs:143-145`),
        /// so there is no wire correlator to check instead. A `BGF-SUB4` line already in flight at
        /// send time can be counted after this snapshot and misattributed to this attempt. Bounded
        /// three ways, matching the false-positive analysis in
        /// [`crate::feed::is_core_update_rejection`]: the
        /// departure check runs first in the arm that reads this field, so a core that actually
        /// began updating is unreachable here; the gate below confines this branch to
        /// `UpdateTarget::Named` attempts, since Release and Named share one wire call; and the
        /// window is only the fraction of a second between this send and the first drain that
        /// follows it. The failure direction stays a lane that advances one attempt early, never
        /// one that wedges.
        rejects0: u64,
    },
    /// The core has been observed leaving `Ready` (`conn_epoch` moved past `epoch0`) and has not
    /// yet been observed settled again.
    Waiting {
        target: UpdateTarget,
        from: Option<u32>,
        /// Letter paired with `from`, carried from `Sent`.
        from_suffix: Option<String>,
        epoch0: u64,
        sent_at_ms: i64,
        left_at_ms: i64,
    },
    /// The core is back and settled on its OLD MoonProto client, whose retained `ServerInfo`
    /// snapshot still describes the pre-update process. This phase forces a fresh client (via the
    /// existing `reconnect_request` drain) and settles only on a build read from it -- see the
    /// module doc and `advance_in_flight_updates` for why a respawn is the only route at the
    /// pinned MoonProto revision.
    ///
    /// RESPAWN-SAFETY VERDICT: proceed. What a respawn costs, at the instant this phase requests
    /// one: a new feed thread and MoonProto client (`spawn_feed`), `endpoint`/`sys`/`startup`
    /// reset and `server_version` cleared (`begin_connection_attempt`), this core's provider
    /// coordination dropped (`clear_core_coordination`) -- which can hand off or briefly leave an
    /// exchange without a provider -- while report replication does NOT restart from zero
    /// (`ReportSink::starts` resumes at `Resume`/`Checkpoint`). For a few seconds the core's row
    /// shows `Connecting`, the build cell blanks then repopulates, sys metrics blank and refill,
    /// and a market pane the core was feeding as provider may hand over.
    ///
    /// Why this is nonetheless right:
    /// 1. The same respawn is already the per-core manual reconnect button, so it is already
    ///    deemed safe to fire at a live, trading core.
    /// 2. The MARGINAL cost here is near zero: by the time this phase fires the core has just
    ///    been away for the entire update (24 s and 28 s measured 2026-08-31), so any provider
    ///    election it held was already re-decided on the first coordination tick after it left
    ///    `Ready` -- the respawn merely re-disturbs an election that just re-settled seconds ago.
    /// 3. The MoonBot process genuinely RESTARTED; MoonProto's retained snapshot describes a
    ///    process that no longer exists, so a full re-init is the correct response, not a trick
    ///    to read a number.
    /// 4. It is scoped to one core, once, only after an update this terminal itself commanded --
    ///    see [`SessionManager::advance_in_flight_updates`]'s apply pass, the single producer.
    ///
    /// No alternative exists at the pinned MoonProto revision `02cbe52`: `set_server_info` has
    /// exactly two production callers, both in the init state machine, and no public handle
    /// re-runs BaseCheck or re-publishes `ServerInfo` without a full respawn.
    ///
    /// RESIDUAL, deliberately out of scope: a core that restarts for any OTHER reason (hand
    /// restart, crash-and-recover) still shows its pre-restart build, because MoonProto's
    /// internal reconnect keeps the snapshot. This phase fires only after an update this queue
    /// itself sent, and must not be described as fixing that broader case.
    Verifying {
        target: UpdateTarget,
        from: Option<u32>,
        /// Letter paired with `from`, carried from `Waiting`.
        from_suffix: Option<String>,
        /// `CoreData::conn_epoch` read at the instant this phase was entered, i.e. at the FIRST
        /// settle after the update. The respawn request emitted alongside it is what moves this
        /// counter; the completion predicate proves the fresh client exists by requiring
        /// `conn_epoch > epoch1`, never `>=`.
        epoch1: u64,
        sent_at_ms: i64,
        left_at_ms: i64,
        /// When the respawn was requested. This leg's own bound is measured from HERE, never
        /// from `sent_at_ms` -- see [`VERIFY_TO_READY_TIMEOUT_MS`].
        verify_at_ms: i64,
    },
    /// The attempt reached a terminal outcome. A core in this phase is eligible to be enqueued
    /// again; [`eligible`] treats `Done` as though nothing were tracked for it.
    Done(CoreUpdateOutcome),
}

/// How one update attempt ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoreUpdateOutcome {
    /// The core came back on a different build than it left on, or a named attempt whose fresh
    /// client reported the expected letter (see [`verified_outcome`]).
    Succeeded {
        from: Option<u32>,
        to: u32,
        /// Letter the fresh client reported with `to`, if it reported one.
        ///
        /// `None` on a row written before this field existed, and when the client sent no letter.
        /// `Some("")` is a release letter and stays distinct from `None`.
        #[serde(default)]
        to_suffix: Option<String>,
    },
    /// A RELEASE attempt whose core came back on the SAME build it left on. This is a success for the queue --
    /// nothing is in flight on that IP once the core is back -- but a NEUTRAL outcome for the row,
    /// never rendered as a failure. It is also a deliberate, recorded deviation from a literal
    /// reading of "never two simultaneous updates on one IP": the invariant bought is exactly
    /// that, and a core that provably departed and returned has nothing in flight, regardless of
    /// whether the build changed. Stalling here would kill every bulk campaign at its first
    /// already-current core, and a fleet-relative "behind" predicate ([`SessionManager::cores_behind`])
    /// cannot avoid selecting those.
    ///
    /// Also a named attempt whose fresh client reported a letter other than the one requested,
    /// including when the number moved. That letter is `to_suffix`. There is no separate
    /// variant: an unknown outcome in `core_updates.json` would wipe the retained ring.
    Unchanged {
        version: u32,
        /// Letter the fresh client reported. `None` on a row written before this field existed.
        #[serde(default)]
        to_suffix: Option<String>,
    },
    /// The core returned from the update, but the terminal could not establish which build it
    /// returned on. NOT a failure of the update (the core provably departed and returned, so
    /// nothing is in flight on that IP) and NOT `Unchanged` (which asserts a build; this asserts
    /// only that we could not tell). See [`SessionManager::advance_in_flight_updates`]'s
    /// `Verifying` arm for why this stalls no lane: to reach `Verifying` at all the queue has
    /// already observed both the core's departure and its settled return, so the update itself is
    /// finished -- the only thing outstanding is the terminal's own read of the build. Stalling on
    /// that would punish every sibling on the IP for a terminal-side observability gap, and would
    /// kill any bulk campaign at the first core whose server or group happens to be inactive in
    /// config, with a stalled lane the user has to clear by hand.
    ///
    /// PERSISTENCE NOTE: `CoreUpdateOutcome` round-trips through `core_updates.json`
    /// (`config::CoreUpdateHistory`). An OLD file read by a NEW binary is safe -- serde resolves
    /// by name, no existing variant moved. A NEW file (containing this variant) read by an OLD
    /// binary is destructive: `serde_json` fails the whole document on an unknown variant and
    /// `CoreUpdateHistory::load` falls back to `Self::default()`, silently resetting the entire
    /// retained ring. Pre-existing behaviour for any variant addition to this enum; not mitigated
    /// here (per-record fault tolerance would need `Vec<serde_json::Value>` plus a hand-rolled
    /// second pass, far larger than the defect warrants).
    Unverified(UnverifiedReason),
    /// The attempt failed; see [`UpdateFailure`] for which way.
    Failed(UpdateFailure),
}

/// Outcome of an attempt that reached `Verifying` and read `to` from a fresh client.
///
/// Reaching `Verifying` proves only that the connection left and came back settled -- an
/// ordinary network reconnect to the SAME process does that too. `restarted` is the stronger
/// proof: a different MoonBot process was observed answering since the command was sent (see
/// `AttemptMeta::restarts0`).
///
/// A named build may carry the same number as the release it replaces. When the fresh client
/// reports a letter (`Some`), a Named attempt is installed only when `restarted` and that
/// letter equals [`UpdateTarget::expected_suffix`] (trimmed, ASCII case-insensitive). Anything
/// else, including a moved number or `Some("")`, is `Unchanged` carrying `to_suffix`. A missing
/// letter (`None`) keeps the older rule: a Named attempt that restarted onto an equal number is
/// installed. A Release attempt ignores the letter and compares numbers.
///
/// Args:
///     target: What this attempt asked the core to install.
///     from: Build reported before the attempt.
///     to: Build the fresh client reported.
///     to_suffix: Letter the fresh client reported with `to`. `None` and `Some("")` stay distinct.
///     restarted: Whether a different MoonBot process answered since the command was sent.
///
/// Returns:
///     `Succeeded` when the attempt installed, otherwise `Unchanged` with the reported letter.
pub fn verified_outcome(
    target: &UpdateTarget,
    from: Option<u32>,
    to: u32,
    to_suffix: Option<&str>,
    restarted: bool,
) -> CoreUpdateOutcome {
    let carried = to_suffix.map(str::to_string);
    if let (UpdateTarget::Named(_), Some(reported)) = (target, to_suffix) {
        let expected = target.expected_suffix().unwrap_or("");
        if restarted && reported.trim().eq_ignore_ascii_case(expected) {
            return CoreUpdateOutcome::Succeeded {
                from,
                to,
                to_suffix: carried,
            };
        }
        return CoreUpdateOutcome::Unchanged {
            version: to,
            to_suffix: carried,
        };
    }
    let installed_same_number = restarted && matches!(target, UpdateTarget::Named(_));
    if from == Some(to) && !installed_same_number {
        CoreUpdateOutcome::Unchanged {
            version: to,
            to_suffix: carried,
        }
    } else {
        CoreUpdateOutcome::Succeeded {
            from,
            to,
            to_suffix: carried,
        }
    }
}

/// Why a `Verifying` attempt could not establish the core's post-update build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnverifiedReason {
    /// `SessionManager::reconnect` declined the respawn -- the core is absent from the current
    /// configuration, or its server or group is inactive.
    RespawnUnavailable,
    /// The respawn was issued but no fresh connection epoch, Ready status and build arrived
    /// within [`VERIFY_TO_READY_TIMEOUT_MS`].
    RespawnTimedOut,
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
    /// The core answered the sent command with [`crate::feed::CORE_UPDATE_REJECT_CODE`]: it
    /// refused the TARGET (the version name did not exist on that build's update channel), so
    /// nothing was started and nothing is in flight on that IP. The lane ADVANCES -- contrast with
    /// [`NeverDropped`](Self::NeverDropped), which is untouched and stays for genuine silence,
    /// where the terminal cannot prove the core did not start an update it simply failed to
    /// observe. See the PERSISTENCE NOTE on [`CoreUpdateOutcome::Unverified`] above: this variant
    /// carries the same round-trip hazard as any other addition to this enum, not mitigated here.
    Rejected,
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
    /// Letter paired with [`Self::from`]. Captured at send, or at enqueue when the attempt never
    /// reached `Sent`. `None` on a row written before this field existed.
    #[serde(default)]
    pub from_suffix: Option<String>,
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
