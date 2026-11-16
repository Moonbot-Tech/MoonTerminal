//! Whether a coin trades on BOTH market kinds of one exchange, or only on the kind a row is on.
//!
//! The Screener prints it per market: `Both`, `Spot Only`, `Futures Only`, or unknown. The words
//! are Moonbot's — its `ListedType` strategy filter uses the same four — but the answer here is
//! assembled in the terminal, because a core only knows the exchange it is connected to.
//!
//! The decision is this file; where the evidence comes from is [`super::source`]'s business. Three
//! kinds of evidence, in the order they are trusted:
//!
//! 1. **The arbitrage stream.** Moonbot's server quotes one price per venue per coin, spot and
//!    futures under separate codes, and maps a coin across exchanges itself — `1000BONK` on one,
//!    `BONK` on another. A market holding a price from its own brand's other kind is on both, with
//!    no core of that kind connected. Measured 2026-11-16 across 28 cores: the stream covers 85–99 %
//!    of a futures core's trading markets and is full within half a minute of a start.
//! 2. **Catalogs of connected cores**, by the coin's identity. The only source for a kind the
//!    stream never quotes — Hyperliquid spot and Binance COIN-M, both measured at zero — and only
//!    ever a POSITIVE one: the same coin is spelled `LUNA2` on Binance futures and `LUNA` on
//!    Binance spot, so a missing identity proves nothing.
//! 3. **The core's own `futures_type` flag**, positive only as well, and only on the venues where
//!    it was measured to mean a cross-listing.
//!
//! `Only` is the hard answer and has one source: the stream. It takes a market the stream DOES
//! quote somewhere, a donor that asks about the other kind, and a kind the stream quotes at all.
//! A market with no quote from anywhere is unknown, never `Only` — that is how a Bybit USDC
//! perpetual (`1000BONKPERP`) and a Hyperliquid `kPEPE` read, which the server maps to nothing
//! although their coin has a spot market. The price of that rule is that a coin genuinely listed
//! nowhere else (`BTCDOM`) reads unknown too; decided by the owner on 2026-11-16.

use std::collections::{HashMap, HashSet};

use crate::feed::ExchangeId;
use crate::venue::Brand;

/// Oldest arbitrage price that still counts as "the coin trades there".
///
/// A slot outlives the listing behind it until the core reconnects, so presence alone would keep
/// a halted or delisted market alive in this answer. Ten minutes rather than the chart column's
/// one: the question here is whether a market exists, not what it costs right now, and a thinly
/// traded coin goes minutes between prices (measured: 20–30 of ~430 Gate spot quotes on one donor
/// were older than a minute and none of them stale).
pub const LISTING_MAX_AGE_MS: i64 = 600_000;

/// How long a donor must have been carrying a venue before its SILENCE about it is believed.
///
/// A slot appears with its first price, so for the first seconds after a connection every market
/// of a donor reads "quoted nowhere" and then fills venue by venue. A positive answer is safe at
/// any moment; a negative one taken that early would call half an exchange futures-only.
pub const DONOR_WARMUP_MS: i64 = 60_000;

/// Fewest markets a donor must hold a venue's price on for that venue to count as carried.
///
/// One stray price for a venue the stream does not really send must not make that venue
/// "answerable" for a whole exchange — every market without it would then read one-sided. Ten is
/// far below any venue the stream does carry (the thinnest measured, a Hyperliquid deployer, sat
/// on about twenty markets of a donor) and far above an accident.
pub const MIN_CARRIED_MARKETS: u32 = 10;

/// Where a coin is listed, relative to the row's own exchange and market kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Listing {
    /// Nothing answers: an exchange this build cannot name, a market the stream maps to nothing,
    /// or no source for the other kind at all.
    #[default]
    Unknown,
    /// On both the spot and the derivatives market of this exchange.
    Both,
    /// On this exchange's spot market only.
    SpotOnly,
    /// On this exchange's derivatives market only.
    FuturesOnly,
}

/// A set of arbitrage platform codes.
///
/// A plain 256-bit mask: a book holds one or two per market of every donor, and the codes are
/// bytes, so a `HashSet` per market would cost more than everything it describes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VenueSet([u64; 4]);

impl VenueSet {
    /// Add one platform code.
    pub fn insert(&mut self, code: u8) {
        self.0[usize::from(code >> 6)] |= 1 << (code & 63);
    }

    /// Whether the set holds `code`.
    pub const fn contains(&self, code: u8) -> bool {
        self.0[(code >> 6) as usize] & (1 << (code & 63)) != 0
    }

    /// Whether the set holds nothing.
    pub const fn is_empty(&self) -> bool {
        self.0[0] == 0 && self.0[1] == 0 && self.0[2] == 0 && self.0[3] == 0
    }

    /// Add every code of `other`.
    pub fn extend(&mut self, other: &Self) {
        for (mine, theirs) in self.0.iter_mut().zip(other.0) {
            *mine |= theirs;
        }
    }

    /// The codes both sets hold.
    pub fn intersection(&self, other: &Self) -> Self {
        let mut out = *self;
        for (mine, theirs) in out.0.iter_mut().zip(other.0) {
            *mine &= theirs;
        }
        out
    }

    /// Whether the two sets share a code.
    pub fn intersects(&self, other: &Self) -> bool {
        !self.intersection(other).is_empty()
    }

    /// Whether every code of this set is in `other`.
    pub fn is_subset(&self, other: &Self) -> bool {
        self.intersection(other) == *self
    }
}

/// What the arbitrage stream says about one market, or about one coin across donors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Evidence {
    /// Venues holding a price no older than [`LISTING_MAX_AGE_MS`].
    pub quoted: VenueSet,
    /// Venues a donor has CARRIED for at least [`DONOR_WARMUP_MS`] — the only ones whose absence
    /// from `quoted` means anything.
    ///
    /// Carried, not merely ticked in the donor's mask: a mask can name a venue the stream never
    /// sends, and a venue ticked a moment ago is in the mask before a single price for it has
    /// arrived. So a code enters here once the donor holds it on enough markets to be receiving
    /// it, and has done so for the warm-up.
    pub asked: VenueSet,
}

impl Evidence {
    /// Fold another reading of the same subject into this one.
    pub fn merge(&mut self, other: &Self) {
        self.quoted.extend(&other.quoted);
        self.asked.extend(&other.asked);
    }
}

/// The arbitrage codes of one brand's OTHER market kind.
///
/// Spot's other kind is every derivatives venue of the brand — perpetuals and Binance's
/// coin-margined contracts alike — and for Hyperliquid also every HIP-3 deployer, which the
/// stream numbers apart from the exchange codes ([`crate::market::ArbVenue::deployer_index`]): a
/// deployer's market is a Hyperliquid perpetual under another roof.
///
/// Args:
///     brand: The row's exchange brand.
///     spot: Whether the row itself is on the spot market.
///
/// Returns:
///     The codes whose quote would prove the coin is on both kinds.
pub fn other_side(brand: Brand, spot: bool) -> VenueSet {
    let mut out = VenueSet::default();
    for code in 0u8..=255 {
        let Some(venue) = crate::venue::venue(code) else {
            continue;
        };
        if venue.brand == brand && venue.is_spot() != spot {
            out.insert(code);
        }
    }
    if spot && brand == Brand::Hyperliquid {
        for code in 0u8..=255 {
            if crate::market::ArbVenue::from_code(code)
                .deployer_index()
                .is_some()
            {
                out.insert(code);
            }
        }
    }
    out
}

/// Whether a core on this platform code sets `futures_type` to mean "listed on the other kind".
///
/// A table of what was MEASURED (2026-11-16), not of what the field is documented to be: on the
/// futures cores of Bybit, Binance, Gate and OKX it was set exactly where the coin has a spot
/// market (OKX: 221 markets, against 221 the stream quotes on OKX spot), and on Binance spot where
/// a futures contract exists in the core's own quote currency. Elsewhere it answers nothing —
/// unset on every market of Gate spot, Hyperliquid and Binance COIN-M — or something else: on the
/// perpetual-only venues every market carries its settlement currency there.
///
/// Args:
///     code: Platform ordinal of the core the market was read from.
///
/// Returns:
///     `true` where a set flag is evidence of a cross-listing.
pub const fn futures_flag_means_listing(code: u8) -> bool {
    matches!(code, 2 | 3 | 4 | 9 | 15)
}

/// One row's facts, as the decision needs them.
#[derive(Clone, Copy, Debug)]
pub struct RowFacts<'a> {
    /// Brand and kind of the row's exchange; `None` for a platform this build cannot name.
    pub venue: Option<crate::venue::Venue>,
    /// The row's exchange, HIP-3 deployer included — the key its own market's evidence is under.
    pub exchange: ExchangeId,
    /// The key the terminal addresses the market by.
    pub market: &'a str,
    /// The coin's cross-exchange identity, as [`super::MarketLabel::identity`] gives it.
    pub identity: &'a str,
    /// Whether the catalog's `futures_type` is set on this market.
    pub futures_flag: bool,
}

/// Everything known about cross-listings at one moment, for every row of a Screener publication.
///
/// A market the stream quotes NOWHERE is simply absent from both evidence maps: whoever fills them
/// drops a reading with no quote, so an entry always means "the stream knows this market", and a
/// donor's mask can never be credited to a market it has no price for.
#[derive(Debug, Default)]
pub struct ListingBook {
    /// The stream's reading of one market on one exchange, merged over that exchange's donors.
    ///
    /// An exchange with a key here has a donor of its own.
    pub by_market: HashMap<ExchangeId, HashMap<Box<str>, Evidence>>,
    /// The same, merged over every donor's markets of one coin.
    ///
    /// What answers for a row on an exchange with no donor of its own: a Binance spot market reads
    /// what a Gate donor's market of the same coin holds for the Binance futures code.
    pub by_identity: HashMap<String, Evidence>,
    /// The codes some donor CARRIES — holds on enough of its markets to be a stream it receives,
    /// not a stray price.
    ///
    /// A code outside it is one the stream does not carry — Hyperliquid spot and Binance COIN-M
    /// were both measured at zero — and its absence from a market is then no answer.
    pub seen: VenueSet,
    /// Identities of TRADING markets per brand and kind, from the catalogs of connected cores.
    pub catalogs: HashMap<(Brand, bool), HashSet<String>>,
}

/// The key two spellings of one coin share in the book: the identity without a contract tail.
///
/// A dated contract keeps its expiry inside the identity on some venues (`BTC_0925` on Binance
/// USD-M) and loses it on others, so the tail is cut here for every reader and writer alike.
pub fn identity_key(identity: &str) -> String {
    crate::symbol::coin_match_key(identity)
}

/// The book, narrowed to one exchange: everything a row's answer needs that its rows share.
///
/// A Screener publication signs every market of every provider about once a second, and all the
/// rows of one provider are on one exchange — so the codes of its other kind, which of them the
/// stream carries, its own evidence map and the other kind's catalog are resolved once here
/// rather than per row.
pub struct Signer<'a> {
    spot: bool,
    /// The codes of the brand's other market kind.
    other: VenueSet,
    /// Those of them the stream carries at all.
    answerable: VenueSet,
    /// The exchange's own evidence; `None` when no core of it holds a quote.
    markets: Option<&'a HashMap<Box<str>, Evidence>>,
    /// Whether the exchange has a donor of its own: a core holding quotes on enough of its
    /// markets to be receiving the stream. A stray price on one market is not a donor, and must
    /// not make every other market of the exchange read as unknown to the stream.
    has_donor: bool,
    by_identity: &'a HashMap<String, Evidence>,
    /// Coins of the brand's other kind, from connected cores.
    other_catalog: Option<&'a HashSet<String>>,
    /// Whether `futures_type` means a cross-listing on this exchange.
    flag_counts: bool,
}

impl ListingBook {
    /// Narrow the book to one exchange.
    ///
    /// Args:
    ///     venue: Brand and kind of the exchange; `None` for a platform this build cannot name.
    ///     exchange: The exchange itself, HIP-3 deployer included.
    ///
    /// Returns:
    ///     The signer, or `None` for an unnamed exchange — it has no other kind to ask about, and
    ///     every row of it is unknown.
    pub fn signer(
        &self,
        venue: Option<crate::venue::Venue>,
        exchange: ExchangeId,
    ) -> Option<Signer<'_>> {
        let venue = venue?;
        let spot = venue.is_spot();
        let other = other_side(venue.brand, spot);
        let markets = self.by_market.get(&exchange);
        Some(Signer {
            spot,
            other,
            answerable: other.intersection(&self.seen),
            markets,
            has_donor: markets.is_some_and(|found| found.len() >= MIN_CARRIED_MARKETS as usize),
            by_identity: &self.by_identity,
            other_catalog: self.catalogs.get(&(venue.brand, !spot)),
            flag_counts: futures_flag_means_listing(exchange.code),
        })
    }

    /// Decide one row on its own; a caller with many rows of one exchange takes a [`Signer`].
    ///
    /// Args:
    ///     row: The row's exchange, market and coin.
    ///
    /// Returns:
    ///     Where the coin is listed, or [`Listing::Unknown`] when nothing answers.
    pub fn sign(&self, row: &RowFacts<'_>) -> Listing {
        match self.signer(row.venue, row.exchange) {
            Some(signer) => signer.sign(row.market, row.identity, row.futures_flag),
            None => Listing::Unknown,
        }
    }
}

impl Signer<'_> {
    /// Decide one market of this exchange.
    ///
    /// Args:
    ///     market: The key the terminal addresses the market by.
    ///     identity: The coin's cross-exchange identity.
    ///     futures_flag: Whether the catalog's `futures_type` is set on the market.
    ///
    /// Returns:
    ///     Where the coin is listed, or [`Listing::Unknown`] when nothing answers.
    pub fn sign(&self, market: &str, identity: &str, futures_flag: bool) -> Listing {
        let key = identity_key(identity);
        let direct = self.markets.and_then(|markets| markets.get(market));
        let by_coin = match key.is_empty() {
            true => None,
            false => self.by_identity.get(&key),
        };

        // A positive is safe from any source: a price from the other kind on this market, on the
        // same coin elsewhere, a connected core that lists the coin, or the core's own flag.
        let quoted_there = [direct, by_coin]
            .iter()
            .flatten()
            .any(|found| found.quoted.intersects(&self.other));
        let in_other_catalog =
            !key.is_empty() && self.other_catalog.is_some_and(|coins| coins.contains(&key));
        if quoted_there || in_other_catalog || (futures_flag && self.flag_counts) {
            return Listing::Both;
        }

        // The other kind's codes the stream carries at all. With none, its silence is structural
        // and says nothing about this coin.
        if self.answerable.is_empty() {
            return Listing::Unknown;
        }
        // Who may say NO. Where the exchange has a donor of its own, the stream has to know THIS
        // market: one with no price from anywhere on its own donor is one the server maps to
        // nothing — a Bybit USDC perpetual, a Hyperliquid `kPEPE` — and that is unknown, not
        // "listed nowhere else", however many namesakes other donors hold (`1000CAT` and `CAT`
        // are both `CAT` on Binance).
        let market_known = direct.is_some_and(|found| !found.quoted.is_empty());
        if self.has_donor && !market_known {
            return Listing::Unknown;
        }
        // Past that, the market's own reading answers and so does the coin's across the other
        // donors: the row's own donor may not carry the other kind's code at all, or may still be
        // warming up, and it is the same coin over there. An exchange with no donor has only the
        // coin's reading. Either way the witness needs a price from SOMEWHERE and a warmed-up
        // donor carrying every answerable code of the other kind.
        let only = [direct, by_coin]
            .iter()
            .flatten()
            .any(|found| !found.quoted.is_empty() && self.answerable.is_subset(&found.asked));
        match (only, self.spot) {
            (false, _) => Listing::Unknown,
            (true, true) => Listing::SpotOnly,
            (true, false) => Listing::FuturesOnly,
        }
    }
}

#[cfg(test)]
mod tests;
