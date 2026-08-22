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
}

impl LabelPreset {
    /// Every preset, in menu order: what the chart is, then how it moves, then what is at risk.
    pub const ALL: [LabelPreset; 6] = [
        LabelPreset::Instrument,
        LabelPreset::CoinDeltas,
        LabelPreset::MarketBackdrop,
        LabelPreset::Position,
        LabelPreset::Funding,
        LabelPreset::Arbitrage,
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
            ],
            LabelPreset::Funding => &[ChartLabelField::Funding, ChartLabelField::FundingIn],
            LabelPreset::Arbitrage => &[ChartLabelField::ArbColumn],
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
            LabelPreset::CoinDeltas | LabelPreset::MarketBackdrop | LabelPreset::Position => {
                LabelZone::ChartTop
            }
        }
    }

    /// Which way the created module's own captions run.
    ///
    /// A column for the arbitrage roster and for nothing else so far: its lines are venues, one
    /// under another, and printing them across a line would be a row of prices with no way to tell
    /// which venue each belongs to.
    pub fn flow(self) -> LabelFlow {
        match self {
            LabelPreset::Arbitrage => LabelFlow::Column,
            _ => LabelFlow::Row,
        }
    }

    /// Where in that band the row sits.
    pub fn align(self) -> LabelAlign {
        match self {
            LabelPreset::Instrument | LabelPreset::Funding => LabelAlign::Right,
            LabelPreset::Arbitrage => LabelAlign::Left,
            LabelPreset::CoinDeltas | LabelPreset::MarketBackdrop | LabelPreset::Position => {
                LabelAlign::Left
            }
        }
    }
}
