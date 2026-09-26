//! Which of the report's deltas the track re-evaluates, and which it cannot.

use super::super::Deltas;

/// A delta the track re-evaluates along the window. Every one is a report column
/// ([`DeltaField::column`]) and a modifier input of one family or both — `MShotAdd*` on the
/// MoonShot corridor, `Add*` on the sell and the stop (`mshot::Modifiers`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeltaField {
    D1m,
    D5m,
    D15m,
    D1h,
    D3h,
    D24h,
    /// The last five seconds' move (`d5s`, RTTI `Last5sDelta`).
    D5s,
    /// The hour's rise from the price an hour ago to its high.
    Pump1h,
    /// The hour's fall from the price an hour ago to its low.
    Dump1h,
    /// BTC's 1-minute range (`dbtc1m`).
    Btc1m,
    /// BTC's 5-minute range (`btc5mdelta`).
    Btc5m,
    /// BTC's signed deviation from its 1-hour average (`btc1hdelta`).
    Btc1h,
}

impl DeltaField {
    /// Every field, in the order the track stores them.
    pub const ALL: [Self; 12] = [
        Self::D1m,
        Self::D5m,
        Self::D15m,
        Self::D1h,
        Self::D3h,
        Self::D24h,
        Self::D5s,
        Self::Pump1h,
        Self::Dump1h,
        Self::Btc1m,
        Self::Btc5m,
        Self::Btc1h,
    ];

    pub const COUNT: usize = Self::ALL.len();

    /// The field's slot in a track's values.
    pub fn index(self) -> usize {
        self as usize
    }

    /// The report column the field is the snapshot of.
    pub fn column(self) -> &'static str {
        match self {
            Self::D1m => "d1m",
            Self::D5m => "d5m",
            Self::D15m => "d15m",
            Self::D1h => "d1h",
            Self::D3h => "d3h",
            Self::D24h => "d24h",
            Self::D5s => "d5s",
            Self::Pump1h => "pump1h",
            Self::Dump1h => "dump1h",
            Self::Btc1m => "dbtc1m",
            Self::Btc5m => "btc5mdelta",
            Self::Btc1h => "btc1hdelta",
        }
    }

    /// The field's value in a set of deltas.
    pub fn of(self, d: &Deltas) -> f64 {
        match self {
            Self::D1m => d.d1m,
            Self::D5m => d.d5m,
            Self::D15m => d.d15m,
            Self::D1h => d.d1h,
            Self::D3h => d.d3h,
            Self::D24h => d.d24h,
            Self::D5s => d.d5s,
            Self::Pump1h => d.pump1h,
            Self::Dump1h => d.dump1h,
            Self::Btc1m => d.btc1m,
            Self::Btc5m => d.btc5m,
            Self::Btc1h => d.btc1h,
        }
    }

    /// Replace the field's value in a set of deltas.
    pub fn set(self, d: &mut Deltas, value: f64) {
        let slot = match self {
            Self::D1m => &mut d.d1m,
            Self::D5m => &mut d.d5m,
            Self::D15m => &mut d.d15m,
            Self::D1h => &mut d.d1h,
            Self::D3h => &mut d.d3h,
            Self::D24h => &mut d.d24h,
            Self::D5s => &mut d.d5s,
            Self::Pump1h => &mut d.pump1h,
            Self::Dump1h => &mut d.dump1h,
            Self::Btc1m => &mut d.btc1m,
            Self::Btc5m => &mut d.btc5m,
            Self::Btc1h => &mut d.btc1h,
        };
        *slot = value;
    }

    /// Whether a report value of exactly zero means the core never filled the field rather than
    /// a market that held still: true for the coin's ranges of a minute and longer — no traded
    /// market prints one price for a minute on end — and for every BTC field: BTC never holds
    /// still for a minute, and a deviation from its average is never exactly zero, while a
    /// report column the replica lacks reads as zero (`deals::read_on`), and a core without BTC's
    /// prices files zeros (a MoonShot trade of 2026-06-25 on a local core: all BTC deltas 0).
    /// Five seconds and an hour's rise or fall can legitimately be zero.
    pub fn zero_is_unfilled(self) -> bool {
        !matches!(self, Self::D5s | Self::Pump1h | Self::Dump1h)
    }

    /// Whether the field is read off BTC's market rather than the deal's own.
    pub fn is_btc(self) -> bool {
        matches!(self, Self::Btc1m | Self::Btc5m | Self::Btc1h)
    }
}

/// A report delta the track does not re-evaluate, and why — the summary says so beside the
/// fields it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotComputed {
    /// `dmark`: the mark price has no history here.
    MarkPrice,
    /// `pricebug`: the core's own lag measure against its price line.
    PriceBug,
    /// `exchange1hdelta`, `exchange24hdelta`: an average over every market of the exchange.
    Market,
}

impl NotComputed {
    pub const ALL: [Self; 3] = [Self::MarkPrice, Self::PriceBug, Self::Market];

    /// The report columns this entry stands for.
    pub fn columns(self) -> &'static str {
        match self {
            Self::MarkPrice => "dmark",
            Self::PriceBug => "pricebug",
            Self::Market => "exchange1hdelta, exchange24hdelta",
        }
    }
}
