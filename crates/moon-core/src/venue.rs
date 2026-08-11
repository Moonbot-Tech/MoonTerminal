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
