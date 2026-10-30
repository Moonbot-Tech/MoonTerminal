//! Price recorder: the market list's prices — bid, ask, mark and funding — around every trade,
//! filed into `market_prices.sqlite` by one mechanism on the terminal and on the station.
//!
//! # Why a recorder of its own
//!
//! The core sends its market list about every two seconds (`UpdateMarketsList`): per market the
//! bid and ask, the mark price with a flag saying the core has one, and funding. The client keeps
//! only the latest list, and its retained rings, where they exist, live in memory alone: after a
//! restart a closed trade has no mark left anywhere, and the core's chart archive never carried
//! it. So the mark before an entry exists only if it was already held when the entry came. That is
//! what this module does: it holds a short ring of every market ([`ring`]) and files the part each
//! trade needs ([`plan`]) as the trade goes, from before its entry to after its exit.
//!
//! # Where the lists come from
//!
//! Every core's client receives its core's list; one core per exchange is the source ([`source`]),
//! read right after the list was applied, before any filter of the station's
//! ([`on_prices_updated`]). Rows are taken as they arrived — nothing is recomputed, nothing
//! interpolated. Only markets with a mark are kept, so spot drops out by itself.
//!
//! # What lands on disk
//!
//! Per `(exchange, market)`, the union of its trades' windows, in segments of the stretch the lists
//! vouch for ([`store`]). A list that stops arriving leaves an honest gap: the segment ends and the
//! next begins when lists come again. Retention follows the tape's settings: a station drops what
//! is older than its tape's age limit, and either host drops the oldest first only when its disk
//! reaches the shared reserve (`disk_reserve`), as the tape does — so the file holds as far back
//! as the tape. Emulator trades are recorded only while the tape records them, and when a switch
//! narrows, the tape's own retention pass hands its keep map here ([`apply_keep`]), so the price
//! history of the trades the tape drops goes with it.
//!
//! # Hosts
//!
//! The terminal and the station's `Account` profile. A `Reports` station builds its clients
//! without the periodic list, so there is nothing to record and nothing here starts.
//!
//! One thread owns the rings, the plans and the writer; nothing here touches the GUI.
//!
//! # From the station
//!
//! A station records while the terminal is off. Once per run the terminal asks it for what its
//! own file lacks and files the answer beside its own segments ([`remote`]).

mod codec;
mod plan;
pub mod remote;
mod ring;
mod source;
mod store;

pub use store::{FundingRate, PriceSample, PriceSpan, read};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use plan::{KeyPlan, Window};
use ring::{ExchangeRing, Row};
use store::{Filed, Store};

use crate::market::tape_recorder::TradeId;
use crate::market::trade_replay::Coverage;
use crate::market::trade_replay::trade_cache::KeepMap;
use crate::session::CoreId;

/// What the file may hold in use: its size on disk plus what the disk can still spare above the
/// shared reserve, the tape's own rule (`disk_reserve`). Below the reserve the ceiling falls under
/// the file's size by the shortfall, and retention frees that much, oldest end first. Freed pages
/// stay in the file and take the next writes, so the disk figure does not move and the next pass
/// finds nothing more to free: the file stops growing rather than emptying itself. A disk that
/// cannot be measured imposes no ceiling.
///
/// Args:
///     file: Bytes of the file's pages, free ones included.
///     disk: `(free, total)` of the file's filesystem, when it could be measured.
fn ceiling(file: u64, disk: Option<(u64, u64)>) -> u64 {
    match disk {
        Some((free, total)) if total != 0 => file
            .saturating_add(free)
            .saturating_sub(crate::disk_reserve::reserve(total)),
        _ => u64::MAX,
    }
}

/// How often the thread looks at its queues.
const TICK: Duration = Duration::from_millis(250);
/// How often what is due is filed. Under [`RING_SPARE_MS`], so nothing due leaves the ring first.
const FLUSH_EVERY: Duration = Duration::from_secs(30);
/// A list stamped this close to a flush may still be on its way: the flush stops short of it.
const SETTLE_MS: i64 = 1_000;
/// The ring holds the tape's margin before an entry, but never more than this: at the longest
/// margin (two hours) a ring of every market would take hundreds of megabytes.
const LOOKBACK_CAP_MS: i64 = 600_000;
/// Ring depth past the margin: a report row may reach the recorder some seconds after the fill,
/// and a flush is up to [`FLUSH_EVERY`] away.
const RING_SPARE_MS: i64 = 60_000;
/// Kept past the tape's tail after an exit.
const TAIL_EXTRA_MS: i64 = 10_000;

/// How often retention runs; the first pass comes a minute after start.
const RETENTION_EVERY: Duration = Duration::from_secs(600);
/// How often the summary line is written to the application log.
const SUMMARY_EVERY: Duration = Duration::from_secs(300);
/// Lists queued for the thread. A full queue drops the list and breaks that exchange's run.
const PRICE_QUEUE: usize = 256;
/// Segments a failing file may hold back for the next flush; past it the oldest are lost.
const MAX_UNFILED: usize = 5_000;
/// An idle recorder writes its summary line only every this many periods (hourly).
const IDLE_SUMMARY_EVERY: u32 = 12;

/// Whether this process records: everywhere but a `Reports` station.
pub fn enabled() -> bool {
    crate::feed::station::profile() != Some(crate::feed::station::Profile::Reports)
}

/// A core named its exchange (`"{code}:{dex:08x}"`, the replay address's spelling).
pub fn core_exchange(core: CoreId, exchange_key: &str) {
    if enabled() {
        source::set_exchange(core, exchange_key);
    }
}

/// A core was removed, or its feed restarts with a new configuration.
pub fn core_down(core: CoreId) {
    source::forget(core);
}

/// A list was applied on `core`'s client: when this core is its exchange's source, read the rows
/// and queue them. Called on the core's feed thread before any event filter; costs one lock when
/// another core is the source.
///
/// Args:
///     core: The core whose list arrived.
///     included_funding: Whether the list carried funding.
///     client: That core's client, its markets already updated.
pub fn on_prices_updated(core: CoreId, included_funding: bool, client: &moonproto::MoonClient) {
    if !enabled() {
        return;
    }
    let now_ms = crate::util::now_unix_ms_i64();
    let Some(exchange) = source::claim(core, now_ms) else {
        return;
    };
    let Some(snapshot) = client.snapshot() else {
        return;
    };
    let rows = source::rows_of(snapshot.markets(), included_funding);
    drop(snapshot);
    send_prices(Batch {
        exchange,
        t_ms: now_ms,
        rows,
        broken: false,
    });
}

/// A trade opened on `exchange_key`/`market` at `open_ms` (true UTC).
///
/// `emulator`: the row's emulator flag; an emulator trade is left out while the tape leaves it
/// out, and an unknown one is recorded.
pub fn trade_opened(
    exchange_key: &str,
    market: &str,
    trade: TradeId,
    open_ms: i64,
    emulator: Option<bool>,
) {
    if enabled() && !excluded(emulator) {
        send(Cmd::Opened {
            key: (exchange_key.to_owned(), market.to_owned()),
            trade,
            open_ms,
        });
    }
}

/// A trade closed on `exchange_key`/`market` (true UTC). An emulator trade the tape leaves out
/// is dropped from its key, so what is still due of it is not filed.
pub fn trade_closed(
    exchange_key: &str,
    market: &str,
    trade: TradeId,
    open_ms: i64,
    close_ms: i64,
    emulator: Option<bool>,
) {
    if !enabled() {
        return;
    }
    let key = (exchange_key.to_owned(), market.to_owned());
    if excluded(emulator) {
        send(Cmd::Dropped { key, trade });
    } else {
        send(Cmd::Closed {
            key,
            trade,
            open_ms,
            close_ms,
        });
    }
}

/// The station's tape retention built `keep` — per market, the ground its tape keeps — under a
/// narrowed emulator or tuner-only switch: drop the price history of the same trades.
pub fn apply_keep(keep: Arc<KeepMap>) {
    if enabled() {
        send(Cmd::Keep(keep));
    }
}

/// Whether a trade with this emulator flag is left out, by the tape's own setting.
fn excluded(emulator: Option<bool>) -> bool {
    emulator == Some(true) && !crate::market::tape_recorder::catchup::record_emulator()
}

/// File what is due up to now and stop, as the process exits. Returns whether the file took it
/// within `timeout`; a recorder that never started returns at once.
pub fn shutdown(timeout: Duration) -> bool {
    let Some(Some(tx)) = TX.get() else {
        return true;
    };
    let (reply, done) = mpsc::channel();
    tx.events.send(Cmd::Shutdown(reply)).is_ok() && done.recv_timeout(timeout).is_ok()
}

/// `(exchange key, market)`.
type Key = (String, String);

/// One list of one exchange.
struct Batch {
    exchange: Arc<str>,
    t_ms: i64,
    rows: Vec<Row>,
    /// A list of this exchange before this one was dropped on a full queue.
    broken: bool,
}

enum Cmd {
    Opened {
        key: Key,
        trade: TradeId,
        open_ms: i64,
    },
    Closed {
        key: Key,
        trade: TradeId,
        open_ms: i64,
        close_ms: i64,
    },
    /// An emulator trade the tape leaves out closed: forget it.
    Dropped {
        key: Key,
        trade: TradeId,
    },
    /// The tape's retention keep map, built under a narrowed emulator or tuner-only switch.
    Keep(Arc<KeepMap>),
    Shutdown(Sender<()>),
}

/// Trade events are rare and never dropped; lists are bounded.
struct Tx {
    events: Sender<Cmd>,
    prices: SyncSender<Batch>,
}

static TX: OnceLock<Option<Tx>> = OnceLock::new();
/// Exchanges whose list a full queue dropped: their next queued list is marked broken.
static DROPPED: Mutex<Vec<Arc<str>>> = Mutex::new(Vec::new());
/// Lists a full queue dropped since the last summary line.
static DROPPED_LISTS: AtomicU64 = AtomicU64::new(0);

/// The thread's queues, starting it on first use.
fn tx() -> Option<&'static Tx> {
    TX.get_or_init(|| {
        let (events, events_rx) = mpsc::channel();
        let (prices, prices_rx) = mpsc::sync_channel(PRICE_QUEUE);
        std::thread::Builder::new()
            .name("price-recorder".into())
            .spawn(move || run(&events_rx, &prices_rx))
            .map_err(|e| log::warn!("price recorder thread not started: {e}"))
            .ok()?;
        Some(Tx { events, prices })
    })
    .as_ref()
}

fn send(cmd: Cmd) {
    if let Some(tx) = tx() {
        let _ = tx.events.send(cmd);
    }
}

fn send_prices(mut batch: Batch) {
    let Some(tx) = tx() else {
        return;
    };
    // The mark rides on the first list that gets through after the loss, so the run breaks at
    // the seam where lists are actually missing, not at whatever the queue held before.
    let mut dropped = DROPPED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let marked = dropped.iter().position(|e| *e == batch.exchange);
    batch.broken = marked.is_some();
    match tx.prices.try_send(batch) {
        Ok(()) => {
            if let Some(at) = marked {
                dropped.swap_remove(at);
            }
        }
        Err(TrySendError::Full(batch)) => {
            if marked.is_none() {
                dropped.push(batch.exchange);
            }
            DROPPED_LISTS.fetch_add(1, Ordering::Relaxed);
        }
        Err(TrySendError::Disconnected(_)) => {}
    }
}

/// The tape window in force now.
fn current_window() -> Window {
    Window {
        margin_ms: crate::market::trade_replay::margin_ms(),
        long_ms: crate::market::trade_replay::long_position_ms(),
        tail_ms: crate::market::trade_replay::record_tail_ms().saturating_add(TAIL_EXTRA_MS),
    }
}

/// The longest stretch one filed segment can speak for: a flush files ground the ring still holds,
/// and a pulled or cut piece is a part of such a segment. Bounds the purge's index seek.
const MAX_SEGMENT_MS: i64 = LOOKBACK_CAP_MS + RING_SPARE_MS + 2 * FLUSH_EVERY.as_millis() as i64;

/// How far back the ring reaches under a margin.
fn ring_depth_ms(margin_ms: i64) -> i64 {
    margin_ms.clamp(0, LOOKBACK_CAP_MS) + RING_SPARE_MS
}

fn run(events: &Receiver<Cmd>, prices: &Receiver<Batch>) {
    let path = crate::config::paths::market_prices_db_path();
    let store = match Store::open(&path) {
        Ok(store) => {
            log::info!("price recorder: started ({})", path.display());
            Some(store)
        }
        Err(e) => {
            log::error!("price recorder: cannot open {}: {e}", path.display());
            None
        }
    };
    let mut rec = Recorder::new(store);
    rec.path = Some(path);
    let started = Instant::now();
    let mut next_flush = started + FLUSH_EVERY;
    let mut next_retention = started + Duration::from_secs(60);
    let mut next_summary = started + SUMMARY_EVERY;
    let mut cmds = Vec::new();
    loop {
        match events.recv_timeout(TICK) {
            Ok(cmd) => cmds.push(cmd),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        cmds.extend(events.try_iter());
        for batch in prices.try_iter() {
            rec.list(batch);
        }
        let now_ms = crate::util::now_unix_ms_i64();
        for cmd in cmds.drain(..) {
            match cmd {
                Cmd::Opened {
                    key,
                    trade,
                    open_ms,
                } => rec.plan(key).opened(trade, open_ms),
                Cmd::Closed {
                    key,
                    trade,
                    open_ms,
                    close_ms,
                } => rec.plan(key).closed(trade, open_ms, close_ms),
                Cmd::Dropped { key, trade } => {
                    if let Some(plan) = rec.keys.get_mut(&key) {
                        plan.forget(trade);
                    }
                }
                Cmd::Keep(keep) => rec.apply_keep(&keep),
                Cmd::Shutdown(reply) => {
                    rec.flush(now_ms, now_ms, current_window());
                    rec.summary(true);
                    let _ = reply.send(());
                    return;
                }
            }
        }
        let now = Instant::now();
        if now >= next_flush {
            next_flush = now + FLUSH_EVERY;
            rec.flush(now_ms, now_ms - SETTLE_MS, current_window());
        }
        if now >= next_retention {
            next_retention = now + RETENTION_EVERY;
            rec.retention(now_ms);
        }
        if now >= next_summary {
            next_summary = now + SUMMARY_EVERY;
            rec.summary(false);
        }
    }
}

/// Counters for the summary line, reset with each line except where noted.
#[derive(Default)]
struct Stats {
    lists: u64,
    points: u64,
    segments: u64,
    bytes: u64,
    /// Milliseconds a trade needed that no list vouched for: before the ring, or a gap.
    lost_ms: i64,
    /// Due stretches of a market the lists carry nothing for (a spot trade).
    no_data: u64,
    write_failed: u64,
    /// Segments a failing file held back past [`MAX_UNFILED`], lost for good.
    unfiled_lost: u64,
}

struct Recorder {
    store: Option<Store>,
    /// Where the store lives, to open it again after a failed open; `None` in tests.
    path: Option<PathBuf>,
    rings: HashMap<String, ExchangeRing>,
    keys: HashMap<Key, KeyPlan>,
    /// What done keys filed, while it can still overlap a new trade's window: a next trade of
    /// the same market must not file that ground a second time.
    recent: HashMap<Key, Coverage>,
    /// Segments the file refused, retried on the next flush.
    unfiled: Vec<Filed>,
    stats: Stats,
    /// Summary periods skipped while idle.
    idle_periods: u32,
}

impl Recorder {
    fn new(store: Option<Store>) -> Self {
        Self {
            store,
            path: None,
            rings: HashMap::new(),
            keys: HashMap::new(),
            recent: HashMap::new(),
            unfiled: Vec::new(),
            stats: Stats::default(),
            idle_periods: 0,
        }
    }

    /// The plan of `key`, taken up again with what its last done plan filed.
    fn plan(&mut self, key: Key) -> &mut KeyPlan {
        let recent = &mut self.recent;
        self.keys
            .entry(key)
            .or_insert_with_key(|key| KeyPlan::resuming(recent.remove(key).unwrap_or_default()))
    }

    /// Take one list into its exchange's ring.
    fn list(&mut self, batch: Batch) {
        let broken = batch.broken;
        if !self.rings.contains_key(&*batch.exchange) {
            self.rings
                .insert(batch.exchange.to_string(), ExchangeRing::default());
        }
        let Some(ring) = self.rings.get_mut(&*batch.exchange) else {
            return;
        };
        self.stats.points += ring.apply(batch.t_ms, batch.rows, broken) as u64;
        self.stats.lists += 1;
    }

    /// File every key's due ground up to `horizon_ms`, and drop the keys that are done.
    fn flush(&mut self, now_ms: i64, horizon_ms: i64, window: Window) {
        let floor = now_ms.saturating_sub(ring_depth_ms(window.margin_ms));
        for ring in self.rings.values_mut() {
            ring.trim(floor);
        }
        let bounds = Coverage::one((floor, horizon_ms));
        let up_to_horizon = Coverage::one((i64::MIN, horizon_ms));
        let mut filed = Vec::new();
        let rings = &self.rings;
        let stats = &mut self.stats;
        let recent = &mut self.recent;
        // Filed ground older than the ring can never be due again: nothing is left to file it from.
        recent.retain(|_, settled| settled.hull().is_some_and(|(_, end)| end >= floor));
        self.keys.retain(|key, plan| {
            let (exchange, market) = key;
            plan.expire(now_ms);
            let due = plan.need(window).clip(&up_to_horizon).minus(&plan.settled);
            if !due.is_empty() {
                let ring = rings.get(exchange.as_str());
                let valid = ring.map_or_else(Coverage::none, |r| r.valid(horizon_ms).clip(&bounds));
                let writable = due.clip(&valid);
                for &(from, to) in writable.spans() {
                    match ring.and_then(|r| r.segment(market, from, to)) {
                        Some(segment) => filed.push(Filed {
                            exchange: exchange.clone(),
                            market: market.clone(),
                            segment,
                        }),
                        None => stats.no_data += 1,
                    }
                }
                stats.lost_ms += due.minus(&writable).width_ms();
                for &span in due.spans() {
                    plan.settled.add(span);
                }
            }
            if plan.done(window, horizon_ms) {
                recent.insert(key.clone(), std::mem::take(&mut plan.settled));
                return false;
            }
            true
        });
        self.file(filed);
    }

    /// Write `filed` behind whatever the file refused before; what it refuses now waits for the
    /// next flush. A store that never opened is tried again here.
    fn file(&mut self, mut filed: Vec<Filed>) {
        let mut batch = std::mem::take(&mut self.unfiled);
        batch.append(&mut filed);
        if batch.is_empty() {
            return;
        }
        if self.store.is_none() {
            if let Some(path) = &self.path {
                if let Ok(store) = Store::open(path) {
                    log::info!("price recorder: opened {} on retry", path.display());
                    self.store = Some(store);
                }
            }
        }
        let result = match self.store.as_mut() {
            Some(store) => store.write(&batch),
            None => Err(rusqlite::Error::InvalidPath(
                self.path.clone().unwrap_or_default(),
            )),
        };
        match result {
            Ok(bytes) => {
                self.stats.segments += batch.len() as u64;
                self.stats.bytes += bytes as u64;
            }
            Err(e) => {
                self.stats.write_failed += 1;
                log::warn!(
                    "price recorder: {} segments not filed yet: {e}",
                    batch.len()
                );
                if batch.len() > MAX_UNFILED {
                    let lost = batch.len() - MAX_UNFILED;
                    self.stats.unfiled_lost += lost as u64;
                    batch.drain(..lost);
                }
                self.unfiled = batch;
            }
        }
    }

    /// Age (a station's tape limit) and disk retention.
    fn retention(&mut self, now_ms: i64) {
        let Some(store) = self.store.as_mut() else {
            return;
        };
        let edge = if crate::feed::station::enabled() {
            crate::market::tape_recorder::catchup::retention_edge_ms(now_ms)
        } else {
            None
        };
        let file = match store.file_bytes() {
            Ok(file) => file,
            Err(e) => {
                log::warn!("price recorder: retention could not size the file: {e}");
                return;
            }
        };
        let disk = self
            .path
            .as_deref()
            .and_then(std::path::Path::parent)
            .and_then(crate::disk_reserve::disk_space);
        match store.trim(edge, ceiling(file, disk)) {
            Ok(0) => {}
            Ok(rows) => log::info!("price recorder: retention removed {rows} segments"),
            Err(e) => log::warn!("price recorder: retention failed: {e}"),
        }
    }

    /// Keep only what the tape's retention keeps (`keep`, per market), as a narrowed emulator or
    /// tuner-only switch drops it from the tape — on every whole pass the tape makes under such a
    /// rule, so what an emulator trade filed before its flag was known goes on the next one. Each
    /// pass reads only the segments inside the keep map's interior gaps, not the file.
    fn apply_keep(&mut self, keep: &KeepMap) {
        let Some(store) = self.store.as_mut() else {
            return;
        };
        let mut removed = 0;
        for ((exchange, market), keep) in keep {
            match store.keep_only(exchange, market, keep, MAX_SEGMENT_MS) {
                Ok(n) => removed += n,
                Err(e) => {
                    log::warn!("price recorder: purge of {exchange} {market} failed: {e}");
                    return;
                }
            }
        }
        if removed > 0 {
            log::info!(
                "price recorder: tape retention applied over {} market(s), {removed} segment(s) removed or cut",
                keep.len()
            );
        }
    }

    /// One line on what the rings hold and what was filed since the last line; while nothing is
    /// recorded and nothing went wrong, one line an hour. `last` writes it whatever: the process
    /// is stopping.
    fn summary(&mut self, last: bool) {
        let s = &self.stats;
        let idle = self.keys.is_empty()
            && self.unfiled.is_empty()
            && s.segments == 0
            && s.lost_ms == 0
            && s.write_failed == 0
            && s.unfiled_lost == 0
            && DROPPED_LISTS.load(Ordering::Relaxed) == 0;
        if !last && idle && self.idle_periods + 1 < IDLE_SUMMARY_EVERY {
            self.idle_periods += 1;
            return;
        }
        self.idle_periods = 0;
        let (mut markets, mut points) = (0, 0);
        for ring in self.rings.values() {
            let (m, p) = ring.size();
            markets += m;
            points += p;
        }
        let s = std::mem::take(&mut self.stats);
        let dropped = DROPPED_LISTS.swap(0, Ordering::Relaxed);
        log::info!(
            "price recorder: {} exchanges, {markets} markets, {points} points held (~{} MB), \
             {} keys; since last line: {} lists, {} points, {} segments, {} KB filed, \
             {} s lost, {} without data, {} lists dropped, {} writes failed, {} unfiled lost",
            self.rings.len(),
            points * std::mem::size_of::<ring::Point>() / 1_000_000,
            self.keys.len(),
            s.lists,
            s.points,
            s.segments,
            s.bytes / 1_000,
            s.lost_ms / 1_000,
            s.no_data,
            dropped,
            s.write_failed,
            s.unfiled_lost,
        );
    }
}

#[cfg(test)]
mod tests;
