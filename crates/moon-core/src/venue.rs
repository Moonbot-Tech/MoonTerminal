//! THE exchange directory: which venue a core is connected to, answered from the code it reports.
//!
//! A core publishes its platform as one byte in `BaseCheck` (`ExchangeCode`, the Delphi
//! `TBotPlatform` ordinal) plus a free-form UI name such as `Binance Futures` or
//! `Binance Quarterly`. The byte is the identity; the name is a caption whose spelling belongs to
//! the core build. Everything the terminal decides about a venue — its brand logo, its label, how
//! its market names are spelled, which order book its provider pulls — is decided HERE, from the
//! byte, so those four answers cannot disagree with each other.
//!
//! Nothing outside this module may recognize an exchange by matching on its reported name. That
//! heuristic is how Binance COIN-M (`QBinance`, code 6) lost its logo: it reports
//! `Binance Quarterly`, which contains no substring any brand table happened to list.
//!
//! An unknown future ordinal resolves to `None` here rather than to a neighbour's answer: callers
//! fall back to the core's reported name for a caption and simply draw no logo — a new exchange
//! must look unbranded, never like the wrong brand. Where an answer is unavoidable, the caller
//! states its own default rather than the directory guessing one; `session::orderbook_kind_for_
//! exchange` picks the futures book, which is also moonproto's default.

use crate::feed::ExchangeId;
use crate::symbol::Exchange;

/// What one core is connected to, as every consumer of a core list needs it.
///
/// [`Self::id`] is the grouping key — two cores on one venue share it, and two Hyperliquid cores on
/// different HIP-3 DEXes deliberately do not. The other two fields exist only to caption it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreVenue {
    /// Platform code plus HIP-3 DEX discriminator, as elected providers are keyed by.
    pub id: ExchangeId,
    /// HIP-3 DEX name as reported, empty for a regular exchange.
    pub dex: String,
    /// Caption the core published, such as `Binance Quarterly`.
    ///
    /// The ONLY use is an ordinal this build does not know: there the directory has no brand to
    /// name and the core's own spelling is the single thing left to show. Never match on it.
    pub reported: String,
}

impl CoreVenue {
    /// Build the venue a core reported.
    ///
    /// THE one place the identification rule lives, so a producer cannot get half of it right: a
    /// known ordinal drops the reported caption, because the directory names it and keeping the
    /// text would put a core build's spelling into this value's equality — where a momentary
    /// snapshot gap could flip it. An unknown ordinal keeps the caption as its only name.
    ///
    /// Every identified core gets a value, nameable or not: identity is what elects a market-data
    /// provider, and the synthetic feed has one too. Whether it can be NAMED is
    /// [`Self::is_nameable`], which is a display question.
    ///
    /// Args:
    ///     code: Platform ordinal from `ServerInfo::exchange_code`.
    ///     dex_name: HIP-3 DEX name, empty for a regular exchange.
    ///     reported: Caption the core published, if any.
    ///
    /// Returns:
    ///     The venue this core is connected to.
    pub fn identify(code: u8, dex_name: &str, reported: Option<&str>) -> Self {
        let known = venue(code).is_some();
        Self {
            id: ExchangeId::with_dex(code, dex_name),
            dex: dex_name.to_string(),
            reported: match known {
                true => String::new(),
                false => Self::printable(reported.unwrap_or_default()),
            },
        }
    }

    /// Whether anything can put a name on this venue.
    ///
    /// `false` for an ordinal the directory does not know whose core published no usable caption
    /// either — the synthetic feed, or an exchange newer than this build connected to a core that
    /// names it with an empty string. Such a venue belongs in the shared "not identified" group:
    /// giving it a section of its own would render a second heading with that same text.
    ///
    /// Returns:
    ///     `true` when the directory or the core itself supplies a name.
    pub fn is_nameable(&self) -> bool {
        self.resolved().is_some() || !self.reported.is_empty()
    }

    /// Keep only what a caption can actually show of a wire-supplied name.
    ///
    /// Control and invisible-format characters are dropped HERE, at the edge, so that "this venue
    /// has a name" and "this venue draws a name" cannot disagree: a caption of nothing but bidi
    /// marks would otherwise count as nameable and then render as the not-identified wording, which
    /// is the duplicate heading [`Self::is_nameable`] exists to prevent. Display-side clamping
    /// stays in the UI, where the row width lives.
    ///
    /// Args:
    ///     reported: Caption as the core sent it.
    ///
    /// Returns:
    ///     The caption with unprintable characters removed and the ends trimmed.
    fn printable(reported: &str) -> String {
        reported
            .chars()
            .filter(|c| !c.is_control() && !is_invisible_format(*c))
            .collect::<String>()
            .trim()
            .to_string()
    }

    /// Build the venue an identity alone denotes, for a selection that outlived its cores.
    ///
    /// Returns:
    ///     A venue with no DEX name — [`ExchangeId`] keeps only its hash — and no caption.
    pub fn from_id(id: ExchangeId) -> Self {
        Self {
            id,
            dex: String::new(),
            reported: String::new(),
        }
    }

    /// Resolve the directory entry for this core's platform code.
    ///
    /// Returns:
    ///     The venue, or `None` for an ordinal this build does not know.
    pub const fn resolved(&self) -> Option<Venue> {
        venue(self.id.code)
    }

    /// Whether this core is connected to the venue an arbitrage row names.
    ///
    /// THE one spelling of that rule, because three places ask it: the chart dims a row with no
    /// core behind it, a click on the row opens the coin on that core, and the column drops the
    /// row for the chart's own venue. Three readings of it would let a row be clickable and
    /// undimmed and still be the chart's own exchange.
    ///
    /// An arbitrage platform code IS the core's platform ordinal for an ordinary exchange — the
    /// protocol builds one from the other by copying the byte — so those compare directly. A
    /// Hyperliquid deployer is the exception: every deployer shares the futures ordinal, and only
    /// the DEX name tells them apart. Which is also why an ordinary exchange must NOT match a core
    /// that has a DEX: `xyz` and plain Hyperliquid futures would otherwise be the same venue.
    ///
    /// Args:
    ///     code: Platform code of the arbitrage row.
    ///     dex: DEX name the row carries, empty for an ordinary exchange.
    ///
    /// Returns:
    ///     `true` when this core is that venue.
    pub fn matches_arb(&self, code: u8, dex: &str) -> bool {
        arb_row_matches((self.id.code, self.dex.as_str()), (code, dex))
    }

    /// Return the brand whose logo represents this venue.
    ///
    /// Returns:
    ///     The brand, or `None` for an ordinal this build does not know.
    pub const fn brand(&self) -> Option<Brand> {
        match venue(self.id.code) {
            Some(venue) => Some(venue.brand),
            None => None,
        }
    }
}

/// Whether a character is an invisible formatting mark that must not enter a caption.
///
/// Public so the UI's display-side clamp filters exactly the same set: two spellings of "what is
/// printable" would let a name pass one check and vanish at the other.
///
/// `char::is_control` covers the C0/C1 controls but leaves the FORMAT class, and a bidi override
/// inside a label reverses the text drawn after it — including text the caption did not supply.
///
/// Args:
///     c: Character from a wire-supplied name.
///
/// Returns:
///     `true` for zero-width marks, bidi controls and isolates, and the byte-order mark.
pub fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
    )
}

/// Trading brand, the identity a logo and a label are chosen by.
///
/// Spot and futures connections to one venue share a brand and differ only in [`MarketKind`];
/// `Huobi` and `HTX` are the same brand under its old and current name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Brand {
    Binance,
    Bybit,
    /// Huobi, trading as HTX since 2023. Cores of either vintage report this brand.
    Htx,
    Gate,
    BitGet,
    Hyperliquid,
    Okx,
}

impl Brand {
    /// Every brand this build knows, for callers that must cover the whole set.
    ///
    /// The logo prewarm decodes exactly these, and a unit test asserts that no reachable venue
    /// names a brand missing from the list — a new brand added to [`venue`] but not here would
    /// otherwise ship with its logo decoded on the first frame that draws it.
    pub const ALL: [Self; 7] = [
        Self::Binance,
        Self::Bybit,
        Self::Htx,
        Self::Gate,
        Self::BitGet,
        Self::Hyperliquid,
        Self::Okx,
    ];

    /// Return the asset file stem under `assets/exchanges`.
    ///
    /// Returns:
    ///     Lowercase stem of the shipped SVG, without its extension.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Binance => "binance",
            Self::Bybit => "bybit",
            Self::Htx => "htx",
            Self::Gate => "gate",
            Self::BitGet => "bitget",
            Self::Hyperliquid => "hyperliquid",
            Self::Okx => "okx",
        }
    }

    /// Return the brand name as it is written in the interface.
    ///
    /// Deliberately not localized and deliberately not the core's reported spelling: a brand is a
    /// proper noun, and one spelling per brand is what lets two cores of different vintage merge
    /// into one row.
    ///
    /// Returns:
    ///     Canonical brand name.
    pub const fn display(self) -> &'static str {
        match self {
            Self::Binance => "Binance",
            Self::Bybit => "Bybit",
            Self::Htx => "HTX",
            Self::Gate => "Gate",
            Self::BitGet => "BitGet",
            Self::Hyperliquid => "Hyperliquid",
            Self::Okx => "OKX",
        }
    }
}

/// Which market of a brand a core trades.
///
/// This is the venue's own kind, fixed by the code it reports — not a capability list. A core is
/// connected to exactly one of these, which is why Binance occupies three codes rather than one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MarketKind {
    Spot,
    /// Perpetual futures, whether USD-M, coin-collateralized perpetuals, or a DEX perp venue.
    Futures,
    /// Binance COIN-M delivery contracts (`QBinance`): `BTCUSD_PERP`, `BNBUSD_260925`.
    Quarterly,
}

impl MarketKind {
    /// Return the locale key of the market-type caption.
    ///
    /// The values are identical in every language — `locales/README.md` lists market types among
    /// the deliberately untranslated industry terms — but they stay in the dictionary so the three
    /// captions are edited in one place rather than compiled in.
    ///
    /// Returns:
    ///     Key into `locales/common.yml`.
    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Spot => "common.exchange_spot",
            Self::Futures => "common.exchange_futures",
            Self::Quarterly => "common.exchange_quarterly",
        }
    }
}

/// One known venue: a brand and the market it trades.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Venue {
    pub brand: Brand,
    pub kind: MarketKind,
}

/// Resolve the venue a platform ordinal names.
///
/// The ordinals are `TBotPlatform` values and therefore part of the wire protocol, so they are
/// matched numerically rather than through a moonproto type: this module stays usable from a
/// report row or a log line that never touched moonproto. The unit tests pin every arm against
/// moonproto's own constants, which is what makes an upstream renumbering fail loudly.
///
/// Args:
///     code: Platform ordinal reported by a core, from `ServerInfo::exchange_code`.
///
/// Returns:
///     The venue, or `None` for `None`/`WasBittrex` and for any ordinal newer than this build.
pub const fn venue(code: u8) -> Option<Venue> {
    match code {
        2 => Some(Venue {
            brand: Brand::Bybit,
            kind: MarketKind::Futures,
        }),
        3 => Some(Venue {
            brand: Brand::Binance,
            kind: MarketKind::Spot,
        }),
        4 => Some(Venue {
            brand: Brand::Binance,
            kind: MarketKind::Futures,
        }),
        5 => Some(Venue {
            brand: Brand::Htx,
            kind: MarketKind::Spot,
        }),
        6 => Some(Venue {
            brand: Brand::Binance,
            kind: MarketKind::Quarterly,
        }),
        7 => Some(Venue {
            brand: Brand::Bybit,
            kind: MarketKind::Spot,
        }),
        8 => Some(Venue {
            brand: Brand::Gate,
            kind: MarketKind::Spot,
        }),
        9 => Some(Venue {
            brand: Brand::Gate,
            kind: MarketKind::Futures,
        }),
        10 => Some(Venue {
            brand: Brand::BitGet,
            kind: MarketKind::Spot,
        }),
        11 => Some(Venue {
            brand: Brand::BitGet,
            kind: MarketKind::Futures,
        }),
        12 => Some(Venue {
            brand: Brand::Hyperliquid,
            kind: MarketKind::Spot,
        }),
        13 => Some(Venue {
            brand: Brand::Hyperliquid,
            kind: MarketKind::Futures,
        }),
        14 => Some(Venue {
            brand: Brand::Okx,
            kind: MarketKind::Spot,
        }),
        15 => Some(Venue {
            brand: Brand::Okx,
            kind: MarketKind::Futures,
        }),
        _ => None,
    }
}

/// Whether a deep-history row from the core reporting this platform code carries its wire
/// `volume` in quote money rather than base coins.
///
/// Keyed on the CODE alone, deliberately NOT on [`MarketKind`]: the only MEASURED fact behind
/// this whole predicate is Binance Futures itself (code 4, below), and it does not generalise.
/// Binance USD-M's own REST kline cell 5 is BASE for the same market, so the core's deep-push
/// convention diverges from the exchange's own REST convention — the core is normalising on its
/// own, by a rule no other venue's API can be read off. Keying on `MarketKind` would have
/// asserted "every futures venue behaves like Binance Futures" from a sample of one; keying on
/// the code keeps the claim exactly as wide as the evidence.
///
/// | code | brand / kind | quote? | status |
/// |---|---|---|---|
/// | 2 | Bybit Futures | `false` | UNVERIFIED |
/// | 4 | Binance Futures | `true` | **MEASURED**: the live kline cache holds deep `kind = 1` rows whose `volume` slot is USDT turnover — 218,653,600 for one BTCUSDT minute — confirmed against a measured `8.07T` band label before this fix |
/// | 6 | Binance Quarterly (COIN-M) | `false` | UNVERIFIED |
/// | 9 | Gate Futures | `false` | UNVERIFIED |
/// | 11 | BitGet Futures | `false` | UNVERIFIED |
/// | 13 | Hyperliquid Futures | `false` | UNVERIFIED, and specifically IMPLAUSIBLE: `market/trade_replay/rest/hyperliquid.rs`'s own `parse_klines` doc states, from measurement, that its `v` cell "is genuinely base volume" and that `candleSnapshot` "carries no turnover figure at all" — the core has no quote figure to relay there |
/// | 15 | OKX Futures | `false` | UNVERIFIED |
///
/// `false` is the STATUS-QUO branch: it is the behaviour that shipped before this task, for
/// every code including 4 before this fix. Widening a code to `true` can improve its future band,
/// but cannot repair already-persisted rows; the separate coarse-kind read migration
/// (`crate::market::kline_cache::legacy_volume_is_quote`) does that. A wrong `true` would corrupt
/// freshly written rows with a 3650-day retention and no migration path. An unknown or newer code
/// — including the shipped fixture's exchange code `200`, where `venue(200) == None` — also
/// returns `false`, keeping that fixture's read path byte-for-byte unchanged.
///
/// **To promote a code**: open that core's chart on a liquid market, read the volume band's
/// magnitude against the market's real per-minute turnover, and if they agree, flip that one
/// code's row in the table above to `true`. Never flip a row on inference from another venue.
///
/// Args:
///     code: Platform ordinal reported by a core, from `ServerInfo::exchange_code`.
///
/// Returns:
///     `true` for code 4 (Binance Futures) only; `false` for every other code, measured or not.
pub const fn deep_volume_is_quote(code: u8) -> bool {
    code == 4
}

/// Every venue the arbitrage column can list, in print order, with the spelling it is listed under.
///
/// THE arbitrage roster, and deliberately here rather than beside the chart: a venue is a venue,
/// and keeping a second list of codes elsewhere is how one of them acquires an exchange the other
/// never hears about. [`venue`] above answers what a code IS — brand, market kind, logo, naming
/// rules — for the codes a core can be CONNECTED to; this table answers what a code is CALLED in an
/// arbitrage column, over the wider set of codes a core can merely QUOTE. The overlap is checked by
/// a unit test rather than by eye.
///
/// Two things the spellings are not. They are not [`Brand::display`]: Moonbot's arbitrage panel
/// writes `Bitget`, `Okx` and `Htx` where the brand is `BitGet`, `OKX` and `HTX`, and a trader
/// reads a spread by the panel's word. And they are not `ArbPlatformCode::name`, which is the
/// protocol library's debug spelling (`FBinance`, `WasBittrex`, `Unknown`). They are transcribed
/// from that panel (screenshot, 2026-08-21).
///
/// The ORDER is the order the arbitrage settings window lists its rows in, grouped by brand rather
/// than by code, so the window reads as a list of exchanges instead of a list of numbers.
///
/// Codes outside this table are not an error: an arbitrage quote from a platform this build has
/// never heard of still prints, under the number that identifies it. See
/// `ArbVenue::default_name`, which is the one place that fallback is spelled.
pub const ARB_VENUES: [(u8, &str); 19] = [
    (3, "BinanceS"),
    (4, "BinanceF"),
    (6, "BinanceQ"),
    // Binance Alpha. Arbitrage-only: no core connects to it, so [`venue`] does not name it.
    (103, "BinAlpha"),
    (7, "BybitS"),
    (2, "BybitF"),
    (8, "GateS"),
    (9, "GateF"),
    (10, "BitgetS"),
    (11, "BitgetF"),
    // The OKX pair a live core actually sends, and the library constant that names the exchange
    // without a side. All three are listed: a venue absent from this table still PRINTS, but only a
    // listed one can be hidden, recoloured or moved.
    //
    // 14 and 15 are in no `ArbPlatformCode` constant — they are `ExchangeCode::OKX`/`FOKX`, read
    // off a live core. The ORDER inside the pair follows every other pair in the range (`8/9` Gate,
    // `10/11` Bitget, `12/13` Hyperliquid): spot first. Verify it in one move rather than trusting
    // the inference: clear the `OkxF` checkbox in Moonbot and watch which of the two codes leaves
    // the mask in the arbitrage trace (`log.market_sources`).
    (14, "OkxS"),
    (15, "OkxF"),
    // The library's own `Okx` constant. No live core has been seen sending it — the pair above is
    // what arrives — so it keeps the unsuffixed name rather than claiming a side.
    (102, "Okx"),
    // Hyperliquid is the one brand the panel abbreviates. A DEPLOYER on it carries an index instead
    // of a code and is named from the core's own `known_dexes`; see `ArbVenue::hl_name`.
    (12, "HL_S"),
    (13, "HL_F"),
    (5, "HtxS"),
    // Arbitrage-only, like `BinAlpha` and `Forex`: price sources rather than venues a core trades.
    (101, "UpBit"),
    (100, "Forex"),
    // Delisted, and `ExchangeCode::WasBittrex` for that reason, but an old core can still quote it.
    (1, "Bittrex"),
];

/// Whether a core's venue is the venue an arbitrage row names.
///
/// Free-standing because one caller has no [`CoreVenue`] in hand: the chart's caption pass carries
/// the connected venues as bare `(code, dex)` pairs, having resolved them once for a whole sync
/// rather than per pane. The rule is stated once here and read three times; see
/// [`CoreVenue::matches_arb`] for why it is not a plain code comparison.
///
/// Args:
///     core: Platform ordinal and DEX name of the core.
///     row: Platform code and DEX name of the arbitrage row.
///
/// Returns:
///     `true` when the core is connected to that venue.
pub fn arb_row_matches(core: (u8, &str), row: (u8, &str)) -> bool {
    match row.1.is_empty() {
        true => core.0 == row.0 && core.1.is_empty(),
        false => core.1 == row.1,
    }
}

/// What one platform code is CALLED in an arbitrage column.
///
/// Args:
///     code: Platform ordinal from an arbitrage slot.
///
/// Returns:
///     The spelling [`ARB_VENUES`] gives it, or `None` for a code this build cannot name.
pub const fn arb_alias(code: u8) -> Option<&'static str> {
    let mut i = 0;
    while i < ARB_VENUES.len() {
        if ARB_VENUES[i].0 == code {
            return Some(ARB_VENUES[i].1);
        }
        i += 1;
    }
    None
}

impl Venue {
    /// Return the market-name spelling rules this venue's names follow.
    ///
    /// Brand and market kind together decide the scheme: Binance USD-M spells `BTCUSDT` while its
    /// COIN-M quarterly venue spells `BTCUSD_PERP`, so the kind is part of the answer.
    ///
    /// Returns:
    ///     Naming family for [`crate::symbol::parse`].
    pub const fn naming(self) -> Exchange {
        match (self.brand, self.kind) {
            (Brand::Binance, MarketKind::Quarterly) => Exchange::BinanceCoinM,
            (Brand::Binance, _) => Exchange::Binance,
            (Brand::Bybit, _) => Exchange::Bybit,
            (Brand::Htx, _) => Exchange::Huobi,
            (Brand::Gate, _) => Exchange::Gate,
            (Brand::BitGet, _) => Exchange::BitGet,
            (Brand::Hyperliquid, _) => Exchange::Hyperliquid,
            (Brand::Okx, _) => Exchange::Okx,
        }
    }

    /// Whether this venue's order book is the spot book rather than the futures one.
    ///
    /// Quarterly counts as futures: COIN-M delivery contracts are derivatives and their book
    /// arrives on the futures channel.
    ///
    /// Returns:
    ///     `true` for a spot venue.
    pub const fn is_spot(self) -> bool {
        matches!(self.kind, MarketKind::Spot)
    }
}

#[cfg(test)]
mod tests;
