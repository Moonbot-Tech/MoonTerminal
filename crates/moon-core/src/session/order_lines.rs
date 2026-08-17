//! Per-core retained store of chart order lines. moonproto keeps terminal orders only until
//! deferred cleanup, so we retain cancelled and filled orders ourselves for the session, up to
//! a memory safety cap, and render them semi-transparently.
//!
//! Each line is a staircase of `(t_ms, price)` steps: from `t_ms`, the price remains at `price`
//! until the next step. Repricing appends a step (knot). The line begins at order creation and
//! ends when the order closes, or at the live right edge — except the ENTRY line, which ends at
//! the fill once the wire dates one (`RetainedOrder::entry_fill_ms`), because the entry leg stops
//! existing there while the order itself lives on. The chart uses this as the canonical source of
//! starts, knots, and ends for markers and segments.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::feed::{OrderRow, OrderTrace};
use crate::util::now_unix_ms;

/// Number of traced line kinds, each with its own start, knots, and end.
///
/// Liquidation is stored separately as `RetainedOrder::liq`, a continuous line without markers.
pub const TRACED_KINDS: usize = 7;

/// Line-kind indices in `RetainedOrder::lines`, matching the style order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineKind {
    Buy = 0,
    Sell = 1,
    Stop = 2,
    Trailing = 3,
    TakeProfit = 4,
    VStop = 5,
    PendingCond = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderCloseReason {
    Cancel,
    Filled,
    BackstopMissing,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrderLineState {
    pub uid: u64,
    pub closed_reason: Option<OrderCloseReason>,
    pub closed_store_ms: Option<f64>,
    pub closed_rev: Option<u64>,
    pub active: bool,
}

/// Per-core capacity of the closed-order ring, equal to the `max_closed_orders` slider maximum.
///
/// New entries are pushed to the back and the oldest fall from the front, without sorting or a
/// pruning scan. Open orders are not capped and remain while active.
const CLOSED_RING_CAP: usize = 5000;

/// Grace period in milliseconds before an order missing from a snapshot is marked closed.
///
/// An order snapshot can briefly be empty or partial during reconnects or subscription churn.
/// Without this grace period, lines would flicker between active and closed states. An order is
/// closed only after it has remained unseen for longer than this interval.
const CLOSE_GRACE_MS: f64 = 2500.0;

/// Relative price-change threshold that prevents float jitter from creating false knots.
fn price_eps(p: f32) -> f32 {
    p.abs() * 1e-5 + 1e-9
}

/// Constrains a wire-supplied line start to the order's own lifetime, or reports it as absent.
///
/// A line cannot begin before the order that owns it, nor in the future: the core clock may lead
/// the local one, and a start past the right edge degenerates the segment. `0.0` means the wire
/// carries no usable time, which [`LineTrace::update`] turns back into "start here and now" — the
/// behaviour every line had before these times were read.
///
/// The two bounds can invert only if the local clock steps BACK behind the order's own creation,
/// and the lower one wins there: a line drawn before the order it belongs to would be a plain lie,
/// while one drawn past a rewound right edge is a cosmetic artifact of the clock jump.
///
/// Args:
///     wire_ms: Leg time from the core in Unix milliseconds; zero, negative, NaN, or infinite
///         means absent.
///     create_ms: Order creation, already clamped, as the earliest admissible start.
///     now_ms: Local wall clock as the latest admissible start.
///
/// Returns:
///     The clamped start, or `0.0` when `wire_ms` carries nothing usable.
fn wire_line_start(wire_ms: f64, create_ms: f64, now_ms: f64) -> f64 {
    if !wire_ms.is_finite() || wire_ms <= 1.0 {
        return 0.0;
    }
    // Written as max-then-min rather than `clamp`, which panics when the local clock steps back
    // far enough to put `now_ms` before `create_ms`.
    let start = wire_ms.max(create_ms);
    if start > now_ms {
        now_ms.max(create_ms)
    } else {
        start
    }
}

/// One order line represented as a staircase of price steps.
#[derive(Clone, Default)]
pub struct LineTrace {
    /// Steps `(t_ms, price)`, where `price` applies from `t_ms` until the next step.
    pub steps: Vec<(f64, f32)>,
    /// Exact server trace for a buy or sell line.
    ///
    /// This is a separate history of line movement; the primary working order line is still drawn
    /// straight at its current price. `steps` remains the fallback for old or incomplete snapshots.
    pub server_points: Vec<(f64, f32)>,
    /// Live temporary server-trace point, drawn dotted from the last point.
    pub tmp_point: Option<(f64, f32)>,
    /// Stop line delivered with the server trace, as in Moonbot `SetStopPrice`.
    pub server_stop_price: Option<f32>,
    pub server_stop_time_ms: Option<f64>,
    /// Time when the line was disabled because its price became unavailable while the order lived.
    pub off_ms: Option<f64>,
}

impl LineTrace {
    /// Updates the staircase with a new price.
    ///
    /// `start_ms` is the first step time: order creation for the entry line and fill time for stop
    /// lines. Returns `true` when a step is added, the line is disabled, or a disabled line is
    /// re-enabled, including at the same price, so the store can bump its revision.
    fn update(&mut self, price: Option<f32>, start_ms: f64, now_ms: f64) -> bool {
        match price {
            Some(p) if p.is_finite() && p > 0.0 => {
                let was_off = self.off_ms.take().is_some();
                match self.steps.last().copied() {
                    None => {
                        let t0 = if start_ms > 1.0 { start_ms } else { now_ms };
                        self.steps.push((t0, p));
                        true
                    }
                    Some((_, last_p)) => {
                        if (last_p - p).abs() > price_eps(p) {
                            self.steps.push((now_ms, p));
                            true
                        } else {
                            was_off
                        }
                    }
                }
            }
            _ => {
                if !self.steps.is_empty() && self.off_ms.is_none() {
                    self.off_ms = Some(now_ms);
                    true
                } else {
                    false
                }
            }
        }
    }

    fn update_server(&mut self, trace: Option<&OrderTrace>) -> bool {
        let Some(trace) = trace else {
            let changed = !self.server_points.is_empty()
                || self.tmp_point.is_some()
                || self.server_stop_price.is_some()
                || self.server_stop_time_ms.is_some();
            if changed {
                self.server_points.clear();
                self.tmp_point = None;
                self.server_stop_price = None;
                self.server_stop_time_ms = None;
            }
            return changed;
        };
        let points: Vec<(f64, f32)> = trace.points.iter().map(|p| (p.time_ms, p.price)).collect();
        let tmp = trace.tmp_point.map(|p| (p.time_ms, p.price));
        let stop_price = trace.stop_price;
        let stop_time = trace.stop_time_ms;
        let changed = self.server_points != points
            || self.tmp_point != tmp
            || self.server_stop_price != stop_price
            || self.server_stop_time_ms != stop_time;
        if changed {
            self.server_points = points;
            self.tmp_point = tmp;
            self.server_stop_price = stop_price;
            self.server_stop_time_ms = stop_time;
        }
        changed
    }

    /// Returns the current line price, or `None` when the line is disabled via `off_ms`.
    ///
    /// A disabled stop must not receive a label or hit testing; otherwise its `-X% [N]` label
    /// remains as an artifact beside the order book until the order closes. Step history remains in
    /// `steps`, and the chart renders it for completed buy and sell lines.
    pub fn current_price(&self) -> Option<f32> {
        if self.off_ms.is_some() {
            return None;
        }
        self.steps.last().map(|(_, p)| *p)
    }
}

/// One retained order with its line traces.
pub struct RetainedOrder {
    /// Order UID used as the store key and for line hit testing and dragging.
    #[allow(dead_code)]
    pub uid: u64,
    pub market: String,
    pub is_short: bool,
    /// Whether the order is EMULATED rather than live, mirrored from the wire row.
    ///
    /// Nothing on the chart reads it any more: the real/emulator setting used to hide order LINES and
    /// now selects closed TRADES instead, where the flag is a predicate on the report replica rather
    /// than a field on an order. Kept because it mirrors the wire faithfully, like `is_short` and
    /// `pending` beside it, and a consumer asking "was this order emulated" has nowhere else to ask.
    pub emulator: bool,
    /// Entry-leg size in base currency, used by the buy-line size label.
    pub size: f32,
    /// Remaining exit-leg size in base currency, used by the sell-line label.
    pub remaining_size: f32,
    /// Entry-leg fill percentage in `0..=100`, used for the bought-quantity label.
    pub fill_pct: f32,
    /// Smallest available positive per-market chart slot, assigned when the order first appears.
    ///
    /// Assignment precedes the closure pass, so a fresh terminal row can reserve a slot for its
    /// first batch. The slot becomes reusable by later assignments after the order closes. Sell and
    /// stop lines share the entry line's slot because they belong to the same order. Zero means
    /// unassigned; labels use `[N]` to associate that order's lines.
    pub chart_num: u32,
    pub pending: bool,
    pub panic_sell: bool,
    pub is_moon_shot: bool,
    pub corridor_price_down: f32,
    pub corridor_price_up: f32,
    /// Creation time and line start in Unix milliseconds.
    pub create_ms: f64,
    /// Wire-dated moment a filled entry leg closed, in Unix milliseconds. `None` when the entry
    /// did not fill or the wire gives no usable date; the chart then draws the line as before.
    pub entry_fill_ms: Option<f64>,
    /// Logical line-end time; backstop closure uses the last-seen time rather than detection time.
    /// `None` means the order is active.
    pub closed_ms: Option<f64>,
    pub closed_reason: Option<OrderCloseReason>,
    /// Local time when the store recorded the closure, including after the backstop grace period.
    pub closed_store_ms: Option<f64>,
    pub closed_rev: Option<u64>,
    /// Last time the order appeared in a snapshot, used by the closure grace period.
    last_seen_ms: f64,
    /// Update generation in which this order last appeared.
    ///
    /// Comparing this marker while iterating `OrderLineStore::open_uids` replaces a per-update
    /// `HashSet` lookup for every retained closed order.
    seen_generation: u64,
    /// Order appearance sequence.
    pub seq: u64,
    /// Traces by line kind, indexed by `LineKind as usize`.
    pub lines: [LineTrace; TRACED_KINDS],
    /// Current liquidation price, rendered as a continuous line without markers.
    pub liq: Option<f32>,
}

impl RetainedOrder {
    /// Earliest time anything belonging to this order is drawn at.
    ///
    /// Usually the creation, but a line may legitimately begin before it: an order the core dates
    /// not at all falls back to the local clock for `create_ms`, while its exit still carries a
    /// real wire time from before that. Time-window culling has to consult this, or such an order
    /// disappears from the very window its line occupies. Kept off the per-frame path — the
    /// renderer only reaches for it when `create_ms` alone would cull the order away.
    pub fn earliest_ms(&self) -> f64 {
        self.lines
            .iter()
            .flat_map(|line| [line.steps.first(), line.server_points.first()])
            .flatten()
            .map(|(t, _)| *t)
            .filter(|t| *t > 1.0)
            .fold(self.create_ms, f64::min)
    }

    fn new(r: &OrderRow, now_ms: f64, seq: u64) -> Self {
        // The start cannot be in the future because the core clock may lead the local clock;
        // otherwise the line segment degenerates or moves beyond the right edge. An order the core
        // dates not at all — a sale from an already-held asset, which has no buy leg — begins here
        // and now, and `update` knows not to treat that fallback as a floor for the other lines.
        let wire_create_ms = wire_line_start(r.create_time_ms, 0.0, now_ms);
        let create_ms = if wire_create_ms > 1.0 {
            wire_create_ms
        } else {
            now_ms
        };
        Self {
            uid: r.uid,
            market: r.market.clone(),
            is_short: r.is_short,
            emulator: r.emulator,
            size: r.size as f32,
            remaining_size: r.remaining_size as f32,
            fill_pct: r.fill_pct,
            chart_num: 0,
            pending: r.pending,
            panic_sell: r.panic_sell,
            is_moon_shot: r.is_moon_shot,
            corridor_price_down: r.corridor_price_down,
            corridor_price_up: r.corridor_price_up,
            create_ms,
            entry_fill_ms: None,
            closed_ms: None,
            closed_reason: None,
            closed_store_ms: None,
            closed_rev: None,
            last_seen_ms: now_ms,
            seen_generation: 0,
            seq,
            lines: Default::default(),
            liq: None,
        }
    }
}

/// Per-core order-line store covering all markets.
#[derive(Default)]
pub struct OrderLineStore {
    orders: HashMap<u64, RetainedOrder>,
    /// Closed-order UIDs in closure order, providing the only cap on retained closed orders.
    ///
    /// A newly closed UID enters at the back and overflow leaves from the front, without sorting or
    /// a pruning scan.
    closed_ring: VecDeque<u64>,
    /// UIDs of currently open orders only.
    ///
    /// Closure removes an entry and revival adds it back, so update-time backstop and auto-fit work
    /// scales with live orders instead of the retained closed-history cap.
    open_uids: Vec<u64>,
    /// Cached per-market price range for auto-Y over open-order buy/sell lines and active protective
    /// stops of filled orders. Stop, trailing, and VStop prices exist only after fill. The cache is
    /// rebuilt only when orders change, together with `rev`, rather than on every prepare pass. Its
    /// ordered map supports borrowed market lookup without SipHash and clones a key once per market.
    auto_fit_ranges: BTreeMap<String, (f32, f32)>,
    /// Increments on any render-affecting change, including geometry, label inputs, and zones.
    pub rev: u64,
    seq_counter: u64,
    /// Monotonic snapshot generation used by `RetainedOrder::seen_generation`.
    update_generation: u64,
}

impl OrderLineStore {
    /// Applies a fresh order-row batch and returns whether retained render state changed.
    ///
    /// Active orders are updated, repricings record knots, explicit terminal rows close immediately,
    /// and missing orders close after the grace period. Geometry, label, and zone changes increment
    /// `rev`.
    pub fn update(&mut self, rows: &[OrderRow]) -> bool {
        let now_ms = now_unix_ms();
        self.update_generation = self.update_generation.wrapping_add(1);
        let generation = self.update_generation;
        let mut changed = false;
        // UIDs closed by an explicit `job_is_done` flag in this update are remembered after the
        // loop because the loop holds a mutable borrow of `self.orders` through its entry.
        let mut close_now: Vec<u64> = Vec::new();
        let mut closed_this_update: Vec<u64> = Vec::new();

        // Assign fresh rows in UID order. Each receives the smallest number not held by a currently
        // open retained order or an earlier fresh row in this batch. A fresh terminal row can reserve
        // a slot here because closure is processed later; after it closes, a later batch can reuse
        // the slot. An empty market therefore starts naturally at 1.
        let mut new_nums: HashMap<u64, u32> = HashMap::new();
        let mut fresh: Vec<&OrderRow> = rows
            .iter()
            .filter(|r| !self.orders.contains_key(&r.uid))
            .collect();
        if !fresh.is_empty() {
            let mut used: HashMap<&str, HashSet<u32>> = HashMap::new();
            for uid in &self.open_uids {
                if let Some(order) = self.orders.get(uid) {
                    if order.chart_num > 0 {
                        used.entry(order.market.as_str())
                            .or_default()
                            .insert(order.chart_num);
                    }
                }
            }
            fresh.sort_by_key(|r| r.uid);
            for r in fresh {
                let set = used.entry(r.market.as_str()).or_default();
                let mut n = 1u32;
                while set.contains(&n) {
                    n += 1;
                }
                set.insert(n);
                new_nums.insert(r.uid, n);
            }
        }

        for r in rows {
            let mut became_open = false;
            let order = match self.orders.entry(r.uid) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let seq = self.seq_counter;
                    self.seq_counter = self.seq_counter.wrapping_add(1);
                    changed = true;
                    let mut ro = RetainedOrder::new(r, now_ms, seq);
                    ro.chart_num = new_nums.get(&r.uid).copied().unwrap_or(0);
                    became_open = true;
                    entry.insert(ro)
                }
            };
            order.last_seen_ms = now_ms;
            order.seen_generation = generation;
            // Revive a previously closed UID only when it is active again, without `job_is_done`.
            // A terminal order may remain in snapshots throughout the core's deferred-removal
            // window; reviving it would make the line flicker from closed to open on every update.
            if order.closed_ms.is_some() && !r.job_is_done {
                order.closed_ms = None;
                order.closed_reason = None;
                order.closed_store_ms = None;
                order.closed_rev = None;
                became_open = true;
                changed = true;
            }
            order.is_short = r.is_short;
            order.pending = r.pending;
            // Stored like the two scalars above and, like them, NOT render-affecting. It used to be:
            // the flag decided whether the order was drawn at all, so a flip had to force a rebuild.
            // That filter is gone — the setting selects closed trades now — and nothing on the chart
            // reads this field, so bumping the revision here would rebuild every zone, line, segment
            // and marker of the market to change nothing visible.
            order.emulator = r.emulator;
            // Size and fill drive line labels. Bump the revision on a meaningful change so the
            // bought-quantity label follows progressive fills, using a threshold against float
            // jitter.
            let new_size = r.size as f32;
            let new_remaining_size = r.remaining_size as f32;
            if (order.size - new_size).abs() > price_eps(new_size)
                || (order.remaining_size - new_remaining_size).abs() > price_eps(new_remaining_size)
                || (order.fill_pct - r.fill_pct).abs() > 0.01
            {
                order.size = new_size;
                order.remaining_size = new_remaining_size;
                order.fill_pct = r.fill_pct;
                changed = true;
            }
            if order.panic_sell != r.panic_sell
                || order.is_moon_shot != r.is_moon_shot
                || order.corridor_price_down != r.corridor_price_down
                || order.corridor_price_up != r.corridor_price_up
            {
                order.panic_sell = r.panic_sell;
                order.is_moon_shot = r.is_moon_shot;
                order.corridor_price_down = r.corridor_price_down;
                order.corridor_price_up = r.corridor_price_up;
                changed = true;
            }
            let f = r.filled;
            // The entry for both long and short orders is represented by the Buy line, visible from
            // order creation. The opposite-side Sell exit appears only after the entry fills and
            // starts when the exit leg was created. Stops, take profit, VStop, and liquidation also
            // appear only after fill and start at the fill itself.
            //
            // Both starts come from the WIRE, not from the local clock: a position older than this
            // process would otherwise date its exit and its stops to the terminal's launch. When
            // the wire carries neither — an exit leg the core never placed, a position inherited
            // without a buy leg — they fall back to "now", where these lines began before.
            //
            // The floor is the order's own creation, but only as far as the wire gave one:
            // `create_ms` itself falls back to the local clock, and that fallback must not pull a
            // genuinely older exit time forward to the launch.
            let floor_ms = wire_line_start(r.create_time_ms, 0.0, now_ms);
            let sell_start_ms = wire_line_start(r.sell_create_time_ms, floor_ms, now_ms);
            let fill_start_ms = {
                // The fill is when a protective line came into existence, so it wins whenever the
                // core supplies it. Taking the EARLIER of fill and exit-leg creation was rejected:
                // it would cover one rare case (an entry filled in part whose remainder is
                // cancelled later dates the leg's close to the cancellation) at the price of a
                // common one — were the exit leg ever materialized at placement instead of at the
                // fill, every stop would be drawn back across the pre-fill window, claiming a stop
                // existed where none did. A late start says less than it could; an early one lies.
                let fill = wire_line_start(r.entry_fill_time_ms, floor_ms, now_ms);
                if fill > 1.0 { fill } else { sell_start_ms }
            };
            // The same wire fill, kept as the ENTRY line's own END so the chart can stop that line
            // where the leg ceased instead of running it on to the pane edge. Gated on `f`:
            // `entry_fill_time_ms` is the buy leg's CLOSE, which a CANCELLED entry carries too, and
            // a fill mark on an order that never filled would be a lie. The `sell_start_ms`
            // fallback above is deliberately NOT inherited — the exit leg's creation is not a fill.
            let wire_fill_ms = wire_line_start(r.entry_fill_time_ms, floor_ms, now_ms);
            let entry_fill_ms = (f && wire_fill_ms > 1.0).then_some(wire_fill_ms);
            // While the core clock LEADS the local one, `wire_line_start` folds the fill back to
            // `now_ms` — a value that advances on every update. Adopting that on a plain equality
            // test would rebuild and re-upload all order geometry every frame for as long as the
            // skew lasts; refusing every later date instead would freeze that stand-in as the
            // permanent fill moment even after the skew resolves. So a FOLDED date is taken only
            // as the first one and never re-adopted, while the settled wire date that eventually
            // replaces it is adopted once — after which it is stable and this test stops firing.
            // The other callers never face the choice: they feed the value to a line's FIRST step
            // and never read it again.
            let folded = r.entry_fill_time_ms > now_ms;
            let adopt = match (order.entry_fill_ms, entry_fill_ms) {
                (stored, fresh) if stored.is_some() != fresh.is_some() => true,
                (Some(stored), Some(fresh)) => !folded && stored != fresh,
                _ => false,
            };
            if adopt {
                order.entry_fill_ms = entry_fill_ms;
                changed = true;
            }
            let new_liq = if f { r.liq.map(|v| v as f32) } else { None };
            if order.liq != new_liq {
                order.liq = new_liq;
                changed = true;
            }
            let g = |show: bool, v: f64| (show && v.is_finite() && v > 0.0).then_some(v as f32);
            let go = |show: bool, v: Option<f64>| if show { v.map(|x| x as f32) } else { None };
            // Value and first-step time by line kind.
            changed |= order.lines[LineKind::Buy as usize].update_server(r.buy_trace.as_ref());
            changed |= order.lines[LineKind::Buy as usize].update(
                g(true, r.buy_price),
                order.create_ms,
                now_ms,
            );
            changed |= order.lines[LineKind::Sell as usize].update_server(r.sell_trace.as_ref());
            changed |= order.lines[LineKind::Sell as usize].update(
                g(f, r.sell_price),
                sell_start_ms,
                now_ms,
            );

            let vals: [(Option<f32>, f64, usize); TRACED_KINDS - 2] = [
                (go(f, r.stop_loss), fill_start_ms, LineKind::Stop as usize),
                (
                    go(f, r.trailing),
                    fill_start_ms,
                    LineKind::Trailing as usize,
                ),
                (
                    go(f, r.take_profit),
                    fill_start_ms,
                    LineKind::TakeProfit as usize,
                ),
                (go(f, r.vstop), fill_start_ms, LineKind::VStop as usize),
                // The pending condition applies only before fill and starts at order creation.
                (
                    go(!f, r.pending_cond),
                    order.create_ms,
                    LineKind::PendingCond as usize,
                ),
            ];
            for (v, start_ms, i) in vals {
                changed |= order.lines[i].update(v, start_ms, now_ms);
            }
            // The explicit core flag `job_is_done` means the order is terminal, either filled or
            // cancelled, and awaits deferred removal. Mark it closed immediately while it remains
            // in the snapshot instead of waiting for disappearance plus the grace period.
            if r.job_is_done && order.closed_ms.is_none() {
                order.closed_ms = Some(now_ms);
                order.closed_reason = Some(explicit_close_reason(r));
                order.closed_store_ms = Some(now_ms);
                close_now.push(r.uid);
                closed_this_update.push(r.uid);
                changed = true;
            }
            let index_open = became_open && order.closed_ms.is_none();
            if index_open {
                self.open_uids.push(r.uid);
            }
        }

        for uid in close_now {
            self.remember_closed(uid);
        }

        // Backstop: close an order after it remains absent from snapshots beyond the grace period.
        // The primary `job_is_done` path above closes while an order is still present. This path is
        // only for orders removed by the core before we observe that flag, such as after a missed
        // frame or gap; the grace period suppresses false closures on brief empty or partial
        // snapshots during reconnects or subscription churn.
        let mut newly_closed = Vec::new();
        for uid in self.open_uids.iter().copied() {
            let Some(ord) = self.orders.get_mut(&uid) else {
                continue;
            };
            if ord.seen_generation != generation && now_ms - ord.last_seen_ms > CLOSE_GRACE_MS {
                ord.closed_ms = Some(ord.last_seen_ms);
                ord.closed_reason = Some(OrderCloseReason::BackstopMissing);
                ord.closed_store_ms = Some(now_ms);
                newly_closed.push(uid);
                closed_this_update.push(uid);
                changed = true;
            }
        }

        for uid in newly_closed {
            self.remember_closed(uid);
            changed = true;
        }
        if !closed_this_update.is_empty() {
            closed_this_update.sort_unstable();
            closed_this_update.dedup();
            self.open_uids
                .retain(|uid| closed_this_update.binary_search(uid).is_err());
        }
        if changed {
            self.rev = self.rev.wrapping_add(1);
            for uid in closed_this_update {
                if let Some(order) = self.orders.get_mut(&uid) {
                    order.closed_rev = Some(self.rev);
                }
            }
            self.rebuild_auto_fit_ranges();
        }
        changed
    }

    /// Rebuilds per-market auto-fit ranges from the compact open-order index.
    ///
    /// This runs only after a real change because line prices mutate only in `update`. The cache
    /// therefore stays current without scanning all orders during every render preparation.
    fn rebuild_auto_fit_ranges(&mut self) {
        self.auto_fit_ranges.clear();
        for uid in &self.open_uids {
            let Some(order) = self.orders.get(uid) else {
                continue;
            };
            // Include Buy/Sell plus active protective stops. Stop, Trailing, and VStop have a
            // current price only after fill because synchronization gates their values on `filled`,
            // so only actual protective stops of filled orders enter the range. Unfilled orders
            // still contribute their Buy line, but no protective stop lines.
            let mut range: Option<(f32, f32)> = None;
            for idx in [
                LineKind::Buy as usize,
                LineKind::Sell as usize,
                LineKind::Stop as usize,
                LineKind::Trailing as usize,
                LineKind::VStop as usize,
            ] {
                if let Some(p) = order.lines[idx].current_price() {
                    if p.is_finite() && p > 0.0 {
                        let entry = range.get_or_insert((p, p));
                        entry.0 = entry.0.min(p);
                        entry.1 = entry.1.max(p);
                    }
                }
            }
            if let Some((min, max)) = range {
                if let Some(entry) = self.auto_fit_ranges.get_mut(order.market.as_str()) {
                    entry.0 = entry.0.min(min);
                    entry.1 = entry.1.max(max);
                } else {
                    self.auto_fit_ranges
                        .insert(order.market.clone(), (min, max));
                }
            }
        }
    }

    /// Records a UID's closure and enforces the retained-history safety cap.
    ///
    /// `update` removes all closed UIDs from the open index in one batch so a snapshot that closes
    /// many orders does not repeatedly scan the live-order vector.
    fn remember_closed(&mut self, uid: u64) {
        self.closed_ring.push_back(uid);
        while self.closed_ring.len() > CLOSED_RING_CAP {
            let Some(old_uid) = self.closed_ring.pop_front() else {
                break;
            };
            if self
                .orders
                .get(&old_uid)
                .is_some_and(|order| order.closed_ms.is_some())
            {
                self.orders.remove(&old_uid);
            }
        }
    }

    /// Iterates orders for one market, as used to render a specific chart panel's lines.
    pub fn iter_market<'a>(
        &'a self,
        market: &'a str,
    ) -> impl Iterator<Item = &'a RetainedOrder> + 'a {
        self.orders.values().filter(move |o| o.market == market)
    }

    /// Returns orders to draw for a market: every open order, followed by at most `max_closed`
    /// newest closed orders, in reverse ring order.
    ///
    /// The result is not sorted, and the store separately caps retained closed history via its ring.
    ///
    /// This once took a caller-supplied `keep` visibility predicate, applied before the cap so a
    /// hidden order could not push a visible one off the chart. The chart's only order-visibility
    /// filter was real-vs-emulator, and that setting now selects TRADES rather than order lines, so
    /// nothing filters here any more and the parameter went with it.
    ///
    /// Args:
    ///     market: Market whose orders are drawn.
    ///     max_closed: Cap on the returned closed orders.
    pub fn market_draw_orders(&self, market: &str, max_closed: usize) -> Vec<&RetainedOrder> {
        let mut out: Vec<&RetainedOrder> = self
            .orders
            .values()
            .filter(|o| o.market == market && o.closed_ms.is_none())
            .collect();
        let mut taken = 0usize;
        let mut seen: HashSet<u64> = HashSet::new();
        for uid in self.closed_ring.iter().rev() {
            if taken >= max_closed {
                break;
            }
            // A revived and reclosed UID may occur in the ring twice, so deduplicate it.
            if !seen.insert(*uid) {
                continue;
            }
            if let Some(o) = self.orders.get(uid) {
                if o.closed_ms.is_some() && o.market == market {
                    out.push(o);
                    taken += 1;
                }
            }
        }
        out
    }

    /// Returns the cached auto-Y price range for a market's open-order lines.
    ///
    /// The range includes Buy/Sell and active Stop/Trailing/VStop lines, with protective stops
    /// available only for filled orders. Pending conditions, take-profit lines, and liquidation are
    /// excluded. `rebuild_auto_fit_ranges` refreshes the cache when orders change, avoiding a scan
    /// during every prepare pass.
    pub fn auto_fit_range(&self, market: &str) -> Option<(f32, f32)> {
        self.auto_fit_ranges.get(market).copied()
    }

    /// Returns whether an open order has panic sell enabled, for disabling it before a Sell drag.
    pub fn order_panic_sell(&self, uid: u64) -> bool {
        self.orders
            .get(&uid)
            .is_some_and(|o| o.closed_ms.is_none() && o.panic_sell)
    }

    pub fn order_state(&self, uid: u64) -> Option<OrderLineState> {
        let order = self.orders.get(&uid)?;
        Some(OrderLineState {
            uid,
            closed_reason: order.closed_reason,
            closed_store_ms: order.closed_store_ms,
            closed_rev: order.closed_rev,
            active: order.closed_ms.is_none(),
        })
    }

    /// Returns the current price of one order line for UI optimistic-preview reconciliation.
    ///
    /// After a drag, the UI keeps the new price visible until the core echoes the same price.
    pub fn current_line_price(&self, uid: u64, kind: LineKind) -> Option<f32> {
        let order = self.orders.get(&uid)?;
        order
            .lines
            .get(kind as usize)
            .and_then(LineTrace::current_price)
    }
}

fn explicit_close_reason(row: &OrderRow) -> OrderCloseReason {
    if row.filled || row.fill_pct >= 99.95 {
        OrderCloseReason::Filled
    } else if row.job_is_done {
        OrderCloseReason::Cancel
    } else {
        OrderCloseReason::Unknown
    }
}

#[cfg(test)]
mod tests;
