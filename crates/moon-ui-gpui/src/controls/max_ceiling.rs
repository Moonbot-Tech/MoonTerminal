//! The toolbar's `MAX`: the smaller of the two exchange ceilings that actually stop an order.
//!
//! Two caps bound how large an order can be, and they are different facts. The per-order maximum
//! ([`MaxOrderReadout`]) is flat across leverage. The position cap
//! ([`MarketLimits::position_cap`], MoonBot's `<max. N>`) bounds the WHOLE position at the APPLIED
//! leverage and falls as leverage rises — on Binance PENDLE it is 5 000 USDT at ×75 against a
//! per-order figure above a million. A readout that showed only the first stated the ceiling that
//! does not bind exactly when a trader reads the number beside the leverage (#1067), so the row
//! shows whichever is lower and its hover says which one that is.
//!
//! The leverage popup keeps printing both figures separately; this type is the ROW's reading only.

use moon_core::market::MarketLimits;
use moon_core::util::fmt;
use rust_i18n::t;

use super::MaxOrderReadout;

/// What the toolbar's `MAX` states, once the per-order cap and the position cap are weighed.
///
/// The per-order classification is kept whole inside every variant rather than flattened into a
/// number, because its own unknowns — not loaded, none stated, out of scope — still decide what
/// the row says whenever the position cap does not take over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MaxCeiling {
    /// No position cap is in play — the core stated none (spot, a venue without one), the limits
    /// have not loaded, or the scope names no account. Exactly the per-order readout as before.
    Order(MaxOrderReadout),
    /// Both caps are known and the per-order maximum is not above the position cap, so it is the
    /// one that stops a larger order.
    OrderBinds {
        /// The per-order readout; always [`MaxOrderReadout::Stated`] or
        /// [`MaxOrderReadout::Derived`] here.
        order: MaxOrderReadout,
        /// The position cap at the applied leverage, in quote currency.
        cap: f64,
    },
    /// The position cap at the applied leverage is the lower ceiling — or the only one.
    CapBinds {
        /// The position cap, in quote currency.
        cap: f64,
        /// The per-order maximum it undercuts, or `None` when the exchange states no per-order cap.
        order: Option<f64>,
    },
}

impl MaxCeiling {
    /// Weigh one market-limits read into the row's ceiling.
    ///
    /// Both figures come from the SAME read, and `moon_core` states both in the market's quote
    /// currency (a coin-margined position cap is converted there), so the comparison is like for
    /// like.
    ///
    /// A per-order cap that exists but cannot be converted yet ([`MaxOrderReadout::NoData`], the
    /// first moments before a price tick) leaves the readout as it was even when a position cap is
    /// known: which of the two binds cannot be told until the other figure arrives, and naming the
    /// position cap the ceiling then could overstate it.
    ///
    /// Args:
    ///     limits: Loaded market limits for the row's (core, market), or `None` before they arrive.
    ///
    /// Returns:
    ///     The row's ceiling, with which cap binds.
    pub fn of(limits: Option<MarketLimits>) -> Self {
        let order = MaxOrderReadout::of(limits);
        let cap = limits
            .and_then(|l| l.position_cap)
            .filter(|c| c.is_finite() && *c > 0.0);
        let Some(cap) = cap else {
            return Self::Order(order);
        };
        match order {
            MaxOrderReadout::NoData | MaxOrderReadout::OutOfScope => Self::Order(order),
            MaxOrderReadout::NoCap => Self::CapBinds { cap, order: None },
            MaxOrderReadout::Stated(v) | MaxOrderReadout::Derived(v) if cap < v => Self::CapBinds {
                cap,
                order: Some(v),
            },
            MaxOrderReadout::Stated(_) | MaxOrderReadout::Derived(_) => {
                Self::OrderBinds { order, cap }
            }
        }
    }

    /// The readout for a scope that names no single account — the Auto workspace Overview.
    ///
    /// Returns:
    ///     The out-of-scope per-order state, which carries no figure.
    pub fn out_of_scope() -> Self {
        Self::Order(MaxOrderReadout::OutOfScope)
    }

    /// The figure the row prints, or `None` when it renders a dash.
    ///
    /// Returns:
    ///     The binding ceiling in quote currency, if one is known.
    pub fn value(self) -> Option<f64> {
        match self {
            Self::Order(order) | Self::OrderBinds { order, .. } => order.value(),
            Self::CapBinds { cap, .. } => Some(cap),
        }
    }

    /// Format the binding figure for the width-constrained row, under the per-order readout's
    /// compact unit rule ([`MaxOrderReadout::format_compact`]).
    ///
    /// Args:
    ///     value_text: Formatter for a present figure.
    ///     quote: Quote token of the market, or empty when the name carried none.
    ///
    /// Returns:
    ///     The compact figure with its unit, or the shared dash.
    pub fn format_compact(self, value_text: impl FnOnce(f64) -> String, quote: &str) -> String {
        // The position cap is printed through the per-order formatter on purpose: one unit rule
        // for the one readout, whichever cap fills it.
        let carrier = match self {
            Self::Order(order) | Self::OrderBinds { order, .. } => order,
            Self::CapBinds { cap, .. } => MaxOrderReadout::Stated(cap),
        };
        carrier.format_compact(value_text, quote)
    }

    /// The hover text: which ceiling binds and, whenever a position cap is in play, that it moves
    /// with the APPLIED leverage only after the exchange accepted it and the core reported back.
    ///
    /// The figures in it are printed in full ([`fmt::usd_grouped`]), as the leverage popup prints
    /// them, because the row itself shows only the compact one.
    ///
    /// Args:
    ///     quote: Quote token of the market, or empty when the name carried none.
    ///
    /// Returns:
    ///     The localized explanation of the readout.
    pub fn tooltip(self, quote: &str) -> String {
        // Through the per-order readout's own unit rule, so the hover cannot label a figure
        // differently from the popup row that prints it.
        let full = |v: f64| MaxOrderReadout::Stated(v).format(fmt::usd_grouped, quote);
        match self {
            Self::Order(order) => t!(order.tooltip_key()).to_string(),
            Self::OrderBinds { order, cap } => format!(
                "{}\n{}",
                t!(
                    "toolbar.max_order_binds",
                    order = order.format(fmt::usd_grouped, quote),
                    cap = full(cap)
                ),
                // The per-order figure's own provenance still applies — a derived one drifts with
                // the price, which the sentence above does not say.
                t!(order.tooltip_key())
            ),
            Self::CapBinds {
                cap,
                order: Some(order),
            } => t!(
                "toolbar.max_cap_binds",
                cap = full(cap),
                order = full(order)
            )
            .to_string(),
            Self::CapBinds { cap, order: None } => {
                t!("toolbar.max_cap_only", cap = full(cap)).to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests;
