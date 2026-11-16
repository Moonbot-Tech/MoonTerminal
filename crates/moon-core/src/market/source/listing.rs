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
//!   not by when this file first looked. While anything is still settling the book is replaced
//!   every few seconds instead ([`MIN_REBUILD_MS`]), and at once when a client connects or gets
//!   its first market list, so the answer waited for arrives with the minute and not up to a
//!   period after it;
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
    Wait, identity_key,
};
use crate::session::CoreId;

use super::read::{platform_code, positive};
use super::{MarketDataSource, MarketLabel};

/// How long one book serves before the cores are read again.
const BOOK_TTL_MS: i64 = 30_000;

/// How long a book serves while something in it is still settling.
///
/// A donor's minute ends a few seconds after some build, and its answer must not then sit out
/// the rest of [`BOOK_TTL_MS`] as well; nor should the prices a just-connected donor is filling
/// with wait half a minute to show. So while a warm-up runs, or a client is younger than the
/// warm-up, the book is replaced this often — a handful of extra builds for the minute it lasts,
/// against one every period afterwards.
const MIN_REBUILD_MS: i64 = 5_000;

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
/// A client the builds HAVE been reading, and which has been connected for the whole warm-up
/// already, gets no such credit for a venue that turns up now: a build ago it was not carrying
/// it, so that venue was switched on in its mask and its slots are filling at this moment,
/// whatever the connection's age. A client still younger than the warm-up is different — every
/// venue of it has been filling since it connected, and the one that crosses the carried
/// threshold a build later is not a new stream. Counting that one from the build would add up to
/// a book period to the minute, on exactly the start the user is watching.
///
/// What this cannot see is a venue switched on in a donor's mask while nobody watched and looked
/// at within the few seconds its slots take to fill; that reads warm early, for one book period
/// at most.
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
    match catalog_age_ms {
        Some(age) if !read_before || age < DONOR_WARMUP_MS => now_ms.saturating_sub(age.max(0)),
        _ => now_ms,
    }
}

/// When a client stops being young: [`DONOR_WARMUP_MS`] after it first held a market list.
///
/// Args:
///     now_ms: Current unix time in milliseconds.
///     catalog_age_ms: How long ago the client first applied a market list.
///
/// Returns:
///     That moment while it is still ahead; `None` for a client past it, or with no list at all —
///     which has no connection to count from, and is not promised an answer.
fn young_until(now_ms: i64, catalog_age_ms: Option<i64>) -> Option<i64> {
    let until = warm_origin(now_ms, false, catalog_age_ms).saturating_add(DONOR_WARMUP_MS);
    (catalog_age_ms.is_some() && until > now_ms).then_some(until)
}

/// Whether a hold counted from `since` has lasted [`DONOR_WARMUP_MS`].
fn warmed_up(now_ms: i64, since: i64) -> bool {
    now_ms.saturating_sub(since) >= DONOR_WARMUP_MS
}

/// One core's client as the cache compares it between builds: the core, and its slot's epoch and
/// catalog version — `None` while the slot is empty.
type ClientRevision = (CoreId, Option<(u64, u64)>);

/// Whether a donor carries a venue: on enough markets to be a stream it receives, or — once it
/// has been seen so — on at least half as many.
///
/// The second half is what keeps a thin venue from flickering. A deployer the stream quotes on
/// twenty markets of a donor drifts under ten as its prices age out and back over as they tick;
/// dropping its clock at each dip restarted its warm-up, took the one-sided answers of every row
/// that needs it back for a minute, and repeated for as long as it hovered. Half, and not "any
/// price at all": a stream that has stopped keeps its last prices for ten minutes, and a venue
/// held on to down to the very last of them would go on answering for markets whose own price
/// had long expired.
///
/// Args:
///     markets: How many of the donor's markets hold a fresh price from the venue.
///     had_clock: Whether the previous build counted the venue as carried.
fn carries(markets: u32, had_clock: bool) -> bool {
    markets >= MIN_CARRIED_MARKETS || (had_clock && markets >= MIN_CARRIED_MARKETS / 2)
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
    /// When the kept book ages out ahead of [`BOOK_TTL_MS`]: [`MIN_REBUILD_MS`] after a build
    /// that found something still settling ([`ListingBook::settling`]). `None` otherwise.
    due_ms: Option<i64>,
    /// The clients the kept book was built from; see [`Self::note_revisions`].
    revisions: Vec<ClientRevision>,
    /// Whether the clients changed while a build was in flight.
    stale_mid_build: bool,
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
        let fresh = !self.expired
            && age.is_some_and(|age| (0..BOOK_TTL_MS).contains(&age))
            && self.due_ms.is_none_or(|due| now_ms < due);
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
        self.expired = !forgotten.is_empty() || std::mem::take(&mut self.stale_mid_build);
        self.built_ms = Some(now_ms);
        self.due_ms = book.settling.then(|| now_ms.saturating_add(MIN_REBUILD_MS));
        self.book = book;
        self.warm = warm;
    }

    /// Compare the clients as they are now with the ones the kept book was built from.
    ///
    /// A client installed, replaced, or holding its first market list since then is one the book
    /// has not read, and the book is stale whatever its age: a donor that connects two seconds
    /// after a build would otherwise stay invisible for the rest of the period, its exchange
    /// reading "nothing answers" while the answer is a minute away.
    ///
    /// Args:
    ///     revisions: Every core's client epoch and catalog version, in a stable order.
    fn note_revisions(&mut self, revisions: Vec<ClientRevision>) {
        if self.revisions != revisions {
            self.revisions = revisions;
            self.expired = true;
            // A build in flight may have read the clients before this change; the book it
            // lands must not pass for fresh.
            self.stale_mid_build = self.building;
        }
    }

    /// Give the build right back without a book, after a build that did not finish.
    fn abandon(&mut self) {
        self.building = false;
        self.forgotten_mid_build.clear();
        self.stale_mid_build = false;
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
        let (cache, revisions) = {
            let inner = self.inner.read().expect("market source poisoned");
            // Sorted, so two readings of the same clients compare equal whatever order the map
            // hands them out in.
            let mut revisions: Vec<ClientRevision> = inner
                .clients
                .iter()
                .map(|(core, slot)| (*core, slot.catalog_revision()))
                .collect();
            revisions.sort_unstable_by_key(|(core, _)| *core);
            (inner.listing_cache.clone(), revisions)
        };
        // One block, so the cache lock is released before the build below.
        let claim = {
            let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
            guard.note_revisions(revisions);
            guard.claim(now_ms)
        };
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
        // The answers still owed, and what decides which of them count; settled once every
        // donor has been read, because one donor's refusal bears on another's promise.
        let mut waits: Vec<Wait> = Vec::new();
        let mut settled = VenueSet::default();
        let mut refused = VenueSet::default();
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
            // Per venue: carried once it sits on enough markets, believed once that has lasted.
            // Until then the answer is on its way, and the book says when: for a carried venue
            // when its own clock runs out, and for one a YOUNG client's mask names but has not
            // filled yet, when that client stops being young. Past that a mask alone promises
            // nothing — it names venues the stream never sends, and a client that has asked for
            // one for a whole warm-up without getting it is the proof. Done before the donor is
            // skipped for having no reading: in its first seconds it has none, and is the one
            // waited for.
            let young_end = young_until(now_ms, catalog_age_ms);
            // A young donor is still filling: what it holds now is not what it will hold in five
            // seconds. So is a young client whose settings have not arrived — it may turn out to
            // be a donor. A young core that is known to have no mask adds nothing to wait for:
            // most cores are such, and each of them reconnecting must not put the book on the
            // short period for a minute.
            book.settling |=
                young_end.is_some() && (!watched.is_empty() || settings.client_settings.is_none());
            let mut asked = VenueSet::default();
            for (byte, _) in &watched {
                let key = (core, epoch, *byte);
                let markets = markets_per_code[usize::from(*byte)];
                if !carries(markets, carried.since.contains_key(&key)) {
                    match young_end {
                        Some(until) => waits.push(Wait {
                            exchange: venue.id,
                            code: *byte,
                            settles_ms: until,
                            promised: true,
                        }),
                        // Only a client that HAS a market list and has held it for the whole
                        // warm-up refuses; one with no list on record proves nothing.
                        None if catalog_age_ms.is_some() => refused.insert(*byte),
                        None => {}
                    }
                    continue;
                }
                book.seen.insert(*byte);
                book.carried_by.entry(venue.id).or_default().insert(*byte);
                let since = carried.clock_of(key, now_ms, catalog_age_ms);
                next.since.insert(key, since);
                match warmed_up(now_ms, since) {
                    true => {
                        asked.insert(*byte);
                        settled.insert(*byte);
                    }
                    false => waits.push(Wait {
                        exchange: venue.id,
                        code: *byte,
                        settles_ms: since.saturating_add(DONOR_WARMUP_MS),
                        promised: false,
                    }),
                }
            }
            if readings.is_empty() {
                continue;
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
        book.note_waits(&waits, &settled, &refused);
        (book, next)
    }

    /// When the kept book expects its last pending answer, in Unix milliseconds.
    ///
    /// For the Screener's "still being collected" line. Reads the kept book and never builds one,
    /// so it costs a lock and may be called from anywhere; `None` while nothing is warming up or
    /// before the first book.
    pub fn listing_settles_ms(&self) -> Option<i64> {
        let cache = self
            .inner
            .read()
            .expect("market source poisoned")
            .listing_cache
            .clone();
        let guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        guard.book.last_settles_ms
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
