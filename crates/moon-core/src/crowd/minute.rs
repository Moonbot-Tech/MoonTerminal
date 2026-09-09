//! The rolling minute: what each coin won, lost and how busy it is.
//!
//! Two decisions shape it:
//!
//! 1. **Won and lost are summed apart, both kept positive.** A coin that made $9k and lost $8k had
//!    a loud minute, not a quiet one, and the table draws exactly these two figures.
//! 2. **Arrivals are counted, never inferred from the window.** The window drops trades as they
//!    age out; an event keyed on it would fire again for a coin that has not traded in a minute.
//!
//! The figures are the WINDOW's, live. An earlier version held each coin's loudest moment so a row
//! could not shrink from the mere passing of time — which is right for something that grows and
//! shrinks on screen, and wrong for a column headed MINUTE: a coin that trades every few seconds
//! never leaves the window, so its peak was really its loudest moment since the screen opened, and
//! the table said a coin had made fifty thousand in the last minute long after that minute had
//! gone.
//!
//! A coin is in the window or it is not; there is no half-life in between. While its minute holds
//! trades it is worth everything that minute is worth, and when the last of them ages out it goes
//! entirely.

use std::collections::HashMap;
use std::collections::VecDeque;

use crate::crowd::standing::{self, Standing};
use crate::crowd::trade::Trade;

/// Width of the window. One minute, as the crowd site itself aggregates.
pub const WINDOW_MS: u64 = 60_000;

/// One coin's standing in the rolling minute.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoinMinute {
    /// Money won inside the window.
    pub plus: f64,
    /// Money lost inside the window, positive.
    pub minus: f64,
    /// `plus + minus` — the size of the minute, and what ranks a coin for a place.
    pub money: f64,
    /// Trades currently inside the window.
    pub trades: u32,
    /// Winning trades behind [`Self::plus`].
    pub trades_plus: u32,
    /// Losing trades behind [`Self::minus`].
    pub trades_minus: u32,
}

impl CoinMinute {
    /// Live net of the window, positive when the crowd is making money on this coin.
    pub fn net(&self) -> f64 {
        self.plus - self.minus
    }
}

/// The rolling minute over every coin the feed has mentioned lately.
#[derive(Debug, Default)]
pub struct Minute {
    buf: VecDeque<(u64, String, f64)>,
    stats: HashMap<String, CoinMinute>,
    /// Whether the buffer has changed since the sums were last derived from it.
    ///
    /// The sums are a function of the buffer and of nothing else, so re-deriving them on a step
    /// where no trade arrived and none aged out is work with a guaranteed answer — and a quiet
    /// minute is most of them.
    dirty: bool,
}

impl Minute {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take one trade off the feed.
    ///
    /// A non-finite profit is dropped at the door rather than propagated: one NaN off the wire
    /// would poison a sum, and every figure derived from that sum after it.
    pub fn push(&mut self, trade: Trade) {
        if !trade.profit.is_finite() || trade.coin.is_empty() {
            return;
        }
        self.dirty = true;
        self.stats.entry(trade.coin.clone()).or_default();
        self.buf.push_back((trade.at_ms, trade.coin, trade.profit));
    }

    /// Age the window out and re-derive the sums.
    ///
    /// Args:
    ///     now_ms: Host clock.
    pub fn tick(&mut self, now_ms: u64) {
        let cut = now_ms.saturating_sub(WINDOW_MS);
        while self.buf.front().is_some_and(|(at, _, _)| *at < cut) {
            self.buf.pop_front();
            self.dirty = true;
        }

        if !self.dirty {
            return;
        }
        self.dirty = false;

        // (won, lost, winning trades, losing trades)
        let mut live: HashMap<&str, (f64, f64, u32, u32)> = HashMap::new();
        for (_, coin, profit) in &self.buf {
            let row = live.entry(coin.as_str()).or_insert((0.0, 0.0, 0, 0));
            if *profit >= 0.0 {
                row.0 += *profit;
                row.2 += 1;
            } else {
                row.1 += -*profit;
                row.3 += 1;
            }
        }

        // A coin with no trade left in the window is gone entirely: it is not a quiet row, it is
        // not a row.
        self.stats
            .retain(|coin, _| live.contains_key(coin.as_str()));

        for (coin, (plus, minus, won, lost)) in live {
            let stat = self.stats.entry(coin.to_string()).or_default();
            stat.money = plus + minus;
            stat.plus = plus;
            stat.minus = minus;
            stat.trades_plus = won;
            stat.trades_minus = lost;
            stat.trades = won + lost;
        }
    }

    /// One coin's standing, if it is still in the window.
    pub fn get(&self, coin: &str) -> Option<&CoinMinute> {
        self.stats.get(coin)
    }

    /// Coins ordered by the size of their minute, loudest first.
    ///
    /// Ranked by `money` rather than by net on purpose: a coin earns its place by being LOUD,
    /// and the sign only decides how that noise is split between the two columns.
    pub fn ranked(&self) -> Vec<(&str, f64)> {
        let mut rows: Vec<(&str, f64)> = self
            .stats
            .iter()
            .map(|(coin, stat)| (coin.as_str(), stat.money))
            .collect();
        // Ties break by name so a run is reproducible: HashMap order is not.
        rows.sort_by(|a, b| standing::louder((a.1, a.0), (b.1, b.0)));
        rows
    }

    /// The loudest coins of the minute, as table rows.
    ///
    /// This is the whole of what the statistics screen asks of this module: the minute itself,
    /// ordered the way it is read.
    ///
    /// Args:
    ///     limit: How many rows at most. The screen shows a table, not a census.
    pub fn standings(&self, limit: usize) -> Vec<Standing> {
        // Built on [`Self::ranked`] rather than sorted again: everything that reads this window
        // has to agree on which coins are the loud ones, and two sorts of the same figures are two
        // chances to disagree.
        self.ranked()
            .into_iter()
            .take(limit)
            .filter_map(|(coin, _)| self.stats.get(coin).map(|stat| Standing::live(coin, stat)))
            .collect()
    }

    /// Trades currently held, for diagnostics and tests.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

#[cfg(test)]
mod tests;
