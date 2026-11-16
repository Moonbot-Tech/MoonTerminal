//! Diagnostic channel for how much of a core's market universe the arbitrage stream covers
//! (`channels.arb_coverage` in `cfg/diagnostics.toml`), built to answer one question the sources
//! cannot: does the stream quote EVERY market a core lists, or only the ones somebody is charting.
//!
//! The Screener is about to state, for each coin, where else it trades — "futures only on this
//! exchange" — and the arbitrage stream looks like the answer: Moonbot's server sends one price per
//! venue per coin, spot and futures under separate codes, with no core of that venue connected. But
//! the only reader so far is the chart, which asks about the one market it shows, so nothing has
//! ever observed the stream across a whole catalog. If it covers a tenth of the markets, a column
//! built on it would print "unknown" nine times out of ten.
//!
//! Three numbers decide that, and all three are runtime facts:
//!
//! - **Coverage** — how many of a core's markets hold at least one venue's price.
//! - **Fill time** — how long after a connection those prices take to arrive. A slot appears with
//!   its first price, so a market reads "quoted nowhere else" until then, and a column that trusted
//!   an early reading would call half the exchange futures-only. Read it from a run STARTED with
//!   the channel on: `seen_s` counts from the channel's first read of a core, so switching it on
//!   over cores connected long ago shows a catalog already full at zero seconds.
//! - **Age** — how old the newest price of a market is. A slot outlives the listing behind it until
//!   the core reconnects, so presence alone cannot tell a live venue from a delisted one.
//!
//! The line also carries the catalog's `futures_type` histogram. That field is the core's OWN
//! cross-listing flag — Moonbot's `ListedType` filter reads it — and whether it is filled on a spot
//! core of each exchange has never been measured either.
//!
//! One summary line per core per sweep, every [`SWEEP_PERIOD_MS`], appended to
//! `logs/arb_coverage.log`; the lines of one core read top to bottom are its fill curve. The sample
//! lines beside it are written only when their content changes.
//!
//! The scan runs on a thread of its own. It reads every market of every core once per venue the
//! core watches — a few hundred thousand lock round trips on a large fleet — and the tick that
//! starts it is the session's reconciliation tick, which runs on the UI thread.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};

/// Gap between two sweeps of every core.
///
/// Coarse on purpose: the questions are about minutes — how long a catalog takes to fill — and a
/// sweep reads every market of every core.
pub const SWEEP_PERIOD_MS: i64 = 30_000;

/// A market whose newest quote is at most this old counts as quoted right now.
///
/// The same minute the chart's arbitrage column uses as its staleness bound, so "fresh" here is
/// "would be printed there". The two are held equal at compile time, beside that bound in
/// `market::source::arb`.
pub const FRESH_MS: i64 = 60_000;

/// A market whose newest quote is at most this old counts as quoted recently.
///
/// The second bucket exists to size the cutoff a listing column would need: a thinly traded coin
/// may go minutes between prices, and the gap between the two buckets is how many markets a
/// one-minute rule would wrongly drop.
pub const RECENT_MS: i64 = 600_000;

/// Most market names written in one sample.
///
/// A sample is read by eye against a venue's own listing; a dozen names say which KIND of market is
/// missing, and the whole list would bury the summary line this channel exists for.
pub const SAMPLE_LEN: usize = 12;

/// Whether the channel is on at all.
pub fn enabled() -> bool {
    crate::diagnostics::arb_coverage()
}

/// When the next sweep may start, and whether one is still running.
#[derive(Default)]
struct Gate {
    last_ms: Option<i64>,
    running: bool,
}

impl Gate {
    /// Admit a sweep when none is running and the period has passed.
    ///
    /// A clock that stepped BACKWARDS admits at once: measured against a `last_ms` now in the
    /// future, the period would otherwise not elapse until the clock caught up, and the channel
    /// would go silent for exactly as long as the step.
    ///
    /// Args:
    ///     now_ms: Current unix time in milliseconds.
    ///
    /// Returns:
    ///     `true` when a sweep may start. `running` then stays set until something clears it,
    ///     which outside the tests is [`SweepGuard`]'s `Drop` and nothing else.
    fn admit(&mut self, now_ms: i64) -> bool {
        if self.running {
            return false;
        }
        let due = match self.last_ms {
            Some(last) => now_ms < last || now_ms - last >= SWEEP_PERIOD_MS,
            None => true,
        };
        if due {
            self.last_ms = Some(now_ms);
            self.running = true;
        }
        due
    }
}

fn gate() -> &'static Mutex<Gate> {
    static GATE: OnceLock<Mutex<Gate>> = OnceLock::new();
    GATE.get_or_init(Mutex::default)
}

/// Proof that a sweep was admitted; dropping it lets the next one start.
///
/// A guard rather than an `end_sweep()` call because the sweep runs on a thread of its own: a panic
/// there, or a thread that failed to spawn, would otherwise leave the gate closed for the rest of
/// the session with nothing in the log to say why the channel stopped.
pub struct SweepGuard(&'static Mutex<Gate>);

impl Drop for SweepGuard {
    fn drop(&mut self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).running = false;
    }
}

/// Ask one gate to admit a sweep.
///
/// Takes the gate as an argument so the whole admission — lock, decision, guard — can be driven
/// against a gate of a test's own, where the process-wide one would race with every other test.
///
/// Two statements, and the order is the point. The guard's `Drop` takes this same mutex, so a
/// guard must never exist while the admission's lock is held, and must never exist for a refusal
/// at all: built eagerly inside the locked expression, a refused one was dropped on the spot and
/// deadlocked the caller — the UI thread — on its second tick.
fn try_begin(gate: &'static Mutex<Gate>, now_ms: i64) -> Option<SweepGuard> {
    let admitted = gate.lock().unwrap_or_else(|e| e.into_inner()).admit(now_ms);
    admitted.then(|| SweepGuard(gate))
}

/// Ask to start a sweep.
///
/// The off case costs one atomic load, which is what lets the caller sit on a tick that runs every
/// second.
///
/// Args:
///     now_ms: Current unix time in milliseconds.
///
/// Returns:
///     A guard to hold for the sweep's duration, or `None` when the channel is off, a sweep is
///     still running, or the period has not passed.
pub fn begin_sweep(now_ms: i64) -> Option<SweepGuard> {
    if !enabled() {
        return None;
    }
    try_begin(gate(), now_ms)
}

/// What one market contributes to its core's coverage.
pub struct MarketFacts<'a> {
    /// The key the terminal addresses the market by.
    pub name: &'a str,
    /// Whether the exchange reports the market as trading.
    pub trading: bool,
    /// The catalog's `futures_type`, by name, for the histogram.
    pub futures_type: &'static str,
    /// Whether `futures_type` is set at all, decided by the caller against the protocol's own
    /// constant rather than by its name here.
    ///
    /// Unset reads "no futures counterpart" on a core that sends the field — and is ALSO what a
    /// catalog packet too old to carry it parses to, so a core where every market is unset has
    /// answered nothing.
    pub futures_flag: bool,
    /// `(venue code, age in milliseconds)` for every venue holding a positive price.
    pub quotes: &'a [(u8, i64)],
}

/// How many markets one venue quotes on a core.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct VenueCount {
    /// Markets holding a price from the venue, of any age.
    markets: usize,
    /// Of those, the ones whose price is at most [`FRESH_MS`] old.
    fresh: usize,
}

/// One core's arbitrage coverage, accumulated market by market.
///
/// Every coverage figure counts TRADING markets only, so they share one denominator:
/// `quoted + unquoted_trading == trading` and `fresh + recent + older == quoted`. A halted market
/// is counted apart — it keeps whatever slot it held when it stopped, and letting those into the
/// buckets would answer the coverage question with dead markets.
#[derive(Debug, Default)]
pub struct CoreCoverage {
    /// Venues the core's own mask asks for, or `None` while its client settings have not arrived.
    ///
    /// Two states, because the mask reads all-false in both: `Some(0)` is a core that stores
    /// nothing by choice, `None` one that has not said yet — and the first sweeps after a
    /// connection are exactly where a fill curve would misread the second as the first.
    mask: Option<usize>,
    /// Whether the core's arbitrage licence is in force right now.
    licence_active: bool,
    /// Every market of the catalog, halted ones included.
    markets: usize,
    trading: usize,
    /// Trading markets holding at least one venue's price.
    quoted: usize,
    /// Of those, the ones whose NEWEST price is at most [`FRESH_MS`] old.
    fresh: usize,
    /// The ones whose newest price is older than that and at most [`RECENT_MS`] old.
    recent: usize,
    /// The ones whose newest price is older still.
    older: usize,
    /// Trading markets holding no price at all.
    unquoted_trading: usize,
    /// Halted markets still holding a price, kept out of every figure above.
    halted_quoted: usize,
    venues: BTreeMap<u8, VenueCount>,
    /// `futures_type` over every market of the catalog, halted ones included.
    futures_types: BTreeMap<&'static str, usize>,
    unquoted_sample: Vec<String>,
    no_futures_sample: Vec<String>,
    with_futures_sample: Vec<String>,
}

impl CoreCoverage {
    /// Start an empty reading for one core.
    ///
    /// Args:
    ///     mask: How many venues the core's mask asks for, `None` while its settings are pending.
    ///     asked: The venue codes that mask names. Each is listed in the summary even at zero, so
    ///         a venue the core watches and never hears from is a visible `0/0` rather than an
    ///         absence, and two cores with different masks read differently.
    ///     licence_active: Whether the core's arbitrage licence is in force.
    ///
    /// Returns:
    ///     A reading with no market counted yet.
    pub fn new(mask: Option<usize>, asked: &[u8], licence_active: bool) -> Self {
        Self {
            mask,
            licence_active,
            venues: asked
                .iter()
                .map(|venue| (*venue, VenueCount::default()))
                .collect(),
            ..Self::default()
        }
    }

    /// Count one market.
    ///
    /// An age below zero — the stamp is the receiving client's own clock, so a clock step can put
    /// it in the future — is read as "just now" rather than as old: the price did arrive, and
    /// filing it under "stale" would report a healthy stream as a dead one.
    ///
    /// Args:
    ///     market: The market's facts, read under its own lock by the caller.
    pub fn add_market(&mut self, market: &MarketFacts<'_>) {
        self.markets += 1;
        *self.futures_types.entry(market.futures_type).or_default() += 1;
        if !market.trading {
            if !market.quotes.is_empty() {
                self.halted_quoted += 1;
            }
            return;
        }
        self.trading += 1;
        // Trading markets only: a halted market's flag describes a listing nobody can check
        // against the exchange's own page.
        let sample = match market.futures_flag {
            false => &mut self.no_futures_sample,
            true => &mut self.with_futures_sample,
        };
        if sample.len() < SAMPLE_LEN {
            sample.push(market.name.to_string());
        }

        let newest = market.quotes.iter().map(|(_, age)| (*age).max(0)).min();
        let Some(newest) = newest else {
            self.unquoted_trading += 1;
            if self.unquoted_sample.len() < SAMPLE_LEN {
                self.unquoted_sample.push(market.name.to_string());
            }
            return;
        };
        self.quoted += 1;
        if newest <= FRESH_MS {
            self.fresh += 1;
        } else if newest <= RECENT_MS {
            self.recent += 1;
        } else {
            self.older += 1;
        }
        for (venue, age) in market.quotes {
            let count = self.venues.entry(*venue).or_default();
            count.markets += 1;
            if (*age).max(0) <= FRESH_MS {
                count.fresh += 1;
            }
        }
    }

    /// Whether the core's catalog was empty, which is "not arrived yet" rather than a reading.
    pub fn is_empty(&self) -> bool {
        self.markets == 0
    }

    /// The summary line's body, without the core's own caption.
    fn summary(&self) -> String {
        let mask = match self.mask {
            Some(count) => count.to_string(),
            None => "pending".to_string(),
        };
        let venues: Vec<String> = self
            .venues
            .iter()
            .map(|(code, count)| {
                format!(
                    "{}:{}/{}",
                    crate::market::ArbVenue::from_code(*code).default_name(),
                    count.markets,
                    count.fresh
                )
            })
            .collect();
        let futures_types: Vec<String> = self
            .futures_types
            .iter()
            .map(|(name, count)| format!("{name}:{count}"))
            .collect();
        format!(
            "licence={licence} mask={mask} markets={markets} trading={trading} quoted={quoted} \
             fresh60s={fresh} recent10m={recent} older={older} unquoted_trading={unquoted} \
             halted_quoted={halted_quoted} venues(all/fresh)=[{venues}] \
             futures_type=[{futures_types}]",
            licence = self.licence_active,
            markets = self.markets,
            trading = self.trading,
            quoted = self.quoted,
            fresh = self.fresh,
            recent = self.recent,
            older = self.older,
            unquoted = self.unquoted_trading,
            halted_quoted = self.halted_quoted,
            venues = venues.join(" "),
            futures_types = futures_types.join(" "),
        )
    }

    /// The sample line's body: which markets are unquoted, and which carry which listing flag.
    fn samples(&self) -> String {
        format!(
            "unquoted_trading: {} | futures_type unset: {} | futures_type set: {}",
            self.unquoted_sample.join(","),
            self.no_futures_sample.join(","),
            self.with_futures_sample.join(","),
        )
    }
}

/// What the channel remembers about a core between sweeps.
#[derive(Default)]
struct Seen {
    /// When the channel first read the core's catalog in this process.
    ///
    /// Not reset by a reconnect or by switching the channel off and on: a reconnect shows in the
    /// line itself, as `quoted` falling back to zero, and the fill after it is read from the
    /// stamps of the lines that follow.
    first_ms: i64,
    /// The last sample line written, so an unchanged one is not written again.
    samples: String,
}

fn seen() -> &'static Mutex<HashMap<u64, Seen>> {
    static SEEN: OnceLock<Mutex<HashMap<u64, Seen>>> = OnceLock::new();
    SEEN.get_or_init(Mutex::default)
}

/// Write one core's coverage: always the summary, and the samples when they changed.
///
/// A core whose catalog has not arrived writes nothing and starts no clock, so `seen_s` counts from
/// the first sweep that could actually read it — the origin a fill curve needs.
///
/// Args:
///     core: Core the coverage was read from.
///     core_name: Server name as the user sees it.
///     venue: Exchange caption with its platform code, for reading the file without resolving ids.
///     coverage: The accumulated reading.
///     now_ms: Current unix time in milliseconds.
pub fn record(core: u64, core_name: &str, venue: &str, coverage: &CoreCoverage, now_ms: i64) {
    if !enabled() || coverage.is_empty() {
        return;
    }
    let samples = coverage.samples();
    let (seen_s, samples_changed) = {
        let mut seen = seen().lock().unwrap_or_else(|e| e.into_inner());
        let entry = seen.entry(core).or_insert_with(|| Seen {
            first_ms: now_ms,
            samples: String::new(),
        });
        let changed = entry.samples != samples;
        if changed {
            entry.samples = samples.clone();
        }
        (now_ms.saturating_sub(entry.first_ms).max(0) / 1000, changed)
    };
    let caption = format!("core={core} ({core_name} · {venue})");
    crate::diagnostics::stamped_line(
        "arb_coverage.log",
        &format!("{caption} seen_s={seen_s} {}", coverage.summary()),
    );
    if samples_changed {
        crate::diagnostics::stamped_line("arb_coverage.log", &format!("{caption} {samples}"));
    }
}

#[cfg(test)]
mod tests;
