//! The tape fetch as a job of the PROCESS, not of the window.
//!
//! The Analytics window is created and dropped with its view; a batch that lived inside the view
//! died with it, and a batch asleep on a venue's backoff had nothing in flight to keep a scope
//! reload from wiping it. Here the queue, the walk order, the per-venue backoff waits and the
//! progress live on one background thread that outlives every window; a view only hands it rows
//! (resolved while the view still had the live source), listens for what lands, and reads the
//! progress for its caption. Closing the window drops the listener, nothing else; reopening it
//! attaches a new one and reads the same progress.
//!
//! One row at a time, because the replay worker is one thread and the venues are rate-limited.
//! A venue that refuses (the gate's backoff, or its own error mid-walk) puts its rows aside for
//! the gate's own number of seconds while the rows of other venues go on; the refused row goes
//! back first once the wait is out, and a row the venue itself refused is asked once more before
//! it counts as final.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
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
    ReplayIntent, ReplayWindow, TickStatus, TradeReplayEmpty, TradeReplayFailure,
    TradeReplayOutcome,
};

/// The wait after a venue's own refusal mid-walk, when the gate names no number: the gate's own
/// floor, so the second ask lands after the backoff it will have recorded.
const VENUE_REFUSAL_WAIT: Duration = Duration::from_secs(30);

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
}

impl QueuedRow {
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
        }
    }
}

/// What the job tells whoever listens.
pub(in crate::analytics::tuner) enum JobEvent {
    /// A request went out for this row.
    Started(i64),
    /// A row's answer: what the row became, tape and verdict included.
    Row(Box<DealRow>),
    /// Progress moved without a row answer — a deferral, a resume, the end of the batch.
    Progress,
}

/// The job as a caption reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::analytics::tuner) struct Progress {
    /// Whether a batch is running — in flight, queued, or asleep on a venue's wait.
    pub(in crate::analytics::tuner) active: bool,
    /// Rows answered since the batch started, and the batch's size.
    pub(in crate::analytics::tuner) done: usize,
    pub(in crate::analytics::tuner) total: usize,
    /// The row a request is out for: its id and its market.
    pub(in crate::analytics::tuner) in_flight: Option<(i64, String)>,
}

/// A row set aside until its venue's wait is out.
struct Deferred {
    row: QueuedRow,
    due: Instant,
}

#[derive(Default)]
struct State {
    /// Rows still to ask for; popped from the end, so the caller orders them newest-first.
    pending: Vec<QueuedRow>,
    deferred: Vec<Deferred>,
    in_flight: Option<(i64, String)>,
    /// The in-flight request's cancel flag, raised by a stop so the walk ends at once.
    in_flight_cancel: Option<Arc<AtomicBool>>,
    done: usize,
    total: usize,
    /// The strategy-field defaults the model runs with, captured when the batch started.
    defaults: HashMap<String, f64>,
    /// Raised by a stop; the thread empties the queue and idles.
    stop: bool,
    /// The window listening, if any; a dead receiver is the window gone, and is dropped.
    listener: Option<mpsc::Sender<JobEvent>>,
    /// When each row of the running batch was answered, for a reload that read the row before
    /// the answer landed to re-read it — see [`finished_after`].
    finished: Vec<(i64, Instant)>,
}

impl State {
    fn active(&self) -> bool {
        self.in_flight.is_some() || !self.pending.is_empty() || !self.deferred.is_empty()
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

/// Start a batch, unless one is running. `rows` is newest-first: the job pops from the end,
/// so the oldest — nearest the venues' retention edge — goes first.
///
/// Returns:
///     Whether the batch was taken.
pub(in crate::analytics::tuner) fn start(
    rows: Vec<QueuedRow>,
    defaults: HashMap<String, f64>,
) -> bool {
    if rows.is_empty() {
        return false;
    }
    ensure_thread();
    let job = job();
    let mut st = lock(job);
    if st.active() {
        return false;
    }
    st.total = rows.len();
    st.done = 0;
    st.pending = rows;
    st.deferred.clear();
    st.finished.clear();
    st.defaults = defaults;
    st.stop = false;
    st.notify(JobEvent::Progress);
    drop(st);
    job.wake.notify_all();
    true
}

/// Abandon the batch: the queue empties, the request in flight is cancelled and its answer is
/// dropped.
pub(in crate::analytics::tuner) fn stop() {
    let job = job();
    let mut st = lock(job);
    st.stop = true;
    st.pending.clear();
    st.deferred.clear();
    if let Some(cancel) = &st.in_flight_cancel {
        cancel.store(true, Ordering::Relaxed);
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

/// The job as the caption reads it.
pub(in crate::analytics::tuner) fn progress() -> Progress {
    let st = lock(job());
    Progress {
        active: st.active(),
        done: st.done,
        total: st.total,
        in_flight: st.in_flight.clone(),
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

/// Split a queue into the rows on `key` and the rest, both in their queue order.
fn split_by_key<T>(rows: Vec<T>, key: &str, key_of: impl Fn(&T) -> &str) -> (Vec<T>, Vec<T>) {
    rows.into_iter().partition(|row| key_of(row) == key)
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

/// The thread: one row at a time, waits included.
fn run(job: &Job) {
    loop {
        let (row, defaults) = {
            let mut st = lock(job);
            loop {
                if st.stop {
                    st.stop = false;
                    st.pending.clear();
                    st.deferred.clear();
                }
                let now = Instant::now();
                if st.resume_due(now) {
                    st.notify(JobEvent::Progress);
                }
                if let Some(row) = st.pending.pop() {
                    break (row, st.defaults.clone());
                }
                if st.deferred.is_empty() {
                    // The batch is over, or none was ever started: sleep until a start.
                    st.notify(JobEvent::Progress);
                    st = job
                        .wake
                        .wait(st)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                } else {
                    let earliest = st.deferred.iter().map(|d| d.due).min().unwrap_or(now);
                    let wait = earliest.saturating_duration_since(now);
                    st = job
                        .wake
                        .wait_timeout(st, wait)
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0;
                }
            }
        };
        serve_one(job, row, &defaults);
    }
}

/// Ask for one row, wait for the worker, replay the row off the tiles, and file the answer.
fn serve_one(job: &Job, mut row: QueuedRow, defaults: &HashMap<String, f64>) {
    let uid = row.deal.report_uid;
    let cancel = Arc::new(AtomicBool::new(false));
    let (reply, rx) = mpsc::channel();
    let progress = {
        let mut st = lock(job);
        st.in_flight = Some((uid, row.address.market.clone()));
        st.in_flight_cancel = Some(cancel.clone());
        st.notify(JobEvent::Started(uid));
        (st.done + 1, st.total)
    };
    let started = Instant::now();
    worker::request(TradeReplayRequest {
        address: row.replay_address.clone(),
        market: row.address.market.clone(),
        window: row.window,
        identity: fetch_identity(uid),
        tick_value: row.tick_value,
        ticks: true,
        intent: ReplayIntent::Model,
        cancel: cancel.clone(),
        reply,
    });
    // The worker streams the candle stage, then the tick stage, then drops the sender: the
    // last outcome before the drop is the tick stage's word. A model's request arms no archive
    // follow-up, so the drop comes right after the stage.
    let mut status = TickStatus::Failed;
    let mut outcomes = 0usize;
    while let Ok(outcome) = rx.recv() {
        outcomes += 1;
        status = match outcome {
            TradeReplayOutcome::Ready(series) => series.tick_status,
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
        // Stopped mid-walk: nothing to file, the queue is already empty.
        let mut st = lock(job);
        st.in_flight = None;
        st.in_flight_cancel = None;
        st.notify(JobEvent::Progress);
        return;
    }
    let lines = archived_lines_of(&row.deal);
    let mut answer = DealRow {
        deal: row.deal.clone(),
        tape: TapeStatus::Missing,
        verdict: None,
        address: Some(row.address.clone()),
        ticks: None,
        entry_start: None,
    };
    replay_row(&mut answer, defaults, lines);
    let mut wait = retry_wait(status, answer.tape).map(|s| Duration::from_secs(u64::from(s)));
    // The venue itself refused mid-walk — the row that put its host into the backoff. Once
    // more, after the wait the gate will name for the rows behind it; the second time is final.
    if wait.is_none()
        && answer.tape == TapeStatus::Missing
        && status == TickStatus::Failed
        && !row.retried
    {
        row.retried = true;
        wait = Some(VENUE_REFUSAL_WAIT);
    }
    if answer.tape == TapeStatus::Missing && wait.is_none() {
        answer.tape = match status {
            TickStatus::Served | TickStatus::Pending | TickStatus::Streaming => TapeStatus::Missing,
            refused => TapeStatus::Refused(refused),
        };
    }
    // One line per row, so a batch that looks stuck can be read instead of guessed: what the
    // worker answered, how long it took, and what the row became. The binary logs at `warn` by
    // default; this target is the one the base filter raises for exactly these lines.
    log::info!(
        target: moon_core::diagnostics::TICKS_AXIS_TARGET,
        "[x] ticks fetch {}/{} {} uid={} window={}..{}: {status:?} after {outcomes} outcome(s) in {} ms, replayed in {} ms -> {}",
        progress.0,
        progress.1,
        row.address.market,
        uid,
        row.window.from_ms,
        row.window.to_ms,
        answered.as_millis(),
        started.elapsed().saturating_sub(answered).as_millis(),
        match wait {
            Some(wait) => format!("retry in {} s", wait.as_secs()),
            None => format!("{:?}", answer.tape),
        }
    );
    let mut st = lock(job);
    st.in_flight = None;
    st.in_flight_cancel = None;
    match wait {
        Some(wait) if !st.stop => {
            st.defer(row, wait);
            st.notify(JobEvent::Row(Box::new(answer)));
        }
        _ => {
            st.done += 1;
            st.finished.push((uid, Instant::now()));
            st.notify(JobEvent::Row(Box::new(answer)));
        }
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
