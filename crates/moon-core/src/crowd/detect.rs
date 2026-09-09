//! When the crowd's minute on one coin is worth announcing.
//!
//! The rule is deliberately about the WINDOW and not about a running total: what it answers is
//! "the crowd is making money on this coin RIGHT NOW", and a figure that only ever grows would
//! answer "the crowd made money on this coin at some point today" instead.
//!
//! **Either line can be switched off with a zero**, and then it is not looked at: `profit = 0`
//! asks about the number of trades whatever the money did, `trades = 0` asks about the money
//! whatever the count was. Both zero asks nothing, and nothing is what it gets — no announcement,
//! and one level up no connection either.
//!
//! **The money line may be negative.** It is read as written — `> -500` is "not worse than five
//! hundred down" — so it widens the rule rather than inverting it. A rule for a coin the crowd is
//! LOSING on would be a different comparison and is not this one.
//!
//! Two decisions carry the whole module:
//!
//! 1. **It fires on the CROSSING, not on the state.** A coin that stays above the line stays above
//!    it for as long as its minute holds, and a rule that fired on "is above" would hand the feed
//!    one card a second for the same coin. So a coin that has fired is held firing until it falls
//!    back, and only a fresh crossing produces another card.
//! 2. **Falling back needs room.** A coin sitting exactly on the line crosses it several times a
//!    minute on nothing but arithmetic, so the way down is [`REARM`] of the way up on every line
//!    that is switched on — the ordinary hysteresis, without which the edge above is an edge in
//!    name only. Whatever the lines are set to, [`REFIRE_MS`] still caps any one coin at a card a
//!    minute.
//!
//! Nothing here knows about a screen, a core or a chart. It is given a window and returns what
//! crossed; who draws that, and what a coin means to a terminal that may not even trade it, is
//! decided by the reader.

use std::collections::HashMap;
use std::collections::VecDeque;

use super::minute::Minute;

/// Net profit a coin's minute has to be worth before it is announced, in dollars.
///
/// Measured, not guessed. Read off the service on 09.09.2026: the whole crowd — every account it
/// counts — turned over 51 426 trades and $342.53 net in twenty-four hours, which is 35.7 trades
/// and a quarter of a dollar a minute across ALL coins together, with the day's best coin (ZEC,
/// +$7 563.93) averaging $5.25 a minute. A hundred dollars of net profit on ONE coin inside one
/// minute is therefore some twenty of that best coin's average minutes arriving at once: loud
/// enough to be worth a card, and reachable often enough that the rule is not silent for weeks.
///
/// The first draft of this was a thousand, which on those figures is four thousand times the
/// crowd's whole average minute — a default that would have looked like a broken feature rather
/// than a quiet market. It is a starting point and not a law: the field beside it exists because a
/// figure that is loud on a quiet market is unremarkable during a pump.
pub const DEFAULT_PROFIT: f64 = 100.0;

/// Trades that minute has to hold as well.
///
/// The money alone is not enough, and this is what stops one lucky position from being reported as
/// a crowd: ten trades is a group of people, one trade of $2000 is a person. Against the 35.7
/// trades a minute the whole crowd makes across every coin, ten of them landing on ONE coin is a
/// real concentration rather than an ordinary minute.
pub const DEFAULT_TRADES: u32 = 10;

/// How far back down a coin has to fall before it can be announced again, as a share of the line
/// it crossed.
///
/// Without it a coin resting on the threshold crosses it repeatedly while nothing about the market
/// changes, and the feed fills with one coin.
const REARM: f64 = 0.8;

/// The shortest gap between two cards for the same coin.
///
/// One window wide, so a coin cannot be announced twice out of the same minute of trades even if
/// it falls back and climbs again inside it.
const REFIRE_MS: u64 = 60_000;

/// How many detections the ring holds.
///
/// It exists so a panel built after the fact can still see what fired; a reader takes what is
/// newer than its cursor and drops whatever has outlived its card anyway.
const RING: usize = 64;

/// How many coins one pass may announce.
///
/// A pass that finds more than this is a threshold set too low, not a market event. The rest are
/// not forgotten — they are still un-fired and still across the line, so they are announced on the
/// following passes, and a genuine market-wide move arrives over a few seconds instead of at once.
///
/// Deliberately [`SEATS`]: announcing more per pass than can ever be on screen would buy a market
/// snapshot for a card that is trimmed in the same breath.
const BURST: usize = SEATS;

/// How many of these a reader is expected to hold at once.
///
/// One number rather than two that must agree — the rule's own burst is this, and the terminal's
/// feed keeps exactly this many seats for these cards. It is a guard against a mistyped line, not a
/// display preference: the lines can be set to fire on every coin that trades, and nothing else in
/// a feed of forty-eight would survive it.
pub const SEATS: usize = 5;

/// What makes a coin's minute worth a card.
///
/// The thresholds are the reader's, so they are carried rather than assumed — a figure that is
/// loud on a quiet market is unremarkable during a pump, and the person watching is the one who
/// knows which they are in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CrowdRule {
    /// Whether the rule is watched at all. While it is off nothing is evaluated and nothing is
    /// remembered — and, one level up, the trade stream is not even opened for it.
    pub enabled: bool,
    /// Net profit the crowd has to be up on the coin, strictly above.
    pub profit: f64,
    /// Trades that minute has to hold, at least.
    pub trades: u32,
}

impl CrowdRule {
    /// Whether this rule is actually watching for something.
    ///
    /// A line set to zero is a line switched OFF, not a line at zero: `profit = 0` asks about the
    /// count whatever the money did. Both of them zero asks nothing at all — and a rule that asks
    /// nothing must not hold a connection open to answer it, which is why the reader consults this
    /// and not [`Self::enabled`] alone.
    pub fn armed(&self) -> bool {
        self.enabled && (self.profit != 0.0 || self.trades != 0)
    }

    /// Whether a minute clears every line that is switched on.
    ///
    /// Args:
    ///     net: The crowd's net money on the coin, inside the window.
    ///     trades: Trades in that window.
    fn clears(&self, net: f64, trades: u32) -> bool {
        (self.profit == 0.0 || net > self.profit) && (self.trades == 0 || trades >= self.trades)
    }

    /// Whether a minute has fallen back far enough, on every line that is switched on, for the next
    /// crossing to be a real one. See [`REARM`].
    ///
    /// Args:
    ///     net: The crowd's net money on the coin, inside the window.
    ///     trades: Trades in that window.
    fn fell_back(&self, net: f64, trades: u32) -> bool {
        (self.profit != 0.0 && net < rearm_profit(self.profit))
            || (self.trades != 0 && trades < rearm_trades(self.trades))
    }
}

impl Default for CrowdRule {
    /// Off, with the thresholds a first-time reader gets when they switch it on.
    fn default() -> Self {
        Self {
            enabled: false,
            profit: DEFAULT_PROFIT,
            trades: DEFAULT_TRADES,
        }
    }
}

/// One announcement: a coin, and what its minute was worth when it crossed.
///
/// The figures are frozen here rather than looked up by whoever draws it, for the same reason a
/// core's detect card freezes its own: the window moves on, and a card has to state what fired it
/// and not what is true a minute later.
#[derive(Clone, Debug, PartialEq)]
pub struct CrowdDetect {
    /// Position in the stream, so a reader can take what it has not seen.
    pub seq: u64,
    /// The coin, as the service spells it.
    pub coin: String,
    /// Net profit of the crowd's minute on it at the crossing.
    pub profit: f64,
    /// Trades in that minute.
    pub trades: u32,
    /// Wall clock of the crossing, Unix milliseconds — a card's age is read against a wall clock,
    /// not against the window's own origin.
    pub at_ms: u64,
}

/// The money line's re-arm, always BELOW the line it belongs to.
///
/// A fifth of the line's MAGNITUDE, moved downwards. Multiplying by [`REARM`] would do for a
/// positive line and quietly invert for a negative one — `-500 * 0.8` is `-400`, which sits ABOVE
/// the line it is meant to sit under, and a coin could then re-arm without falling at all.
///
/// Args:
///     profit: The rule's money line.
fn rearm_profit(profit: f64) -> f64 {
    profit - profit.abs() * (1.0 - REARM)
}

/// How far the trade count has to fall back before a coin can be announced again.
///
/// The same band the money gets, and for the same reason: a coin sitting on the count threshold
/// crosses it several times a minute on nothing but arithmetic. Floored at one, because a band of
/// zero can only be reached by a coin that has left the window entirely — at which point its watch
/// is dropped anyway.
///
/// Args:
///     trades: The rule's line.
fn rearm_trades(trades: u32) -> u32 {
    (f64::from(trades) * REARM).floor().max(1.0) as u32
}

/// Whether a coin fired recently enough that another card would be the same news twice.
///
/// Args:
///     fired_ms: When it last fired, or `None` if it never has.
///     now_ms: Wall clock of this pass.
fn recently(fired_ms: Option<u64>, now_ms: u64) -> bool {
    fired_ms.is_some_and(|at| now_ms.saturating_sub(at) < REFIRE_MS)
}

/// What one coin is doing about the line.
#[derive(Clone, Copy, Debug, Default)]
struct Watch {
    /// Whether it is currently above and has already been announced for it.
    firing: bool,
    /// When it was last announced, for [`REFIRE_MS`]; `None` while it never has been.
    ///
    /// An Option and not a zero: a coin first seen a few seconds into a run would otherwise be
    /// read as having fired at the epoch, and every one of them would be silenced for the first
    /// minute of the terminal's life.
    fired_ms: Option<u64>,
}

/// The rule, what each coin is doing about it, and what has fired.
pub struct Detector {
    rule: CrowdRule,
    watch: HashMap<String, Watch>,
    ring: VecDeque<CrowdDetect>,
    seq: u64,
    /// Whether the next pass only TAKES the state of the market rather than announcing it.
    ///
    /// Set whenever the thresholds change: a reader who lowers the line is asking what crosses it
    /// from now on, and a pass that announced everything already above would empty a whole board
    /// into the feed for a single keystroke.
    reseed: bool,
}

impl Default for Detector {
    fn default() -> Self {
        Self::new()
    }
}

impl Detector {
    pub fn new() -> Self {
        Self {
            rule: CrowdRule::default(),
            watch: HashMap::new(),
            ring: VecDeque::new(),
            seq: 0,
            reseed: false,
        }
    }

    /// The rule as it stands.
    pub fn rule(&self) -> CrowdRule {
        self.rule
    }

    /// Watch this rule from now on.
    ///
    /// A changed THRESHOLD re-seeds rather than firing; see [`Detector::reseed`]. Switching the
    /// rule off forgets every coin, because what it was doing about a line nobody is watching is
    /// not worth carrying.
    ///
    /// Args:
    ///     rule: The new rule.
    ///
    /// Returns:
    ///     Whether anything changed, so a caller can skip the work a no-op would cause.
    pub fn set_rule(&mut self, rule: CrowdRule) -> bool {
        if self.rule == rule {
            return false;
        }
        let lines_moved = self.rule.profit != rule.profit || self.rule.trades != rule.trades;
        self.rule = rule;
        if !rule.armed() {
            self.watch.clear();
            // And what was already announced goes with it: a reader that arrives after the rule was
            // switched off would otherwise be handed cards for a rule nobody is watching.
            self.ring.clear();
            // Including a re-seed that was owed and never spent. It exists to swallow the pass that
            // follows a MOVED LINE; carried across a switch-off it would swallow the first pass
            // after the switch back on instead, and that pass is the one somebody just asked for.
            self.reseed = false;
        } else if lines_moved {
            self.reseed = true;
        }
        true
    }

    /// Look at the window and announce what has just crossed.
    ///
    /// Args:
    ///     minute: The crowd's rolling minute.
    ///     now_ms: Wall clock, Unix milliseconds, stamped on whatever fires.
    ///
    /// Returns:
    ///     How many detections were added.
    pub fn scan(&mut self, minute: &Minute, now_ms: u64) -> usize {
        if !self.rule.armed() {
            return 0;
        }
        let seeding = std::mem::take(&mut self.reseed);
        // What crossed on this pass, gathered before any of it is announced. Gathering first is
        // what lets the loudest be chosen: the window is a hash map, so taking them as they come
        // would make an over-full pass keep an arbitrary handful.
        let mut crossed: Vec<(&str, f64, u32)> = Vec::new();
        for (coin, stat) in minute.coins() {
            let net = stat.net();
            let over = self.rule.clears(net, stat.trades);
            let watch = self.watch.entry(coin.to_string()).or_default();
            if seeding {
                // Take the state without announcing it: everything above the new line is treated
                // as having crossed it before anybody was watching.
                watch.firing = over;
                continue;
            }
            if watch.firing {
                // Back below, with room to spare: the next crossing is a real one.
                if self.rule.fell_back(net, stat.trades) {
                    watch.firing = false;
                }
                continue;
            }
            if !over || recently(watch.fired_ms, now_ms) {
                continue;
            }
            crossed.push((coin, net, stat.trades));
        }
        // Loudest first, ties by name so a pass is reproducible: hash-map order is not.
        crossed.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let fired = crossed.len().min(BURST);
        for (coin, net, trades) in crossed.into_iter().take(BURST) {
            let watch = self.watch.entry(coin.to_string()).or_default();
            watch.firing = true;
            watch.fired_ms = Some(now_ms);
            self.seq += 1;
            self.ring.push_back(CrowdDetect {
                seq: self.seq,
                coin: coin.to_string(),
                profit: net,
                trades,
                at_ms: now_ms,
            });
        }
        while self.ring.len() > RING {
            self.ring.pop_front();
        }
        // A coin that has left the window is forgotten — unless it fired recently, because the gap
        // that keeps it from being announced twice is the one thing about it still worth knowing.
        self.watch
            .retain(|coin, watch| minute.get(coin).is_some() || recently(watch.fired_ms, now_ms));
        fired
    }

    /// Everything announced after `cursor`, oldest first.
    ///
    /// Args:
    ///     cursor: The last sequence this reader has seen; `0` for everything still held.
    pub fn since(&self, cursor: u64) -> impl Iterator<Item = &CrowdDetect> {
        self.ring.iter().filter(move |row| row.seq > cursor)
    }

    /// The newest sequence issued, which is where a reader that does not want the backlog starts.
    pub fn head(&self) -> u64 {
        self.seq
    }
}

#[cfg(test)]
mod tests;
