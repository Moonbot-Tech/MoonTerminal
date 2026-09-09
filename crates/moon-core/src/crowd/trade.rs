//! Where trades come from.
//!
//! This crate does not know about SockJS or STOMP: a host drains an [`EventSource`]
//! and hands the trades over. Three implementations are foreseen — the live crowd feed, a
//! recorded dump replayed for balance work, and the [`SyntheticFeed`] here, which is the only one
//! that must always exist: without it the crate cannot be tested and nothing can run offline.

use crate::crowd::rng::Rng;

/// One closed trade, stripped to what the figures use.
///
/// The wire carries the telegram handle, the user id, prices and the strategy code as well; none
/// of it reaches us. Only the ticker and the money matter, and the identities are nobody's
/// business here.
#[derive(Clone, Debug, PartialEq)]
pub struct Trade {
    /// Arrival time, milliseconds on the host's own clock.
    pub at_ms: u64,
    /// Ticker, base asset only (`"BTC"`), exactly as the feed spells it.
    pub coin: String,
    /// Profit in dollars, signed: positive is counted in the won column, negative in the lost.
    pub profit: f64,
}

impl Trade {
    pub fn new(at_ms: u64, coin: impl Into<String>, profit: f64) -> Self {
        Self {
            at_ms,
            coin: coin.into(),
            profit,
        }
    }
}

/// A stream of closed trades.
pub trait EventSource {
    /// Return everything that has arrived since the last call.
    ///
    /// Args:
    ///     now_ms: The host clock, so a replaying source knows how far to advance.
    fn drain(&mut self, now_ms: u64) -> Vec<Trade>;
}

/// Tickers the synthetic feed trades. Real names, so an offline run looks like the real thing.
const SYNTHETIC_COINS: &[&str] = &[
    "BTC", "ETH", "SOL", "PUMP", "DOGE", "TON", "ARB", "SUI", "PEPE", "AVAX", "LINK", "ADA", "APT",
    "OP", "INJ",
];

/// Deterministic stand-in for the live feed.
///
/// Its job is not to imitate the market in detail but to reproduce the three shapes a reader is
/// built around: quiet stretches where nothing trades at all, bursts where one coin takes over,
/// and coins whose sign flips inside a minute. Seeded, so a balance change can be told apart from
/// a change in the stream.
/// Trades a second the storm aims for, over every coin at once.
///
/// A hundred times the busiest second the live feed has ever shown. It is not a market and is not
/// meant to look like one: it exists to answer "what does the screen do when the wire goes mad",
/// which is a question about the SCREEN and cannot be asked of a stream that behaves.
///
/// What comes out is a little over it — about 330 a second, measured — because the gap between
/// two trades is rolled around this figure and then truncated to whole milliseconds. That is a
/// storm being a storm, and it is named rather than tuned away: a measuring stick may be blunt,
/// but it may not lie about which way it is out.
pub const STORM_PER_SECOND: f32 = 300.0;
/// How many trades one drain may hand over at once.
///
/// A guard, not a rule of the market: a host that stalls for a minute must not make the feed emit a
/// minute of trades in one step. The storm needs its own, or the guard is what would be measured
/// instead of the screen.
const DRAIN_BUDGET: usize = 64;
const STORM_BUDGET: usize = 4_096;
/// Closest two trades can be, in milliseconds.
///
/// A market's gap is rolled around its regime and this keeps a roll of nearly nothing from
/// stacking a hundred trades on one instant. The STORM needs a floor of its own, or the floor is
/// what gets measured instead of the storm: at twenty milliseconds it could not exceed fifty
/// trades a second whatever it was asked for.
const GAP_FLOOR_MS: u64 = 20;
const STORM_FLOOR_MS: u64 = 1;

pub struct SyntheticFeed {
    rng: Rng,
    /// Whether this is the storm rather than a market.
    storm: bool,
    /// Next trade's due time on the host clock.
    next_at_ms: u64,
    /// Coin currently in a burst, and when the burst ends.
    hot: Option<(usize, u64)>,
    /// Whether the current burst is making money or losing it.
    hot_wins: bool,
    /// When the current calm/busy regime ends.
    regime_until_ms: u64,
    /// Mean seconds between trades in the current regime.
    regime_gap_s: f32,
}

impl SyntheticFeed {
    /// Start a feed at the host clock's zero.
    ///
    /// Args:
    ///     seed: Any value; the same seed replays the same market.
    pub fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed),
            storm: false,
            next_at_ms: 0,
            hot: None,
            hot_wins: true,
            regime_until_ms: 0,
            regime_gap_s: 1.2,
        }
    }

    /// The same feed, opened as a STORM: [`STORM_PER_SECOND`] trades a second, without let-up.
    ///
    /// For measuring, not for playing. What it is for is the one question a table screen has to
    /// be able to answer out loud — whether the repaint rate follows the TRADE rate, which is how
    /// a screen wedges itself solid on a busy night.
    pub fn storm(seed: u64) -> Self {
        Self {
            storm: true,
            ..Self::new(seed)
        }
    }

    /// Pick the next regime: a busy stretch, an ordinary one, or near-silence.
    fn roll_regime(&mut self, now_ms: u64) {
        if self.storm {
            // One regime, for ever: the storm has no quiet stretches to fall into.
            self.regime_gap_s = 1.0 / STORM_PER_SECOND;
            self.regime_until_ms = now_ms + 3_600_000;
            return;
        }
        let roll = self.rng.next_f32();
        // Modelled on the measured feed: 0.24-1.45 trades per second overall, with long quiet
        // stretches at night. Silence is a legitimate state of the market, not a failure.
        self.regime_gap_s = if roll < 0.25 {
            self.rng.range(3.0, 9.0)
        } else if roll < 0.8 {
            self.rng.range(0.7, 2.0)
        } else {
            self.rng.range(0.25, 0.6)
        };
        let span = self.rng.range(12.0, 40.0);
        self.regime_until_ms = now_ms + (span * 1000.0) as u64;
    }

    /// Start or end a burst on one coin.
    fn roll_burst(&mut self, now_ms: u64) {
        if let Some((_, until)) = self.hot {
            if now_ms < until {
                return;
            }
        }
        if self.rng.next_f32() < 0.45 {
            let coin = (self.rng.next_u64() as usize) % SYNTHETIC_COINS.len();
            let span = self.rng.range(8.0, 30.0);
            self.hot = Some((coin, now_ms + (span * 1000.0) as u64));
            self.hot_wins = self.rng.next_f32() < 0.55;
        } else {
            self.hot = None;
        }
    }
}

impl EventSource for SyntheticFeed {
    fn drain(&mut self, now_ms: u64) -> Vec<Trade> {
        let mut out = Vec::new();
        if self.next_at_ms == 0 {
            self.next_at_ms = now_ms;
            self.roll_regime(now_ms);
        }
        let mut budget = if self.storm {
            STORM_BUDGET
        } else {
            DRAIN_BUDGET
        };
        while self.next_at_ms <= now_ms && budget > 0 {
            budget -= 1;
            let at = self.next_at_ms;
            if at >= self.regime_until_ms {
                self.roll_regime(at);
            }
            self.roll_burst(at);

            let (index, win_bias) = match self.hot {
                // A burst does not own the feed: two thirds of its trades are the hot coin's.
                Some((coin, _)) if self.rng.next_f32() < 0.66 => {
                    (coin, if self.hot_wins { 0.78 } else { 0.22 })
                }
                _ => ((self.rng.next_u64() as usize) % SYNTHETIC_COINS.len(), 0.52),
            };
            let winning = self.rng.next_f32() < win_bias;
            // Long tail: most trades are small, a few carry the minute. Squaring a uniform gives
            // that shape without a distribution crate.
            let roll = self.rng.next_f32();
            let magnitude = 0.4 + 90.0 * roll * roll * roll;
            let profit = if winning { magnitude } else { -magnitude };
            out.push(Trade::new(at, SYNTHETIC_COINS[index], f64::from(profit)));

            let gap = self.rng.range(0.35, 1.75) * self.regime_gap_s;
            let floor = if self.storm {
                STORM_FLOOR_MS
            } else {
                GAP_FLOOR_MS
            };
            self.next_at_ms = at + ((gap * 1000.0) as u64).max(floor);
        }
        out
    }
}

#[cfg(test)]
mod tests;
