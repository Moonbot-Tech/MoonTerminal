//! Market readout values and wire emission types.

use super::*;

/// Market-wide context a chart caption can state beside the coin's own numbers.
///
/// Two different subjects on purpose. The BACKGROUND deltas — the exchange's own average and BTC's
/// — answer "is this the coin or the whole market"; the funding pair answers "what does holding
/// cost, and when is it charged". Both come from one snapshot read, because a caption asking for
/// either has already paid for it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MarketContextReadout {
    /// Signed average movement across the exchange's markets, in percent.
    pub exchange_1h_pct: f64,
    pub exchange_24h_pct: f64,
    /// Signed BTC movement over the retained windows, in percent.
    pub btc_1h_pct: f64,
    pub btc_24h_pct: f64,
    pub btc_72h_pct: f64,
    /// Funding rate as a percentage, or `None` on a market that has none (spot).
    pub funding_pct: Option<f64>,
    /// When funding is next charged, in Unix milliseconds. `None` when the core reports no time —
    /// spot markets, and futures before the first funding message arrives.
    pub funding_at_ms: Option<i64>,
}

/// Exchange tag a coin carries, as the venue itself classifies it.
///
/// The names are the EXCHANGE's own labels, not prose, so they are printed verbatim in every locale
/// — "Seed" and "Alpha" are what the venue calls those listings and what its own interface shows.
/// Translating them would leave the caption saying something no exchange page repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoinTag {
    Monitoring,
    Fan,
    Seed,
    Launch,
    Gaming,
    New,
    Old,
    Bnb,
    Alpha,
    OiCapped,
    TradFi,
}

impl CoinTag {
    /// Tags in the order a caption prints them, with the bit each one occupies on the wire.
    ///
    /// Bit 0 is the venue's "no tag" marker and carries no tag of its own, which is why the table
    /// starts at bit 1 — a coin with only that bit set prints nothing.
    const BITS: [(CoinTag, u32); 11] = [
        (CoinTag::Monitoring, 1 << 1),
        (CoinTag::Fan, 1 << 2),
        (CoinTag::Seed, 1 << 3),
        (CoinTag::Launch, 1 << 4),
        (CoinTag::Gaming, 1 << 5),
        (CoinTag::New, 1 << 6),
        (CoinTag::Old, 1 << 7),
        (CoinTag::Bnb, 1 << 8),
        (CoinTag::Alpha, 1 << 9),
        (CoinTag::OiCapped, 1 << 10),
        (CoinTag::TradFi, 1 << 11),
    ];

    pub fn name(self) -> &'static str {
        match self {
            CoinTag::Monitoring => "Monitoring",
            CoinTag::Fan => "Fan",
            CoinTag::Seed => "Seed",
            CoinTag::Launch => "Launch",
            CoinTag::Gaming => "Gaming",
            CoinTag::New => "New",
            CoinTag::Old => "Old",
            CoinTag::Bnb => "BNB",
            CoinTag::Alpha => "Alpha",
            CoinTag::OiCapped => "OI-capped",
            CoinTag::TradFi => "TradFi",
        }
    }

    /// Every tag the given wire bits carry, in print order.
    pub fn from_bits(bits: u32) -> Vec<CoinTag> {
        Self::BITS
            .iter()
            .filter(|(_, bit)| bits & bit != 0)
            .map(|(tag, _)| *tag)
            .collect()
    }
}

/// A venue the core watches for arbitrage against the market being charted.
///
/// The core reports a numeric PLATFORM CODE, not a name: the codes are Moonbot's own
/// `TBotPlatform` ordinals plus arbitrage-only ones, and nothing on the wire says how to spell
/// them. Which codes exist and how each is spelled is the venue directory's answer —
/// [`crate::venue::ARB_VENUES`] — so an exchange is described in ONE place whether it is asked
/// about as a connection or as a price to compare against. This type is the code itself: the
/// deployer arithmetic the directory has no opinion on, and the fallbacks for a code it cannot
/// name.
///
/// A Hyperliquid DEPLOYER carries only an index in its arbitrage slot, and the directory has no
/// entry to give it, so [`Self::default_name`] numbers it here instead.
/// The real name usually exists elsewhere — `AuthCheck` hands over `known_dexes`, and the same
/// index reads into that list — and a live quote carries it (see [`ArbQuote::dex_name`]); the
/// numbered form is what remains when a core sent no list at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArbVenue(u8);

impl ArbVenue {
    /// Every venue this build can name, in the order the settings window lists them.
    ///
    /// The codes and the order are [`crate::venue::ARB_VENUES`]'s, not a second list beside it:
    /// this used to be one array of codes and [`Self::default_name`] a second one of the same
    /// codes, which nothing checked against each other — a venue added to one and forgotten in the
    /// other printed as a bare number in the column while the settings window offered it a colour.
    ///
    /// Deployers are deliberately absent: they exist per core, are discovered from the data, and
    /// are appended after this list wherever one actually reports a price.
    pub const KNOWN: [ArbVenue; crate::venue::ARB_VENUES.len()] = {
        let mut out = [ArbVenue(0); crate::venue::ARB_VENUES.len()];
        let mut i = 0;
        while i < out.len() {
            out[i] = ArbVenue(crate::venue::ARB_VENUES[i].0);
            i += 1;
        }
        out
    };

    /// First deployer code; everything from here to [`Self::DEPLOYER_END`] is one.
    pub const DEPLOYER_BASE: u8 = 50;
    const DEPLOYER_END: u8 = 100;

    /// How many deployer indices a read actually asks about.
    ///
    /// The protocol reserves fifty codes for them; a core watches a handful. Every candidate costs
    /// a market-lock round trip on a read (see [`super::MarketDataSource::market_arb`]), so the
    /// scan stops where the reference terminal's own column does rather than paying for forty-two
    /// venues nobody deploys.
    pub const DEPLOYERS_SCANNED: u8 = 8;

    /// The deployer at `index`, as a venue.
    pub const fn deployer(index: u8) -> Self {
        Self(Self::DEPLOYER_BASE.wrapping_add(index))
    }

    /// This deployer's index, or `None` for anything else.
    pub const fn deployer_index(self) -> Option<u8> {
        match self.is_deployer() {
            true => Some(self.0 - Self::DEPLOYER_BASE),
            false => None,
        }
    }

    /// Whether this build would ask about the venue with no core settings to go by.
    ///
    /// The FALLBACK roster, used only until `client_settings` arrives: everything this build can
    /// name, plus the deployer indices it scans. With settings in hand the core's own mask decides
    /// instead, and it can name venues this list cannot.
    pub fn is_known_or_scanned_deployer(self) -> bool {
        Self::KNOWN.contains(&self)
            || self
                .deployer_index()
                .is_some_and(|index| index < Self::DEPLOYERS_SCANNED)
    }

    pub const fn from_code(code: u8) -> Self {
        Self(code)
    }

    pub const fn code(self) -> u8 {
        self.0
    }

    pub const fn is_deployer(self) -> bool {
        self.0 >= Self::DEPLOYER_BASE && self.0 < Self::DEPLOYER_END
    }

    /// What this venue is CALLED, in the REFERENCE TERMINAL's spelling.
    ///
    /// The spelling itself lives in the venue directory — [`crate::venue::arb_alias`] — beside the
    /// brand, market kind and logo the same code already answers for. Here is only what to do when
    /// the directory has no word for it.
    ///
    /// The one name that does come over the wire is a Hyperliquid deployer's: `AuthCheck` carries
    /// `known_dexes`, and Moonbot prints those with an `HL_` prefix (`HL_hyna`, `HL_para`). That is
    /// handled where the live quote is — see `ArbVenueCfg::label_for` — and this is the fallback
    /// when no list has arrived.
    ///
    /// A code no spelling covers prints its NUMBER: it says "the core sent a platform this build
    /// has never seen" plainly, and the number is what identifies it.
    pub fn default_name(self) -> String {
        if let Some(name) = crate::venue::arb_alias(self.0) {
            return name.to_string();
        }
        if self.is_deployer() {
            return format!("HL #{}", self.0 - Self::DEPLOYER_BASE);
        }
        format!("#{}", self.0)
    }

    /// A deployer's name as the reference terminal prints it: its DEX name behind an `HL_` prefix.
    ///
    /// The prefix is the terminal's, the word after it is the core's — which is why this is one
    /// function and not a format string repeated at every call site.
    pub fn hl_name(dex_name: &str) -> String {
        format!("HL_{dex_name}")
    }
}

/// One venue's price on the charted coin, against the price of the market being charted.
#[derive(Clone, Debug, PartialEq)]
pub struct ArbQuote {
    pub venue: ArbVenue,
    /// The venue's own name, when the CORE supplies one.
    ///
    /// Hyperliquid deployers are the case this exists for: the arbitrage slot carries an index and
    /// the index alone, but `AuthCheck` hands over `known_dexes` — the deployer names the reference
    /// terminal shows as `HL_hyna`, `HL_para`. Empty for every other venue, whose name this build
    /// spells itself, and empty for a deployer whose core sent no list.
    pub dex_name: String,
    /// The other venue's price.
    pub price: f64,
    /// The CHARTED market's own price at the moment that one was recorded.
    ///
    /// Taken from the same ring entry rather than from the live ticker, so the percentage below
    /// compares two prices that existed at the same instant. A spread computed against a price that
    /// has moved since is the classic way to see arbitrage that was never there.
    pub my_price: f64,
    /// How far the other venue is from this one, in percent of this one's price.
    pub spread_pct: f64,
    /// Whether the venue is not accepting deposits or withdrawals for this coin — an arbitrage that
    /// cannot be settled. Reported by the core alongside the price.
    pub deposit_blocked: bool,
    pub withdraw_blocked: bool,
}

/// One retained-history window, in the figures a caption can print from it.
///
/// The figure is `Option` for the same reason [`MarketContextReadout`]'s funding is: a coin that
/// has not traded in the window and a coin whose history has not arrived both produce zero, and a
/// caption that printed it would claim a quiet market rather than an unknown one.
///
/// VOLUME is deliberately not here. It used to be, from a second set of sources — trade buckets for
/// the short windows, 5-minute candles beyond them — and once the chart learned to print the buying
/// and the selling halves separately, that became two answers to one question: the halves come from
/// the split-carrying sources, the total came from candles that carry no split, and `Bv + Sv` did
/// not have to equal `Vol`. Every traded amount now goes through [`volume::VolumeSpanReadout`],
/// which produces the halves and their sum from the SAME rows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowFigures {
    /// Price movement over the window, in percent. UNSIGNED — this is the range magnitude the
    /// Screener's `Δ` columns show, not a signed change from an average.
    pub delta_pct: Option<f64>,
}

/// Retained-history figures for every window a caption may ask for.
///
/// Indexed by the window's position in [`crate::config::LabelWindow::ALL`], which is the ONE order
/// the two crates agree on; the readout carries no window names of its own so the config stays the
/// single place a window is spelled.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MarketWindowsReadout {
    pub windows: [WindowFigures; crate::config::LABEL_WINDOW_COUNT],
}

/// Per-market figures a caption can print beside the price: the quote side, what the venue says
/// about the market, and what THIS core holds in it.
///
/// Two sources in one value, deliberately. The market half comes from the deduplicated PROVIDER —
/// the ask on `BTCUSDT@Binance` is the same for every core on that exchange — while the position
/// half comes from the core the pane is actually looking at, because a position is an account fact.
/// Reading them separately at the call site is what let the Screener's overlay drift from its
/// market columns; here one readout answers both.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarketFiguresReadout {
    /// Best bid and ask; `None` until the book has arrived.
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    /// Exchange mark price, `None` when the venue reports none (spot).
    pub mark: Option<f64>,
    /// Absolute chart price step.
    pub price_step: Option<f64>,
    /// 24-hour volume from the venue's own market list, in the quote currency.
    pub vol_24h: Option<f64>,
    /// Maximum leverage the market allows; `None` on spot or before it arrives.
    pub max_leverage: Option<i32>,
    /// Exchange maximum order size in the quote currency, through the shared rule.
    pub max_order: MaxOrder,
    /// The venue's own tags for this coin, in print order. Empty when none arrived.
    pub tags: Vec<CoinTag>,
    /// Open position on THIS core, in the base coin; negative while short.
    pub pos_size: Option<f64>,
    /// Liquidation price the venue reports for it.
    pub liq_price: Option<f64>,
    /// Account leverage in force on this market; `None` when unset.
    pub leverage_x: Option<i32>,
    /// Whether margin is isolated; `None` when the venue stated no position type.
    pub isolated: Option<bool>,
    /// The core's own per-coin profit counter (`b + l + s`), which MoonBot prints as `PnL`.
    ///
    /// NOT the `Session` figure beside it in MoonBot's chart header — that one is [`Self::session`].
    /// Zero on part of the venues even where MoonBot shows an amount, so a reader must not take a
    /// zero here for "traded to break even".
    pub core_pnl: Option<f64>,
    /// The `Session` figure MoonBot prints in its chart header, IN USDT.
    ///
    /// The counter the markets table's "Reset Session" clears. A real zero arrives as `Some(0.0)`,
    /// which is what lets a caption tell "nothing was earned since the reset" from the three ways
    /// this can be `None`: no caption asked for it, the core publishes no session-profit snapshot
    /// at all (every build predating the protocol field), or its base currency could not be valued
    /// in USDT — which also covers the moments right after connect, before `BaseCheck` names that
    /// currency. All three print nothing, and none of them may print a zero.
    ///
    /// Converted here rather than at the caption, because the raw value is in the CORE's base
    /// currency: a coin-margined core states it in BTC, and printing that under a dollar sign is
    /// how `0.0004 BTC` reads as nothing at all. A base this build cannot value is therefore
    /// withheld rather than shown unconverted.
    pub session: Option<f64>,
    /// Free balance of the coin itself, for a spot market.
    pub coin_balance: Option<f64>,
}

/// Frozen snapshot for a detection card, built exactly once when the detection occurs.
///
/// The mini-chart combines recent 5-minute candles from the provider's retained
/// `candles_5m`, the local kline cache, and the live trade-ring tail without calling the exchange
/// API. `server_info` supplies the connection identity. Missing history leaves the chart data
/// empty while exchange metadata may still be present; a missing provider, client, or client
/// snapshot returns the fully empty default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DetectSnapshot {
    /// Recent 5-minute candles as `(open, high, low, close)`, ordered oldest to newest.
    ///
    /// Full OHLC lets the mini-chart draw bodies and wicks. The retained `candles_5m` fallback is
    /// range-only, so its candle orientation is synthesized. Empty when no history is available.
    pub bars: Vec<(f32, f32, f32, f32)>,
    /// Close prices for line mode, ordered oldest to newest, with up to about 24 hours of history.
    pub line: Vec<f32>,
    /// Actual 24-hour price change in percent, comparing now with a close from about 24 hours ago.
    ///
    /// This is derived from our buckets so it matches the line movement. It is not MoonProto's
    /// `coin_24h_delta`, which measures deviation from a retained average.
    pub delta_24h: f32,
    /// Actual 1-hour price change in percent, comparing now with a close from about one hour ago.
    pub delta_1h: f32,
    /// Venue the detecting core's provider is connected to, frozen with the rest of the card.
    ///
    /// The card captions this through the venue directory like every other core list, so a
    /// detection on Binance COIN-M reads as the same venue the Orders picker shows. `None` when the
    /// provider reported no identity this build can name.
    pub venue: Option<crate::venue::CoreVenue>,
    /// Short Russian-language exchange type label derived from `exchange_type_mask`.
    ///
    /// The label distinguishes spot, futures, DEX, and combined connections. Empty when the type
    /// was not reported.
    pub exchange_kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LatestPriceError {
    NoProvider,
    NoClient,
    NoSnapshot,
    NoPrice,
}

impl std::fmt::Display for LatestPriceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoProvider => f.write_str("no provider"),
            Self::NoClient => f.write_str("no client"),
            Self::NoSnapshot => f.write_str("no snapshot"),
            Self::NoPrice => f.write_str("no price"),
        }
    }
}

/// What a chart's next candle emission must ship.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CandleEmit {
    /// Nothing changed since the last emission.
    #[default]
    Clean,
    /// Only the composed entries from this index on changed.
    Patch(usize),
    /// The composed list was recomposed; ship all of it.
    Full,
}
