//! Tape recorder: the station's tape around every trade (`docs-internal/STATION.md` §4.3), built
//! and measured inside the terminal first (phase T5).
//!
//! # Why a client of its own
//!
//! The terminal already holds every trade's prints in its live rings and files them at the close
//! (`trade_replay::worker::capture`). The station has no such rings: it selects a pair only for
//! as long as a trade of it needs recording. That cannot run through the terminal's own client —
//! its archive request would REPLACE the overlap in the live ring with the coarsened archive
//! (moonproto `docs/trades.md`), spoiling both the capture and the reference it is compared with.
//! So the recorder runs one extra client per exchange — a donor, any core of that exchange, with
//! the same key — in the station's mode ([`donor`]).
//!
//! # What it does
//!
//! Every trade the terminal sees open or close becomes a task of its key `(exchange, market)`
//! ([`plan`]). The recording follows moonproto's capture-station recipe ("Compact Capture
//! Stations"), its schedule set by the terminal's `[trade_replay]` window:
//!
//! - **entry** — the pair joins the donor's selection and its chart archive is asked ONCE: the
//!   run-up, as far back as the core still holds it;
//! - **position** — the pair stays selected and the donor's ring is drained by cursor every
//!   second; a ring that overwrote unread rows leaves an honest gap. No archive is asked again;
//! - **long position** — past `long_position_min` the pair is let go; at the exit it is selected
//!   again and the archive asked for the stretch before the exit;
//! - **exit** — the pair stays selected until `exit + margin_s` is filed, then is let go;
//! - trades of one key, from any core, share one selection until the last of them is filed.
//!
//! There is no cap on pairs and no cancelled recording. Each flush lands in
//! `tape_recorder.sqlite` — the layout of `trades.sqlite`, a second [`TradeCache`] on its own
//! file — and is logged to `logs/tape_recorder.log`, with a summary line every minute.
//!
//! # A station's restart and its disk
//!
//! A trade open while the station restarts reaches the new process only through the core's
//! open-row check, stamped with its old entry. The terminal takes such an open for no entry and
//! skips it; a station RESUMES it: what the file already holds of the trade counts as filed, the
//! rest is recorded from the core's archive and the live stream as for any trade, the part out of
//! reach named lost. The file keeps everything until the server runs short of space; then the
//! station asks for room ([`free_disk`]) and the prints filed longest ago go first, their pages
//! handed back to the disk.
//!
//! Once a closed trade is settled here, and the terminal's own close-time capture has had time to
//! land, the two tapes of it are read back and compared ([`compare`]): coverage, prints, volume and
//! the level touches the entry model fills on. The ring capture is the reference. A station has
//! no ring capture to compare with, so it never compares — nor opens `trades.sqlite` to find that
//! out.
//!
//! # The switch
//!
//! `channels.tape_recorder` in `cfg/diagnostics.toml`, off by default and re-read live. It is the
//! one key in that file that changes what the program does: on, it opens a connection to one core
//! per exchange. It lives there because it is a measuring instrument, switched by the developer for
//! a run and off again, not a setting a user keeps. Off, nothing is connected and the trade events
//! are not even queued; switched off while running, the donors disconnect and the tasks are dropped.
//!
//! The thread, once started, stays for the rest of the process: switched off it only reads the
//! switch every two seconds.
//!
//! A host whose whole job this is — the station — turns it on for good with [`set_always_on`],
//! whatever the file says.
//!
//! Nothing here depends on the GUI, so it moves into the station crate as it is.

mod compare;
mod donor;
mod peek;
mod plan;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use donor::{Donor, Reply};
pub use peek::{TapeMiss, TradeTape, trade_tape};
pub use plan::TradeId;
use plan::{Filing, KeyTask, QUIET_TAIL_MS};

use crate::config::ServerConfig;
use crate::feed::Tick;
use crate::market::trade_replay::Coverage;
use crate::market::trade_replay::tick_tiles::TileSource;
use crate::market::trade_replay::trade_cache::TradeCache;
use crate::session::CoreId;

/// The recorder's log file under `logs/`.
const LOG_FILE: &str = "tape_recorder.log";
/// How often the running recorder looks at its donors and its schedule.
const TICK: Duration = Duration::from_millis(250);
/// How often every recording pair's ring is drained.
const DRAIN_EVERY: Duration = Duration::from_secs(1);
/// How often a switched-off recorder looks at the switch.
const IDLE_TICK: Duration = Duration::from_secs(2);
/// Archive requests in flight across every donor. Not a cap on pairs — a selected pair records
/// its live rows while it waits — but on memory: an archive is unpacked at the core's size before
/// it is trimmed to the pair's ring (moonproto "Memory Estimate").
const MAX_IN_FLIGHT: usize = 4;
/// How long a core whose donor never finished Init is left alone.
const SHUN: Duration = Duration::from_secs(300);
/// A donor with nothing selected for this long disconnects.
const DONOR_IDLE: Duration = Duration::from_secs(600);
/// An open trade whose exit never arrived is forgotten after this long.
const OPEN_HORIZON_MS: i64 = 48 * 3_600_000;
/// An open older than this when it reaches the recorder is not an entry happening now: after a
/// (re)connect the core resends every open row (the open-row check), and the feed's tracker,
/// rebuilt with the connection, sees them for the first time.
const FRESH_OPEN_MS: i64 = 120_000;
/// How long a stopping recorder waits for its last writes.
const SHUTDOWN_SYNC: Duration = Duration::from_secs(5);
/// How often the summary line is written.
const SUMMARY_EVERY: Duration = Duration::from_secs(60);
/// How long after a trade settles here its comparison waits: the terminal's own capture files the
/// trail on its settle pass (`margin + 5 s` after the exit) through a queue of its own.
const COMPARE_DELAY: Duration = Duration::from_secs(30);

/// Set by a host that records always — the station — whatever the diagnostics file says.
static ALWAYS_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the recorder is switched on (`channels.tape_recorder`, or [`set_always_on`]).
pub fn enabled() -> bool {
    ALWAYS_ON.load(std::sync::atomic::Ordering::Relaxed) || crate::diagnostics::tape_recorder()
}

/// Record whatever `channels.tape_recorder` says: for a host whose whole job this is.
pub fn set_always_on() {
    ALWAYS_ON.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// A configured core the recorder may take as a donor.
struct Core {
    server: ServerConfig,
    /// `"{code}:{dex:08x}"`, once the core named its exchange.
    exchange: Option<String>,
}

/// Every running core, whether or not the recorder is on: switching it on must find them.
static CORES: Mutex<Vec<Core>> = Mutex::new(Vec::new());
/// The recorder thread's queue, started by the first event that finds the switch on.
static TX: OnceLock<Option<Sender<Cmd>>> = OnceLock::new();

enum Cmd {
    Opened {
        exchange: String,
        market: String,
        trade: TradeId,
        open_ms: i64,
    },
    Closed {
        exchange: String,
        market: String,
        trade: TradeId,
        open_ms: i64,
        close_ms: i64,
    },
    /// A core's connection went away or was replaced: a donor on it must not be used again.
    CoreGone(CoreId),
    /// The server is short of this many bytes: evict the oldest prints.
    FreeDisk(i64),
    /// The process is stopping: file what is drained, and answer once it is on disk.
    Shutdown(Sender<()>),
    /// What the recorder holds of a closed trade ([`trade_tape`]).
    Peek {
        trade: TradeId,
        lead_ms: i64,
        tail_ms: i64,
        reply: Sender<Result<TradeTape, TapeMiss>>,
    },
}

fn cores() -> std::sync::MutexGuard<'static, Vec<Core>> {
    CORES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A core's feed started (or restarted with a new configuration).
pub fn core_up(server: &ServerConfig) {
    let mut cores = cores();
    cores.retain(|c| c.server.id != server.id);
    cores.push(Core {
        server: server.clone(),
        exchange: None,
    });
    drop(cores);
    // A donor built from the previous configuration is stale.
    notify(Cmd::CoreGone(server.id));
}

/// A core named its exchange.
pub fn core_exchange(id: CoreId, exchange_key: String) {
    if let Some(core) = cores().iter_mut().find(|c| c.server.id == id) {
        core.exchange = Some(exchange_key);
    }
}

/// A core was removed or deactivated.
pub fn core_down(id: CoreId) {
    cores().retain(|c| c.server.id != id);
    notify(Cmd::CoreGone(id));
}

/// A trade opened on `exchange_key`/`market` at `open_ms` (true UTC).
pub fn trade_opened(exchange_key: &str, market: &str, trade: TradeId, open_ms: i64) {
    if enabled() {
        send(Cmd::Opened {
            exchange: exchange_key.to_string(),
            market: market.to_string(),
            trade,
            open_ms,
        });
    }
}

/// A trade closed on `exchange_key`/`market` (true UTC).
pub fn trade_closed(exchange_key: &str, market: &str, trade: TradeId, open_ms: i64, close_ms: i64) {
    if enabled() {
        send(Cmd::Closed {
            exchange: exchange_key.to_string(),
            market: market.to_string(),
            trade,
            open_ms,
            close_ms,
        });
    }
}

/// The server is short of `bytes` of free space (a station's own reckoning): the prints filed
/// longest ago leave the file until that much is gone, and the file shrinks by it. Starts the
/// recorder if it is not running yet — a file from before the restart is on the disk all the same.
pub fn free_disk(bytes: i64) {
    if enabled() && bytes > 0 {
        send(Cmd::FreeDisk(bytes));
    }
}

/// Stop the recorder for good, as the process exits: every recording files what it drained up to
/// its stream's frontier, the donors disconnect, the summary is written, and the call returns once
/// the file has taken it — or after `timeout`. A recorder that never started returns at once.
///
/// Returns:
///     Whether everything was on disk in time.
pub fn shutdown(timeout: Duration) -> bool {
    let Some(Some(tx)) = TX.get() else {
        return true;
    };
    let (reply, done) = mpsc::channel();
    tx.send(Cmd::Shutdown(reply)).is_ok() && done.recv_timeout(timeout).is_ok()
}

/// Send to a thread that is already running; never starts one.
fn notify(cmd: Cmd) {
    if let Some(Some(tx)) = TX.get() {
        let _ = tx.send(cmd);
    }
}

/// Send, starting the thread on first use.
fn send(cmd: Cmd) {
    let tx = TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("tape-recorder".into())
            .spawn(move || run(&rx))
            .map_err(|e| log::warn!("tape recorder thread not started: {e}"))
            .ok()?;
        Some(tx)
    });
    if let Some(tx) = tx {
        let _ = tx.send(cmd);
    }
}

fn line(msg: &str) {
    crate::diagnostics::stamped_line(LOG_FILE, msg);
}

fn run(rx: &Receiver<Cmd>) {
    let mut recorder: Option<Recorder> = None;
    let mut pending = Vec::new();
    loop {
        let wait = if recorder.is_some() { TICK } else { IDLE_TICK };
        match rx.recv_timeout(wait) {
            Ok(cmd) => pending.push(cmd),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        pending.extend(rx.try_iter());
        if let Some(at) = pending.iter().position(|c| matches!(c, Cmd::Shutdown(_))) {
            let Cmd::Shutdown(reply) = pending.remove(at) else {
                return;
            };
            if let Some(mut rec) = recorder.take() {
                // What arrived in the same wake still counts: a trade that closed as the stop was
                // asked is filed with its close.
                let now_ms = crate::util::now_unix_ms_i64();
                for cmd in pending.drain(..) {
                    rec.apply(cmd, now_ms);
                }
                rec.shutdown();
            }
            let _ = reply.send(());
            return;
        }
        if !enabled() {
            if let Some(stopped) = recorder.take() {
                stopped.summary("stopped");
            }
            pending.clear();
            continue;
        }
        let rec = match recorder.as_mut() {
            Some(rec) => rec,
            None => match Recorder::start() {
                Some(rec) => recorder.insert(rec),
                None => {
                    pending.clear();
                    continue;
                }
            },
        };
        let now_ms = crate::util::now_unix_ms_i64();
        for cmd in pending.drain(..) {
            rec.apply(cmd, now_ms);
        }
        rec.tick(Instant::now(), now_ms);
    }
}

/// Counters for the summary line.
#[derive(Default)]
struct Stats {
    /// Archives asked, answered, failed or never answered.
    requests: u64,
    answers: u64,
    failed: u64,
    prints: u64,
    spans: u64,
    gaps: u64,
    lost_ms: i64,
    /// Drains that found unread rows overwritten.
    clipped: u64,
    /// Prints that arrived stamped behind what was already filed.
    late: u64,
    /// Recordings whose stream broke and were seeded again.
    breaks: u64,
    /// Opens that were no entry: resent open rows after a (re)connect.
    stale_opens: u64,
    /// Open trades a station took up again after its own restart (resent open rows).
    resumed: u64,
    /// Settled trades compared with the ring capture, and their sums.
    compared: u64,
    needed_ms: i64,
    recorded_ms: i64,
    captured_ms: i64,
    common_ms: i64,
    touches: compare::Touches,
}

/// A settled trade waiting for the terminal's capture to land before it is compared.
struct PendingCompare {
    due: Instant,
    exchange: String,
    market: String,
    open_ms: i64,
    close_ms: i64,
    needed: Coverage,
}

type Key = (String, String);

struct Recorder {
    cache: TradeCache,
    tasks: HashMap<Key, KeyTask>,
    donors: HashMap<String, Donor>,
    shunned: HashMap<CoreId, Instant>,
    stats: Stats,
    last_summary: Instant,
    next_drain: Instant,
    replies: Vec<Reply>,
    ticks: Vec<Tick>,
    compares: Vec<PendingCompare>,
    /// Closes a reader may still ask about ([`trade_tape`]).
    closed: peek::ClosedTrades,
}

impl Recorder {
    fn start() -> Option<Self> {
        let path = crate::config::paths::tape_recorder_db_path();
        // No ceiling — not the Storage tab's `max_mb`, which is for `trades.sqlite` and would
        // evict spans before they are compared: kept whole until a station runs short of disk
        // ([`free_disk`]), then shrunk by what it must give back.
        let Some(cache) = TradeCache::open_evictable(path.clone()) else {
            line("not started: the database thread could not be spawned");
            return None;
        };
        line(&format!("started, writing {}", path.display()));
        let now = Instant::now();
        Some(Self {
            cache,
            tasks: HashMap::new(),
            donors: HashMap::new(),
            shunned: HashMap::new(),
            stats: Stats::default(),
            last_summary: now,
            next_drain: now,
            replies: Vec::new(),
            ticks: Vec::new(),
            compares: Vec::new(),
            closed: peek::ClosedTrades::default(),
        })
    }

    fn apply(&mut self, cmd: Cmd, now_ms: i64) {
        let margin = crate::market::trade_replay::margin_ms();
        let long = crate::market::trade_replay::long_position_ms();
        match cmd {
            Cmd::Opened {
                exchange,
                market,
                trade,
                open_ms,
            } => {
                let key = (exchange, market);
                let station = crate::feed::station::enabled();
                let known = self.tasks.get(&key).is_some_and(|t| t.knows(trade));
                let verdict = open_verdict(station, now_ms - open_ms, known);
                if verdict == OpenVerdict::Skip {
                    // Hundreds at every (re)connect: counted, not logged one by one.
                    self.stats.stale_opens += 1;
                    return;
                }
                let what = match verdict {
                    OpenVerdict::Resume => {
                        self.stats.resumed += 1;
                        "resume"
                    }
                    _ => "open",
                };
                line(&format!("{what} {} {} at {open_ms}", key.0, key.1));
                // What the file already holds of a trade open across the station's restart is
                // not recorded twice.
                if verdict == OpenVerdict::Resume {
                    self.prime(&key, open_ms.saturating_sub(margin), now_ms);
                }
                self.tasks
                    .entry(key)
                    .or_insert_with(KeyTask::new)
                    .opened(trade, open_ms, margin, long);
            }
            Cmd::Closed {
                exchange,
                market,
                trade,
                open_ms,
                close_ms,
            } => {
                line(&format!("close {exchange} {market} {open_ms}..{close_ms}"));
                let key = (exchange, market);
                self.closed.closed(trade, &key, open_ms, close_ms);
                // A station's close of a trade it never saw open — it opened before a restart and
                // its resent open did not come first: what the file holds of it counts as filed.
                let known = self.tasks.get(&key).is_some_and(|t| t.knows(trade));
                if crate::feed::station::enabled() && !known {
                    self.prime(
                        &key,
                        open_ms.saturating_sub(margin),
                        close_ms.saturating_add(margin),
                    );
                }
                self.tasks
                    .entry(key)
                    .or_insert_with(KeyTask::new)
                    .closed(trade, open_ms, close_ms, margin, long);
            }
            // Taken by `run` before a command reaches here.
            Cmd::Shutdown(_) => {}
            Cmd::Peek {
                trade,
                lead_ms,
                tail_ms,
                reply,
            } => {
                let _ = reply.send(self.peek(trade, lead_ms, tail_ms));
            }
            Cmd::FreeDisk(bytes) => {
                line(&format!(
                    "the server is short of {} KB: the oldest prints go",
                    bytes / 1_000
                ));
                self.cache.evict_oldest(bytes);
            }
            Cmd::CoreGone(core) => {
                let gone: Vec<String> = self
                    .donors
                    .iter()
                    .filter(|(_, d)| d.core == core)
                    .map(|(exchange, _)| exchange.clone())
                    .collect();
                for exchange in gone {
                    self.drop_donor(&exchange, "its core went away");
                }
            }
        }
    }

    /// Count as filed what the file already holds of `key` over `[from_ms, to_ms]`. A read the
    /// worker did not answer in time leaves nothing counted: the stretch is recorded again, and
    /// the file keeps only what it does not already hold.
    fn prime(&mut self, key: &Key, from_ms: i64, to_ms: i64) {
        let held = self.cache.held_spans(&key.0, &key.1, from_ms, to_ms);
        match &held {
            Some(spans) if !spans.is_empty() => line(&format!(
                "{} {}: {} span(s) already in the file",
                key.0,
                key.1,
                spans.len()
            )),
            Some(_) => {}
            None => line(&format!(
                "{} {}: the file did not answer what it holds; recorded again where held",
                key.0, key.1
            )),
        }
        if let Some(spans) = held {
            self.tasks
                .entry(key.clone())
                .or_insert_with(KeyTask::new)
                .filed_before(&spans);
        }
    }

    fn tick(&mut self, now: Instant, now_ms: i64) {
        self.collect_replies(now, now_ms);
        self.expire(now);
        self.closed.prune(now);
        if now >= self.next_drain {
            self.next_drain = now + DRAIN_EVERY;
            self.drain();
        }
        self.reconcile(now, now_ms);
        let compares = !crate::feed::station::enabled();
        for (key, task) in &mut self.tasks {
            task.drop_stale_opens(now_ms - OPEN_HORIZON_MS);
            if task.overdue(now_ms) {
                let lost = task.give_up();
                note_lost(&mut self.stats, key, &lost, "gave up");
            }
            let settled = task.take_settled();
            if !compares {
                continue;
            }
            for (open_ms, close_ms, needed) in settled {
                self.compares.push(PendingCompare {
                    due: now + COMPARE_DELAY,
                    exchange: key.0.clone(),
                    market: key.1.clone(),
                    open_ms,
                    close_ms,
                    needed,
                });
            }
        }
        self.run_compares(now);
        // A key done here is dropped from its donor's selection by the next reconcile.
        self.tasks.retain(|_, task| !task.is_done());
        let idle: Vec<String> = self
            .donors
            .iter()
            .filter(|(exchange, d)| {
                d.selected() == 0
                    && now.duration_since(d.last_used) > DONOR_IDLE
                    && !self.tasks.keys().any(|(e, _)| e == *exchange)
            })
            .map(|(exchange, _)| exchange.clone())
            .collect();
        for exchange in idle {
            self.drop_donor(&exchange, "idle");
        }
        if now.duration_since(self.last_summary) >= SUMMARY_EVERY {
            self.last_summary = now;
            self.summary("summary");
        }
    }

    /// Pump every donor and seed what its archive answers brought.
    fn collect_replies(&mut self, now: Instant, now_ms: i64) {
        let exchanges: Vec<String> = self.donors.keys().cloned().collect();
        for exchange in exchanges {
            let mut replies = std::mem::take(&mut self.replies);
            let Some(donor) = self.donors.get_mut(&exchange) else {
                continue;
            };
            let was_ready = donor.is_ready();
            donor.pump(now, now_ms, &mut replies);
            if let Some(took) = donor.init_took().filter(|_| !was_ready) {
                line(&format!(
                    "donor {exchange} core={} ready after {} ms",
                    crate::feed::core_label(donor.core),
                    took.as_millis()
                ));
            }
            for reply in replies.drain(..) {
                self.on_reply(&exchange, reply, now_ms);
            }
            self.replies = replies;
        }
    }

    fn on_reply(&mut self, exchange: &str, reply: Reply, now_ms: i64) {
        let Some(donor) = self.donors.get_mut(exchange) else {
            return;
        };
        let market = match &reply {
            Reply::Ready(market) | Reply::Failed(market, _) => market.clone(),
        };
        let key = (exchange.to_string(), market);
        // An answer for a pair let go in the meantime.
        let Some(task) = self.tasks.get_mut(&key).filter(|t| t.wants_pair()) else {
            return;
        };
        match reply {
            Reply::Ready(market) => {
                self.stats.answers += 1;
                // A recording already running files what it drained before the ring is read
                // again from its oldest row: the new ring may no longer reach back that far.
                let alive_ms = task.live_epoch().and_then(|epoch| donor.alive_ms(epoch));
                if let Some(filing) = flush_to(task, alive_ms, true) {
                    file(&self.cache, &mut self.stats, &key, filing);
                }
                donor.restart(&market);
                let mut ticks = std::mem::take(&mut self.ticks);
                ticks.clear();
                // Overwritten while it was read: the recording starts where the read resumed.
                if let Some(resumed_ms) = donor.drain(&market, &mut ticks).resumed_ms {
                    self.stats.clipped += 1;
                    ticks.retain(|t| t.time_ms as i64 >= resumed_ms);
                }
                // An answer nobody waits for any more — moonproto's own retry of an ask that
                // timed out here — still rewrote the ring: it seeds the recording again.
                let (asked_ms, epoch) = task
                    .seed()
                    .map_or((now_ms, donor.epoch()), |s| (s.asked_ms, s.epoch));
                let oldest = ticks.iter().map(|t| t.time_ms as i64).min().unwrap_or(0);
                let newest = ticks.iter().map(|t| t.time_ms as i64).max().unwrap_or(0);
                let count = ticks.len();
                let lost = task.seeded(ticks, asked_ms, epoch);
                line(&format!(
                    "archive {} {} core={}: ring {count} prints {oldest}..{newest} ({} s deep), lost {lost}",
                    key.0,
                    key.1,
                    crate::feed::core_label(donor.core),
                    (newest - oldest) / 1_000,
                ));
                note_lost(&mut self.stats, &key, &lost, "");
            }
            Reply::Failed(market, error) => {
                self.stats.failed += 1;
                line(&format!("failed {} {}: {error}", key.0, key.1));
                if task.seed().is_some() {
                    let fresh = !task.is_live();
                    let lost = task.seed_failed();
                    if fresh {
                        donor.restart(&market);
                    }
                    note_lost(
                        &mut self.stats,
                        &key,
                        &lost,
                        "recording the live rows alone",
                    );
                }
            }
        }
    }

    /// Record archives that never came from the live rows alone, and drop donors that never
    /// finished Init.
    fn expire(&mut self, now: Instant) {
        for (exchange, donor) in &mut self.donors {
            for market in donor.expired(now) {
                self.stats.failed += 1;
                line(&format!("no answer {exchange} {market}"));
                let key = (exchange.clone(), market);
                if let Some(task) = self.tasks.get_mut(&key) {
                    let fresh = !task.is_live();
                    let lost = task.seed_failed();
                    if fresh {
                        donor.restart(&key.1);
                    }
                    note_lost(
                        &mut self.stats,
                        &key,
                        &lost,
                        "recording the live rows alone",
                    );
                }
            }
        }
        let stalled: Vec<(String, CoreId)> = self
            .donors
            .iter()
            .filter(|(_, d)| d.stalled(now))
            .map(|(exchange, d)| (exchange.clone(), d.core))
            .collect();
        for (exchange, core) in stalled {
            self.shunned.insert(core, now);
            self.drop_donor(&exchange, "Init did not finish");
        }
        self.shunned.retain(|_, at| now.duration_since(*at) < SHUN);
    }

    /// Drain every recording pair's ring into its key and file what the frontier passed.
    fn drain(&mut self) {
        for (key, task) in &mut self.tasks {
            let Some(epoch) = task.live_epoch() else {
                continue;
            };
            let Some(donor) = self.donors.get_mut(&key.0) else {
                task.stop();
                continue;
            };
            self.ticks.clear();
            let drained = donor.drain(&key.1, &mut self.ticks);
            if let Some(resumed_ms) = drained.resumed_ms {
                self.stats.clipped += 1;
                let lost = task.clipped(resumed_ms);
                note_lost(&mut self.stats, key, &lost, "ring overwrote unread rows");
            }
            self.stats.late += task.drained(&self.ticks) as u64;
            let broke = epoch != donor.epoch();
            if let Some(filing) = flush_to(task, donor.alive_ms(epoch), broke) {
                file(&self.cache, &mut self.stats, key, filing);
            }
            if broke {
                self.stats.breaks += 1;
                task.stop();
                line(&format!(
                    "stream broke {} {} core={}, seeding again",
                    key.0,
                    key.1,
                    crate::feed::core_label(donor.core)
                ));
            }
        }
    }

    /// Let go of the pairs nobody needs, keep every donor's selection the complete set still
    /// needed, and ask the archive for every pair that must be seeded.
    fn reconcile(&mut self, now: Instant, now_ms: i64) {
        let mut wanted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (key, task) in &mut self.tasks {
            if task.wants_pair() {
                wanted
                    .entry(key.0.clone())
                    .or_default()
                    .insert(key.1.clone());
                continue;
            }
            if !task.is_live() && task.seed().is_none() {
                continue;
            }
            let alive_ms = task
                .live_epoch()
                .and_then(|epoch| self.donors.get(&key.0)?.alive_ms(epoch));
            if let Some(filing) = flush_to(task, alive_ms, true) {
                file(&self.cache, &mut self.stats, key, filing);
            }
            task.stop();
            line(&format!("released {} {}", key.0, key.1));
        }
        for exchange in self.donors.keys() {
            wanted.entry(exchange.clone()).or_default();
        }
        let mut in_flight: usize = self.donors.values().map(Donor::in_flight).sum();
        for (exchange, markets) in wanted {
            if !markets.is_empty()
                && !self.donors.contains_key(&exchange)
                && !self.connect_donor(&exchange, now)
            {
                continue;
            }
            let Some(donor) = self.donors.get_mut(&exchange) else {
                continue;
            };
            if !donor.is_ready() {
                continue;
            }
            if let Err(e) = donor.select(&markets, now) {
                line(&format!("select {exchange} ({} pairs): {e}", markets.len()));
                continue;
            }
            for market in markets {
                if in_flight >= MAX_IN_FLIGHT {
                    break;
                }
                let key = (exchange.clone(), market);
                let Some(task) = self.tasks.get_mut(&key).filter(|t| t.needs_seed()) else {
                    continue;
                };
                self.stats.requests += 1;
                let fresh = !task.is_live();
                task.seed_asked(now_ms, donor.epoch());
                match donor.ask(&key.1, now) {
                    Ok(()) => {
                        in_flight += 1;
                        line(&format!(
                            "asked {} {} core={}{}",
                            key.0,
                            key.1,
                            crate::feed::core_label(donor.core),
                            if fresh {
                                ""
                            } else {
                                " again, for a need behind the frontier"
                            }
                        ));
                    }
                    Err(e) => {
                        self.stats.failed += 1;
                        line(&format!("refused {} {}: {e}", key.0, key.1));
                        let lost = task.seed_failed();
                        if fresh {
                            donor.restart(&key.1);
                        }
                        note_lost(
                            &mut self.stats,
                            &key,
                            &lost,
                            "recording the live rows alone",
                        );
                    }
                }
            }
        }
    }

    /// Connect a donor for `exchange` from the first core of it that is not shunned.
    fn connect_donor(&mut self, exchange: &str, now: Instant) -> bool {
        let server = cores()
            .iter()
            .filter(|c| c.exchange.as_deref() == Some(exchange))
            .map(|c| &c.server)
            .filter(|s| !self.shunned.contains_key(&s.id))
            .min_by_key(|s| s.id)
            .cloned();
        let Some(server) = server else {
            return false;
        };
        match Donor::connect(&server, now) {
            Ok(donor) => {
                line(&format!(
                    "donor {exchange} core={}: connecting",
                    crate::feed::core_label(server.id)
                ));
                self.donors.insert(exchange.to_string(), donor);
                true
            }
            Err(e) => {
                self.shunned.insert(server.id, now);
                line(&format!(
                    "donor {exchange} core={}: connect failed: {e:#}",
                    crate::feed::core_label(server.id)
                ));
                false
            }
        }
    }

    /// Disconnect a donor. Its recordings file what its stream brought and stop; the next donor
    /// of the exchange seeds them again.
    fn drop_donor(&mut self, exchange: &str, why: &str) {
        let Some(donor) = self.donors.remove(exchange) else {
            return;
        };
        for (key, task) in &mut self.tasks {
            if key.0 != exchange {
                continue;
            }
            let alive_ms = task.live_epoch().and_then(|epoch| donor.alive_ms(epoch));
            if let Some(filing) = flush_to(task, alive_ms, true) {
                file(&self.cache, &mut self.stats, key, filing);
            }
            task.stop();
        }
        line(&format!(
            "donor {exchange} core={}: dropped ({why})",
            crate::feed::core_label(donor.core)
        ));
    }

    /// File every recording up to its frontier, disconnect the donors, write the summary, and
    /// wait for the file to take it all.
    fn shutdown(&mut self) {
        let exchanges: Vec<String> = self.donors.keys().cloned().collect();
        for exchange in exchanges {
            self.drop_donor(&exchange, "stopping");
        }
        self.summary("stopped");
        if !self.cache.sync(SHUTDOWN_SYNC) {
            line("stopped before the last writes reached the file");
        }
    }

    /// Compare every settled trade whose delay ran out.
    fn run_compares(&mut self, now: Instant) {
        let (due, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.compares)
            .into_iter()
            .partition(|c| c.due <= now);
        self.compares = waiting;
        for pending in due {
            self.compare_one(&pending);
        }
    }

    /// Read one trade back from both files and log the verdict.
    fn compare_one(&mut self, p: &PendingCompare) {
        let Some((from_ms, to_ms)) = p.needed.hull() else {
            return;
        };
        let what = format!(
            "compare {} {} {}..{}",
            p.exchange, p.market, p.open_ms, p.close_ms
        );
        let Some(reference) = crate::market::trade_replay::trade_cache::handle() else {
            line(&format!(
                "{what}: trades.sqlite is switched off, nothing to compare with"
            ));
            return;
        };
        let (Some(recorded), Some(captured)) = (
            self.cache.read(&p.exchange, &p.market, from_ms, to_ms),
            reference.read(&p.exchange, &p.market, from_ms, to_ms),
        ) else {
            line(&format!("{what}: a read timed out, skipped"));
            return;
        };
        // The ring capture alone: a venue walk in the same file is another source altogether.
        let captured: Vec<_> = captured
            .into_iter()
            .filter(|s| s.source == TileSource::Core)
            .collect();
        let (recorded_cov, recorded_ticks) = flatten(recorded);
        let (captured_cov, captured_ticks) = flatten(captured);
        let c = compare::compare(
            &p.needed,
            compare::Tape {
                covered: &recorded_cov,
                ticks: &recorded_ticks,
            },
            compare::Tape {
                covered: &captured_cov,
                ticks: &captured_ticks,
            },
        );
        let s = &mut self.stats;
        s.compared += 1;
        s.needed_ms += c.needed_ms;
        s.recorded_ms += c.recorded_ms;
        s.captured_ms += c.captured_ms;
        s.common_ms += c.common_ms;
        s.touches.add(c.touches);
        let t = c.touches;
        line(&format!(
            "{what}: needed {} s, recorded {} s, ring {} s, both {} s; prints recorded {} / ring {}, volume {:.4} / {:.4}; touches both {} (same ms {}, <=100 ms {}), ring only {}, recorded only {}",
            c.needed_ms / 1_000,
            c.recorded_ms / 1_000,
            c.captured_ms / 1_000,
            c.common_ms / 1_000,
            c.recorded_prints,
            c.captured_prints,
            c.recorded_volume,
            c.captured_volume,
            t.both,
            t.same_ms,
            t.near,
            t.only_captured,
            t.only_recorded,
        ));
    }

    fn summary(&self, what: &str) {
        let s = &self.stats;
        let ready = self.donors.values().filter(|d| d.is_ready()).count();
        let pairs: usize = self.donors.values().map(Donor::selected).sum();
        let asking: usize = self.donors.values().map(Donor::in_flight).sum();
        let recording = self.tasks.values().filter(|t| t.is_live()).count();
        line(&format!(
            "{what}: keys {} (recording {recording}), pairs {pairs} (asking {asking}), donors {} ({ready} ready), requests {}, answers {}, failed {}, filed {} prints in {} spans, lost {} s in {} gaps, clipped {}, late {}, breaks {}, stale opens skipped {}, resumed {}",
            self.tasks.len(),
            self.donors.len(),
            s.requests,
            s.answers,
            s.failed,
            s.prints,
            s.spans,
            s.lost_ms / 1_000,
            s.gaps,
            s.clipped,
            s.late,
            s.breaks,
            s.stale_opens,
            s.resumed,
        ));
        if s.compared > 0 {
            let t = s.touches;
            line(&format!(
                "{what}: compared {} trades - needed {} s, recorded {} s, ring {} s, both {} s; touches both {} (same ms {}, <=100 ms {}), ring only {}, recorded only {}",
                s.compared,
                s.needed_ms / 1_000,
                s.recorded_ms / 1_000,
                s.captured_ms / 1_000,
                s.common_ms / 1_000,
                t.both,
                t.same_ms,
                t.near,
                t.only_captured,
                t.only_recorded,
            ));
        }
    }
}

/// What an open does to the recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenVerdict {
    /// An entry happening now — or a resend of a trade already recorded.
    Open,
    /// A station's trade open across its own restart: taken up again.
    Resume,
    /// No entry: a resent open row the terminal does not record, or one past the horizon.
    Skip,
}

/// The verdict on an open `age_ms` old when it reaches the recorder.
///
/// Args:
///     station: The process is a station, which resumes its trades after a restart.
///     age_ms: How long ago the trade opened.
///     known: The trade is already in its key's plan — a resend after a reconnect.
fn open_verdict(station: bool, age_ms: i64, known: bool) -> OpenVerdict {
    if age_ms <= FRESH_OPEN_MS {
        OpenVerdict::Open
    } else if station && !known && age_ms <= OPEN_HORIZON_MS {
        OpenVerdict::Resume
    } else {
        OpenVerdict::Skip
    }
}

/// Flush `task` up to [`QUIET_TAIL_MS`] behind its stream's last sign of life, if it has one.
fn flush_to(task: &mut KeyTask, alive_ms: Option<i64>, force: bool) -> Option<Filing> {
    task.flush(alive_ms? - QUIET_TAIL_MS, force)
}

/// Write one flush of `key` and log it.
fn file(cache: &TradeCache, stats: &mut Stats, key: &Key, filing: Filing) {
    if filing.file.is_empty() {
        return;
    }
    for &(from_ms, to_ms) in filing.file.spans() {
        let inside = slice(&filing.ticks, from_ms, to_ms);
        stats.prints += inside.len() as u64;
        cache.insert(&key.0, &key.1, from_ms, to_ms, inside, TileSource::Core);
    }
    stats.spans += filing.file.spans().len() as u64;
    line(&format!(
        "filed {} {} {} ({} prints)",
        key.0,
        key.1,
        filing.file,
        filing.ticks.len()
    ));
}

/// Count and log what a key just lost; nothing for an empty loss.
fn note_lost(stats: &mut Stats, key: &Key, lost: &Coverage, why: &str) {
    if lost.is_empty() {
        return;
    }
    stats.gaps += lost.spans().len() as u64;
    stats.lost_ms += lost.width_ms();
    let why = if why.is_empty() {
        String::new()
    } else {
        format!(" ({why})")
    };
    line(&format!("lost {} {} {lost}{why}", key.0, key.1));
}

/// Stored spans as one coverage and one ascending run of prints.
fn flatten(
    spans: Vec<crate::market::trade_replay::trade_cache::StoredSpan>,
) -> (Coverage, Vec<Tick>) {
    let mut covered = Coverage::none();
    let mut ticks = Vec::new();
    for span in spans {
        covered.add((span.from_ms, span.to_ms));
        ticks.extend(span.ticks);
    }
    ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
    (covered, ticks)
}

/// The prints of an ascending run inside `[from_ms, to_ms]`.
fn slice(ticks: &[Tick], from_ms: i64, to_ms: i64) -> Vec<Tick> {
    ticks
        .iter()
        .filter(|t| {
            let at = t.time_ms as i64;
            at >= from_ms && at <= to_ms
        })
        .copied()
        .collect()
}

#[cfg(test)]
mod tests;
