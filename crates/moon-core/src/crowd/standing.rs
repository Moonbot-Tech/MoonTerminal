//! One line of the market table.
//!
//! A caller picks which coins to show — the loudest of the minute, or a fixed set it is already
//! keeping on screen — and the line must READ the same either way. So the shape of a line lives
//! here, in the module that owns the figures, and callers differ only in which lines they hand
//! over.
//!
//! A line carries what a trader looks up: the coin, what the crowd won and lost on it inside the
//! rolling minute, and how many trades each of those two figures is made of. Never a net: a coin
//! that made $9k and lost $8k had a loud minute, and one number would report it as a quiet one.

use crate::crowd::minute::CoinMinute;

/// One coin, as the table shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Standing {
    pub coin: String,
    /// Money won inside the window.
    pub plus: f64,
    /// Money lost inside the window, positive.
    pub minus: f64,
    /// Winning trades behind [`Self::plus`].
    pub trades_plus: u32,
    /// Losing trades behind [`Self::minus`].
    pub trades_minus: u32,
}

impl Standing {
    /// A coin that is trading.
    ///
    /// Args:
    ///     coin: Ticker.
    ///     stat: Its minute.
    pub fn live(coin: impl Into<String>, stat: &CoinMinute) -> Self {
        Self {
            coin: coin.into(),
            plus: stat.plus,
            minus: stat.minus,
            trades_plus: stat.trades_plus,
            trades_minus: stat.trades_minus,
        }
    }

    /// How loud this minute was — what ranks the row.
    pub fn money(&self) -> f64 {
        self.plus + self.minus
    }
}

/// Which of two coins had the louder minute: more money first, and the name to break a tie.
///
/// The tie-break is not cosmetic. The rows are built off a `HashMap`, whose order changes between
/// runs, and a table that reshuffled two equal rows every second would repaint for nothing.
///
/// Args:
///     a: One coin's money and ticker.
///     b: The other's.
pub fn louder(a: (f64, &str), b: (f64, &str)) -> std::cmp::Ordering {
    b.0.partial_cmp(&a.0)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a.1.cmp(b.1))
}

#[cfg(test)]
mod tests;
