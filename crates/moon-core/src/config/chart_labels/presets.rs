//! Ready-made rows: the caption families a chart is usually read with.
//!
//! A preset is a STARTING POINT, not a type — it fills a new row with a set of fields, a band and a
//! name, and from that moment the row is an ordinary row: parts can be added, removed, restyled and
//! the whole thing renamed. Nothing downstream knows a row came from one.
//!
//! They exist because the alternative is a menu of twenty-two fields and no answer to "what do
//! people usually put here", and because the families they name — the instrument, the deltas, the
//! position — are exactly the rows the reference terminal prints.

use serde::{Deserialize, Serialize};

use super::{ChartLabelField, LabelAlign, LabelFlow, LabelZone};

/// One ready-made row.
///
/// Serialized, because a module REMEMBERS the preset it was created from: that is what keeps its
/// name translatable. The serde name is the wire form, so a variant may be reordered freely but
/// never renamed without migrating `charts.json` and `layout.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelPreset {
    /// What is being looked at: the coin, the core that trades it, the venue.
    Instrument,
    /// How far this coin has moved, over the windows the header ticker uses.
    CoinDeltas,
    /// The background that movement is read against: the whole exchange, and BTC.
    MarketBackdrop,
    /// What is open on this market right now.
    Position,
    /// The perpetual's funding charge and the time to the next one.
    Funding,
    /// What the same coin costs on every other venue the core watches.
    Arbitrage,
    /// How much was bought and how much was sold over a period, with the period named above them.
    Volumes,
    /// The same figures MEASURED: read around wherever the pointer is, with the liquidations that
    /// landed there. The reference terminal's measuring tool, as a caption module.
    CursorVolumes,
    /// The Y-scale badge on its own, which is where a glance checks how far the pane reaches.
    Scale,
    /// What this core has made on this coin: its own counter and the one MoonBot prints.
    Session,
    /// The last detect this core fired on this coin, and the strategy behind what is open.
    Detect,
    /// What ONE closed trade was: the strategy that opened it, its detect line, why it closed.
    ///
    /// Only a chart that was HANDED a trade can fill it — the trade-detail window — so it is also
    /// the module that view opens with. On any other chart its captions have nothing to state and
    /// print nothing, which is the same way every optional figure behaves here.
    Trade,
}

impl LabelPreset {
    /// Every preset, in menu order: what the chart is, then how it moves, then what is at risk.
    pub const ALL: [LabelPreset; 12] = [
        LabelPreset::Instrument,
        LabelPreset::Scale,
        LabelPreset::CoinDeltas,
        LabelPreset::MarketBackdrop,
        LabelPreset::Volumes,
        LabelPreset::CursorVolumes,
        LabelPreset::Position,
        LabelPreset::Session,
        LabelPreset::Funding,
        LabelPreset::Arbitrage,
        LabelPreset::Detect,
        LabelPreset::Trade,
    ];

    /// Locale key of the preset's name, which is also the name the created row takes.
    pub fn locale_key(self) -> &'static str {
        match self {
            LabelPreset::Instrument => "chart_labels.preset.instrument",
            LabelPreset::CoinDeltas => "chart_labels.preset.coin_deltas",
            LabelPreset::MarketBackdrop => "chart_labels.preset.market_backdrop",
            LabelPreset::Position => "chart_labels.preset.position",
            LabelPreset::Funding => "chart_labels.preset.funding",
            LabelPreset::Arbitrage => "chart_labels.preset.arbitrage",
            LabelPreset::Volumes => "chart_labels.preset.volumes",
            LabelPreset::CursorVolumes => "chart_labels.preset.cursor_volumes",
            LabelPreset::Scale => "chart_labels.preset.scale",
            LabelPreset::Session => "chart_labels.preset.session",
            LabelPreset::Detect => "chart_labels.preset.detect",
            LabelPreset::Trade => "chart_labels.preset.trade",
        }
    }

    /// The captions the row is created with, in print order.
    pub fn fields(self) -> &'static [ChartLabelField] {
        match self {
            LabelPreset::Instrument => &[
                ChartLabelField::Coin,
                ChartLabelField::Core,
                ChartLabelField::Venue,
            ],
            LabelPreset::CoinDeltas => &[ChartLabelField::Delta1h, ChartLabelField::Delta24h],
            LabelPreset::MarketBackdrop => &[
                ChartLabelField::ExchangeDelta1h,
                ChartLabelField::ExchangeDelta24h,
                ChartLabelField::BtcDelta1h,
                ChartLabelField::BtcDelta24h,
            ],
            LabelPreset::Position => &[
                ChartLabelField::OpenOrders,
                ChartLabelField::Exposure,
                ChartLabelField::OpenPnlMoney,
                ChartLabelField::OpenPnlPct,
                ChartLabelField::SessionPnl,
            ],
            LabelPreset::Funding => &[ChartLabelField::Funding, ChartLabelField::FundingIn],
            LabelPreset::Arbitrage => &[ChartLabelField::ArbColumn],
            // The period first, then the two sides under it — the reference terminal's own block.
            // The whole volume and the trade count are left OUT and switched on from the chart's
            // own menu: they answer a second question, and a four-line block over the candles is
            // already as tall as this corner takes.
            LabelPreset::Volumes => &[
                ChartLabelField::WindowSpanName,
                ChartLabelField::WindowBuyVolume,
                ChartLabelField::WindowSellVolume,
            ],
            // The measuring block carries the liquidations as well: what got blown out around a
            // spike is half of what a reader points at one for.
            LabelPreset::CursorVolumes => &[
                ChartLabelField::WindowSpanName,
                ChartLabelField::WindowBuyVolume,
                ChartLabelField::WindowSellVolume,
                ChartLabelField::WindowLiquidations,
            ],
            LabelPreset::Scale => &[ChartLabelField::ScaleBadge],
            LabelPreset::Session => &[
                ChartLabelField::SessionPnl,
                ChartLabelField::SessionProfit,
            ],
            LabelPreset::Detect => &[
                ChartLabelField::DetectStrategy,
                ChartLabelField::DetectMsg,
                ChartLabelField::OrderStrategy,
            ],
            LabelPreset::Trade => &[
                ChartLabelField::TradeStrategy,
                ChartLabelField::TradeDetect,
                ChartLabelField::TradeSellReason,
            ],
        }
    }

    /// Band the row is created in.
    ///
    /// A wide family goes over the PLOT, where a row has the whole chart to spread across; a short
    /// one goes into the control strip, which is only as wide as the order book beside it.
    pub fn zone(self) -> LabelZone {
        match self {
            LabelPreset::Instrument | LabelPreset::Funding => LabelZone::ZoneTop,
            // Over the PLOT, on the left, which is where the reference terminal prints it and the
            // only band tall enough for a column of a dozen venues.
            LabelPreset::Arbitrage => LabelZone::ChartTop,
            LabelPreset::CoinDeltas
            | LabelPreset::MarketBackdrop
            | LabelPreset::Position
            | LabelPreset::Session
            | LabelPreset::Scale
            // Over the plot, where the reference terminal prints it: the block is three lines and
            // the control strip is only as wide as the order book.
            | LabelPreset::Volumes
            | LabelPreset::CursorVolumes
            | LabelPreset::Detect
            | LabelPreset::Trade => LabelZone::ChartTop,
        }
    }

    /// Period the created module's captions read over.
    ///
    /// A MINUTE for the volume block, which is what the reference terminal opens it at — and the
    /// only answer that is also cheap: one, three and five minutes are maintained incrementally by
    /// the protocol itself, while anything longer is accumulated from retained rows. Creating the
    /// block at the catalogue's default hour made every new one pay for that accumulation.
    ///
    /// `None` leaves the caption's own default alone, which is what every other preset wants.
    pub fn window(self) -> Option<super::LabelWindow> {
        match self {
            LabelPreset::Volumes | LabelPreset::CursorVolumes => Some(super::LabelWindow::M1),
            _ => None,
        }
    }

    /// Where the created module MEASURES: at the live edge, or around the pointer.
    pub fn anchor(self) -> super::SpanAnchor {
        match self {
            LabelPreset::CursorVolumes => super::SpanAnchor::Cursor,
            _ => super::SpanAnchor::Now,
        }
    }

    /// A custom period the created module reads over, when a fixed window is not what it is for.
    ///
    /// Ten seconds for the measuring block: it answers what happened right AT the point, and a
    /// minute around a spike is mostly the calm on either side of it.
    pub fn span(self) -> super::LabelSpan {
        match self {
            LabelPreset::CursorVolumes => super::LabelSpan::Seconds(10),
            _ => super::LabelSpan::Window,
        }
    }

    /// Which way the created module's own captions run.
    ///
    /// A column for the arbitrage roster and for nothing else so far: its lines are venues, one
    /// under another, and printing them across a line would be a row of prices with no way to tell
    /// which venue each belongs to.
    pub fn flow(self) -> LabelFlow {
        match self {
            // A column for the roster, and for the detect module: its three captions are a line of
            // prose, a strategy name and another strategy name, and side by side they read as one
            // run-on sentence. The volume block stacks for the same reason its bars do: the two
            // sides are compared against each other, and a comparison reads down, not across.
            LabelPreset::Arbitrage
            | LabelPreset::Volumes
            | LabelPreset::CursorVolumes
            | LabelPreset::Detect
            // The trade module stacks for the detect module's reason, and harder: its middle
            // caption is a whole sentence, and a strategy name printed beside it would be read as
            // part of that sentence.
            | LabelPreset::Trade => LabelFlow::Column,
            _ => LabelFlow::Row,
        }
    }

    /// Where in that band the row sits.
    pub fn align(self) -> LabelAlign {
        match self {
            LabelPreset::Instrument | LabelPreset::Funding => LabelAlign::Right,
            LabelPreset::Arbitrage => LabelAlign::Left,
            LabelPreset::CoinDeltas
            | LabelPreset::MarketBackdrop
            | LabelPreset::Position
            | LabelPreset::Session => LabelAlign::Left,
            // The volume blocks and the badge ride the plot's RIGHT edge, under the price scale,
            // which is where the eye already is when it reads a figure off the axis.
            LabelPreset::Volumes | LabelPreset::CursorVolumes | LabelPreset::Scale => {
                LabelAlign::Right
            }
            // Centred: a detect line is the widest thing the chart prints, and either edge would
            // put it under a module that is already there.
            LabelPreset::Detect | LabelPreset::Trade => LabelAlign::Center,
        }
    }
}
