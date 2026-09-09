//! One reader of the crowd's statistics for the whole terminal.
//!
//! The screens that draw these figures come and go, and there can be several of them at once; the
//! rule that watches for a loud coin has to keep counting whether or not anything is drawn. So the
//! wire is owned HERE, once, and everybody else reads what it has already counted.
//!
//! That is not a matter of taste. The trade feed arrives on an `mpsc` channel, which has exactly
//! one receiver: two screens draining it would each get half the trades and both would be wrong
//! about the market. One owner is the only shape that is correct.
//!
//! **Nothing here goes near `Backend::notify`.** The backend is observed by seventeen views, so a
//! tick routed through it would repaint the whole terminal once a second — the notify-everything
//! hammer, in the one place that ticks forever. Instead:
//!
//! * this entity notifies ITS OWN observers, which is the statistics screen and nothing else, and
//!   only when the minute actually changed, a board was replaced, or the wire found a new word for
//!   itself. A quiet market wakes nobody;
//! * a detection wakes [`CrowdDetectRevision`], which the Detects panel observes and nothing else.
//!
//! **What is read is the union of what is wanted.** Every reader holds a [`CrowdLease`] naming the
//! halves it needs, and the rule adds the trade stream while it is switched on. The union decides
//! which threads the feed runs; when it empties, the feed closes, the tick stops and the window is
//! thrown away. A lease is a plain `Rc`, so a screen that is simply dropped — a window closed, the
//! last table switched off — releases its claim with no teardown call to forget.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui::{AppContext as _, Context, Entity};
use moon_core::crowd::board::{CoinDay, DaySummary, Trader};
use moon_core::crowd::{
    CrowdDetect, CrowdRule, Detector, EventSource, Feed, FeedConfig, Minute, Wants, Wire,
};

use crate::diag;

/// How often the window is aged and the feed drained.
///
/// A minute-wide window is not read any better at thirty samples a second, and a second is the
/// shortest interval at which a figure in it can visibly change.
const TICK: Duration = Duration::from_secs(1);

/// A gap between ticks longer than this means the terminal was frozen — dragged, blocked or asleep
/// — and whatever the wire buffered meanwhile is no longer a minute of anything.
const STALE_AFTER: f32 = 10.0;

/// How many more ticks the service keeps draining after nothing is wanted any more.
///
/// A thread that has been told to stop notices between requests, and for the long poll that can be
/// most of a minute; everything it says in the meantime lands in a channel nobody is reading. So
/// the tick outlives the demand by this much and does nothing but empty it — visible as
/// `crowd_tick` continuing for a minute and a half after the last switch went off, with
/// `crowd_trades` at zero.
const DRAIN_AFTER: u32 = 90;

/// Nothing wanted: the feed runs no thread and holds no socket.
const NOTHING: Wants = Wants {
    trades: false,
    coins: false,
    traders: false,
};

/// Notification-only entity woken when the crowd's rule announces a coin.
///
/// Its own channel rather than the service's, because the two have different audiences and very
/// different rates: the statistics screen wants to hear about every changed figure, and the Detects
/// panel wants to hear about a crossing — which on an ordinary market is a few times an hour.
pub(crate) struct CrowdDetectRevision;

/// One reader's standing claim on the feed.
///
/// Held by whoever is reading; dropping it releases the claim, and the service collects the dead
/// ones on its next tick. There is nothing to close and nothing to remember to call.
#[derive(Clone)]
pub(crate) struct CrowdLease(Rc<Cell<Wants>>);

/// The wire, the window, the rule, and who is asking for them.
pub(crate) struct CrowdService {
    feed: Feed,
    minute: Minute,
    detector: Detector,
    /// The channel a detection wakes. See [`CrowdDetectRevision`].
    detects: Entity<CrowdDetectRevision>,
    /// Every live reader's claim. Dead ones are dropped on each tick.
    leases: Vec<Weak<Cell<Wants>>>,
    /// What the feed is currently reading, so an unchanged union costs nothing.
    open: Wants,
    /// What arrived on THIS tick, by coin, with the net money it brought.
    ///
    /// Kept for the readers rather than passed to them: the screen lights up a row that traded, and
    /// nothing on the row itself can answer that — a coin taking a $10 win every second while a $10
    /// win ages out shows the same figures minute after minute, and that is exactly the coin worth
    /// lighting up.
    landed: HashMap<String, f64>,
    /// The wire's word and the board counters as they last stood, so a tick that changed neither
    /// wakes nobody.
    wire: Wire,
    boards_seen: (u64, u64),
    /// Start of the clock the window is measured on.
    started: Instant,
    last_tick: Instant,
    /// Whether a tick chain is running. One at a time; the chain ends itself when nothing is
    /// wanted, and [`Self::arm`] starts a new one when something is again.
    ticking: bool,
    /// Ticks left of the drain that follows a demand going away. See [`DRAIN_AFTER`].
    drain_for: u32,
}

impl CrowdService {
    /// Open the service with nothing wanted yet.
    ///
    /// Args:
    ///     live: Whether a real connection is allowed. A bench run (`--fixture`) and FireTest both
    ///         assert that nothing reaches the network, so they get the seeded stand-in instead.
    ///     rule: The saved rule, so a profile that opted in is watching from the first second
    ///         rather than from whenever a window first drew its empty screen.
    ///     cx: The service's context.
    pub(crate) fn new(live: bool, rule: CrowdRule, cx: &mut Context<Self>) -> Self {
        let now = Instant::now();
        let feed = Feed::open(live.then(FeedConfig::default), seed(), NOTHING);
        let mut detector = Detector::new();
        detector.set_rule(rule);
        let mut service = Self {
            wire: feed.wire(),
            feed,
            minute: Minute::new(),
            detector,
            detects: cx.new(|_| CrowdDetectRevision),
            leases: Vec::new(),
            open: NOTHING,
            landed: HashMap::new(),
            boards_seen: (0, 0),
            started: now,
            last_tick: now,
            ticking: false,
            drain_for: 0,
        };
        service.resync(cx);
        service
    }

    /// Claim these halves of the service until the returned lease is dropped.
    ///
    /// Args:
    ///     wants: What this reader needs.
    ///     cx: The service's context.
    pub(crate) fn lease(&mut self, wants: Wants, cx: &mut Context<Self>) -> CrowdLease {
        let lease = CrowdLease(Rc::new(Cell::new(wants)));
        self.leases.push(Rc::downgrade(&lease.0));
        self.resync(cx);
        lease
    }

    /// Change what one reader is claiming.
    ///
    /// Args:
    ///     lease: That reader's lease.
    ///     wants: What it needs from now on.
    ///     cx: The service's context.
    pub(crate) fn relet(&mut self, lease: &CrowdLease, wants: Wants, cx: &mut Context<Self>) {
        if lease.0.get() == wants {
            return;
        }
        lease.0.set(wants);
        self.resync(cx);
    }

    /// Watch this rule from now on.
    ///
    /// Args:
    ///     rule: The rule as the settings now state it.
    ///     cx: The service's context.
    pub(crate) fn set_rule(&mut self, rule: CrowdRule, cx: &mut Context<Self>) {
        if !self.detector.set_rule(rule) {
            return;
        }
        // Switching it on adds the trade stream to the union; switching it off may close it.
        self.resync(cx);
    }

    /// The channel a detection wakes.
    pub(crate) fn detect_revision(&self) -> Entity<CrowdDetectRevision> {
        self.detects.clone()
    }

    /// Everything announced after `cursor`, oldest first.
    pub(crate) fn detects_since(&self, cursor: u64) -> impl Iterator<Item = &CrowdDetect> {
        self.detector.since(cursor)
    }

    /// The newest detection sequence issued.
    pub(crate) fn detects_head(&self) -> u64 {
        self.detector.head()
    }

    /// The crowd's rolling minute, as it stands.
    pub(crate) fn minute(&self) -> &Minute {
        &self.minute
    }

    /// What arrived on the current tick, by coin.
    pub(crate) fn landed(&self) -> &HashMap<String, f64> {
        &self.landed
    }

    /// What can honestly be said about the trade stream.
    pub(crate) fn wire(&self) -> Wire {
        self.wire
    }

    /// The service's coin board for the day.
    pub(crate) fn coins(&self) -> &[CoinDay] {
        self.feed.coins()
    }

    /// Its trader board.
    pub(crate) fn traders(&self) -> &[Trader] {
        self.feed.traders()
    }

    /// The day's own totals, or `None` before the first answer.
    pub(crate) fn summary(&self) -> Option<DaySummary> {
        self.feed.summary()
    }

    /// How many times each board has been replaced.
    pub(crate) fn boards_seen(&self) -> (u64, u64) {
        self.boards_seen
    }

    /// Bring the feed into line with what is wanted, and keep the tick running while anything is.
    fn resync(&mut self, cx: &mut Context<Self>) {
        let union = self.demand();
        if union != self.open {
            self.feed.want(union);
            // Read back at once: a screen that has just switched the minute on would otherwise be
            // handed last tick's word about a stream that has only now been asked for, and say "no
            // connection" about a wire nobody had given a chance to answer.
            self.wire = self.feed.wire();
            if !wanted(union) {
                // Nothing is being read any more. What is left in the window is a minute of
                // whenever this was last on, and showing it as "the last minute" when the screen
                // comes back would be a lie with a timestamp on it.
                self.minute = Minute::new();
                self.landed.clear();
                self.drain_for = DRAIN_AFTER;
            } else {
                // Demand came back — during the grace, perhaps. The countdown is spent: leaving it
                // standing would let a stale number answer for whether the chain may stop.
                self.drain_for = 0;
            }
            self.open = union;
        }
        self.arm(cx);
    }

    /// The union of every live claim, plus what the rule needs.
    ///
    /// Collecting the dead leases here is the whole of the teardown: a closed window, or a screen
    /// whose last table was switched off, drops its `Rc` and stops counting towards the union.
    fn demand(&mut self) -> Wants {
        let mut live = Vec::with_capacity(self.leases.len());
        self.leases.retain(|lease| match lease.upgrade() {
            Some(claim) => {
                live.push(claim.get());
                true
            }
            None => false,
        });
        // `armed`, not `enabled`: a rule whose two lines are both switched off asks nothing, and a
        // question nobody asked must not hold a socket open to be answered.
        union(self.detector.rule().armed(), live.into_iter())
    }

    /// Start the tick chain if something is wanted and one is not already running.
    fn arm(&mut self, cx: &mut Context<Self>) {
        if self.ticking || (!wanted(self.open) && self.drain_for == 0) {
            return;
        }
        self.ticking = true;
        // A chain that has been stopped for a while would otherwise measure its first interval from
        // whenever it last ran and throw away the trades it opened with.
        self.last_tick = Instant::now();
        crate::pulse::arm_every(TICK, cx, |this: &mut Self, cx| this.tick(cx));
    }

    /// Drain whatever arrived, age the window, look at the rule, and wake only who has to be woken.
    ///
    /// Returns:
    ///     Whether the chain carries on — that is, whether anything is still being read.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        diag::bump(&diag::CROWD_TICK);
        let now = Instant::now();
        // The real interval, uncapped: a machine that slept for an hour ages an hour out of the
        // window.
        let gap = now.duration_since(self.last_tick).as_secs_f32();
        self.last_tick = now;
        let now_ms = now.duration_since(self.started).as_millis() as u64;

        // Arrivals are stamped with the moment they were DRAINED, so a backlog that piled up while
        // the terminal was frozen would enter the window as one simultaneous burst — a minute of
        // trades reported as one second of them. After a gap this long the honest window is an
        // empty one, and that means EMPTYING it: dropping only the backlog would leave the trades
        // from before the freeze standing beside a hole where the freeze was, and both the table
        // and the rule would read that as a full minute.
        let trades = self.feed.drain(now_ms);
        if !wanted(self.open) {
            // Nothing is being read: this tick exists only to empty the channel the stopping
            // threads are still writing into, and what they say belongs to a screen that has gone.
            self.drain_for = self.drain_for.saturating_sub(1);
            self.ticking = self.drain_for > 0;
            return self.ticking;
        }
        // Counted AFTER the staleness gate below would be a different number; counted here it is
        // "what came off the wire", which is what tells a quiet market from a dead socket. The two
        // differ only on the tick that follows a freeze.
        diag::bump_by(&diag::CROWD_TRADES, trades.len() as u64);
        self.landed.clear();
        let stale = gap >= STALE_AFTER;
        if stale {
            self.minute = Minute::new();
        } else {
            for trade in trades {
                *self.landed.entry(trade.coin.clone()).or_insert(0.0) += trade.profit;
                self.minute.push(trade);
            }
        }
        // A window that was just thrown away has changed by definition, and an emptied one reports
        // nothing of its own: without this the screen would keep drawing the rows it had before the
        // freeze until the next trade happened to arrive.
        let moved = self.minute.tick(now_ms) | stale;

        let seen = self.feed.boards_seen();
        let wire = self.feed.wire();
        let changed = moved || seen != self.boards_seen || wire != self.wire;
        self.boards_seen = seen;
        self.wire = wire;

        // The rule is asked only about a window that MOVED. It fires on a crossing, and a window
        // holding the same trades as it did a second ago cannot have crossed anything.
        if moved {
            let fired = self.detector.scan(&self.minute, wall_ms());
            if fired > 0 {
                diag::bump_by(&diag::CROWD_DETECT, fired as u64);
                self.detects.update(cx, |_, detects_cx| detects_cx.notify());
            }
        }
        // The only broadcast in this module, and it reaches the statistics screens and nothing
        // else.
        if changed {
            cx.notify();
        }

        self.resync(cx);
        let carry_on = wanted(self.open) || self.drain_for > 0;
        self.ticking = carry_on;
        carry_on
    }
}

/// What has to be read for these claims and this rule.
///
/// Split out of [`CrowdService::demand`] so it can be exercised directly: it is the whole decision
/// about which connections exist, and a test that re-implemented it would prove only itself.
///
/// Args:
///     rule_on: Whether the rule is being watched, which needs the trade stream and nothing else.
///     claims: What each live reader is asking for.
fn union(rule_on: bool, claims: impl Iterator<Item = Wants>) -> Wants {
    let mut all = Wants {
        trades: rule_on,
        coins: false,
        traders: false,
    };
    for want in claims {
        all.trades |= want.trades;
        all.coins |= want.coins;
        all.traders |= want.traders;
    }
    all
}

/// Whether a claim asks for anything at all.
fn wanted(want: Wants) -> bool {
    want.trades || want.boards()
}

/// Wall clock in Unix milliseconds.
///
/// The window runs on a monotonic clock of its own, but a detection outlives the window and is
/// drawn on a card whose age is counted against the wall — so the crossing is stamped with the
/// same clock the card is read by.
fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

/// A seed for the offline stand-in. Nothing depends on it being unpredictable; it exists so two
/// bench runs are not bit-identical in what the tables happen to show.
fn seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0x5EED)
}

#[cfg(test)]
mod tests;
