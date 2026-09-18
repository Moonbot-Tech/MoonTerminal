//! The terminal's ONE resolver of archived order traces.
//!
//! Every surface that draws a closed trade's order lines — the trade window today, the live chart
//! in its "Moonbot lines" mode next — asks here and nowhere else: [`Backend::ensure_traces`] with
//! the rows it wants and a cap on how many the core may be asked about, then
//! [`Backend::trace_state_stamped`] per row after the [`TracesRevision`] wake. The resolver reads the local
//! archive (`moon_core::db::order_traces`) off the UI thread first, asks the core only for what
//! that did not answer, and adopts the core's answers from the session store on the feed drain —
//! one observer for the whole process instead of one per window. The feed files every answer in
//! the local archive on its way here, so a row resolved once is a local read forever after.
//!
//! # Why a store on the backend and not an entity of its own
//!
//! Asking the core goes through `SessionManager`, which the backend owns; a separate entity would
//! need a weak handle back. The wake channel IS separate ([`TracesRevision`]), like the report and
//! market-data revisions, so a backfill filing hundreds of answers a minute never repaints the
//! seventeen views that observe the backend itself.
//!
//! # States
//!
//! A row the resolver never met is [`TraceState::Unknown`]; `ensure_traces` moves it through
//! [`TraceState::Loading`] (the archive read) and, on a miss, [`TraceState::Pending`] (the core
//! asked), to [`TraceState::Lines`], [`TraceState::Empty`] or [`TraceState::Failed`]. `Empty` is
//! the core's own answer and final for the session; `Failed` is no answer at all — old core,
//! timeout, core replaced mid-request — and both take a user-driven [`Backend::retry_trace`],
//! never a timer, by the protocol's own advice.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use gpui::*;
use moon_core::db::order_traces::{self, TraceEntry};
use moon_core::feed::{ArchivedOrderTrace, ReportTracesOutcome};
use moon_core::session::store::CoreData;
use moon_core::session::{CoreId, CoreStore};

use crate::Backend;

/// Notification-only entity: fires when any row's [`TraceState`] changed.
pub(crate) struct TracesRevision;

/// Where the resolver stands with one row.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TraceState {
    /// Never asked about.
    Unknown,
    /// The local archive is being read.
    Loading,
    /// The archive holds nothing current and the core has not been asked: the row fell past the
    /// caller's cap. A later `ensure_traces` that reaches it asks the core without another read.
    Unasked,
    /// The core has been asked; the answer is awaited from the session store.
    Pending,
    /// Lines, from the archive or the core.
    Lines(Arc<[ArchivedOrderTrace]>),
    /// The core holds no archive for this row. Final until a user retry.
    Empty,
    /// No answer: the request failed, timed out, or could not be sent. Retryable.
    Failed,
}

impl TraceState {
    /// The lines, when there are any.
    pub(crate) fn lines(&self) -> Option<&Arc<[ArchivedOrderTrace]>> {
        match self {
            Self::Lines(lines) => Some(lines),
            _ => None,
        }
    }
}

/// One row's state, plus the store revision its core ask went out under: an answer filed at or
/// before it belongs to an earlier ask and is not this one's.
struct Slot {
    state: TraceState,
    seen_rev: u64,
    /// The resolver's own revision when this state was set: what a consumer fingerprints a row
    /// by. Never an `Arc` address — a dropped allocation can be reused by the next answer.
    stamp: u64,
}

/// Most rows kept in memory. The archive on disk is the real store; this is what open surfaces
/// are looking at, and a chart's history is a thousand rows at most. Evicted oldest-first, never
/// while loading or pending.
const SLOT_CAP: usize = 8192;

/// The resolver's memory. Pure state with no GPUI in it, so its transitions are testable.
#[derive(Default)]
pub(crate) struct TraceResolver {
    slots: HashMap<(CoreId, i64), Slot>,
    order: VecDeque<(CoreId, i64)>,
    /// The rows in `Pending`, so the adopt pass on every feed drain walks these and not the map.
    pending: HashSet<(CoreId, i64)>,
    /// `report_traces_epoch` per core at the last adopt: a moved epoch means the core process was
    /// replaced and every pending ask on it can no longer be answered.
    epochs: HashMap<CoreId, u64>,
    /// `rev` at the last change of any row of each core: what a consumer bound to one core
    /// compares before walking its rows, so a backfill on another core costs it nothing.
    core_revs: HashMap<CoreId, u64>,
    /// Advances on every state change; how the adopt pass reports "anything moved".
    rev: u64,
}

impl TraceResolver {
    /// The state of one row.
    pub(crate) fn state(&self, core: CoreId, uid: i64) -> TraceState {
        self.stamped(core, uid).0
    }

    /// The state of one row and the stamp it was set under; `0` for a row never met. Two reads
    /// with the same stamp saw the same state.
    pub(crate) fn stamped(&self, core: CoreId, uid: i64) -> (TraceState, u64) {
        self.slots
            .get(&(core, uid))
            .map_or((TraceState::Unknown, 0), |slot| {
                (slot.state.clone(), slot.stamp)
            })
    }

    fn bump(&mut self) {
        self.rev = self.rev.wrapping_add(1);
    }

    fn set(&mut self, core: CoreId, uid: i64, state: TraceState, seen_rev: u64) {
        let key = (core, uid);
        if !self.slots.contains_key(&key) {
            self.order.push_back(key);
        }
        if state == TraceState::Pending {
            self.pending.insert(key);
        } else {
            self.pending.remove(&key);
        }
        self.bump();
        self.core_revs.insert(core, self.rev);
        self.slots.insert(
            key,
            Slot {
                state,
                seen_rev,
                stamp: self.rev,
            },
        );
        self.evict();
    }

    /// The revision of the last change to any row of `core`; `0` while none was ever set.
    pub(crate) fn core_rev(&self, core: CoreId) -> u64 {
        self.core_revs.get(&core).copied().unwrap_or(0)
    }

    /// Drop settled rows past [`SLOT_CAP`], oldest first.
    fn evict(&mut self) {
        let mut scanned = 0;
        while self.slots.len() > SLOT_CAP && scanned < self.order.len() {
            let Some(key) = self.order.pop_front() else {
                break;
            };
            scanned += 1;
            let settled = self.slots.get(&key).is_some_and(|slot| {
                !matches!(slot.state, TraceState::Loading | TraceState::Pending)
            });
            if settled {
                self.slots.remove(&key);
            } else {
                self.order.push_back(key);
            }
        }
    }

    /// Rows among `uids` the archive has not been read for: they become `Loading` and are returned
    /// for the read, in the caller's order, with zeros and repeats dropped.
    pub(crate) fn begin_read(&mut self, core: CoreId, uids: &[i64]) -> Vec<i64> {
        let mut out = Vec::new();
        for &uid in uids {
            if uid == 0 || out.contains(&uid) {
                continue;
            }
            if self.state(core, uid) == TraceState::Unknown {
                self.set(core, uid, TraceState::Loading, 0);
                out.push(uid);
            }
        }
        out
    }

    /// Fold one archive read in and name the rows the core must be asked about.
    ///
    /// A row the archive holds becomes `Lines` or — while the empty answer is younger than the
    /// recheck age — `Empty`. Every other requested row is a miss; the first `cap` of them, in the
    /// caller's order, become `Pending` and are returned; the rest become `Unasked`, so a later
    /// call that reaches them asks the core without reading the archive again.
    ///
    /// Args:
    ///     core: The rows' core.
    ///     read: What the archive answered.
    ///     requested: The rows this read was for, in priority order.
    ///     cap: Most rows to ask the core about.
    ///     now_ms: Terminal wall clock, for the empty-answer age.
    ///     marks: The core's `(report_traces_rev, report_traces_epoch)` right now.
    pub(crate) fn apply_read(
        &mut self,
        core: CoreId,
        read: &HashMap<i64, TraceEntry>,
        requested: &[i64],
        cap: usize,
        now_ms: i64,
        marks: (u64, u64),
    ) -> Vec<i64> {
        let (seen_rev, epoch) = marks;
        self.epochs.insert(core, epoch);
        let mut ask = Vec::new();
        for &uid in requested {
            if self.state(core, uid) != TraceState::Loading {
                // Retried or evicted meanwhile; this read no longer speaks for it.
                continue;
            }
            match read.get(&uid) {
                Some(TraceEntry::Lines(lines)) => {
                    self.set(core, uid, TraceState::Lines(lines.clone()), seen_rev);
                }
                Some(entry @ TraceEntry::Empty { .. }) if entry.is_current(now_ms) => {
                    self.set(core, uid, TraceState::Empty, seen_rev);
                }
                _ if ask.len() < cap => {
                    self.set(core, uid, TraceState::Pending, seen_rev);
                    ask.push(uid);
                }
                _ => self.set(core, uid, TraceState::Unasked, 0),
            }
        }
        ask
    }

    /// Rows among `uids` the archive already missed on: the first `cap` become `Pending` and are
    /// returned for the ask, in the caller's order.
    pub(crate) fn begin_unasked(
        &mut self,
        core: CoreId,
        uids: &[i64],
        cap: usize,
        marks: (u64, u64),
    ) -> Vec<i64> {
        let (seen_rev, epoch) = marks;
        let mut ask = Vec::new();
        for &uid in uids {
            if ask.len() >= cap {
                break;
            }
            if self.state(core, uid) == TraceState::Unasked {
                self.epochs.insert(core, epoch);
                self.set(core, uid, TraceState::Pending, seen_rev);
                ask.push(uid);
            }
        }
        ask
    }

    /// A user asked again: the row is `Pending` whatever it was, and the core is asked by the
    /// caller. The archive is deliberately skipped — a retry exists to get past what it holds.
    pub(crate) fn begin_retry(&mut self, core: CoreId, uid: i64, marks: (u64, u64)) {
        let (seen_rev, epoch) = marks;
        self.epochs.insert(core, epoch);
        self.set(core, uid, TraceState::Pending, seen_rev);
    }

    /// The ask could not even be queued.
    pub(crate) fn send_failed(&mut self, core: CoreId, uid: i64) {
        self.set(core, uid, TraceState::Failed, 0);
    }

    /// Take the core's filed answers for this core's pending rows.
    ///
    /// Only an entry filed AFTER the ask (`entry.rev > seen_rev`) is its answer. A moved epoch
    /// fails every pending row of the core: the replacement process cannot answer them.
    ///
    /// Args:
    ///     core: The core whose store to read.
    ///     data: Its session data.
    ///
    /// Returns:
    ///     Whether any row changed.
    pub(crate) fn adopt(&mut self, core: CoreId, data: &CoreData) -> bool {
        let before = self.rev;
        let epoch_moved = self
            .epochs
            .get(&core)
            .is_some_and(|epoch| *epoch != data.report_traces_epoch);
        if epoch_moved {
            self.epochs.insert(core, data.report_traces_epoch);
        }
        let pending: Vec<(i64, u64)> = self
            .pending
            .iter()
            .filter(|(c, _)| *c == core)
            .filter_map(|key| self.slots.get(key).map(|slot| (key.1, slot.seen_rev)))
            .collect();
        for (uid, seen_rev) in pending {
            if epoch_moved {
                log::warn!(
                    "[x] traces uid={uid}: core {core} process replaced while the request was out"
                );
                self.set(core, uid, TraceState::Failed, 0);
                continue;
            }
            let Some(entry) = data.report_traces.get(&uid).filter(|e| e.rev > seen_rev) else {
                continue;
            };
            let state = match &entry.outcome {
                ReportTracesOutcome::Ready(lines) if lines.is_empty() => TraceState::Empty,
                ReportTracesOutcome::Ready(lines) => TraceState::Lines(lines.clone()),
                ReportTracesOutcome::Failed(_) => TraceState::Failed,
            };
            self.set(core, uid, state, entry.rev);
        }
        self.rev != before
    }

    /// [`Self::adopt`] for every core in the store, then fail what is pending on a core the store
    /// no longer holds: removed from the session (not merely disconnected — a reconnect keeps its
    /// data), nothing can answer it.
    ///
    /// Returns:
    ///     Whether any row changed.
    pub(crate) fn adopt_all(&mut self, store: &CoreStore) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        let before = self.rev;
        for (core, data) in store.cores() {
            self.adopt(core, data);
        }
        let orphaned: Vec<(CoreId, i64)> = self
            .pending
            .iter()
            .filter(|(core, _)| store.core(*core).is_none())
            .copied()
            .collect();
        for (core, uid) in orphaned {
            self.set(core, uid, TraceState::Failed, 0);
        }
        self.rev != before
    }
}

impl Backend {
    /// The wake channel for trace consumers.
    pub(crate) fn traces_revision(&self) -> Entity<TracesRevision> {
        self.traces_revision.clone()
    }

    /// Where the resolver stands with one row, with the stamp the state was set under — what a
    /// consumer fingerprints the row by to tell a wake that changed it from one that did not.
    pub(crate) fn trace_state_stamped(&self, core: CoreId, uid: i64) -> (TraceState, u64) {
        self.traces.stamped(core, uid)
    }

    /// The revision of the last change to any row of `core`: a consumer bound to one core skips
    /// its walk when this did not move since its last one.
    pub(crate) fn traces_core_rev(&self, core: CoreId) -> u64 {
        self.traces.core_rev(core)
    }

    /// Resolve `uids` on `core`: the archive first, then the core for at most `cap` misses.
    ///
    /// Rows already loading, pending or settled are left alone, so a caller may hand over the same
    /// list on every change of its own state. Order is priority: the first misses get the asks,
    /// and a row that fell past the cap last time is asked now if it is within it.
    ///
    /// Args:
    ///     core: The rows' core.
    ///     uids: `ReportUID`s in priority order; zeros are skipped.
    ///     cap: Most rows the core may be asked about for this call.
    ///     cx: Backend context.
    pub(crate) fn ensure_traces(
        &mut self,
        core: CoreId,
        uids: Vec<i64>,
        cap: usize,
        cx: &mut Context<Self>,
    ) {
        let marks = self.trace_marks(core);
        let ask_now = self.traces.begin_unasked(core, &uids, cap, marks);
        for uid in &ask_now {
            self.send_trace_ask(core, *uid);
        }
        // What the read may still ask for is what this call has left of its cap.
        let cap = cap.saturating_sub(ask_now.len());
        let to_read = self.traces.begin_read(core, &uids);
        if ask_now.is_empty() && to_read.is_empty() {
            return;
        }
        self.notify_traces(cx);
        if to_read.is_empty() {
            return;
        }
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let requested = to_read.clone();
            let read = executor
                .spawn(async move { order_traces::read_many(core, &to_read) })
                .await;
            let _ = this.update(cx, |b, cx| {
                b.traces_read_done(core, requested, read, cap, cx)
            });
        })
        .detach();
    }

    /// The archive read came back: settle what it held and ask the core for the rest.
    fn traces_read_done(
        &mut self,
        core: CoreId,
        requested: Vec<i64>,
        read: moon_core::db::ReadResult<HashMap<i64, TraceEntry>>,
        cap: usize,
        cx: &mut Context<Self>,
    ) {
        let read = match read {
            Ok(read) => read,
            Err(error) => {
                // An unreadable archive is a miss for every row, not a failure: the core is still
                // there to ask, and its answers are filed for the next time.
                log::warn!("[x] traces: local archive read failed, asking the core: {error}");
                HashMap::new()
            }
        };
        let hits = read.len();
        let marks = self.trace_marks(core);
        let ask = self.traces.apply_read(
            core,
            &read,
            &requested,
            cap,
            moon_core::util::now_unix_ms_i64(),
            marks,
        );
        log::info!(
            "[traces] core {core}: {} row(s) — {hits} from the archive, {} asked of the core",
            requested.len(),
            ask.len()
        );
        for uid in ask {
            self.send_trace_ask(core, uid);
        }
        self.notify_traces(cx);
    }

    /// Ask the core again for one row, past whatever the archive or the session holds.
    pub(crate) fn retry_trace(&mut self, core: CoreId, uid: i64, cx: &mut Context<Self>) {
        if uid == 0 {
            return;
        }
        let marks = self.trace_marks(core);
        self.traces.begin_retry(core, uid, marks);
        self.send_trace_ask(core, uid);
        self.notify_traces(cx);
    }

    /// Queue one ask on the session; a failure to queue settles the row as `Failed` at once.
    fn send_trace_ask(&mut self, core: CoreId, uid: i64) {
        if let Err(error) = self.session.request_report_traces(core, uid) {
            log::warn!("[x] traces uid={uid} on core {core} not sent: {error}");
            self.traces.send_failed(core, uid);
        }
    }

    /// The core's `(report_traces_rev, report_traces_epoch)`, or zeros when it is not in the store.
    fn trace_marks(&self, core: CoreId) -> (u64, u64) {
        self.session.store().core(core).map_or((0, 0), |data| {
            (data.report_traces_rev, data.report_traces_epoch)
        })
    }

    /// Adopt the cores' filed answers. Called from the feed drain on the `report_traces` edge and
    /// on a core's removal; cheap when nothing is pending.
    pub(crate) fn adopt_traces(&mut self, cx: &mut Context<Self>) {
        if self.traces.adopt_all(self.session.store()) {
            self.notify_traces(cx);
        }
    }

    fn notify_traces(&self, cx: &mut Context<Self>) {
        self.traces_revision.update(cx, |_, cx| cx.notify());
    }
}

#[cfg(test)]
mod tests;
