//! The arbitrage book: every venue's price on a COIN, read once and shared by every chart.
//!
//! The prices do not belong to a core. Moonbot's server sends one arbitrage stream and a core only
//! files it into its own market objects (`apply_arb_payload`), gated on that core's own venue mask
//! — so `ENAUSDT`'s Gate price is the same number whichever connected core happens to hold it. What
//! IS per core is the second half of the old reading: `my_price`, the core's own market price at
//! the instant the quote landed, which is what the spread used to be stated against.
//!
//! That is the whole design. A quote is keyed by COIN and venue; the spread is computed against
//! whatever market is being charted at the moment it is read. A terminal with arbitrage configured
//! on one Binance core therefore prints a column on its Bybit charts too — same prices, spread
//! restated against Bybit's own last price.
//!
//! Two consequences worth stating, because both are load-bearing:
//!
//! - **Coverage is the union of the donors' universes.** A slot exists only on a market its core
//!   knows, so a Binance-only donor cannot quote a coin that trades nowhere on Binance. Enabling
//!   arbitrage on a second core of another exchange widens the set of coins rather than duplicating
//!   the numbers — the duplicates deduplicate to one row here.
//! - **A donor's own venue is not in its quotes.** Binance's price sits in `my_price`, not in a
//!   slot, so a Bybit chart fed by a Binance donor lists what the donor was WATCHING. Restating the
//!   donor itself as a row was considered and deliberately rejected: the column answers "what does
//!   this coin cost elsewhere", not "where did this reading come from".
//!
//! Cost. The protocol hands slots over one venue at a time, each behind the market lock, so the
//! read is priced in lock round trips. The book cuts them three ways: one read per COIN rather than
//! per pane (a stack of eight panes on one coin used to pay eight times), the venue walk is
//! restricted to the donor's own mask — a slot cannot exist outside it, `apply_arb_price` filters
//! on exactly that array — and the coin-to-market resolution behind it is cached far longer than
//! the quotes are.
//!
//! Locking. The book is a `Mutex` INSIDE the source's `RwLock`, and every critical section here is
//! a copy in or out with no call in between. Holding it across a source read would invert the order
//! `remove_client` takes (source write, then book) and deadlock.

use std::collections::HashMap;

use crate::session::CoreId;
use crate::util::time::now_unix_ms_i64;

use super::read::{platform_code, positive};
use super::{ArbQuote, ArbVenue, MarketDataSource};

/// How long a coin's quotes are reused before the slots are read again.
///
/// Matched to the chart's own arbitrage read period (`ARB_READ_PERIOD_MS`, 250 ms): the panes ask
/// on that clock, so one tick of the book serves the whole stack and a second pane on the same coin
/// costs nothing. Anything longer would make a coin switch visibly lag its chart.
const BOOK_TTL_MS: i64 = 250;

/// How long "which market is this coin on that core" is trusted.
///
/// A core's market universe changes when a listing appears or the core reconnects — minutes apart,
/// not milliseconds — while resolving it costs a search over that universe plus a label read per
/// hit. The miss is cheap and self-correcting; paying it four times a second would not be.
const MARKET_PICK_TTL_MS: i64 = 30_000;

/// How long the donor roster is trusted.
///
/// Rebuilding it takes a versioned snapshot per connected core, which on a terminal watching two
/// hundred cores is the one part of this read that scales with the core count. Whether a core has
/// arbitrage configured is a setting a human changes, so seconds of staleness cost nothing.
const DONORS_TTL_MS: i64 = 5_000;

/// Oldest quote the column will print, in milliseconds.
///
/// The spread is now stated against a LIVE price on the reader's own exchange, so a quote from
/// minutes ago would not read as stale — it would read as an opportunity that is not there. Sixty
/// seconds is far past the cadence of a coin anyone arbitrages and still catches the case this
/// exists for: a donor whose stream stopped while its snapshot stayed in memory.
const QUOTE_STALE_MS: i64 = 60_000;

/// Markets a coin's name is searched against on one donor.
///
/// The search ranks exact before prefix before contains, and the pick below wants the perpetual
/// among them; a coin token such as `ENA` matches a handful of contracts on any one exchange.
const SEARCH_LIMIT: usize = 32;

/// One venue's quote as the book holds it: about the COIN, before any chart's price is applied.
#[derive(Clone, Debug)]
pub(super) struct ArbRaw {
    pub venue: ArbVenue,
    pub dex_name: String,
    /// The venue's price, exactly as it arrived.
    pub price: f64,
    /// The DONOR's own market price at the same instant, or `0.0` when its market had no price yet.
    ///
    /// Kept so a chart on the donor itself keeps reading its spread from the pair of prices that
    /// existed at one moment — the reading this file inherited, which is strictly better than a
    /// live-price comparison and is available only there.
    pub my_price: f64,
    /// When the quote was filed, in unix milliseconds.
    ///
    /// Stamped by the receiving client from its own clock in UTC (`delphi_now_raw`), so it is
    /// comparable with local time and carries no server-clock skew.
    pub at_ms: i64,
    pub donor: CoreId,
    /// The DONOR's market the slot was read from.
    ///
    /// Carried so [`ArbRaw::my_price`] is only ever used for the market it belongs to: one core can
    /// hold a coin twice (`ENAUSDT` and `ENAUSDC`), and a pane on one of them must not state its
    /// spread from the other's price.
    pub market: String,
    pub deposit_blocked: bool,
    pub withdraw_blocked: bool,
}

impl ArbRaw {
    /// The key two quotes are the same row under.
    ///
    /// The same rule the chart's click and its dimming already use: an ordinary exchange is its
    /// platform code and must not match a venue carrying a DEX name, while every Hyperliquid
    /// deployer shares one code and is told apart by that name alone.
    fn key(&self) -> (u8, &str) {
        (self.venue.code(), self.dex_name.as_str())
    }
}

/// Quotes for one coin, and when they were read.
struct CoinEntry {
    built_ms: i64,
    rows: Vec<ArbRaw>,
}

/// Which market carries a coin on one donor, and when that was resolved.
///
/// `None` is cached as deliberately as a hit: "this donor does not trade this coin" is the common
/// answer for a coin listed on one exchange, and re-searching the universe for it four times a
/// second is the cost this cache exists to avoid.
struct MarketPick {
    at_ms: i64,
    market: Option<String>,
}

/// The read-once-per-coin cache behind [`MarketDataSource::market_arb`].
#[derive(Default)]
pub(super) struct ArbBook {
    coins: HashMap<String, CoinEntry>,
    markets: HashMap<(String, String, CoreId), MarketPick>,
    /// Cores with arbitrage configured, ascending, and when the roster was built.
    donors: Option<(i64, Vec<CoreId>)>,
    /// Bumped whenever a core is forgotten.
    ///
    /// A rebuild reads slots with the book unlocked, so one that started before a core was removed
    /// would otherwise write its rows back afterwards and resurrect the core that just went away.
    /// The writer compares this and drops its result instead.
    generation: u64,
}

impl ArbBook {
    /// Drop entries nothing will read again.
    ///
    /// Both maps are keyed by COIN, so without this they would grow by one entry for every coin
    /// ever charted in the session and never shrink — a slow leak that looks like a cache. The
    /// bounds are generous multiples of the two lifetimes: an entry past them would be rebuilt on
    /// its next read anyway, so keeping it buys nothing.
    ///
    /// Args:
    ///     now: Current unix time in milliseconds.
    fn prune(&mut self, now: i64) {
        self.coins
            .retain(|_, entry| now.saturating_sub(entry.built_ms) < BOOK_TTL_MS * 40);
        self.markets
            .retain(|_, pick| now.saturating_sub(pick.at_ms) < MARKET_PICK_TTL_MS * 4);
    }

    /// Forget everything about one core.
    ///
    /// Called when a client is replaced or removed: its quotes name it as their donor and its
    /// market picks are answers about a universe that is gone.
    pub(super) fn forget_core(&mut self, core: CoreId) {
        self.coins.clear();
        self.markets.retain(|(_, _, donor), _| *donor != core);
        self.donors = None;
        self.generation = self.generation.wrapping_add(1);
    }
}

/// What one market costs right now: its last trade, or the ask when it has not traded yet.
///
/// Stated once because both readings here ask it — the chart's own price, which every borrowed
/// spread is divided by, and the donor's, which stands in for a quote that arrived before its
/// market had a price. Two spellings of it would let a spread's two halves come from two rules.
fn last_or_ask(handle: &moonproto::state::MarketHandle) -> Option<f64> {
    handle.with(|m| positive(m.price.p_last).or_else(|| positive(m.price.ask)))
}

/// Whether two markets' quote currencies price a coin the same way.
///
/// Identical currencies obviously do; so do two USD stablecoins, whose rates differ by a fraction
/// of the spreads this column exists to show — and a COIN-M chart quoted in `USD` has to be able to
/// borrow from a USDT donor, which is the case that makes this more than an equality test.
///
/// Everything else is refused rather than converted: a `BTC`-quoted market's price is a different
/// number about a different thing, and no rate is at hand here to make it one.
///
/// An unknown quote on either side is treated as comparable. The catalog leaves it empty for a
/// market it does not hold, and refusing those would empty the column for a coin the terminal is
/// showing.
///
/// Args:
///     chart: Quote currency of the charted market, uppercase.
///     donor: Quote currency of the candidate market on the donor, uppercase.
///
/// Returns:
///     `true` when a price from the donor's market can be compared with the chart's.
fn quotes_comparable(chart: &str, donor: &str) -> bool {
    if chart.is_empty() || donor.is_empty() || chart.eq_ignore_ascii_case(donor) {
        return true;
    }
    crate::symbol::is_usd_stable(chart) && crate::symbol::is_usd_stable(donor)
}

impl MarketDataSource {
    /// Every arbitrage price known for the coin on `market`, restated against that market's price.
    ///
    /// See [`super::read::MarketDataSource::market_arb`] for the caller's contract; this is its
    /// whole body.
    pub(super) fn arb_quotes(&self, core: CoreId, market: &str) -> Option<Vec<ArbQuote>> {
        let label = self.market_label(core, market);
        // The CORE's own identity for the coin, not its spelling: a Bybit chart on `1kBONK` and a
        // Binance donor holding `1000BONK` are the same book entry, and only the catalog can say
        // so — see `MarketLabel::canonic`.
        let coin_key = label.identity();
        // No coin, no book: the market has not been labelled yet, which happens between a pane
        // being assigned a market and the catalog arriving. Reading the pane's OWN core directly is
        // the one answer available, and it is the answer this used to give.
        let rows = match coin_key.is_empty() {
            true => self.scan_donor(core, market),
            false => self.arb_rows(&coin_key, &label, core, market),
        };
        // The chart's own price, which every borrowed row states its spread against.
        let own_price = self.arb_own_price(core, market);
        // The pane's own venue never appears in its own column: a row reading `BybitF +0.00%` on a
        // Bybit chart is not a spread, and it is the one row a reader would act on by mistake. Its
        // SPOT counterpart is a different venue and stays.
        let own_venue = self.core_venue_of(core);
        let mut out: Vec<ArbQuote> = Vec::new();
        for row in Self::arb_dedupe(rows, core) {
            if own_venue
                .as_ref()
                .is_some_and(|v| v.matches_arb(row.venue.code(), &row.dex_name))
            {
                continue;
            }
            // The donor's own pair of prices where it exists — both stamped at one instant, which
            // is what makes a percentage a spread rather than a subtraction of two moments. Only
            // for the SAME market: one core can hold `ENAUSDT` and `ENAUSDC`, and the quote a pane
            // borrows from the other one carries the other one's price.
            let base = match row.donor == core && row.market == market {
                true => positive(row.my_price).or(own_price),
                false => own_price,
            };
            let Some(my_price) = base else {
                continue;
            };
            out.push(ArbQuote {
                venue: row.venue,
                dex_name: row.dex_name,
                price: row.price,
                my_price,
                spread_pct: (row.price - my_price) / my_price * 100.0,
                deposit_blocked: row.deposit_blocked,
                withdraw_blocked: row.withdraw_blocked,
            });
        }
        Some(out)
    }

    /// The charted market's own price, for the spread's base.
    ///
    /// The market-data PROVIDER first, exactly like `market_ticker` and every other readout the
    /// chart draws its price from — the spread has to be stated against the number on screen. A
    /// pane's own core is second and not first for the same reason: under deduplication it may not
    /// be the one draining prices, and a core that has stopped draining keeps its last price
    /// frozen in the snapshot, which would state a spread that grows on its own.
    fn arb_own_price(&self, core: CoreId, market: &str) -> Option<f64> {
        let read = |from: CoreId| -> Option<f64> {
            let snapshot = self.core_client(from)?.snapshot_versioned()?;
            last_or_ask(&snapshot.markets().get(market)?)
        };
        self.provider_of(core).and_then(read).or_else(|| read(core))
    }

    /// Keep one row per venue: the reader's OWN core first, then the freshest quote.
    ///
    /// The numbers are the same wherever they were filed, so this picks for two other reasons. The
    /// reader's own core wins because only there does a quote carry the matching `my_price`, which
    /// makes a strictly better spread. Otherwise the freshest wins, and a tie goes to the lowest
    /// core id — cores live in a `HashMap`, and letting iteration order decide would let two panes
    /// on one coin state two different prices.
    fn arb_dedupe(rows: Vec<ArbRaw>, core: CoreId) -> Vec<ArbRaw> {
        let mut best: Vec<ArbRaw> = Vec::with_capacity(rows.len());
        for row in rows {
            match best.iter_mut().find(|kept| kept.key() == row.key()) {
                Some(kept) => {
                    let better = match (kept.donor == core, row.donor == core) {
                        (true, false) => false,
                        (false, true) => true,
                        _ => {
                            (row.at_ms, std::cmp::Reverse(row.donor))
                                > (kept.at_ms, std::cmp::Reverse(kept.donor))
                        }
                    };
                    if better {
                        *kept = row;
                    }
                }
                None => best.push(row),
            }
        }
        // Stable print order for the venues the roster does not list: `arrange` appends those in
        // the order they arrive, and iteration over cores must not reshuffle them between reads.
        best.sort_by(|a, b| a.key().cmp(&b.key()));
        best
    }

    /// The book's rows for one coin, read afresh when the entry has expired.
    ///
    /// Args:
    ///     coin_key: The coin's cross-exchange identity, the book's key.
    ///     label: How the CHARTED market is named, for the coin to search donors by and the quote
    ///         currency their markets have to agree with.
    ///     core: The pane's core, read directly when no core has arbitrage configured.
    ///     market: The pane's market, for that same fallback.
    fn arb_rows(
        &self,
        coin_key: &str,
        label: &super::MarketLabel,
        core: CoreId,
        market: &str,
    ) -> Vec<ArbRaw> {
        let now = now_unix_ms_i64();
        let book = self.arb_book();
        let generation = {
            let guard = book.lock().expect("arb book poisoned");
            if let Some(entry) = guard.coins.get(coin_key) {
                if now.saturating_sub(entry.built_ms) < BOOK_TTL_MS {
                    return entry.rows.clone();
                }
            }
            guard.generation
        };
        let donors = self.arb_donors(now);
        // No core has arbitrage configured at all — or none has sent its settings yet, which is
        // what an older core build does. Then there is no book to build: read the pane's own core
        // the way this did before the book existed, and cache nothing, because the answer belongs
        // to that core rather than to the coin.
        if donors.is_empty() {
            return self.scan_donor(core, market);
        }
        let mut rows = Vec::new();
        for donor in donors {
            let Some(donor_market) = self.arb_donor_market(coin_key, label, donor, now) else {
                continue;
            };
            rows.extend(self.scan_donor(donor, &donor_market));
        }
        {
            let mut guard = book.lock().expect("arb book poisoned");
            // A core was forgotten while the slots were being read, so these rows may name it. They
            // are one read old at most; the next one rebuilds them without it.
            if guard.generation == generation {
                guard.prune(now);
                guard.coins.insert(
                    coin_key.to_string(),
                    CoinEntry {
                        built_ms: now,
                        rows: rows.clone(),
                    },
                );
            }
        }
        rows
    }

    /// Cores that have arbitrage configured, ascending.
    ///
    /// The mask is the test, and it is exact rather than a guess: `apply_arb_price` files a slot
    /// only for a platform the mask asks for, so a core with an empty mask holds no arbitrage at
    /// all and a core with a non-empty one holds whatever the server sent for it.
    fn arb_donors(&self, now: i64) -> Vec<CoreId> {
        let book = self.arb_book();
        if let Some((at, donors)) = book.lock().expect("arb book poisoned").donors.as_ref() {
            if now.saturating_sub(*at) < DONORS_TTL_MS {
                return donors.clone();
            }
        }
        let cores = {
            let inner = self.inner.read().expect("market source poisoned");
            let mut cores: Vec<CoreId> = inner.clients.keys().copied().collect();
            cores.sort_unstable();
            cores
        };
        let donors: Vec<CoreId> = cores
            .into_iter()
            .filter(|core| {
                self.core_client(*core)
                    .and_then(|client| client.snapshot_versioned())
                    .and_then(|snapshot| {
                        snapshot
                            .settings()
                            .client_settings
                            .as_ref()
                            .map(|s| s.arb_config.wanted_platforms().next().is_some())
                    })
                    .unwrap_or(false)
            })
            .collect();
        book.lock().expect("arb book poisoned").donors = Some((now, donors.clone()));
        donors
    }

    /// Which market carries this coin on one donor, resolved through the shared coin search.
    ///
    /// The same enumeration the coin picker and the arbitrage click use, so a Hyperliquid spot
    /// index or a contract tail resolves here the way it does everywhere else rather than by a
    /// reading of the market's name.
    ///
    /// Two things it will not do. It searches by the coin WITHOUT its contract tail — a COIN-M
    /// chart's `BTC_RP` matches nothing on a USD-M donor, and the miss would be cached for half a
    /// minute — and it accepts only a market whose QUOTE currency is comparable with the chart's.
    /// Without that a spot donor's `ENABTC` would be divided by a USDT price and print −99.99 % on
    /// every venue, which reads as a market-wide collapse rather than as a mismatched pair.
    ///
    /// Which of several matching markets it takes is [`super::pick_market_for_identity`]'s rule:
    /// the perpetual over an expiry, the chart's own quote currency first. A donor read from a
    /// dated contract would state every spread against that contract's basis.
    fn arb_donor_market(
        &self,
        coin_key: &str,
        label: &super::MarketLabel,
        donor: CoreId,
        now: i64,
    ) -> Option<String> {
        // Keyed by the QUOTE as well as the coin: the answer depends on it twice — a candidate
        // whose currency is not comparable is refused outright, and the pick prefers the chart's
        // own. Without it a USDC chart's answer would be served to a USDT one, and a `None` cached
        // by either would blank the other's column for half a minute.
        let cache_key = (
            coin_key.to_string(),
            label.quote.to_ascii_uppercase(),
            donor,
        );
        let book = self.arb_book();
        if let Some(pick) = book
            .lock()
            .expect("arb book poisoned")
            .markets
            .get(&cache_key)
        {
            if now.saturating_sub(pick.at_ms) < MARKET_PICK_TTL_MS {
                return pick.market.clone();
            }
        }
        // Searched by the IDENTITY, not by this chart's own spelling. A donor's catalog is matched
        // against the literal query — `market_search_quality` compares it with the six name fields
        // — and `1kBONK` appears in none of a Binance market's, so searching the chart's token
        // found nothing and cached the miss. The identity is `canonic`, which every core's catalog
        // does carry and which its search ranks among those fields.
        let labelled: Vec<(String, super::MarketLabel)> = self
            .labelled_search(donor, coin_key, SEARCH_LIMIT)
            .into_iter()
            .filter(|(_, found)| quotes_comparable(&label.quote, &found.quote))
            .collect();
        let market =
            super::pick_market_for_identity(&labelled, coin_key, &label.quote).map(str::to_string);
        book.lock().expect("arb book poisoned").markets.insert(
            cache_key,
            MarketPick {
                at_ms: now,
                market: market.clone(),
            },
        );
        market
    }

    /// Every arbitrage slot ONE core holds for one of its markets.
    ///
    /// Venues the core is not watching (`enabled == false`) never appear: the switch is the core's
    /// own, and a row for a venue that reports nothing would be a permanently empty line.
    fn scan_donor(&self, donor: CoreId, market: &str) -> Vec<ArbRaw> {
        let Some(snapshot) = self
            .core_client(donor)
            .and_then(|client| client.snapshot_versioned())
        else {
            return Vec::new();
        };
        let Some(handle) = snapshot.markets().get(market) else {
            return Vec::new();
        };
        // This market's own price, for a point that carries none of its own.
        let own_price = last_or_ask(&handle).unwrap_or(0.0);
        // WHICH venues to ask about. The core's own mask when it has one, because a slot cannot
        // exist outside it — `apply_arb_price` tests exactly this array before filing — and the
        // mask can name a venue this build has never heard of, such as the reference terminal's
        // `OkxF` column. An all-false mask is treated as no mask at all: it arrives that way before
        // the settings land, and trusting it emptied a column that had been printing one.
        let settings = snapshot.settings();
        let wanted = settings
            .client_settings
            .as_ref()
            .map(|s| &s.arb_config)
            .filter(|cfg| cfg.wanted_platforms().next().is_some());
        // Resolved on the first deployer that actually reports, not up front: the list costs an
        // auth-info read and a clone per scan, and most cores quote no deployer at all.
        let mut dex_names: Option<Vec<String>> = None;
        let now = now_unix_ms_i64();
        let mut out = Vec::new();
        let mut dropped_stale = 0usize;
        for byte in 0u8..=255 {
            let venue = ArbVenue::from_code(byte);
            let asked = match wanted {
                Some(cfg) => cfg.is_wanted(platform_code(byte)),
                None => venue.is_known_or_scanned_deployer(),
            };
            if !asked {
                continue;
            }
            let Some(slot) = handle.arb_slot(platform_code(byte)).filter(|s| s.enabled) else {
                continue;
            };
            let point = slot.latest_point();
            // The ring's newest point, or the "now" entry when the ring has not been written yet:
            // the core stamps both, and dropping the venue because only one of them has arrived
            // hides a price the reference terminal is already showing. Each carries its own stamp,
            // so the age below is the age of the price actually taken.
            let (price, at_ms) = match positive(f64::from(point.price)) {
                Some(price) => (price, point.time().unix_millis()),
                None => match positive(f64::from(slot.now.price)) {
                    Some(price) => (price, slot.now.time().unix_millis()),
                    None => continue,
                },
            };
            // A price this old is not stale data, it is a wrong answer: the spread a reader sees is
            // stated against a live price on their own exchange, so an old quote invents a gap that
            // has since closed. An unstamped point lands here too, far in the past.
            if now.saturating_sub(at_ms) > QUOTE_STALE_MS {
                dropped_stale += 1;
                continue;
            }
            out.push(ArbRaw {
                venue,
                dex_name: match venue.deployer_index() {
                    Some(index) => dex_names
                        .get_or_insert_with(|| self.arb_dex_names(donor))
                        .get(usize::from(index))
                        .cloned()
                        .unwrap_or_default(),
                    None => String::new(),
                },
                price,
                my_price: positive(f64::from(point.my_price)).unwrap_or(own_price),
                at_ms,
                donor,
                market: market.to_string(),
                deposit_blocked: slot.isolated_flags.deposit_blocked(),
                withdraw_blocked: slot.isolated_flags.withdraw_blocked(),
            });
        }
        // What the donor watches, what actually reported, and what went out on age. Behind
        // `log.market_sources` in cfg/diagnostics.toml, because this is the one question the
        // sources cannot answer: the codes are the core's, and a venue this build cannot name shows
        // up here as its raw byte.
        if log::log_enabled!(super::SOURCE_TRACE_LEVEL) {
            log::log!(
                super::SOURCE_TRACE_LEVEL,
                "арбитраж {market} донор {donor:?}: маска {} · слотов отдали {} {:?} · протухших {dropped_stale} · дексы {:?}",
                match wanted {
                    Some(cfg) => format!("{:?}", cfg.wanted_platforms().collect::<Vec<_>>()),
                    None => "НЕТ (ядро не сохраняет арб вообще)".to_string(),
                },
                out.len(),
                out.iter().map(|q| q.venue.code()).collect::<Vec<_>>(),
                dex_names.unwrap_or_default(),
            );
        }
        out
    }

    /// The shared book handle, taken without holding the source lock across the work behind it.
    fn arb_book(&self) -> std::sync::Arc<std::sync::Mutex<ArbBook>> {
        self.inner
            .read()
            .expect("market source poisoned")
            .arb_book
            .clone()
    }

    /// The venue one core is connected to, as the session last identified it.
    fn core_venue_of(&self, core: CoreId) -> Option<crate::venue::CoreVenue> {
        self.inner
            .read()
            .expect("market source poisoned")
            .core_venue
            .get(&core)
            .cloned()
    }
}

#[cfg(test)]
mod tests;
