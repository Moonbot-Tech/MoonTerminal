//! What ONE caption prints: the catalogue of fields, the menu sections they fall into, and the
//! style each one takes when its part overrides nothing.
//!
//! Split out of the model beside it because it is a catalogue and grows with every new figure the
//! chart learns to print, while the model — rows, parts, zones — does not.

use serde::{Deserialize, Serialize};

use super::{LABEL_SIZE_MULT_DEFAULT, LabelColor, ResolvedLabelStyle};

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
    /// Buying half of that volume — the reference terminal's `Bv`.
    ///
    /// Its own field rather than a mode of [`Self::WindowVolume`] because a chart prints the two
    /// halves TOGETHER, one under the other, and a reader compares them: two captions is what that
    /// block is made of.
    WindowBuyVolume,
    /// Selling half of it — the reference terminal's `Sv`.
    WindowSellVolume,
    /// How many trades printed over the window.
    WindowTrades,
    /// What was LIQUIDATED over the window — the reference terminal's `L`.
    ///
    /// A separate stream from the trades beside it, and a shallower one: liquidations are retained
    /// as raw rows only. Nothing compacts them into aggregates the way trades become mini-candles,
    /// so a period reaching past the ring is reported incomplete rather than filled in.
    WindowLiquidations,
    /// The window itself, spelled out: `1 мин`, `500 сделок`.
    ///
    /// A caption that prints no figure at all, and the one thing that makes the volume block
    /// readable: `Bv 12.7k` over `Sv 3.5k` says nothing about the period it covers, and the period
    /// is exactly what the right-click menu changes. The reference terminal heads its own block
    /// with the same line.
    WindowSpanName,
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
    /// Liquidation price the venue reports for it.
    LiqPrice,
    /// Account leverage in force on this market.
    Leverage,
    /// Whether margin here is cross or isolated.
    MarginMode,
    /// Per-coin profit counter the core keeps, which MoonBot prints as `PnL` in its chart header.
    ///
    /// NOT the `Session` figure beside it there: that one is MoonBot's own accumulator, reset from
    /// the markets table, and it is not carried on the wire at all. This is the sum of the balance
    /// row's buy/long/short profit, and the core leaves it at zero on part of its venues — which is
    /// why a zero here prints NOTHING rather than a confident `+0`.
    SessionPnl,
    /// The `Session` figure MoonBot prints in its own chart header, in USDT.
    ///
    /// The counter its markets table resets, which the core publishes as an authoritative snapshot
    /// of its own. Absent on a core too old to publish one — the caption then prints nothing rather
    /// than a zero, which is the whole reason the protocol carries "unknown" apart from "none".
    SessionProfit,
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
    /// Time left until the current candle of a timeframe closes.
    ///
    /// Reads NOTHING from the market: candle buckets are floored on the Unix epoch
    /// ([`crate::market::candles::bucket_open_ms`]), so the answer is wall-clock arithmetic and is
    /// the same figure on every coin. Which timeframe is the caption's own
    /// [`super::LabelTf`] — `Авто` follows the chart, and a fixed one lets a minute chart carry the
    /// hour's and the day's countdowns beside it.
    TfCloseIn,
}

impl ChartLabelField {
    /// Every assignable field, in the order the "add label" menu offers them.
    pub const ALL: [ChartLabelField; 51] = [
        ChartLabelField::Coin,
        ChartLabelField::Core,
        ChartLabelField::Venue,
        ChartLabelField::Quote,
        ChartLabelField::TfCloseIn,
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
        ChartLabelField::WindowSpanName,
        ChartLabelField::WindowBuyVolume,
        ChartLabelField::WindowSellVolume,
        ChartLabelField::WindowVolume,
        ChartLabelField::WindowBuyShare,
        ChartLabelField::WindowTrades,
        ChartLabelField::WindowLiquidations,
        ChartLabelField::MaxLeverage,
        ChartLabelField::MaxOrder,
        ChartLabelField::ExchPosSize,
        ChartLabelField::LiqPrice,
        ChartLabelField::Leverage,
        ChartLabelField::MarginMode,
        ChartLabelField::SessionPnl,
        ChartLabelField::SessionProfit,
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
            ChartLabelField::TfCloseIn => ChartLabelGroup::Time,
            ChartLabelField::LastPrice => ChartLabelGroup::Price,
            ChartLabelField::Delta1h
            | ChartLabelField::Delta24h
            | ChartLabelField::ScaleBadge
            | ChartLabelField::CompareDelta
            | ChartLabelField::ExchangeDelta1h
            | ChartLabelField::ExchangeDelta24h
            | ChartLabelField::BtcDelta1h
            | ChartLabelField::BtcDelta24h
            | ChartLabelField::BtcDelta72h => ChartLabelGroup::Move,
            ChartLabelField::Bid
            | ChartLabelField::Ask
            | ChartLabelField::Spread
            | ChartLabelField::MarkPrice
            | ChartLabelField::MarkDelta
            | ChartLabelField::PriceStep => ChartLabelGroup::Price,
            ChartLabelField::WindowDelta => ChartLabelGroup::Move,
            ChartLabelField::Volume24h
            | ChartLabelField::WindowVolume
            | ChartLabelField::WindowBuyShare
            | ChartLabelField::WindowBuyVolume
            | ChartLabelField::WindowSellVolume
            | ChartLabelField::WindowTrades
            | ChartLabelField::WindowLiquidations
            | ChartLabelField::WindowSpanName => ChartLabelGroup::Volume,
            ChartLabelField::Funding
            | ChartLabelField::FundingIn
            | ChartLabelField::MaxLeverage
            | ChartLabelField::MaxOrder => ChartLabelGroup::Contract,
            ChartLabelField::ArbColumn => ChartLabelGroup::Arbitrage,
            ChartLabelField::OpenPnlPct
            | ChartLabelField::OpenPnlMoney
            | ChartLabelField::OpenOrders
            | ChartLabelField::PosSize
            | ChartLabelField::Exposure => ChartLabelGroup::Position,
            ChartLabelField::ExchPosSize
            | ChartLabelField::LiqPrice
            | ChartLabelField::Leverage
            | ChartLabelField::MarginMode
            | ChartLabelField::SessionPnl
            | ChartLabelField::SessionProfit
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
            ChartLabelField::WindowBuyVolume => "chart_labels.field.window_buy_volume",
            ChartLabelField::WindowSellVolume => "chart_labels.field.window_sell_volume",
            ChartLabelField::WindowTrades => "chart_labels.field.window_trades",
            ChartLabelField::WindowLiquidations => "chart_labels.field.window_liquidations",
            ChartLabelField::WindowSpanName => "chart_labels.field.window_span_name",
            ChartLabelField::MaxLeverage => "chart_labels.field.max_leverage",
            ChartLabelField::MaxOrder => "chart_labels.field.max_order",
            ChartLabelField::ExchPosSize => "chart_labels.field.exch_pos_size",
            ChartLabelField::LiqPrice => "chart_labels.field.liq_price",
            ChartLabelField::Leverage => "chart_labels.field.leverage",
            ChartLabelField::MarginMode => "chart_labels.field.margin_mode",
            ChartLabelField::SessionPnl => "chart_labels.field.session_pnl",
            ChartLabelField::SessionProfit => "chart_labels.field.session_profit",
            ChartLabelField::CoinBalance => "chart_labels.field.coin_balance",
            ChartLabelField::DetectStrategy => "chart_labels.field.detect_strategy",
            ChartLabelField::DetectMsg => "chart_labels.field.detect_msg",
            ChartLabelField::ArbColumn => "chart_labels.field.arb_column",
            ChartLabelField::TfCloseIn => "chart_labels.field.tf_close_in",
        }
    }

    /// Locale key of the SHORT prefix a caption prints when its style asks for one.
    ///
    /// Separate from [`Self::locale_key`]: the menu needs a name a reader can pick from a list
    /// ("Open, %"), while a caption drawn over candles needs the shortest thing that still
    /// identifies the figure ("Open"). A field with nothing worth prefixing returns `None`.
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
            // The word alone would not tell two countdowns apart, which is why the timeframe rides
            // in front of it and survives the switch — see `caption_prefix` in the terminal.
            ChartLabelField::TfCloseIn => Some("chart_labels.short.tf_close_in"),
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
            // `Bv`/`Sv` are the reference terminal's own spelling, and they are not translated:
            // they are read as a pair of symbols beside two numbers, the way `Δ` is.
            ChartLabelField::WindowBuyVolume => Some("chart_labels.short.window_buy_volume"),
            ChartLabelField::WindowSellVolume => Some("chart_labels.short.window_sell_volume"),
            ChartLabelField::WindowTrades => Some("chart_labels.short.window_trades"),
            ChartLabelField::WindowLiquidations => Some("chart_labels.short.window_liquidations"),
            ChartLabelField::MaxLeverage => Some("chart_labels.short.max_leverage"),
            ChartLabelField::MaxOrder => Some("chart_labels.short.max_order"),
            ChartLabelField::ExchPosSize => Some("chart_labels.short.exch_pos_size"),
            ChartLabelField::LiqPrice => Some("chart_labels.short.liq_price"),
            ChartLabelField::Leverage => Some("chart_labels.short.leverage"),
            ChartLabelField::SessionPnl => Some("chart_labels.short.session_pnl"),
            ChartLabelField::SessionProfit => Some("chart_labels.short.session_profit"),
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

    /// Whether this caption is PROSE, and so may be wrapped onto another line instead of cut.
    ///
    /// One field so far, and it is the only one shaped like a sentence: the core's own detect line,
    /// which routinely states half a dozen figures and does not fit the plot's width. Everything
    /// else the chart prints is a number or a name — wrapping those would move a figure onto a line
    /// of its own, where it reads as a caption of its own.
    pub fn wraps(self) -> bool {
        matches!(self, ChartLabelField::DetectMsg)
    }

    /// Whether what this field prints is a PERCENTAGE.
    ///
    /// The colour threshold is stated in percent, so it can only be applied to a caption whose
    /// value is one: "colour from 1%" means nothing to a price, a size, or a countdown, and
    /// applying it there would silently drop the colour from figures the user never set a
    /// threshold for.
    pub fn is_percent(self) -> bool {
        matches!(
            self,
            ChartLabelField::ScaleBadge
                | ChartLabelField::CompareDelta
                | ChartLabelField::Delta1h
                | ChartLabelField::Delta24h
                | ChartLabelField::OpenPnlPct
                | ChartLabelField::ExchangeDelta1h
                | ChartLabelField::ExchangeDelta24h
                | ChartLabelField::BtcDelta1h
                | ChartLabelField::BtcDelta24h
                | ChartLabelField::BtcDelta72h
                | ChartLabelField::Funding
                | ChartLabelField::Spread
                | ChartLabelField::MarkDelta
                | ChartLabelField::WindowDelta
                | ChartLabelField::WindowBuyShare
                // Every line of the column is a spread, so the whole column is.
                | ChartLabelField::ArbColumn
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
    /// Every field takes every window now. The split figures used to stop at five minutes, which is
    /// as far as MoonProto's own rolling trade buckets reach — but the retained MINI-CANDLES carry
    /// a buy/sell split of their own at five-second resolution, and reading the long windows off
    /// them is what removed that ceiling. What a window can still fail to be is COVERED: see
    /// [`Self::splits_sides`] and the readout's own completeness flag.
    pub fn window_choices(self) -> &'static [super::LabelWindow] {
        let _ = self;
        &super::LabelWindow::ALL
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
                | ChartLabelField::WindowBuyVolume
                | ChartLabelField::WindowSellVolume
                | ChartLabelField::WindowTrades
                | ChartLabelField::WindowLiquidations
                | ChartLabelField::WindowSpanName
        )
    }

    /// Whether this field reads a [`super::LabelTf`], and therefore shows that control.
    ///
    /// Asked of the FIELD, like [`Self::uses_window`], so a timeframe cannot linger on a caption
    /// whose field was changed to something that ignores one.
    pub fn uses_tf(self) -> bool {
        matches!(self, ChartLabelField::TfCloseIn)
    }

    /// Whether this field reads a traded AMOUNT, and therefore costs a history read.
    ///
    /// The block's own heading is deliberately not one: it prints the period, which is a setting
    /// rather than a reading, so a chart showing only the heading asks the history for nothing.
    pub fn reads_volume(self) -> bool {
        matches!(
            self,
            ChartLabelField::WindowVolume
                | ChartLabelField::WindowBuyVolume
                | ChartLabelField::WindowSellVolume
                | ChartLabelField::WindowBuyShare
                | ChartLabelField::WindowTrades
                | ChartLabelField::WindowLiquidations
        )
    }

    /// Whether this field belongs to a VOLUME block, and so opens its right-click menu.
    ///
    /// Wider than [`Self::reads_volume`] by the block's heading, which prints the period and reads
    /// nothing — and narrower than [`Self::uses_window`], which also covers the movement figure: a
    /// module printing only `Δ15м` is not a volume block and must keep the plot's own right-click.
    pub fn in_volume_block(self) -> bool {
        self.reads_volume() || self == ChartLabelField::WindowSpanName
    }

    /// Whether this field reads one SIDE of the traded volume, and so needs the buy/sell split.
    ///
    /// The split is what limits where a figure can come from: the whole volume exists in the
    /// 5-minute candle ring as well, a side does not.
    pub fn splits_sides(self) -> bool {
        matches!(
            self,
            ChartLabelField::WindowBuyVolume
                | ChartLabelField::WindowSellVolume
                | ChartLabelField::WindowBuyShare
        )
    }

    /// Whether this field prints an AMOUNT that can be stated in either currency, and therefore
    /// shows the money/coin control.
    ///
    /// A share and a trade count have no unit to choose; a volume does.
    pub fn uses_volume_units(self) -> bool {
        matches!(
            self,
            ChartLabelField::WindowVolume
                | ChartLabelField::WindowBuyVolume
                | ChartLabelField::WindowSellVolume
                | ChartLabelField::WindowLiquidations
        )
    }

    /// Whether this field can draw a proportion BAR beside its figure.
    ///
    /// Only the two sides: a bar states how the buying and the selling compare, and a bar beside a
    /// figure that has nothing to be compared against would be a full bar, always.
    pub fn uses_volume_bar(self) -> bool {
        matches!(
            self,
            ChartLabelField::WindowBuyVolume | ChartLabelField::WindowSellVolume
        )
    }

    /// Style this field draws with when its part overrides nothing.
    ///
    /// These are the colors the hard-coded caption used, so the default configuration reproduces it
    /// without the popup restating them. The SIZE is no longer part of that: every field defaults to
    /// [`LABEL_SIZE_MULT_DEFAULT`], and a caption that wants to lead its neighbours says so on the
    /// popup's size strip.
    pub fn default_style(self) -> ResolvedLabelStyle {
        match self {
            ChartLabelField::Coin => ResolvedLabelStyle {
                value_only: true,
                color_min_pct: 0.0,
                color: LabelColor::Theme,
                size_mult: LABEL_SIZE_MULT_DEFAULT,
                caption: false,
            },
            // The comparison delta is the one figure a broom-mode pane exists to show.
            ChartLabelField::CompareDelta => ResolvedLabelStyle {
                value_only: true,
                color_min_pct: 0.0,
                color: LabelColor::BySign,
                size_mult: LABEL_SIZE_MULT_DEFAULT,
                caption: false,
            },
            ChartLabelField::ScaleBadge => ResolvedLabelStyle {
                value_only: true,
                color_min_pct: 0.0,
                color: LabelColor::Theme,
                size_mult: LABEL_SIZE_MULT_DEFAULT,
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
            | ChartLabelField::SessionProfit
            // Every line of the column is a spread against this market, and which SIDE it is on is
            // the whole reading. The venue's name is the line's prefix and keeps the theme colour.
            | ChartLabelField::ArbColumn
            | ChartLabelField::OpenPnlMoney => ResolvedLabelStyle {
                value_only: true,
                color_min_pct: 0.0,
                color: LabelColor::BySign,
                size_mult: LABEL_SIZE_MULT_DEFAULT,
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
            // `Bv 12.7k` over `Sv 3.5k` is the whole point of the pair: two bare numbers under
            // each other say nothing about which side is which.
            | ChartLabelField::WindowBuyVolume
            | ChartLabelField::WindowSellVolume
            | ChartLabelField::WindowTrades
            | ChartLabelField::WindowLiquidations
            | ChartLabelField::MaxLeverage
            | ChartLabelField::MaxOrder
            | ChartLabelField::ExchPosSize
            | ChartLabelField::LiqPrice
            | ChartLabelField::Leverage
            | ChartLabelField::CoinBalance
            // Two strategy captions can sit on one chart — the one holding an order and the one
            // that last fired — and a bare name says nothing about which is which.
            | ChartLabelField::DetectStrategy => ResolvedLabelStyle {
                value_only: true,
                color_min_pct: 0.0,
                color: LabelColor::Theme,
                size_mult: LABEL_SIZE_MULT_DEFAULT,
                caption: true,
            },
            // Captioned like the funding countdown beside it, and for a sharper reason: this
            // caption's prefix carries its TIMEFRAME, which is what tells two countdowns apart.
            // Falling into the bare arm below would have shipped a field whose locale key, prefix
            // branch and editor switch all existed and never showed.
            ChartLabelField::TfCloseIn => ResolvedLabelStyle {
                value_only: true,
                color_min_pct: 0.0,
                color: LabelColor::Theme,
                size_mult: LABEL_SIZE_MULT_DEFAULT,
                caption: true,
            },
            _ => ResolvedLabelStyle {
                value_only: true,
                color_min_pct: 0.0,
                color: LabelColor::Theme,
                size_mult: LABEL_SIZE_MULT_DEFAULT,
                caption: false,
            },
        }
    }
}

/// Section a field appears under in the "add label" menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartLabelGroup {
    Instrument,
    /// WHEN, rather than what: figures read off the clock and the chart's own bucket grid rather
    /// than off the market. Its own section because a reader hunting for "how long until this
    /// candle closes" scans the headings first, and no other heading claims time.
    Time,
    /// What the coin COSTS: the quote side and the venue's own marks.
    Price,
    /// How far it moved — this coin's own change, the exchange's, BTC's.
    Move,
    /// How much traded.
    Volume,
    /// What the contract itself charges and allows: funding, caps.
    Contract,
    Position,
    /// What the EXCHANGE reports on this market for this core's account, as opposed to what this
    /// core's own orders add up to. Its own section because the two answer different questions, and
    /// a reader picking "position size" has to be told which one they are picking.
    Exchange,
    Strategy,
    /// The column of other venues' prices, which is one field and its own subject.
    Arbitrage,
}

impl ChartLabelGroup {
    /// Sections in menu order.
    /// Sections in picker order: what it IS, when its candle closes, what it costs, how it moves,
    /// how much traded, what the contract charges, what is open — ours then the venue's — who
    /// acted, and the column.
    pub const ALL: [ChartLabelGroup; 10] = [
        ChartLabelGroup::Instrument,
        ChartLabelGroup::Time,
        ChartLabelGroup::Price,
        ChartLabelGroup::Move,
        ChartLabelGroup::Volume,
        ChartLabelGroup::Contract,
        ChartLabelGroup::Position,
        ChartLabelGroup::Exchange,
        ChartLabelGroup::Strategy,
        ChartLabelGroup::Arbitrage,
    ];

    pub fn locale_key(self) -> &'static str {
        match self {
            ChartLabelGroup::Instrument => "chart_labels.group.instrument",
            ChartLabelGroup::Time => "chart_labels.group.time",
            ChartLabelGroup::Price => "chart_labels.group.price",
            ChartLabelGroup::Move => "chart_labels.group.move",
            ChartLabelGroup::Volume => "chart_labels.group.volume",
            ChartLabelGroup::Contract => "chart_labels.group.contract",
            ChartLabelGroup::Arbitrage => "chart_labels.group.arbitrage",
            ChartLabelGroup::Position => "chart_labels.group.position",
            ChartLabelGroup::Exchange => "chart_labels.group.exchange",
            ChartLabelGroup::Strategy => "chart_labels.group.strategy",
        }
    }
}
