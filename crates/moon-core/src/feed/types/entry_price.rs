//! The core's notice that it moved a manual entry price onto an exchange price limit.
//!
//! MoonBot 7.73 N2 sends it when a manual entry asked for a price the exchange does not accept
//! and the core placed the order at the nearest allowed price instead. It is a market-level hint
//! for the chart — the requested price and the allowed range — not an acknowledgement of any
//! order: the order's real price still arrives through the ordinary order updates. Best effort,
//! normally at most one per two seconds across all markets of a core, and older cores never send
//! it at all.

use std::time::{Duration, Instant};

/// How long the store keeps a market's notice after receiving it.
///
/// Only a bound on the per-core map, which would otherwise keep one row for every market a manual
/// entry was ever corrected on. It has to outlast the chart's own display time
/// (`moon_chart::entry_range::SHOW`, asserted there), or a notice would vanish mid-fade.
pub const ENTRY_NOTICE_RETAIN: Duration = Duration::from_secs(60);

/// One price correction, as the terminal keeps it.
///
/// The wire spells "no limit on this side" as `0`; here that is `None`, so no reader can mistake
/// a missing bound for a price of zero.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryPriceCorrection {
    /// The market the manual entry was for, in the core's own spelling.
    pub market: String,
    /// The price the manual entry asked for before the core corrected it.
    pub requested_price: f64,
    /// The lowest price the exchange accepts, when the core reported one.
    pub min_price: Option<f64>,
    /// The highest price the exchange accepts, when the core reported one.
    pub max_price: Option<f64>,
}

impl EntryPriceCorrection {
    /// Build a correction from the wire's four values, or `None` when it describes nothing.
    ///
    /// A requested price that is not a finite positive number cannot be drawn and is dropped
    /// whole. A bound that is zero, negative or not finite is read as absent: zero is the wire's
    /// own "no limit", and the other two are not a price an exchange could have reported.
    ///
    /// Args:
    ///     market: The market name the core sent.
    ///     requested_price: The price the entry asked for.
    ///     min_price: The lower limit, `0` for none.
    ///     max_price: The upper limit, `0` for none.
    ///
    /// Returns:
    ///     The correction, or `None` for an empty market name or an unusable requested price.
    pub fn from_wire(
        market: &str,
        requested_price: f64,
        min_price: f64,
        max_price: f64,
    ) -> Option<Self> {
        if market.is_empty() || !is_price(requested_price) {
            return None;
        }
        Some(Self {
            market: market.to_string(),
            requested_price,
            min_price: is_price(min_price).then_some(min_price),
            max_price: is_price(max_price).then_some(max_price),
        })
    }
}

/// Whether a wire value is a usable price.
fn is_price(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

/// The latest correction of one market on one core, with when the terminal received it.
///
/// The SDK keeps no timer or list of these: how long one stays on the chart is the terminal's
/// decision, measured from `received_at`, a monotonic instant so a wall-clock step can neither
/// stretch a notice nor cut it short.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryPriceNotice {
    pub correction: EntryPriceCorrection,
    /// When the store filed it.
    pub received_at: Instant,
    /// Numbers the notices of one core in arrival order, so a repeat of the very same correction
    /// still reads as a new notice that restarts its display.
    pub seq: u64,
}

#[cfg(test)]
mod tests;
