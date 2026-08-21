//! What ONE caption prints: the catalogue of fields, the menu sections they fall into, and the
//! style each one takes when its part overrides nothing.
//!
//! Split out of the model beside it because it is a catalogue and grows with every new figure the
//! chart learns to print, while the model — rows, parts, zones — does not.

use serde::{Deserialize, Serialize};

use super::{LabelColor, ResolvedLabelStyle};

/// What one caption prints.
///
/// The wire form is the serde name, so a variant may be REORDERED here freely but never RENAMED
/// without migrating `charts.json` and `layout.toml`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartLabelField {
    /// Empty part: prints nothing and takes no place on its row.
    #[default]
    None,
    /// Coin ticker as the pane resolved it (`BEAT-USDT`, `@206`'s classic name).
    Coin,
    /// Name of the core the pane's market belongs to.
    Core,
    /// Venue the pane's core trades on, through the shared venue directory.
    Venue,
    /// Quote currency this market is priced and settled in: `USDT`, `USDC`, `BTC`.
    ///
    /// The unit behind every money figure on the chart. A COIN-M contract reports none, and the
    /// caption then prints nothing rather than guessing one.
    Quote,
    /// Current Y-scale badge as a whole percentage of the visible range.
    ScaleBadge,
    /// Comparison-mode difference from the locked anchor's price, signed.
    CompareDelta,
    /// Last traded price.
    LastPrice,
    /// Signed one-hour price change, from the same readout the header ticker uses.
    Delta1h,
    /// Signed 24-hour price change, from the same readout the header ticker uses.
    Delta24h,
    /// Unrealized result of the orders open RIGHT NOW on this market, as a percentage of what they
    /// spent. See [`PnlBasis`] for which orders count.
    OpenPnlPct,
    /// The same result in the market's quote money.
    OpenPnlMoney,
    /// How many orders are open on this market.
    OpenOrders,
    /// Open position size in the base coin.
    PosSize,
    /// Current notional of what is open, in the market's quote currency: position size times the
    /// mark price.
    ///
    /// Counted over a WIDER set than the PnL figures: an order whose entry price has not arrived
    /// still has a size and a mark, and withholding it would understate what is actually at risk.
    Exposure,
    /// User-assigned name of the strategy that owns the newest open order on this market.
    OrderStrategy,
    /// Signed average movement across the whole exchange, over the retained hour and day.
    ///
    /// The background the coin's own delta is read against: it answers "is this the coin, or the
    /// whole market". One figure per core, not per market.
    ExchangeDelta1h,
    ExchangeDelta24h,
    /// Signed BTC movement over the retained windows.
    BtcDelta1h,
    BtcDelta24h,
    BtcDelta72h,
    /// Funding rate charged on this market, as a percentage.
    Funding,
    /// Time remaining until funding is next charged.
    FundingIn,
}

impl ChartLabelField {
    /// Every assignable field, in the order the "add label" menu offers them.
    pub const ALL: [ChartLabelField; 22] = [
        ChartLabelField::Coin,
        ChartLabelField::Core,
        ChartLabelField::Venue,
        ChartLabelField::Quote,
        ChartLabelField::LastPrice,
        ChartLabelField::Delta1h,
        ChartLabelField::Delta24h,
        ChartLabelField::ScaleBadge,
        ChartLabelField::CompareDelta,
        ChartLabelField::OpenPnlPct,
        ChartLabelField::OpenPnlMoney,
        ChartLabelField::OpenOrders,
        ChartLabelField::PosSize,
        ChartLabelField::Exposure,
        ChartLabelField::OrderStrategy,
        ChartLabelField::ExchangeDelta1h,
        ChartLabelField::ExchangeDelta24h,
        ChartLabelField::BtcDelta1h,
        ChartLabelField::BtcDelta24h,
        ChartLabelField::BtcDelta72h,
        ChartLabelField::Funding,
        ChartLabelField::FundingIn,
    ];

    /// Menu section this field belongs to.
    pub fn group(self) -> ChartLabelGroup {
        match self {
            ChartLabelField::Coin
            | ChartLabelField::Core
            | ChartLabelField::Venue
            | ChartLabelField::Quote
            | ChartLabelField::None => ChartLabelGroup::Instrument,
            ChartLabelField::LastPrice
            | ChartLabelField::Delta1h
            | ChartLabelField::Delta24h
            | ChartLabelField::ScaleBadge
            | ChartLabelField::CompareDelta
            | ChartLabelField::ExchangeDelta1h
            | ChartLabelField::ExchangeDelta24h
            | ChartLabelField::BtcDelta1h
            | ChartLabelField::BtcDelta24h
            | ChartLabelField::BtcDelta72h
            | ChartLabelField::Funding
            | ChartLabelField::FundingIn => ChartLabelGroup::Market,
            ChartLabelField::OpenPnlPct
            | ChartLabelField::OpenPnlMoney
            | ChartLabelField::OpenOrders
            | ChartLabelField::PosSize
            | ChartLabelField::Exposure => ChartLabelGroup::Position,
            ChartLabelField::OrderStrategy => ChartLabelGroup::Strategy,
        }
    }

    /// Locale key for this field's menu and list label.
    pub fn locale_key(self) -> &'static str {
        match self {
            ChartLabelField::None => "chart_labels.field.none",
            ChartLabelField::Coin => "chart_labels.field.coin",
            ChartLabelField::Core => "chart_labels.field.core",
            ChartLabelField::Venue => "chart_labels.field.venue",
            ChartLabelField::Quote => "chart_labels.field.quote",
            ChartLabelField::ScaleBadge => "chart_labels.field.scale_badge",
            ChartLabelField::CompareDelta => "chart_labels.field.compare_delta",
            ChartLabelField::LastPrice => "chart_labels.field.last_price",
            ChartLabelField::Delta1h => "chart_labels.field.delta_1h",
            ChartLabelField::Delta24h => "chart_labels.field.delta_24h",
            ChartLabelField::OpenPnlPct => "chart_labels.field.open_pnl_pct",
            ChartLabelField::OpenPnlMoney => "chart_labels.field.open_pnl_money",
            ChartLabelField::OpenOrders => "chart_labels.field.open_orders",
            ChartLabelField::PosSize => "chart_labels.field.pos_size",
            ChartLabelField::Exposure => "chart_labels.field.exposure",
            ChartLabelField::OrderStrategy => "chart_labels.field.order_strategy",
            ChartLabelField::ExchangeDelta1h => "chart_labels.field.exchange_delta_1h",
            ChartLabelField::ExchangeDelta24h => "chart_labels.field.exchange_delta_24h",
            ChartLabelField::BtcDelta1h => "chart_labels.field.btc_delta_1h",
            ChartLabelField::BtcDelta24h => "chart_labels.field.btc_delta_24h",
            ChartLabelField::BtcDelta72h => "chart_labels.field.btc_delta_72h",
            ChartLabelField::Funding => "chart_labels.field.funding",
            ChartLabelField::FundingIn => "chart_labels.field.funding_in",
        }
    }

    /// Locale key of the SHORT prefix a caption prints when its style asks for one.
    ///
    /// Separate from [`Self::locale_key`]: the menu needs a name a reader can pick from a list
    /// ("PnL открытых, %"), while a caption drawn over candles needs the shortest thing that still
    /// identifies the figure ("PnL"). A field with nothing worth prefixing returns `None`.
    pub fn caption_key(self) -> Option<&'static str> {
        match self {
            ChartLabelField::Delta1h => Some("chart_labels.short.delta_1h"),
            ChartLabelField::Delta24h => Some("chart_labels.short.delta_24h"),
            ChartLabelField::OpenPnlPct | ChartLabelField::OpenPnlMoney => {
                Some("chart_labels.short.pnl")
            }
            ChartLabelField::OpenOrders => Some("chart_labels.short.orders"),
            ChartLabelField::PosSize => Some("chart_labels.short.position"),
            ChartLabelField::Exposure => Some("chart_labels.short.exposure"),
            ChartLabelField::ExchangeDelta1h => Some("chart_labels.short.exchange_1h"),
            ChartLabelField::ExchangeDelta24h => Some("chart_labels.short.exchange_24h"),
            ChartLabelField::BtcDelta1h => Some("chart_labels.short.btc_1h"),
            ChartLabelField::BtcDelta24h => Some("chart_labels.short.btc_24h"),
            ChartLabelField::BtcDelta72h => Some("chart_labels.short.btc_72h"),
            ChartLabelField::Funding => Some("chart_labels.short.funding"),
            ChartLabelField::FundingIn => Some("chart_labels.short.funding_in"),
            _ => None,
        }
    }

    /// Whether this field takes a [`PnlBasis`], and therefore shows that control in the popup.
    ///
    /// Asked of the FIELD rather than stored per part so a basis cannot linger on a part whose
    /// field was changed to something that ignores it.
    pub fn uses_pnl_basis(self) -> bool {
        matches!(
            self,
            ChartLabelField::OpenPnlPct
                | ChartLabelField::OpenPnlMoney
                | ChartLabelField::OpenOrders
                | ChartLabelField::PosSize
                | ChartLabelField::Exposure
        )
    }

    /// Style this field draws with when its part overrides nothing.
    ///
    /// These are the sizes and colors the hard-coded caption used, so the default configuration
    /// reproduces it without the popup restating them.
    pub fn default_style(self) -> ResolvedLabelStyle {
        match self {
            // The coin leads, one size up: it is the fact a glance needs.
            ChartLabelField::Coin => ResolvedLabelStyle {
                color: LabelColor::Theme,
                size_mult: 1.25,
                plate: true,
                caption: false,
            },
            // The comparison delta is the one figure a broom-mode pane exists to show.
            ChartLabelField::CompareDelta => ResolvedLabelStyle {
                color: LabelColor::BySign,
                size_mult: 1.7,
                plate: true,
                caption: false,
            },
            // Deliberately smaller than the comparison delta beside it: a secondary indicator must
            // not compete with the figure the pane is being read for.
            ChartLabelField::ScaleBadge => ResolvedLabelStyle {
                color: LabelColor::Theme,
                size_mult: 1.45,
                plate: true,
                caption: false,
            },
            ChartLabelField::Delta1h
            | ChartLabelField::Delta24h
            | ChartLabelField::ExchangeDelta1h
            | ChartLabelField::ExchangeDelta24h
            | ChartLabelField::BtcDelta1h
            | ChartLabelField::BtcDelta24h
            | ChartLabelField::BtcDelta72h
            | ChartLabelField::Funding
            | ChartLabelField::OpenPnlPct
            | ChartLabelField::OpenPnlMoney => ResolvedLabelStyle {
                color: LabelColor::BySign,
                size_mult: 1.0,
                plate: true,
                caption: true,
            },
            // Counts and sizes carry their caption too: a bare "2" over the candles names nothing.
            ChartLabelField::OpenOrders
            | ChartLabelField::PosSize
            | ChartLabelField::Exposure
            | ChartLabelField::FundingIn => ResolvedLabelStyle {
                color: LabelColor::Theme,
                size_mult: 1.0,
                plate: true,
                caption: true,
            },
            _ => ResolvedLabelStyle {
                color: LabelColor::Theme,
                size_mult: 1.0,
                plate: true,
                caption: false,
            },
        }
    }
}

/// Section a field appears under in the "add label" menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartLabelGroup {
    Instrument,
    Market,
    Position,
    Strategy,
}

impl ChartLabelGroup {
    /// Sections in menu order.
    pub const ALL: [ChartLabelGroup; 4] = [
        ChartLabelGroup::Instrument,
        ChartLabelGroup::Market,
        ChartLabelGroup::Position,
        ChartLabelGroup::Strategy,
    ];

    pub fn locale_key(self) -> &'static str {
        match self {
            ChartLabelGroup::Instrument => "chart_labels.group.instrument",
            ChartLabelGroup::Market => "chart_labels.group.market",
            ChartLabelGroup::Position => "chart_labels.group.position",
            ChartLabelGroup::Strategy => "chart_labels.group.strategy",
        }
    }
}
