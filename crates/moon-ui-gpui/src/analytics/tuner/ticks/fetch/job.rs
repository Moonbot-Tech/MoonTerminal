//! The tape fetch as a job of the PROCESS, not of the window.
//!
//! The Analytics window is created and dropped with its view; a batch that lived inside the view
//! died with it, and a batch asleep on a venue's backoff had nothing in flight to keep a scope
//! reload from wiping it. Here the queue, the walk order, the per-venue backoff waits and the
//! progress live on one background thread that outlives every window; a view only hands it rows
//! (resolved while the view still had the live source), listens for what lands, and reads the
//! progress for its caption. Closing the window drops the listener, nothing else; reopening it
//! attaches a new one and reads the same progress. The startup autoload ([`enqueue`]) adds rows
//! to the same queue; a batch is whatever is in it, wherever it came from.
//!
//! One request at a time PER VENUE, several venues at once. The replay worker walks the hosts
//! in parallel lanes and paces each on its own, so two requests on one exchange key would only
//! queue behind each other there while the second one's slot could have been another venue's.
//! A request is a CLUSTER: the next pending row of a free key — a row the user is looking at
//! first ([`prioritize`]), else the oldest — plus every pending row of the
//! same market whose window overlaps it, as long as the first entry and the last exit stay
//! within a long position's length (`[trade_replay] long_position_min`, past which the worker
//! walks only the two ends). One walk of the whole stretch serves them all — a pumped coin closes dozens
//! of trades in minutes, and asked one by one each of them re-walked the same seconds and paid the same page
//! budget, and on a venue with small pages (OKX, 100 prints) each of them died on that budget
//! in turn. Every row of the cluster is then replayed off the tiles on its own and answered on
//! its own. A venue that refuses (the gate's backoff, or its own error mid-walk) puts its rows
//! aside for the gate's own number of seconds while the rows of other venues go on; the
//! refused rows go back first once the wait is out, and a row the venue itself refused is asked
//! once more before it counts as final. A walk the worker cut short on its own page budget or
//! deadline — a pumped coin on a venue with small pages — is CONTINUED: the rows it did not
//! reach go back to the end of their venue's turn, and the next walk picks up where the tiles
//! end, for as long as each walk gains tape ([`MAX_CONTINUATIONS`] at most). An autoload
//! row dropped while that walk is out does not go back.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use super::super::load::{ArchivedLines, replay_row};
use super::super::state::{DealRow, RowAddress, TapeStatus};
use moon_core::db::order_traces::read_many;
use moon_core::db::tuner::ticks::Deal;
use moon_core::market::ReplayAddress;
use moon_core::market::trade_replay::venue_caps::TickValue;
use moon_core::market::trade_replay::worker::{self, TradeReplayRequest};
use moon_core::market::trade_replay::{
    Coverage, ReplayIntent, ReplayWindow, TickStatus, TradeReplayEmpty, TradeReplayFailure,
    TradeReplayOutcome, replay_window_ms,
};

/// The wait after a venue's own refusal mid-walk, when the gate names no number: the gate's own
/// floor, so the second ask lands after the backoff it will have recorded.
const VENUE_REFUSAL_WAIT: Duration = Duration::from_secs(30);

/// How many times a row goes back for the rest of its tape after a walk that stopped on the
/// worker's own budget. Each continuation is a walk of up to the trade budget (240 pages);
/// twelve of them are ~20 minutes of a pumped coin's tape on OKX (100 prints a page, ~240 a
/// second measured 2026-09-20), which is more than any one position needs.
const MAX_CONTINUATIONS: u8 = 12;

/// Ceiling on the rows out at once, whatever the number of exchange keys: one per key is the
/// rule, this is the guard against a fleet on many small venues fanning into many threads.
const MAX_IN_FLIGHT: usize = 8;

/// One deal as the job asks for it: everything the request needs, resolved by the view while it
/// still had the live source for the exchange identity and the contract terms.
pub(in crate::analytics::tuner) struct QueuedRow {
    pub(in crate::analytics::tuner) deal: Deal,
    pub(in crate::analytics::tuner) address: Arc<RowAddress>,
    pub(in crate::analytics::tuner) replay_address: ReplayAddress,
    pub(in crate::analytics::tuner) tick_value: TickValue,
    pub(in crate::analytics::tuner) window: ReplayWindow,
    /// Whether the venue itself already refused this row once; the second refusal is final.
    retried: bool,
    /// How many walks were continued for this row's tape, and how much of the request's focus
    /// the last walk's answer covered — a continuation must gain on it.
    continued: u8,
    covered_ms: i64,
    /// Whether the user is looking at this row — the dispatcher takes such rows before the
    /// rest of their venue's queue ([`prioritize`]). A mark, not a position: it survives a
    /// deferral and a continuation, which reorder the queue.
    priority: bool,
}

impl QueuedRow {
    /// Whether a venue or recent core archive can still supply the uncovered request.
    pub(super) fn fetchable_after_station(&self, now_ms: i64) -> bool {
        use moon_core::db::tuner::ticks::tail::{floor_ms, required_with_floor};
        use moon_core::market::trade_replay::trade_cache;
        let need = required_with_floor(&self.window, floor_ms(super::super::tail::effective_s()));
        let Some((from, to)) = need.hull() else {
            return false;
        };
        let held = trade_cache::handle()
            .and_then(|cache| {
                cache.held_spans(&self.address.exchange_key, &self.address.market, from, to)
            })
            .map(Coverage::from_spans)
            .unwrap_or_default();
        let Some((from, _)) = need.minus(&held).hull() else {
            return false;
        };
        reachable_after_station(
            self.address.venue,
            &self.address.market,
            from,
            to,
            self.window.close_ms,
            now_ms,
        )
    }

    /// Capture an automatic row's window and continuation state before admission.
    pub(in crate::analytics::tuner) fn new(
        deal: Deal,
        address: Arc<RowAddress>,
        replay_address: ReplayAddress,
        tick_value: TickValue,
        window: ReplayWindow,
    ) -> Self {
        Self {
            deal,
            address,
            replay_address,
            tick_value,
            window,
            retried: false,
            continued: 0,
            covered_ms: 0,
            priority: false,
        }
    }
}

/// Known venues keep their source reach; unlisted windows need overlap with core retention.
pub(in crate::analytics::tuner) fn reachable_after_station(
    venue: Option<moon_core::venue::Venue>,
    market: &str,
    from_ms: i64,
    to_ms: i64,
    close_ms: i64,
    now_ms: i64,
) -> bool {
    use moon_core::market::trade_replay::venue_caps::{Fetchable, fetchable};
    let venue_from = if from_ms > close_ms { from_ms } else { to_ms };
    matches!(
        fetchable(venue, market, venue_from, now_ms),
        Fetchable::Rest | Fetchable::Archive
    ) || if venue.is_none() {
        to_ms >= now_ms.saturating_sub(moon_core::market::CORE_ARCHIVE_RETENTION_MS)
    } else {
        from_ms >= now_ms.saturating_sub(moon_core::market::CORE_ARCHIVE_RETENTION_MS)
    }
}

/// What the job tells whoever listens.
pub(in crate::analytics::tuner) enum JobEvent {
    /// Station filing or refusal changed; the view must reload tape for the current sample.
    StationFilled,
    /// A request went out for this row.
    Started(i64),
    /// Every answer from one cluster: tape and verdicts included, with no row dropped.
    Rows(Vec<DealRow>),
    /// Progress moved without a row answer — a deferral, a resume, the end of the batch.
    Progress,
    /// The autoload switch dropped this in-flight row, so the fetching mark comes off now.
    /// The cancelled walk cannot start another continuation.
    Released(i64),
}

/// Notify an attached view after station filing or refusal, even without queued venue rows.
pub(super) fn station_filled() {
    lock(job()).notify(JobEvent::StationFilled);
}

/// The job as a caption reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::analytics::tuner) struct Progress {
    /// Whether a batch is running — in flight, queued, or asleep on a venue's wait.
    pub(in crate::analytics::tuner) active: bool,
    /// Rows answered since the batch started, and the batch's size.
    pub(in crate::analytics::tuner) done: usize,
    pub(in crate::analytics::tuner) total: usize,
    /// The requests out, in the order they went out: the ids of the rows each one serves, and
    /// its market.
    pub(in crate::analytics::tuner) in_flight: Vec<(Vec<i64>, String)>,
    /// Time until the soonest waiting host may be asked again.
    ///
    /// `None` while a walk is out, or while no host is waiting: the caption then names the
    /// markets in flight, or the bare waiting line. When present it is the same deadline the
    /// dispatcher wakes on, read at paint time so a later repaint shows less of it.
    pub(in crate::analytics::tuner) venue_wait: Option<Duration>,
}

/// A row set aside until its venue's wait is out.
struct Deferred {
    row: QueuedRow,
    due: Instant,
}

/// A request out for a cluster of rows.
struct InFlight {
    /// Identity of this walk. A later flight for the same rows gets another id, so this
    /// walk's return cannot take that flight off the books.
    id: u64,
    uids: Vec<i64>,
    market: String,
    exchange_key: String,
    /// Raised when this automatic walk is cancelled.
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    /// Rows still to ask for; popped from the end, so the caller orders them newest-first.
    pending: Vec<QueuedRow>,
    deferred: Vec<Deferred>,
    in_flight: Vec<InFlight>,
    done: usize,
    total: usize,
    /// The strategy-field defaults the model runs with, captured when the batch started.
    defaults: HashMap<String, f64>,
    /// The window listening, if any; a dead receiver is the window gone, and is dropped.
    listener: Option<mpsc::Sender<JobEvent>>,
    /// When each row of the running batch was answered, for a reload that read the row before
    /// the answer landed to re-read it — see [`finished_after`].
    finished: Vec<(i64, Instant)>,
}

impl State {
    fn active(&self) -> bool {
        !self.in_flight.is_empty() || !self.pending.is_empty() || !self.deferred.is_empty()
    }

    fn notify(&mut self, event: JobEvent) {
        if self
            .listener
            .as_ref()
            .is_some_and(|listener| listener.send(event).is_err())
        {
            self.listener = None;
        }
    }

    /// Send one channel event for a completed cluster while retaining every answered row.
    fn notify_answers(&mut self, answers: Vec<DealRow>) {
        if !answers.is_empty() {
            self.notify(JobEvent::Rows(answers));
        }
    }

    /// Publish the last batch before marking the flight idle, while the listener lock is held.
    fn finish_flight(&mut self, id: u64, answers: Vec<DealRow>) {
        self.notify_answers(answers);
        self.detach_flight(id);
    }

    /// Set aside `row` and every pending row on the same exchange key until `wait` is out;
    /// rows on other venues stay in the queue and keep going. The refused row rejoins first.
    fn defer(&mut self, row: QueuedRow, wait: Duration) {
        let key = row.address.exchange_key.clone();
        let due = Instant::now() + wait;
        let (same, other) = split_by_key(std::mem::take(&mut self.pending), &key, |r| {
            &r.address.exchange_key
        });
        self.pending = other;
        // `pending` pops from the end: the refused row goes in last, so it comes out first.
        for row in same.into_iter().chain(std::iter::once(row)) {
            self.deferred.push(Deferred { row, due });
        }
    }

    /// Return every row whose wait is out to the queue, in the order it was set aside.
    fn resume_due(&mut self, now: Instant) -> bool {
        let (due, later): (Vec<Deferred>, Vec<Deferred>) =
            self.deferred.drain(..).partition(|d| d.due <= now);
        self.deferred = later;
        let any = !due.is_empty();
        self.pending.extend(due.into_iter().map(|d| d.row));
        any
    }

    /// The next row that may go out — see [`pick_dispatchable`].
    fn dispatchable(&self) -> Option<usize> {
        if crate::backend::station::known_cached() {
            return None;
        }
        let busy: HashSet<&str> = self
            .in_flight
            .iter()
            .map(|f| f.exchange_key.as_str())
            .collect();
        pick_dispatchable(
            self.pending
                .iter()
                .map(|row| (row.address.exchange_key.as_str(), row.priority)),
            &busy,
            self.in_flight.len(),
        )
    }

    /// Remove the flight `id` names.
    ///
    /// Returns:
    ///     Whether that flight was still booked. A newer flight for the same rows has
    ///     another id and stays.
    fn detach_flight(&mut self, id: u64) -> bool {
        let still = self.in_flight.iter().any(|flight| flight.id == id);
        self.in_flight.retain(|flight| flight.id != id);
        still
    }

    /// Cancel automatic work and return in-flight ids whose fetching mark must clear.
    fn drop_autoload(&mut self) -> Vec<i64> {
        let unmark: Vec<_> = self
            .in_flight
            .iter()
            .flat_map(|flight| flight.uids.iter().copied())
            .collect();
        let dropped: Vec<_> = self
            .pending
            .iter()
            .map(|row| row.deal.report_uid)
            .chain(self.deferred.iter().map(|entry| entry.row.deal.report_uid))
            .chain(unmark.iter().copied())
            .collect();
        self.pending.clear();
        self.deferred.clear();
        for flight in self.in_flight.drain(..) {
            flight.cancel.store(true, Ordering::Relaxed);
        }
        let already_done = self.finished.iter().map(|(uid, _)| *uid).collect();
        self.total = self
            .total
            .saturating_sub(rows_leaving_total(&dropped, &already_done));
        unmark
    }

    /// Every id the batch already knows — queued, waiting, out, or answered — so a row is
    /// never asked for twice by two sources of rows.
    fn known(&self) -> HashSet<i64> {
        self.pending
            .iter()
            .map(|r| r.deal.report_uid)
            .chain(self.deferred.iter().map(|d| d.row.deal.report_uid))
            .chain(self.in_flight.iter().flat_map(|f| f.uids.iter().copied()))
            .chain(self.finished.iter().map(|(uid, _)| *uid))
            .collect()
    }
}

struct Job {
    state: Mutex<State>,
    wake: Condvar,
}

static JOB: OnceLock<Job> = OnceLock::new();

fn job() -> &'static Job {
    JOB.get_or_init(|| Job {
        state: Mutex::new(State::default()),
        wake: Condvar::new(),
    })
}

fn lock(job: &Job) -> std::sync::MutexGuard<'_, State> {
    job.state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The thread, started with the first batch and never stopped.
static THREAD: OnceLock<()> = OnceLock::new();

fn ensure_thread() {
    THREAD.get_or_init(|| {
        std::thread::Builder::new()
            .name("tuner-ticks-fetch".into())
            .spawn(|| run(job()))
            .expect("spawn the tuner fetch thread");
    });
}

/// Add rows to the batch — the running one, or a fresh one when none runs. Rows the batch
/// already knows (queued, waiting, out, or answered since it started) are dropped, so the
/// startup autoload and a table load cannot ask for the same trade twice. `rows` is
/// newest-first: the job pops from the end, so the oldest — nearest the venues' retention edge
/// — goes first. `defaults` are taken only when they open a fresh batch.
///
/// Returns:
///     How many rows were added.
pub(in crate::analytics::tuner) fn enqueue(
    rows: Vec<QueuedRow>,
    defaults: HashMap<String, f64>,
) -> usize {
    if rows.is_empty() {
        return 0;
    }
    ensure_thread();
    let job = job();
    let mut st = lock(job);
    if !st.active() {
        st.total = 0;
        st.done = 0;
        st.deferred.clear();
        st.finished.clear();
        st.defaults = defaults;
    }
    let known = st.known();
    let fresh: Vec<QueuedRow> = rows
        .into_iter()
        .filter(|row| !known.contains(&row.deal.report_uid))
        .collect();
    let added = fresh.len();
    if added == 0 {
        return 0;
    }
    // In FRONT of what is queued: `pending` pops from the end, and the rows already there —
    // an earlier autoload pass — keep their turn.
    let mut pending = fresh;
    pending.append(&mut st.pending);
    st.pending = pending;
    st.total += added;
    st.notify(JobEvent::Progress);
    drop(st);
    job.wake.notify_all();
    added
}

/// Mark the rows among `uids` as the ones the user is looking at: the dispatcher takes a
/// marked row of a free venue before the venue's other rows, oldest marked row first. The
/// autoload queues a month of trades oldest-first, so the freshest rows, the ones at the top
/// of a table, would otherwise be the last of hundreds; each axis load marks its visible rows. A mark rather than a move: a venue's backoff sweeps rows out
/// of the queue and back, and a continuation re-queues a row — a position would not survive
/// either, the mark does. Rows out, answered or unknown are left alone.
///
/// Returns:
///     How many rows were newly marked, queued or waiting.
pub(in crate::analytics::tuner) fn prioritize(uids: &HashSet<i64>) -> usize {
    if uids.is_empty() {
        return 0;
    }
    let job = job();
    let mut st = lock(job);
    let mut marked = 0usize;
    let State {
        pending, deferred, ..
    } = &mut *st;
    for row in pending
        .iter_mut()
        .chain(deferred.iter_mut().map(|d| &mut d.row))
    {
        if !row.priority && uids.contains(&row.deal.report_uid) {
            row.priority = true;
            marked += 1;
        }
    }
    drop(st);
    if marked > 0 {
        job.wake.notify_all();
    }
    marked
}

/// Drop every queued automatic row and cancel all of its in-flight walks.
pub(crate) fn stop_autoload() {
    let job = job();
    let mut st = lock(job);
    let unmark = st.drop_autoload();
    for uid in unmark {
        st.notify(JobEvent::Released(uid));
    }
    st.notify(JobEvent::Progress);
    drop(st);
    job.wake.notify_all();
}

/// Listen to the job from now on; the previous listener, if any, is replaced.
pub(in crate::analytics::tuner) fn attach() -> mpsc::Receiver<JobEvent> {
    let (tx, rx) = mpsc::channel();
    lock(job()).listener = Some(tx);
    rx
}

/// Time until the soonest waiting host may be asked again.
///
/// Each host contributes the deadline its deferred rows share (`State::defer`). The
/// dispatcher wakes at the earliest of those instants, so the caption names the same wait
/// the next try will honour. A walk still out is the progress line: a wait beside it would
/// hide the market the batch is on.
///
/// Args:
///     in_flight: Whether any walk is out.
///     hosts: Deadline of each host that is waiting. Empty when nothing is waiting.
///     now: The instant the caption is read.
///
/// Returns:
///     Time until the soonest deadline. `Some(Duration::ZERO)` when that deadline has
///     already passed and the next try is due. `None` when a walk is out, or when no host
///     is waiting.
pub(super) fn soonest_venue_wait(
    in_flight: bool,
    hosts: impl IntoIterator<Item = Instant>,
    now: Instant,
) -> Option<Duration> {
    if in_flight {
        return None;
    }
    let earliest = hosts.into_iter().min()?;
    Some(earliest.saturating_duration_since(now))
}

/// The job as the caption reads it.
pub(in crate::analytics::tuner) fn progress() -> Progress {
    let st = lock(job());
    let now = Instant::now();
    let in_flight: Vec<(Vec<i64>, String)> = st
        .in_flight
        .iter()
        .map(|flight| (flight.uids.clone(), flight.market.clone()))
        .collect();
    // One deadline per deferred row. Rows of one host share the deadline `defer` wrote, so
    // the minimum is the soonest host — the same instant `run` waits for.
    let venue_wait = soonest_venue_wait(
        !in_flight.is_empty(),
        st.deferred.iter().map(|row| row.due),
        now,
    );
    Progress {
        active: st.active(),
        done: st.done,
        total: st.total,
        in_flight,
        venue_wait,
    }
}

/// Rows of the running batch answered after `since` — what a reload that started reading at
/// `since` may have read before the answer landed, and must read again.
pub(in crate::analytics::tuner) fn finished_after(since: Instant) -> Vec<i64> {
    lock(job())
        .finished
        .iter()
        .filter(|(_, at)| *at >= since)
        .map(|(uid, _)| *uid)
        .collect()
}

/// Drop `id` from the flights still booked.
///
/// Args:
///     booked: Flight ids still out.
///     id: The walk that is returning or that failed to start.
///
/// Returns:
///     Whether `id` was still booked. A newer flight for the same rows has another id and
///     stays in `booked`.
#[cfg(test)]
pub(super) fn retain_flight(booked: &mut Vec<u64>, id: u64) -> bool {
    let still = booked.contains(&id);
    booked.retain(|booked_id| *booked_id != id);
    still
}

/// The next flight id. The first walk is 1, and every walk after it is distinct.
fn next_flight_id() -> u64 {
    static NEXT_FLIGHT: AtomicU64 = AtomicU64::new(1);
    NEXT_FLIGHT.fetch_add(1, Ordering::Relaxed)
}

/// How many dropped ids still belong to the batch total.
///
/// An id already in `done` was counted when it was filed and must not be subtracted again.
/// Duplicate ids across the pending, deferred and in-flight lists leave once.
///
/// Args:
///     dropped: Report ids removed from the queue or from a cancelled walk.
///     already_done: Ids already filed into `done`.
///
/// Returns:
///     The count to subtract from the batch total.
pub(super) fn rows_leaving_total(dropped: &[i64], already_done: &HashSet<i64>) -> usize {
    let mut seen = HashSet::new();
    dropped
        .iter()
        .filter(|uid| seen.insert(**uid) && !already_done.contains(*uid))
        .count()
}

/// Split a queue into the rows on `key` and the rest, both in their queue order.
fn split_by_key<T>(rows: Vec<T>, key: &str, key_of: impl Fn(&T) -> &str) -> (Vec<T>, Vec<T>) {
    rows.into_iter().partition(|row| key_of(row) == key)
}

/// The index of the next row that may go out, while the in-flight ceiling allows one more: the
/// OLDEST pending row (the queue pops from its end) whose exchange key has nothing in flight —
/// among the rows the user is looking at ([`prioritize`]) when any of those is on a free key,
/// else among all.
///
/// Args:
///     pending: The exchange key and the priority mark of every pending row, in queue order
///         (newest first).
///     busy: The exchange keys with a request out.
///     out: How many requests are out.
///
/// Returns:
///     The queue index to take, or `None` when nothing may go out now.
pub(super) fn pick_dispatchable<'a>(
    pending: impl Iterator<Item = (&'a str, bool)>,
    busy: &HashSet<&str>,
    out: usize,
) -> Option<usize> {
    if out >= MAX_IN_FLIGHT {
        return None;
    }
    let rows: Vec<(&str, bool)> = pending.collect();
    let free = |(key, _): &(&str, bool)| !busy.contains(key);
    rows.iter()
        .rposition(|row| row.1 && free(row))
        .or_else(|| rows.iter().rposition(free))
}

/// What the cluster rule reads of a row: its market on its exchange, and its window — opened
/// where the row's own window opens (`model_window`: the entry order's creation, else the buy).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ClusterKey<'a> {
    pub(super) exchange_key: &'a str,
    pub(super) market: &'a str,
    pub(super) open_ms: i64,
    pub(super) close_ms: i64,
    pub(super) margin_ms: i64,
}

/// The rows that go out with the seed in one request: every pending row of the seed's market
/// whose margined window overlaps the cluster's hull, taken while the hull's first open and
/// last exit stay within `long_position_ms` — the seed's own threshold, captured when its window
/// was built (`ReplayWindow::long_position_ms`), past which the worker walks a stretch as its
/// two ends. Grows until nothing more joins — a row that joins can bridge to the next one.
///
/// Args:
///     rows: The pending rows' keys, in queue order.
///     seed: The index of the row the dispatcher picked.
///     long_position_ms: The longest hull, first entry to last exit, walked as one stretch.
///
/// Returns:
///     The indices of the cluster, the seed included, ascending.
pub(super) fn pick_cluster(
    rows: &[ClusterKey<'_>],
    seed: usize,
    long_position_ms: i64,
) -> Vec<usize> {
    let anchor = rows[seed];
    let mut taken = vec![seed];
    let (mut first_open, mut last_close) = (anchor.open_ms, anchor.close_ms);
    loop {
        let mut grew = false;
        for (index, row) in rows.iter().enumerate() {
            if taken.contains(&index)
                || row.exchange_key != anchor.exchange_key
                || row.market != anchor.market
            {
                continue;
            }
            let overlaps = row.open_ms.saturating_sub(row.margin_ms)
                <= last_close.saturating_add(anchor.margin_ms)
                && row.close_ms.saturating_add(row.margin_ms)
                    >= first_open.saturating_sub(anchor.margin_ms);
            let hull_from = first_open.min(row.open_ms);
            let hull_to = last_close.max(row.close_ms);
            if overlaps && hull_to.saturating_sub(hull_from) <= long_position_ms {
                taken.push(index);
                first_open = hull_from;
                last_close = hull_to;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    taken.sort_unstable();
    taken
}

/// Whether a row goes back for the rest of its tape: the walk ran (no refusal to wait out —
/// the caller checks that first), left the row uncovered, stopped SHORT of the request's focus
/// on the worker's own budget or deadline, covered more of it than the row's previous walk
/// did — the same answer serves every row of a cluster, so a row the walk has not reached yet
/// still sees the walk advance — and the row has continuations left. A walk that covered the
/// whole focus and still left the row missing found no prints for it, which no continuation
/// changes; one that gained nothing found a stretch the venue serves nothing for.
///
/// Args:
///     status: The walk's tick status.
///     tape: What the row became after the replay off the tiles.
///     walk_short: Whether the answer's coverage stops short of the request's focus.
///     covered_ms: Width of the answer's coverage.
///     previous_ms: Width after the row's previous walk; zero before the first.
///     continued: Continuations the row already had.
pub(super) fn continues(
    status: TickStatus,
    tape: TapeStatus,
    walk_short: bool,
    covered_ms: i64,
    previous_ms: i64,
    continued: u8,
) -> bool {
    tape == TapeStatus::Missing
        && matches!(status, TickStatus::Served | TickStatus::Streaming)
        && walk_short
        && covered_ms > previous_ms
        && continued < MAX_CONTINUATIONS
}

/// How long the batch waits before asking for the same row again, when it should: the gate
/// refused the host and the row is still uncovered. A row the tiles covered anyway needs no
/// second ask, and every other status is the row's final word.
pub(super) fn retry_wait(status: TickStatus, tape: TapeStatus) -> Option<u32> {
    match (status, tape) {
        (TickStatus::RateLimited { retry_in_s }, TapeStatus::Missing) => Some(retry_in_s),
        _ => None,
    }
}

/// File an exhausted fetch refusal even when the model pad was already covered.
/// A continuing walk keeps its current coverage; a fully satisfied row keeps Covered.
fn final_tape(
    tape: TapeStatus,
    pending: TapeStatus,
    status: TickStatus,
    retrying: bool,
) -> TapeStatus {
    if pending != TapeStatus::Missing || retrying {
        return tape;
    }
    match status {
        TickStatus::Served | TickStatus::Pending | TickStatus::Streaming => tape,
        refused => TapeStatus::Refused(refused),
    }
}

/// The thread: dispatches one row per free exchange key onto its own walk, waits included.
/// It never walks itself, so a venue's three-minute walk holds nobody else's turn.
fn run(job: &'static Job) {
    loop {
        let mut st = lock(job);
        let now = Instant::now();
        if st.resume_due(now) {
            st.notify(JobEvent::Progress);
        }
        if let Some(index) = st.dispatchable() {
            let keys: Vec<ClusterKey<'_>> = st
                .pending
                .iter()
                .map(|row| ClusterKey {
                    exchange_key: &row.address.exchange_key,
                    market: &row.address.market,
                    open_ms: row.window.open_ms,
                    close_ms: row.deal.close_ms,
                    margin_ms: row.window.margin_ms,
                })
                .collect();
            // The seed's own threshold, captured when its window was built — the same one
            // the walk and the post-walk check judge the row by.
            let indices = pick_cluster(&keys, index, st.pending[index].window.long_position_ms);
            // Removed from the back, so each index still names the row it was picked for.
            let mut rows: Vec<QueuedRow> = indices
                .iter()
                .rev()
                .map(|&i| st.pending.remove(i))
                .collect();
            rows.reverse();
            let defaults = st.defaults.clone();
            let cancel = Arc::new(AtomicBool::new(false));
            let uids: Vec<i64> = rows.iter().map(|r| r.deal.report_uid).collect();
            let flight_id = next_flight_id();
            let first = &rows[0];
            st.in_flight.push(InFlight {
                id: flight_id,
                uids: uids.clone(),
                market: match rows.len() {
                    1 => first.address.market.clone(),
                    n => format!("{}×{n}", first.address.market),
                },
                exchange_key: first.address.exchange_key.clone(),
                cancel: cancel.clone(),
            });
            for &uid in &uids {
                st.notify(JobEvent::Started(uid));
            }
            drop(st);
            // A thread per walk rather than a lane per venue: the walk blocks on the worker's
            // reply for up to its trade deadline, and the number of them is bounded by the
            // exchange keys and `MAX_IN_FLIGHT`, never by the batch.
            let spawned = std::thread::Builder::new()
                .name("tuner-ticks-walk".into())
                .spawn(move || serve_cluster(job, rows, cancel, &defaults, flight_id));
            if let Err(error) = spawned {
                // No thread, no walk: the rows went with the closure. Counted as done so the
                // batch's total still balances, and said once.
                log::warn!(
                    "[x] ticks fetch: no thread for {} row(s): {error}",
                    uids.len()
                );
                let mut st = lock(job);
                st.detach_flight(flight_id);
                st.done += uids.len();
                st.notify(JobEvent::Progress);
            }
            continue;
        }
        if st.deferred.is_empty() && st.pending.is_empty() {
            // Nothing queued and nothing waiting: the batch is over once the walks out come
            // back, or none was ever started. Sleep until a start, an enqueue, or a walk's end.
            if st.in_flight.is_empty() {
                st.notify(JobEvent::Progress);
            }
            drop(
                job.wake
                    .wait(st)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        } else {
            // Rows queued but every one of them is on a busy key, or rows waiting out a
            // venue's backoff: wake at the earliest due time, or when a walk ends.
            let earliest = st.deferred.iter().map(|d| d.due).min().unwrap_or(now);
            let wait = match st.deferred.is_empty() {
                true => Duration::from_secs(60),
                false => earliest.saturating_duration_since(now),
            };
            drop(
                job.wake
                    .wait_timeout(st, wait)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        }
    }
}

/// Ask for one cluster of rows — one request over the hull of their windows — wait for the
/// worker, replay every row off the tiles, and file each answer. Runs on its own thread.
fn serve_cluster(
    job: &'static Job,
    rows: Vec<QueuedRow>,
    cancel: Arc<AtomicBool>,
    defaults: &HashMap<String, f64>,
    flight_id: u64,
) {
    if cancel.load(Ordering::Relaxed) || crate::backend::station::known_cached() {
        let mut st = lock(job);
        st.detach_flight(flight_id);
        st.notify(JobEvent::Progress);
        drop(st);
        job.wake.notify_all();
        return;
    }
    let uids: Vec<i64> = rows.iter().map(|r| r.deal.report_uid).collect();
    let first = &rows[0];
    // The hull: the first window's open (an entry order's creation, else a buy) to the last
    // exit, with the seed's margin — what `pick_cluster` kept within the long-position
    // threshold, so the worker walks it as one stretch.
    let first_open = rows
        .iter()
        .map(|r| r.window.open_ms)
        .min()
        .unwrap_or(first.window.open_ms);
    let last_close = rows
        .iter()
        .map(|r| r.deal.close_ms)
        .max()
        .unwrap_or(first.deal.close_ms);
    let window = replay_window_ms(first_open, last_close, first.window.margin_ms)
        .map(|hull| ReplayWindow {
            // The seed's threshold, not a fresh read: the hull was clustered by it, and the
            // walk and the post-walk check must split it the same way.
            long_position_ms: first.window.long_position_ms,
            ..hull
        })
        .unwrap_or(first.window);
    let window = with_record_tail(
        window,
        moon_core::db::tuner::ticks::tail::record_tail_ms(
            super::super::tail::current_s(),
            moon_core::market::trade_replay::margin_ms(),
        ),
    );
    let (reply, rx) = mpsc::channel();
    let started = Instant::now();
    worker::request(TradeReplayRequest {
        address: first.replay_address.clone(),
        market: first.address.market.clone(),
        window,
        identity: fetch_identity(uids[0]),
        tick_value: first.tick_value,
        ticks: true,
        intent: ReplayIntent::Model,
        cancel: cancel.clone(),
        reply,
    });
    // The worker streams the candle stage, then the tick stage, then drops the sender: the
    // last outcome before the drop is the tick stage's word. A model's request arms no archive
    // follow-up, so the drop comes right after the stage.
    let mut status = TickStatus::Failed;
    // What the walk's answer covers of the request's focus — the tiles it found plus what it
    // fetched. Short of the focus, the walk stopped on its own budget or deadline, and the
    // rows it did not reach may be worth a continuation; wider than the last answer, it gained.
    let mut walk_covered = Coverage::none();
    let mut outcomes = 0usize;
    while let Ok(outcome) = rx.recv() {
        outcomes += 1;
        status = match outcome {
            TradeReplayOutcome::Ready(series) => {
                walk_covered = series.covered.clone();
                series.tick_status
            }
            // The candle stage refused by the gate: the same wait as a refused walk.
            TradeReplayOutcome::Failed(TradeReplayFailure::RateLimited { retry_in_s }) => {
                TickStatus::RateLimited { retry_in_s }
            }
            // A venue this build has no route to was never asked, and says so; any other
            // empty — no bars in the window at all — is a final word, not a refusal to ask
            // again for.
            TradeReplayOutcome::Empty(TradeReplayEmpty::NoEndpoint { .. }) => TickStatus::NoRoute,
            TradeReplayOutcome::Empty(_) => TickStatus::NoTrades,
            TradeReplayOutcome::Failed(_) => TickStatus::Failed,
        };
    }
    let answered = started.elapsed();
    if cancel.load(Ordering::Relaxed) {
        // Stopped mid-walk: nothing to file. An autoload cancel has already taken this flight
        // off the books and unmarked its rows. Matching the
        // flight id leaves a newer flight for the same rows alone.
        let mut st = lock(job);
        let still = st.detach_flight(flight_id);
        if still {
            st.notify(JobEvent::Progress);
        }
        drop(st);
        job.wake.notify_all();
        return;
    }
    let cluster = rows.len();
    let market = first.address.market.clone();
    let (from_ms, to_ms) = (window.from_ms, window.to_ms);
    let walk_short = !walk_covered.covers(&window.focus_spans());
    let covered_ms = walk_covered.width_ms();
    // Every row on its own off the tiles: the walk's one answer says how the venue behaved,
    // the coverage says which rows it reached.
    let admission = super::super::tail::Admission::current();
    // One read of the strategies for the cluster, shared by its rows.
    let reads = super::super::PassReads::new();
    let mut answers = Vec::with_capacity(cluster);
    for (position, mut row) in rows.into_iter().enumerate() {
        let uid = row.deal.report_uid;
        let replayed_at = Instant::now();
        let lines = archived_lines_of(&row.deal);
        let mut answer = DealRow {
            deal: row.deal.clone(),
            tape: TapeStatus::Missing,
            verdict: None,
            address: Some(row.address.clone()),
            ticks: None,
            entry_line: None,
            held: None,
        };
        replay_row(
            &mut answer,
            defaults,
            super::super::model_cfg::current(),
            lines,
            row.window.long_position_ms,
            row.replay_address.cache.as_ref(),
            Some(&reads),
        );
        // Pad coverage reproduces the fact but does not finish a requested short-tail refill.
        // Keep the existing retry/continuation budget until that tail reaches the sample floor.
        let pending_tape = if answer.reproduced()
            && !admission.holds(answer.held)
            && !answer.short_tail_with(&admission)
        {
            TapeStatus::Missing
        } else {
            answer.tape
        };
        let mut wait = retry_wait(status, pending_tape).map(|s| Duration::from_secs(u64::from(s)));
        // Back to the end of the venue's turn, for the next walk to continue from where the
        // tiles end — see [`continues`]; the ceiling, no gain, or any other word is final.
        let continue_walk = wait.is_none()
            && continues(
                status,
                pending_tape,
                walk_short,
                covered_ms,
                row.covered_ms,
                row.continued,
            );
        if continue_walk {
            row.continued += 1;
            row.covered_ms = covered_ms;
        }
        // The venue itself refused mid-walk — the row that put its host into the backoff.
        // Once more, after the wait the gate will name for the rows behind it; the second
        // time is final.
        if wait.is_none()
            && pending_tape == TapeStatus::Missing
            && status == TickStatus::Failed
            && !row.retried
        {
            row.retried = true;
            wait = Some(VENUE_REFUSAL_WAIT);
        }
        answer.tape = final_tape(
            answer.tape,
            pending_tape,
            status,
            wait.is_some() || continue_walk,
        );
        let replayed_ms = replayed_at.elapsed().as_millis();
        let outcome_text = match (wait, continue_walk) {
            (Some(wait), _) => format!("retry in {} s", wait.as_secs()),
            (None, true) => format!(
                "continue {}/{MAX_CONTINUATIONS}, {covered_ms} ms of the focus covered",
                row.continued
            ),
            (None, false) => format!("{:?}", answer.tape),
        };
        let mut st = lock(job);
        // Cancellation removes this flight before any subsequent continuation can be queued.
        let still = st.in_flight.iter().any(|flight| flight.id == flight_id);
        if still {
            if let Some(wait) = wait {
                st.defer(row, wait);
            } else if continue_walk {
                st.pending.insert(0, row);
            } else {
                st.done += 1;
                st.finished.push((uid, Instant::now()));
            }
        }
        // One line per row, so a batch that looks stuck can be read instead of guessed: what
        // the worker answered for the cluster, how long the walk took, and what the row
        // became. The count is the batch's as of THIS answer — walks run in parallel, so a
        // number taken at dispatch would repeat. The binary logs at `warn` by default; this
        // target is the one the base filter raises for exactly these lines.
        log::info!(
            target: moon_core::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks fetch {}/{} {market} uid={uid} cluster={}/{cluster} window={from_ms}..{to_ms}: {status:?} after {outcomes} outcome(s) in {} ms, replayed in {replayed_ms} ms -> {outcome_text}",
            st.done,
            st.total,
            position + 1,
            answered.as_millis(),
        );
        if still {
            answers.push(answer);
            if position + 1 == cluster {
                st.finish_flight(flight_id, std::mem::take(&mut answers));
            }
        }
        drop(st);
    }
    // The key is free again, or the batch is over: the dispatcher decides which.
    job.wake.notify_all();
}

/// ReplayWindow is symmetric: admit the tail while preserving the original outer lead bound.
fn with_record_tail(window: ReplayWindow, tail_ms: i64) -> ReplayWindow {
    ReplayWindow {
        margin_ms: window.margin_ms.max(tail_ms),
        to_ms: window.to_ms.max(window.close_ms.saturating_add(tail_ms)),
        ..window
    }
}

/// A replay identity for a fetch, distinct from every chart window's: the row's own id, which
/// no window uses as a series discriminator.
fn fetch_identity(report_uid: i64) -> u64 {
    // FNV-1a over the id, salted so a window's `identity` (an entity number) cannot collide.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325 ^ 0x7469_636b_7300_0000;
    for byte in report_uid.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash | 1
}

/// The archived lines of one deal.
fn archived_lines_of(deal: &Deal) -> ArchivedLines {
    read_many(deal.core_uid, &[deal.report_uid])
        .ok()
        .and_then(|entries| entries.get(&deal.report_uid).map(ArchivedLines::of))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
