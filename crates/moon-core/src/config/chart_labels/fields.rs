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
    /// The venue's own tags for this coin: `Seed`, `Alpha`, `Monitoring`.
    CoinTags,
    /// Best bid and best ask.
    Bid,
    Ask,
    /// Distance between them as a percentage of the ask: what a market order gives up.
    Spread,
    /// Exchange mark price — what liquidation and funding are computed against.
    MarkPrice,
    /// Signed distance of that mark from the last traded price, in percent.
    MarkDelta,
    /// The market's own price step.
    PriceStep,
    /// 24-hour volume as the venue states it, in the quote currency.
    Volume24h,
    /// Price movement over the caption's [`super::LabelWindow`], as an unsigned magnitude.
    ///
    /// The Screener's delta figure, not a signed change: it answers "how far did it travel", which
    /// is what a short window is read for. The signed hour and day changes are [`Self::Delta1h`]
    /// and [`Self::Delta24h`], and they stay separate because they answer which WAY.
    WindowDelta,
    /// Traded volume over that window, in the quote currency.
    WindowVolume,
    /// Share of that volume that was buying, in percent. Only short windows carry the split.
    WindowBuyShare,
    /// Maximum leverage the venue allows on this market.
    MaxLeverage,
    /// Largest order the venue accepts here, in the quote currency.
    MaxOrder,
    /// Position the EXCHANGE reports on this market, in the base coin, signed by direction.
    ///
    /// A different fact from [`Self::PosSize`] beside it: this is what the account actually holds,
    /// including anything traded by hand or by another terminal, while the open-order figures count
    /// only what this core's strategies opened.
    ExchPosSize,
    /// Average entry price of that exchange position.
    ExchPosPrice,
    /// Liquidation price the venue reports for it.
    LiqPrice,
    /// Account leverage in force on this market.
    Leverage,
    /// Whether margin here is cross or isolated.
    MarginMode,
    /// Profit this core booked on this coin during the session.
    SessionPnl,
    /// Free balance of the coin itself.
    CoinBalance,
    /// Strategy that produced the newest detect THIS core fired on this market.
    ///
    /// A different question from [`Self::OrderStrategy`] beside it: that one names the strategy
    /// holding an open order, this one names the strategy that last SAW something here — which is
    /// often a strategy that took no position at all.
    DetectStrategy,
    /// The line that detect carried, as the core wrote it.
    DetectMsg,
    /// Every arbitrage venue the core watches for this coin, as a COLUMN: one line per venue.
    ///
    /// The one caption that is not a single figure, because the figure it states is a comparison
    /// and a comparison against one venue is not what anybody reads it for. Which venues appear, in
    /// what order, under what name and in what colour is [`crate::config::ArbViewCfg`] — a global
    /// roster, not a per-chart setting, because "Gate is green to me" is not a fact about one tab.
    ArbColumn,
}

impl ChartLabelField {
    /// Every assignable field, in the order the "add label" menu offers them.
    pub const ALL: [ChartLabelField; 45] = [
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
        ChartLabelField::CoinTags,
        ChartLabelField::Bid,
        ChartLabelField::Ask,
        ChartLabelField::Spread,
        ChartLabelField::MarkPrice,
        ChartLabelField::MarkDelta,
        ChartLabelField::PriceStep,
        ChartLabelField::Volume24h,
        ChartLabelField::WindowDelta,
        ChartLabelField::WindowVolume,
        ChartLabelField::WindowBuyShare,
        ChartLabelField::MaxLeverage,
        ChartLabelField::MaxOrder,
        ChartLabelField::ExchPosSize,
        ChartLabelField::ExchPosPrice,
        ChartLabelField::LiqPrice,
        ChartLabelField::Leverage,
        ChartLabelField::MarginMode,
        ChartLabelField::SessionPnl,
        ChartLabelField::CoinBalance,
        ChartLabelField::DetectStrategy,
        ChartLabelField::DetectMsg,
        ChartLabelField::ArbColumn,
    ];

    /// Menu section this field belongs to.
    pub fn group(self) -> ChartLabelGroup {
        match self {
            ChartLabelField::Coin
            | ChartLabelField::Core
            | ChartLabelField::Venue
            | ChartLabelField::Quote
            | ChartLabelField::CoinTags
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
            | ChartLabelField::FundingIn
            | ChartLabelField::Bid
            | ChartLabelField::Ask
            | ChartLabelField::Spread
            | ChartLabelField::MarkPrice
            | ChartLabelField::MarkDelta
            | ChartLabelField::PriceStep
            | ChartLabelField::Volume24h
            | ChartLabelField::WindowDelta
            | ChartLabelField::WindowVolume
            | ChartLabelField::WindowBuyShare
            | ChartLabelField::MaxLeverage
            | ChartLabelField::MaxOrder
            | ChartLabelField::ArbColumn => ChartLabelGroup::Market,
            ChartLabelField::OpenPnlPct
            | ChartLabelField::OpenPnlMoney
            | ChartLabelField::OpenOrders
            | ChartLabelField::PosSize
            | ChartLabelField::Exposure => ChartLabelGroup::Position,
            ChartLabelField::ExchPosSize
            | ChartLabelField::ExchPosPrice
            | ChartLabelField::LiqPrice
            | ChartLabelField::Leverage
            | ChartLabelField::MarginMode
            | ChartLabelField::SessionPnl
            | ChartLabelField::CoinBalance => ChartLabelGroup::Exchange,
            ChartLabelField::OrderStrategy
            | ChartLabelField::DetectStrategy
            | ChartLabelField::DetectMsg => ChartLabelGroup::Strategy,
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
            ChartLabelField::CoinTags => "chart_labels.field.coin_tags",
            ChartLabelField::Bid => "chart_labels.field.bid",
            ChartLabelField::Ask => "chart_labels.field.ask",
            ChartLabelField::Spread => "chart_labels.field.spread",
            ChartLabelField::MarkPrice => "chart_labels.field.mark_price",
            ChartLabelField::MarkDelta => "chart_labels.field.mark_delta",
            ChartLabelField::PriceStep => "chart_labels.field.price_step",
            ChartLabelField::Volume24h => "chart_labels.field.volume_24h",
            ChartLabelField::WindowDelta => "chart_labels.field.window_delta",
            ChartLabelField::WindowVolume => "chart_labels.field.window_volume",
            ChartLabelField::WindowBuyShare => "chart_labels.field.window_buy_share",
            ChartLabelField::MaxLeverage => "chart_labels.field.max_leverage",
            ChartLabelField::MaxOrder => "chart_labels.field.max_order",
            ChartLabelField::ExchPosSize => "chart_labels.field.exch_pos_size",
            ChartLabelField::ExchPosPrice => "chart_labels.field.exch_pos_price",
            ChartLabelField::LiqPrice => "chart_labels.field.liq_price",
            ChartLabelField::Leverage => "chart_labels.field.leverage",
            ChartLabelField::MarginMode => "chart_labels.field.margin_mode",
            ChartLabelField::SessionPnl => "chart_labels.field.session_pnl",
            ChartLabelField::CoinBalance => "chart_labels.field.coin_balance",
            ChartLabelField::DetectStrategy => "chart_labels.field.detect_strategy",
            ChartLabelField::DetectMsg => "chart_labels.field.detect_msg",
            ChartLabelField::ArbColumn => "chart_labels.field.arb_column",
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
            ChartLabelField::Bid => Some("chart_labels.short.bid"),
            ChartLabelField::Ask => Some("chart_labels.short.ask"),
            ChartLabelField::Spread => Some("chart_labels.short.spread"),
            ChartLabelField::MarkPrice => Some("chart_labels.short.mark_price"),
            ChartLabelField::MarkDelta => Some("chart_labels.short.mark_delta"),
            ChartLabelField::PriceStep => Some("chart_labels.short.price_step"),
            ChartLabelField::Volume24h => Some("chart_labels.short.volume_24h"),
            // The window fields name themselves with the WINDOW rather than a fixed word: the
            // caption builder appends it, so one prefix says both which figure and over what.
            ChartLabelField::WindowDelta => Some("chart_labels.short.window_delta"),
            ChartLabelField::WindowVolume => Some("chart_labels.short.window_volume"),
            ChartLabelField::WindowBuyShare => Some("chart_labels.short.window_buy_share"),
            ChartLabelField::MaxLeverage => Some("chart_labels.short.max_leverage"),
            ChartLabelField::MaxOrder => Some("chart_labels.short.max_order"),
            ChartLabelField::ExchPosSize => Some("chart_labels.short.exch_pos_size"),
            ChartLabelField::ExchPosPrice => Some("chart_labels.short.exch_pos_price"),
            ChartLabelField::LiqPrice => Some("chart_labels.short.liq_price"),
            ChartLabelField::Leverage => Some("chart_labels.short.leverage"),
            ChartLabelField::SessionPnl => Some("chart_labels.short.session_pnl"),
            ChartLabelField::CoinBalance => Some("chart_labels.short.coin_balance"),
            ChartLabelField::DetectStrategy => Some("chart_labels.short.detect_strategy"),
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

    /// Whether this field prints a COLUMN of its own rather than one value.
    ///
    /// Asked by the drawing pass, which addresses such a caption's lines in their own run range,
    /// and by the editor, which offers the roster's settings instead of a prefix switch.
    pub fn is_column(self) -> bool {
        matches!(self, ChartLabelField::ArbColumn)
    }

    /// Windows this field can actually be read over.
    ///
    /// Not every figure exists over every window: the buy/sell split comes from the retained trade
    /// buckets, which cover five minutes, while the longer windows are built from candles that
    /// carry no split at all. Offering the day there would offer a caption that prints nothing and
    /// reads as broken.
    pub fn window_choices(self) -> &'static [super::LabelWindow] {
        match self {
            ChartLabelField::WindowBuyShare => super::LabelWindow::TRADE_WINDOWS,
            _ => &super::LabelWindow::ALL,
        }
    }

    /// Whether this field reads a [`super::LabelWindow`], and therefore shows that control.
    ///
    /// Asked of the FIELD, like [`Self::uses_pnl_basis`], so a window cannot linger on a caption
    /// whose field was changed to something that ignores one.
    pub fn uses_window(self) -> bool {
        matches!(
            self,
            ChartLabelField::WindowDelta
                | ChartLabelField::WindowVolume
                | ChartLabelField::WindowBuyShare
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
            | ChartLabelField::MarkDelta
            | ChartLabelField::SessionPnl
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
            | ChartLabelField::FundingIn
            | ChartLabelField::Bid
            | ChartLabelField::Ask
            | ChartLabelField::Spread
            | ChartLabelField::MarkPrice
            | ChartLabelField::PriceStep
            | ChartLabelField::Volume24h
            | ChartLabelField::WindowDelta
            | ChartLabelField::WindowVolume
            | ChartLabelField::WindowBuyShare
            | ChartLabelField::MaxLeverage
            | ChartLabelField::MaxOrder
            | ChartLabelField::ExchPosSize
            | ChartLabelField::ExchPosPrice
            | ChartLabelField::LiqPrice
            | ChartLabelField::Leverage
            | ChartLabelField::CoinBalance
            // Two strategy captions can sit on one chart — the one holding an order and the one
            // that last fired — and a bare name says nothing about which is which.
            | ChartLabelField::DetectStrategy => ResolvedLabelStyle {
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
    /// What the EXCHANGE reports on this market for this core's account, as opposed to what this
    /// core's own orders add up to. Its own section because the two answer different questions, and
    /// a reader picking "position size" has to be told which one they are picking.
    Exchange,
    Strategy,
}

impl ChartLabelGroup {
    /// Sections in menu order.
    pub const ALL: [ChartLabelGroup; 5] = [
        ChartLabelGroup::Instrument,
        ChartLabelGroup::Market,
        ChartLabelGroup::Position,
        ChartLabelGroup::Exchange,
        ChartLabelGroup::Strategy,
    ];

    pub fn locale_key(self) -> &'static str {
        match self {
            ChartLabelGroup::Instrument => "chart_labels.group.instrument",
            ChartLabelGroup::Market => "chart_labels.group.market",
            ChartLabelGroup::Position => "chart_labels.group.position",
            ChartLabelGroup::Exchange => "chart_labels.group.exchange",
            ChartLabelGroup::Strategy => "chart_labels.group.strategy",
        }
    }
}
