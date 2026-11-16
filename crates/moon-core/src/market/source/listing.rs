//! Building the [`ListingBook`] from what the connected cores hold, and keeping it for a while.
//!
//! The decision lives in [`crate::market::listing`]; this file only gathers its evidence, and
//! gathering is the expensive half. Every market of every core is read once for its coin, and
//! every market of every arbitrage donor once more per venue the donor watches — a few hundred
//! thousand market-lock round trips on a fleet of thirty cores. So:
//!
//! - it is built on the CALLER's thread and never on the UI thread: the one caller is the
//!   Screener's background publication ([`MarketDataSource::screener_rows`]);
//! - it is kept for [`BOOK_TTL_MS`]. A listing changes when an exchange lists a coin, which is
//!   days apart; half a minute is how often it is worth paying for. The donor warm-up is carried
//!   from book to book, but a donor first seen is aged by its own connection ([`warm_origin`]),
//!   not by when this file first looked;
//! - one build at a time: a caller that finds the book expired while another is building it gets
//!   the old one rather than reading every core a second time in parallel;
//! - nothing is built while no Screener is open, because nothing asks.
//!
//! Locking. The cache is a `Mutex` INSIDE the source's `RwLock`, taken out as a handle and locked
//! only for a copy in or out, never across the build — the same shape as the arbitrage book beside
//! it, and for the same reason.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::market::ScreenerRow;
use crate::market::listing::{
    DONOR_WARMUP_MS, Evidence, LISTING_MAX_AGE_MS, ListingBook, MIN_CARRIED_MARKETS, VenueSet,
    identity_key,
};
use crate::session::CoreId;

use super::read::{platform_code, positive};
use super::{MarketDataSource, MarketLabel};

/// How long one book serves before the cores are read again.
const BOOK_TTL_MS: i64 = 30_000;

/// Oldest previous book whose warm-up clocks are still trusted.
///
/// The clocks say "this donor has carried this venue without a break since then", and that is
/// only known while somebody keeps looking. With the Screener closed nobody does: a venue can be
/// switched off in a donor's mask and on again, and a clock carried across that gap would vouch
/// for a hold nobody saw. Past it, every client is aged afresh by its own connection
/// ([`warm_origin`]). Three builds' worth — enough to ride out one slow publication, not a closed
/// window.
const WARM_CARRY_MS: i64 = BOOK_TTL_MS * 3;

/// When a donor's hold on a venue is counted from, on the build that first sees it carried.
///
/// The warm-up asks how long the donor has been RECEIVING, and nothing builds a book while no
/// Screener is open — so a clock started at first sight would begin a fresh minute on every open,
/// with cores connected for hours, and for that minute no row could read one-sided. A client no
/// build has read is therefore aged by its own record: how long ago it first held a market
/// list, before which no per-market price can have arrived.
///
/// A client the builds HAVE been reading gets no such credit for a venue that turns up now: a
/// build ago it was not carrying it, so its slots for that venue are filling at this moment,
/// whatever the connection's age. What this cannot see is a venue switched on in a donor's mask
/// while nobody watched and looked at within the few seconds its slots take to fill; that reads
/// warm early, for one book period at most.
///
/// Args:
///     now_ms: Current unix time in milliseconds.
///     read_before: Whether an earlier build already read this client.
///     catalog_age_ms: How long ago the client first applied a market list, on the monotonic
///         clock; `None` when it has applied none.
///
/// Returns:
///     The start of the warm-up clock, never ahead of `now_ms`.
fn warm_origin(now_ms: i64, read_before: bool, catalog_age_ms: Option<i64>) -> i64 {
    match (read_before, catalog_age_ms) {
        (false, Some(age)) => now_ms.saturating_sub(age.max(0)),
        _ => now_ms,
    }
}

/// Whether a hold counted from `since` has lasted [`DONOR_WARMUP_MS`].
fn warmed_up(now_ms: i64, since: i64) -> bool {
    now_ms.saturating_sub(since) >= DONOR_WARMUP_MS
}

/// One donor's hold on one venue: the core, the client it was read through, and the venue code.
///
/// The client epoch is in the key so a replaced client — a reconnect that installs a new one —
/// starts every clock of that core over even when it happens between two builds.
type WarmKey = (CoreId, u64, u8);

/// What one build hands the next about the donors.
#[derive(Clone, Debug, Default, PartialEq)]
struct WarmClocks {
    /// When each donor's hold on each venue began counting, as [`warm_origin`] set it on the
    /// build that first saw the venue carried.
    since: HashMap<WarmKey, i64>,
    /// Every client a build read, whether it carried a venue or not.
    ///
    /// Apart from the clocks on purpose: a donor can be read for minutes while it carries nothing
    /// — its mask ticked later, its price stream late — and has no clock to show for it. Without
    /// this record it would pass for a client nobody had looked at, and the venue it then starts
    /// to carry would be aged by the whole connection while its slots are still filling.
    read: HashSet<(CoreId, u64)>,
}

impl WarmClocks {
    /// The clock of one hold: the carried one, or the origin of a hold first seen on this build.
    ///
    /// Args:
    ///     key: The donor, its client epoch and the venue code.
    ///     now_ms: Current unix time in milliseconds.
    ///     catalog_age_ms: How long ago the donor's client first applied a market list.
    fn clock_of(&self, key: WarmKey, now_ms: i64, catalog_age_ms: Option<i64>) -> i64 {
        match self.since.get(&key) {
            Some(&since) => since,
            None => warm_origin(now_ms, self.read.contains(&(key.0, key.1)), catalog_age_ms),
        }
    }

    /// Drop everything known about one core, whichever client it was read through.
    fn forget_core(&mut self, core: CoreId) {
        self.since.retain(|(donor, _, _), _| *donor != core);
        self.read.retain(|(donor, _)| *donor != core);
    }
}

/// What a caller gets when it asks for the book.
enum Claim {
    /// A book to use as it is: fresh, or stale while somebody else is rebuilding.
    Serve(Arc<ListingBook>),
    /// The caller builds, starting from what the previous build knew about the donors.
    Build(WarmClocks),
}

/// The last book built, and what has to outlive it.
#[derive(Default)]
pub(super) struct ListingCache {
    /// When [`Self::book`] was built; `None` before the first build.
    built_ms: Option<i64>,
    /// Whether a core changed since the book was built, which makes it stale whatever its age.
    ///
    /// A flag of its own rather than a cleared `built_ms`: the age still decides whether the
    /// OTHER cores' warm-up clocks are carried, and losing it would restart every donor's
    /// warm-up each time any one core reconnects.
    expired: bool,
    book: Arc<ListingBook>,
    /// The donors' warm-up clocks and the clients read, as the last build left them.
    warm: WarmClocks,
    /// Whether a build is in flight.
    building: bool,
    /// Cores that changed while that build was in flight.
    ///
    /// The build read them before the change, so its clocks for them describe a client that is
    /// gone — and a respawned core's new slot counts its epochs from the start again, so those
    /// clocks would even carry the new client's keys. [`Self::store`] drops them.
    forgotten_mid_build: Vec<CoreId>,
}

impl ListingCache {
    /// Hand out the kept book, or the right to build the next one.
    ///
    /// A clock that stepped back makes the kept book look as if it came from the future; that
    /// reads as expired, so the channel of answers cannot freeze until the clock catches up.
    ///
    /// Args:
    ///     now_ms: Current unix time in milliseconds.
    ///
    /// Returns:
    ///     [`Claim::Build`] to exactly one caller per expiry — who then owes [`Self::store`] or
    ///     [`Self::abandon`] — and the kept book to everyone else. While the very first build
    ///     runs, "the kept book" is the empty one: a publication racing it shows every listing as
    ///     unknown, and the next one, a second later, has the answers. Building twice in parallel
    ///     to avoid that would read every core twice at the moment the window opens.
    fn claim(&mut self, now_ms: i64) -> Claim {
        let age = self.built_ms.map(|built| now_ms - built);
        let fresh = !self.expired && age.is_some_and(|age| (0..BOOK_TTL_MS).contains(&age));
        if fresh || self.building {
            return Claim::Serve(self.book.clone());
        }
        self.building = true;
        let carried = age.is_some_and(|age| (0..WARM_CARRY_MS).contains(&age));
        Claim::Build(match carried {
            true => self.warm.clone(),
            false => WarmClocks::default(),
        })
    }

    /// Keep a finished build.
    ///
    /// A core that changed while it ran is not forgotten twice: its clocks are dropped from the
    /// result, and the book — which read that core's previous client — is stored already expired,
    /// so the next publication rebuilds it.
    fn store(&mut self, now_ms: i64, book: Arc<ListingBook>, mut warm: WarmClocks) {
        let forgotten = std::mem::take(&mut self.forgotten_mid_build);
        for core in &forgotten {
            warm.forget_core(*core);
        }
        self.building = false;
        self.expired = !forgotten.is_empty();
        self.built_ms = Some(now_ms);
        self.book = book;
        self.warm = warm;
    }

    /// Give the build right back without a book, after a build that did not finish.
    fn abandon(&mut self) {
        self.building = false;
        self.forgotten_mid_build.clear();
    }

    /// Forget one core: its warm-up clocks, and the freshness of a book that may quote it.
    ///
    /// Called when a client is replaced or removed, like the arbitrage book beside this cache. The
    /// kept book stays readable until the next build — it is at most one period old, and an
    /// empty one would blank the column — but it is no longer fresh, so that build is the next
    /// publication's. A build already in flight read the core before it changed, so the core is
    /// remembered for [`Self::store`] to forget again when that build lands.
    pub(super) fn forget_core(&mut self, core: CoreId) {
        self.warm.forget_core(core);
        self.expired = true;
        if self.building {
            self.forgotten_mid_build.push(core);
        }
    }
}

impl MarketDataSource {
    /// The current listing book, rebuilt when the kept one has aged out.
    ///
    /// Blocks for the whole build when it is the one to rebuild. Call it off the UI thread.
    ///
    /// Args:
    ///     now_ms: Current unix time in milliseconds.
    ///
    /// Returns:
    ///     A book at most [`BOOK_TTL_MS`] old, or the previous one while another caller rebuilds.
    pub(in crate::market) fn listing_book(&self, now_ms: i64) -> Arc<ListingBook> {
        let cache = self
            .inner
            .read()
            .expect("market source poisoned")
            .listing_cache
            .clone();
        // One statement, so the cache lock is released before the build below.
        let claim = cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .claim(now_ms);
        let carried = match claim {
            Claim::Serve(book) => return book,
            Claim::Build(carried) => carried,
        };
        // The build right has to come back whatever happens in the build: left taken, the book
        // would be served stale for the rest of the session. Caught and re-raised rather than
        // released from a `Drop`, which would lock this mutex from wherever the value happened
        // to be dropped.
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.build_listing_book(now_ms, &carried)
        }));
        let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        match built {
            Ok((book, warm)) => {
                let book = Arc::new(book);
                guard.store(now_ms, book.clone(), warm);
                book
            }
            Err(panic) => {
                guard.abandon();
                drop(guard);
                std::panic::resume_unwind(panic)
            }
        }
    }

    /// Read every connected core once and assemble the evidence.
    ///
    /// A core contributes twice. Its catalog — the coins of its TRADING markets — goes under its
    /// brand and kind, whoever it is. And if it holds arbitrage prices, each of its markets
    /// contributes the venues quoting it.
    ///
    /// What a donor may say NO about is decided per venue, not per donor: a venue counts once the
    /// donor holds it on [`MIN_CARRIED_MARKETS`] markets, and its silence is believed once that
    /// has lasted [`DONOR_WARMUP_MS`]. A mask alone proves nothing — two cores here were measured
    /// with a full mask and no price at all, and every donor's mask names venues the stream never
    /// sends.
    ///
    /// Args:
    ///     now_ms: Current unix time in milliseconds, the instant every quote's age is taken at.
    ///     carried: The donors' clocks and the clients read, from the previous book.
    ///
    /// Returns:
    ///     The book, and the same record as this build leaves it: the clocks of the venues still
    ///     carried, and every client it read.
    fn build_listing_book(&self, now_ms: i64, carried: &WarmClocks) -> (ListingBook, WarmClocks) {
        // Client, epoch and catalog age are read together under one guard, and the cores in
        // ascending order so which donor's reading lands first never depends on a `HashMap`.
        let cores: Vec<_> = {
            let inner = self.inner.read().expect("market source poisoned");
            let mut cores: Vec<_> = inner
                .clients
                .iter()
                .filter_map(|(core, slot)| {
                    let (client, epoch, catalog_age) = slot.get_with_epoch_and_catalog_age()?;
                    let venue = inner.core_venue.get(core)?.clone();
                    Some((*core, venue, client, epoch, catalog_age))
                })
                .collect();
            cores.sort_unstable_by_key(|(core, ..)| *core);
            cores
        };
        let mut book = ListingBook::default();
        let mut next = WarmClocks::default();
        for (core, venue, client, epoch, catalog_age) in cores {
            let Some(snapshot) = client.snapshot_versioned() else {
                continue;
            };
            // Read from here on, carried venue or not: see `WarmClocks::read`.
            next.read.insert((core, epoch));
            let catalog_age_ms =
                catalog_age.map(|age| i64::try_from(age.as_millis()).unwrap_or(i64::MAX));
            let resolved = venue.resolved();
            let settings = snapshot.settings();
            // The mask is exact when the core has one — a slot cannot exist outside it — and a
            // core without one stores nothing, so there is nothing to walk for it.
            let watched: Vec<(u8, moonproto::ArbPlatformCode)> = settings
                .client_settings
                .as_ref()
                .map(|s| &s.arb_config)
                .map(|cfg| {
                    (0u8..=255)
                        .filter_map(|byte| {
                            let code = platform_code(byte);
                            cfg.is_wanted(code).then_some((byte, code))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let mut readings: Vec<(Box<str>, String, VenueSet)> = Vec::new();
            let mut markets_per_code = [0u32; 256];
            for handle in snapshot.markets().iter() {
                let (trading, identity) = handle.with(|m| {
                    let label = MarketLabel {
                        coin: m.market_currency.clone(),
                        canonic: m.market_currency_canonic.clone(),
                        quote: String::new(),
                        contract: None,
                    };
                    (m.status_trading, identity_key(&label.identity()))
                });
                // Trading markets only: a market the exchange lists and does not trade is not a
                // place the coin can be traded, which is the question the column answers.
                if let (true, Some(found), false) = (trading, resolved, identity.is_empty()) {
                    book.catalogs
                        .entry((found.brand, found.is_spot()))
                        .or_default()
                        .insert(identity.clone());
                }
                if watched.is_empty() {
                    continue;
                }
                let mut quoted = VenueSet::default();
                for (byte, code) in &watched {
                    let Some(now) = handle.arb_now(*code) else {
                        continue;
                    };
                    if positive(f64::from(now.price)).is_none() {
                        continue;
                    }
                    // Within the window on BOTH sides. An unstamped entry lands decades in the
                    // past; a stamp far ahead of this clock is garbage of the other sign, and one
                    // slightly ahead — the two clocks are the same machine's, read through
                    // different calls — is a price that did arrive.
                    let age = now_ms.saturating_sub(now.time().unix_millis());
                    if !(-LISTING_MAX_AGE_MS..=LISTING_MAX_AGE_MS).contains(&age) {
                        continue;
                    }
                    quoted.insert(*byte);
                    markets_per_code[usize::from(*byte)] += 1;
                }
                // A market with no quote is not a reading at all: the stream does not know it,
                // and the donor's mask must not be credited to it.
                if !quoted.is_empty() {
                    readings.push((handle.name().into(), identity, quoted));
                }
            }
            if readings.is_empty() {
                continue;
            }
            // Per venue: carried once it sits on enough markets, believed once that has lasted.
            let mut asked = VenueSet::default();
            for (byte, _) in &watched {
                if markets_per_code[usize::from(*byte)] < MIN_CARRIED_MARKETS {
                    continue;
                }
                book.seen.insert(*byte);
                let key = (core, epoch, *byte);
                let since = carried.clock_of(key, now_ms, catalog_age_ms);
                next.since.insert(key, since);
                if warmed_up(now_ms, since) {
                    asked.insert(*byte);
                }
            }
            let markets = book.by_market.entry(venue.id).or_default();
            for (market, identity, quoted) in readings {
                let evidence = Evidence { quoted, asked };
                markets.entry(market).or_default().merge(&evidence);
                if !identity.is_empty() {
                    book.by_identity
                        .entry(identity)
                        .or_default()
                        .merge(&evidence);
                }
            }
        }
        (book, next)
    }

    /// Fill in where each market of one provider's catalog is listed.
    ///
    /// Args:
    ///     provider: The catalog provider the rows were read from; its venue is every row's.
    ///     rows: The provider's market rows, signed in place.
    ///     now_ms: Current unix time in milliseconds.
    pub(in crate::market) fn sign_listings(
        &self,
        provider: CoreId,
        rows: &mut [ScreenerRow],
        now_ms: i64,
    ) {
        let venue = self
            .inner
            .read()
            .expect("market source poisoned")
            .core_venue
            .get(&provider)
            .cloned();
        // An unidentified core — the synthetic feed, a connection still at its handshake — has no
        // exchange to ask about, and its rows keep the default: unknown.
        let Some(venue) = venue else {
            return;
        };
        let book = self.listing_book(now_ms);
        // One exchange for the whole catalog, so the book is narrowed to it once. An exchange
        // this build cannot name has no other kind to ask about.
        let Some(signer) = book.signer(venue.resolved(), venue.id) else {
            return;
        };
        for row in rows {
            row.listing = signer.sign(&row.market, &row.identity, row.futures_flag);
        }
    }
}

#[cfg(test)]
mod tests;
