//! Ready-made rows: the caption families a chart is usually read with.
//!
//! A preset is a STARTING POINT, not a type — it fills a new row with a set of fields, a band and a
//! name, and from that moment the row is an ordinary row: parts can be added, removed, restyled and
//! the whole thing renamed. Nothing downstream knows a row came from one.
//!
//! They exist because the alternative is a menu of twenty-two fields and no answer to "what do
//! people usually put here", and because the families they name — the instrument, the deltas, the
//! position — are exactly the rows the reference terminal prints.

use super::{ChartLabelField, LabelAlign, LabelZone};

/// One ready-made row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

impl LabelPreset {
    /// Every preset, in menu order: what the chart is, then how it moves, then what is at risk.
    pub const ALL: [LabelPreset; 5] = [
        LabelPreset::Instrument,
        LabelPreset::CoinDeltas,
        LabelPreset::MarketBackdrop,
        LabelPreset::Position,
        LabelPreset::Funding,
    ];

    /// Locale key of the preset's name, which is also the name the created row takes.
    pub fn locale_key(self) -> &'static str {
        match self {
            LabelPreset::Instrument => "chart_labels.preset.instrument",
            LabelPreset::CoinDeltas => "chart_labels.preset.coin_deltas",
            LabelPreset::MarketBackdrop => "chart_labels.preset.market_backdrop",
            LabelPreset::Position => "chart_labels.preset.position",
            LabelPreset::Funding => "chart_labels.preset.funding",
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
        }
    }

    /// Band the row is created in.
    ///
    /// A wide family goes over the PLOT, where a row has the whole chart to spread across; a short
    /// one goes into the control strip, which is only as wide as the order book beside it.
    pub fn zone(self) -> LabelZone {
        match self {
            LabelPreset::Instrument | LabelPreset::Funding => LabelZone::ZoneTop,
            LabelPreset::CoinDeltas | LabelPreset::MarketBackdrop | LabelPreset::Position => {
                LabelZone::ChartTop
            }
        }
    }

    /// Where in that band the row sits.
    pub fn align(self) -> LabelAlign {
        match self {
            LabelPreset::Instrument | LabelPreset::Funding => LabelAlign::Right,
            LabelPreset::CoinDeltas | LabelPreset::MarketBackdrop | LabelPreset::Position => {
                LabelAlign::Left
            }
        }
    }
}
