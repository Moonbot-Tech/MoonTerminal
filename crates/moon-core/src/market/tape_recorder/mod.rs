//! Tape recorder: the station's request-only tape (`docs-internal/STATION.md` §4.3), built and
//! measured inside the terminal first (phase T5).
//!
//! # Why a client of its own
//!
//! The terminal already holds every trade's prints in its live rings and files them at the close
//! (`trade_replay::worker::capture`). The station will have no live rings: it can only ask a core
//! for its chart archive around each trade. Whether that is enough is what this measures — and it
//! cannot ask through the terminal's own client, because a second `request_chart` there REPLACES
//! the overlap in the live ring with the coarsened archive (moonproto `docs/trades.md`), spoiling
//! both the capture and the reference it is to be compared with. So the recorder runs one extra
//! client per exchange — a donor, any core of that exchange, with the same key — in the station's
//! mode ([`donor`]).
//!
//! # What it does
//!
//! Every trade the terminal sees open or close becomes a task of its key `(exchange, market)`
//! ([`plan`]): ask at the open, poll while the position may still be short, ask at the close and
//! poll until the trail is settled. Each answer is filed into `tape_recorder.sqlite` — the layout
//! of `trades.sqlite`, a second [`TradeCache`] on its own file — and logged to
//! `logs/tape_recorder.log`, with a summary line every minute.
//!
//! # The switch
//!
//! `channels.tape_recorder` in `cfg/diagnostics.toml`, off by default and re-read live. It is the
//! one key in that file that changes what the program does: on, it opens a connection to one core
//! per exchange. It lives there because it is a measuring instrument, switched by the developer for
//! a run and off again, not a setting a user keeps. Off, nothing is connected and the trade events
//! are not even queued; switched off while running, the donors disconnect and the tasks are dropped.
//!
//! Nothing here depends on the GUI, so it moves into the station crate as it is.

mod donor;
mod plan;

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use donor::{Donor, Reply};
use plan::KeyTask;

use crate::config::ServerConfig;
use crate::feed::Tick;
use crate::market::trade_replay::tick_tiles::TileSource;
use crate::market::trade_replay::trade_cache::TradeCache;
use crate::session::CoreId;

/// The recorder's log file under `logs/`.
const LOG_FILE: &str = "tape_recorder.log";
/// How often the running recorder looks at its donors and its schedule.
const TICK: Duration = Duration::from_millis(250);
/// How often a switched-off recorder looks at the switch.
const IDLE_TICK: Duration = Duration::from_secs(2);
/// Archive requests in flight across every donor — each is a multi-megabyte transfer.
const MAX_IN_FLIGHT: usize = 4;
/// How long a core whose donor never finished Init is left alone.
const SHUN: Duration = Duration::from_secs(300);
/// A donor with nothing asked for this long disconnects.
const DONOR_IDLE: Duration = Duration::from_secs(600);
/// An open trade whose exit never arrived is forgotten after this long.
const OPEN_HORIZON_MS: i64 = 48 * 3_600_000;
/// How often the summary line is written.
const SUMMARY_EVERY: Duration = Duration::from_secs(60);

/// Whether the recorder is switched on (`channels.tape_recorder`).
pub fn enabled() -> bool {
    crate::diagnostics::tape_recorder()
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
        open_ms: i64,
    },
    Closed {
        exchange: String,
        market: String,
        open_ms: i64,
        close_ms: i64,
    },
    /// A core's connection went away or was replaced: a donor on it must not be used again.
    CoreGone(CoreId),
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
pub fn trade_opened(exchange_key: &str, market: &str, open_ms: i64) {
    if enabled() {
        send(Cmd::Opened {
            exchange: exchange_key.to_string(),
            market: market.to_string(),
            open_ms,
        });
    }
}

/// A trade closed on `exchange_key`/`market` (true UTC).
pub fn trade_closed(exchange_key: &str, market: &str, open_ms: i64, close_ms: i64) {
    if enabled() {
        send(Cmd::Closed {
            exchange: exchange_key.to_string(),
            market: market.to_string(),
            open_ms,
            close_ms,
        });
    }
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
    requests: u64,
    answers: u64,
    failed: u64,
    prints: u64,
    spans: u64,
    gaps: u64,
    lost_ms: i64,
}

struct Recorder {
    cache: TradeCache,
    tasks: HashMap<(String, String), KeyTask>,
    donors: HashMap<String, Donor>,
    shunned: HashMap<CoreId, Instant>,
    stats: Stats,
    last_summary: Instant,
    replies: Vec<Reply>,
}

impl Recorder {
    fn start() -> Option<Self> {
        let path = crate::config::paths::tape_recorder_db_path();
        let Some(cache) = TradeCache::open(path.clone()) else {
            line("not started: the database thread could not be spawned");
            return None;
        };
        line(&format!("started, writing {}", path.display()));
        Some(Self {
            cache,
            tasks: HashMap::new(),
            donors: HashMap::new(),
            shunned: HashMap::new(),
            stats: Stats::default(),
            last_summary: Instant::now(),
            replies: Vec::new(),
        })
    }

    fn apply(&mut self, cmd: Cmd, now_ms: i64) {
        let margin = crate::market::trade_replay::margin_ms();
        let long = crate::market::trade_replay::long_position_ms();
        match cmd {
            Cmd::Opened {
                exchange,
                market,
                open_ms,
            } => {
                line(&format!("open {exchange} {market} at {open_ms}"));
                self.tasks
                    .entry((exchange, market))
                    .or_insert_with(KeyTask::new)
                    .opened(now_ms, open_ms, margin, long);
            }
            Cmd::Closed {
                exchange,
                market,
                open_ms,
                close_ms,
            } => {
                line(&format!("close {exchange} {market} {open_ms}..{close_ms}"));
                self.tasks
                    .entry((exchange, market))
                    .or_insert_with(KeyTask::new)
                    .closed(now_ms, open_ms, close_ms, margin, long);
            }
            Cmd::CoreGone(core) => {
                let gone: Vec<String> = self
                    .donors
                    .iter()
                    .filter(|(_, d)| d.core == core)
                    .map(|(exchange, _)| exchange.clone())
                    .collect();
                for exchange in gone {
                    self.drop_donor(&exchange, now_ms, "its core went away");
                }
            }
        }
    }

    fn tick(&mut self, now: Instant, now_ms: i64) {
        self.collect_replies(now, now_ms);
        self.expire(now, now_ms);
        self.ask_due(now, now_ms);
        for task in self.tasks.values_mut() {
            task.drop_stale_opens(now_ms - OPEN_HORIZON_MS);
        }
        self.tasks.retain(|_, task| !task.is_done());
        let idle: Vec<String> = self
            .donors
            .iter()
            .filter(|(exchange, d)| {
                d.in_flight() == 0
                    && now.duration_since(d.last_used) > DONOR_IDLE
                    && !self.tasks.keys().any(|(e, _)| e == *exchange)
            })
            .map(|(exchange, _)| exchange.clone())
            .collect();
        for exchange in idle {
            self.drop_donor(&exchange, now_ms, "idle");
        }
        if now.duration_since(self.last_summary) >= SUMMARY_EVERY {
            self.last_summary = now;
            self.summary("summary");
        }
    }

    /// Drain every donor and file what came back.
    fn collect_replies(&mut self, now: Instant, now_ms: i64) {
        let exchanges: Vec<String> = self.donors.keys().cloned().collect();
        for exchange in exchanges {
            let mut replies = std::mem::take(&mut self.replies);
            let Some(donor) = self.donors.get_mut(&exchange) else {
                continue;
            };
            let was_ready = donor.is_ready();
            donor.pump(now, &mut replies);
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
        let (market, failure) = match reply {
            Reply::Ready(market) => (market, None),
            Reply::Failed(market, error) => (market, Some(error)),
        };
        // An answer to a pair already forgotten (expired, or asked by a previous selection).
        if !donor.is_asking(&market) {
            return;
        }
        let ticks = match failure {
            None => donor.copy_ring(&market),
            Some(_) => Vec::new(),
        };
        let core = donor.core;
        donor.forget(&market);
        let key = (exchange.to_string(), market);
        let Some(task) = self.tasks.get_mut(&key) else {
            return;
        };
        if let Some(error) = failure {
            self.stats.failed += 1;
            task.on_failure(now_ms);
            line(&format!("failed {} {}: {error}", key.0, key.1));
            return;
        }
        self.stats.answers += 1;
        let ring = ticks
            .first()
            .zip(ticks.last())
            .map(|(first, last)| (first.time_ms as i64, last.time_ms as i64));
        let filing = task.on_answer(now_ms, ring);
        let mut prints = 0usize;
        for &(from_ms, to_ms) in filing.file.spans() {
            let inside = slice(&ticks, from_ms, to_ms);
            prints += inside.len();
            self.cache
                .insert(&key.0, &key.1, from_ms, to_ms, inside, TileSource::Core);
        }
        self.stats.prints += prints as u64;
        self.stats.spans += filing.file.spans().len() as u64;
        self.stats.gaps += filing.lost.spans().len() as u64;
        self.stats.lost_ms += filing.lost.width_ms();
        let next = task.due_ms().map_or("done".to_string(), |due| {
            format!("{} s", (due - now_ms) / 1_000)
        });
        line(&format!(
            "answer {} {} core={}: ring {} prints {}..{} ({} s deep), filed {} ({prints} prints), lost {}, next {next}",
            key.0,
            key.1,
            crate::feed::core_label(core),
            ticks.len(),
            ring.map_or(0, |r| r.0),
            ring.map_or(0, |r| r.1),
            ring.map_or(0, |r| (r.1 - r.0) / 1_000),
            filing.file,
            filing.lost,
        ));
    }

    /// Forget pairs whose answer never came, and donors that never finished Init.
    fn expire(&mut self, now: Instant, now_ms: i64) {
        for (exchange, donor) in &mut self.donors {
            for market in donor.expired(now) {
                donor.forget(&market);
                self.stats.failed += 1;
                if let Some(task) = self.tasks.get_mut(&(exchange.clone(), market.clone())) {
                    task.on_failure(now_ms);
                }
                line(&format!("no answer {exchange} {market}, forgotten"));
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
            self.drop_donor(&exchange, now_ms, "Init did not finish");
        }
        self.shunned.retain(|_, at| now.duration_since(*at) < SHUN);
    }

    /// Ask every due key whose donor is ready, earliest first, within [`MAX_IN_FLIGHT`].
    fn ask_due(&mut self, now: Instant, now_ms: i64) {
        let mut due: Vec<(i64, (String, String))> = self
            .tasks
            .iter()
            .filter_map(|(key, task)| {
                task.due_ms()
                    .filter(|&d| d <= now_ms)
                    .map(|d| (d, key.clone()))
            })
            .collect();
        due.sort();
        for (_, key) in due {
            let in_flight: usize = self.donors.values().map(Donor::in_flight).sum();
            if in_flight >= MAX_IN_FLIGHT {
                return;
            }
            let (exchange, market) = &key;
            if !self.donors.contains_key(exchange) && !self.connect_donor(exchange, now) {
                continue;
            }
            let Some(donor) = self.donors.get_mut(exchange) else {
                continue;
            };
            if !donor.is_ready() || donor.is_asking(market) {
                continue;
            }
            self.stats.requests += 1;
            if let Err(e) = donor.ask(market, now) {
                self.stats.failed += 1;
                line(&format!("refused {exchange} {market}: {e}"));
                if let Some(task) = self.tasks.get_mut(&key) {
                    task.on_failure(now_ms);
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

    /// Disconnect a donor; whatever it was asked is asked again through the next one.
    fn drop_donor(&mut self, exchange: &str, now_ms: i64, why: &str) {
        let Some(donor) = self.donors.remove(exchange) else {
            return;
        };
        for market in donor.asked_markets() {
            if let Some(task) = self.tasks.get_mut(&(exchange.to_string(), market)) {
                task.on_failure(now_ms);
            }
        }
        line(&format!(
            "donor {exchange} core={}: dropped ({why})",
            crate::feed::core_label(donor.core)
        ));
    }

    fn summary(&self, what: &str) {
        let s = &self.stats;
        let ready = self.donors.values().filter(|d| d.is_ready()).count();
        let asking: usize = self.donors.values().map(Donor::in_flight).sum();
        line(&format!(
            "{what}: keys {} (asking {asking}), donors {} ({ready} ready), requests {}, answers {}, failed {}, filed {} prints in {} spans, lost {} s in {} gaps",
            self.tasks.len(),
            self.donors.len(),
            s.requests,
            s.answers,
            s.failed,
            s.prints,
            s.spans,
            s.lost_ms / 1_000,
            s.gaps,
        ));
    }
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
