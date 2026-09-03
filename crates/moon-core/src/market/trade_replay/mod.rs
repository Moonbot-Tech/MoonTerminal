//! Frozen market history for ONE closed trade, fetched from the exchange's public REST.
//!
//! # Why this exists
//!
//! Clicking a closed trade in the Report used to reposition the main chart's viewport onto the
//! trade's interval and fetch nothing, so the entry and exit arrows regularly landed over empty
//! space. Neither of the two live sources can fix that: MoonProto keeps trades in a bounded
//! in-process ring rather than a history store, and a core answers `request_coin_card` with about
//! five hundred recent bars and accepts no time range. The exchange's own public REST is the only
//! thing that can answer "what was this market doing between 14:02 and 14:20 last Tuesday".
//!
//! # What a replay is, and what it is NOT
//!
//! A [`TradeReplaySeries`] is an IMMUTABLE, BOUNDED answer to one such question: the rows covering
//! one trade's window, already fetched, already clipped. It has no live edge, no subscription and
//! no incremental drain, and [`TradeReplaySeries::read_into`] is a PURE function of the series and
//! the requested window — no lock, no client, no I/O.
//!
//! It is deliberately NOT registered anywhere global. The consumer holds it directly on the chart
//! engine it owns, so a replay cannot reach the user's live main chart even by mistake: there is
//! no key to collide on. Contrast [`crate::fixture`], whose bench state is a process-wide
//! `OnceLock` — that module is the SHAPE this one copies, never a mechanism it reuses.
//!
//! # Layout
//!
//! - [`venue_caps`] — which venues can be asked, and through which route. Keyed off
//!   [`crate::venue::Venue`], never off an exchange name.
//! - Everything a caller renders arrives as a TYPE ([`TradeReplayOutcome`]), never as a built
//!   sentence: `moon-core` has no `rust_i18n` and must not decide the user's wording.

pub mod gate;
pub mod rest;
pub mod venue_caps;
pub mod worker;

use crate::feed::types::Tick;
use crate::market::candles::ChartCandle;
use crate::market::{CandleReadParams, ChartHistoryBuffers, ChartHistoryRead};
use crate::venue::{Brand, Venue};

/// Milliseconds in one minute, the only timeframe a replay is fetched at.
const MINUTE_MS: i64 = 60_000;

/// Smallest distance from the ENTRY to the window's left edge, in milliseconds.
///
/// A FLOOR, not padding added on top: a trade whose proportional context already reaches further
/// back keeps its own, larger lead. The point of looking at a trade's picture is to see what the
/// market was doing BEFORE the entry, and half of a forty-second scalp is twenty seconds — which
/// is no context at all. Sixty minutes is the user's stated minimum, raised from thirty on
/// 2026-08-23 after looking at real trades in the shipped build.
///
/// This also SUBSUMES the ten-minute minimum span this module used to widen to: the two floors
/// together guarantee at least eighty minutes plus the position's own duration, so that widening
/// step could never fire again and was removed rather than left as unreachable code.
const LEAD_FLOOR_MS: i64 = 60 * MINUTE_MS;

/// Smallest distance from the EXIT to the window's right edge, in milliseconds.
///
/// A FLOOR on the same terms as [`LEAD_FLOOR_MS`], and deliberately much smaller: what happened
/// after an exit is worth a glance, not a study, and every extra minute here is a bar fetched
/// through a public, rate-limited endpoint. Twenty minutes is the user's stated minimum, raised
/// from five on 2026-08-23 — still a third of the lead, so the asymmetry the paragraph argues
/// for survives the widening.
const TRAIL_FLOOR_MS: i64 = 20 * MINUTE_MS;

/// Budget on the CONTEXT a replay pays for, in milliseconds — not a ceiling on the window.
///
/// A position held for weeks would otherwise page thousands of one-minute bars through a public,
/// rate-limited endpoint to draw a picture no denser than the pixels available. Seven days is the
/// point past which the request cost stops buying visible detail.
///
/// Past it the proportional padding is
/// trimmed back toward [`LEAD_FLOOR_MS`] and [`TRAIL_FLOOR_MS`] and no further: a position long
/// enough that its floors alone outrun this budget keeps them, and the window is marked
/// [`ReplayWindow::over_budget`]. It used to CENTRE the window here instead, which on a long
/// enough position pushed the entry and the exit outside their own picture — the defect this
/// module exists to remove, not one it may reintroduce at the top of its range.
const MAX_SPAN_MS: i64 = 7 * 24 * 60 * MINUTE_MS;

/// Fraction of the trade's own duration added on each side as market context.
///
/// The goal is a picture of the trade IN CONTEXT — what price did BEFORE the entry and AFTER the
/// exit — so a window clipped exactly to the position would answer the wrong question.
const CONTEXT_FRACTION: f64 = 0.5;

/// Margin added around the trade's own span when computing [`ReplayWindow::focus`].
///
/// Wide enough that the entry and exit sit comfortably inside the focus tiles [`tick_plan`]
/// fetches first, rather than landing on the very edge of one; not wider, because every extra
/// millisecond here is lead/trail context pulled ahead of a slice that is actually IN the trade.
const FOCUS_MARGIN_MS: i64 = 5 * MINUTE_MS;

/// Width of one tick-fetch tile in [`tick_plan`], before a route's own cap narrows it further.
///
/// An 80-minute scalp window under Binance USD-M's one-hour query cap would otherwise tile into
/// two requests that both straddle the focus, making trade-priority ordering inert — chopping
/// finer than the route cap is what gives [`tick_plan`] something to actually prioritise.
const TICK_SLICE_MS: i64 = 10 * MINUTE_MS;

/// Bucket widths [`fit_ticks`] tries in order, coarsest last.
///
/// Each rung roughly doubles to triples the previous one, so a run that barely overflows the
/// budget loses little precision while a run that overflows it by orders of magnitude still
/// terminates in a handful of steps instead of walking one millisecond at a time.
const THIN_LADDER_MS: [i64; 9] = [
    1_000, 2_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000,
];

/// How the tick stage for one window ended — the thing the window's caption NAMES.
///
/// Each variant is a DIFFERENT sentence to show the user, and two of them (`NoRoute`,
/// `OutOfRetention`) are known before a single request is spent, so they cost nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickStatus {
    /// A tick stage is queued and has not answered yet. The first outcome of every window that
    /// earns one carries this.
    Pending,
    /// This build knows no public trades route for the venue (Bybit, Hyperliquid). Retrying
    /// cannot help.
    NoRoute,
    /// The window is older than the route's documented trade retention.
    OutOfRetention {
        /// How far back the venue's own tick retention actually reaches, in milliseconds.
        retention_ms: i64,
    },
    /// The venue answered and held no trade inside the window, while klines exist.
    NoTrades,
    /// The tick fetch itself did not produce an answer.
    Failed,
    /// Ticks were served. Only a [`TradeReplaySource::Ticks`] series carries this.
    Served,
}

/// Which data a replay actually carries, so the window can say which it is showing.
///
/// This is user-visible and load-bearing: a one-minute picture of a forty-second scalp is an
/// honest answer only while it is LABELLED as one, and the caller cannot label what it cannot
/// distinguish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeReplaySource {
    /// Individual public trades, drawn as chart points.
    Ticks,
    /// One-minute bars — the fallback wherever ticks cannot be had.
    Klines1m,
}

/// Why a replay carries nothing to draw, stated as a fact rather than as a sentence.
///
/// Each variant is a DIFFERENT thing to tell the user, and two of them decide whether a retry
/// button appears at all, so they are never collapsed into one "no data".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TradeReplayEmpty {
    /// The venue answered, and its answer held no row inside the window.
    ///
    /// A real outcome for a market that was delisted, halted, or simply never traded there.
    NoDataInWindow,
    /// This build knows no public REST route for that venue.
    ///
    /// Carries the brand so the window can NAME it. Retrying cannot help, so the caller offers no
    /// retry.
    NoEndpoint { brand: Brand },
    /// The core's platform ordinal resolves to no venue this build knows.
    ///
    /// Either the core never reported one, or it is newer than this build. `venue.rs` returns
    /// `None` rather than a neighbour's answer on purpose, and this variant carries that refusal
    /// through instead of guessing.
    UnknownVenue,
    /// The trade's own core is not connected, so its venue and market cannot be identified.
    ///
    /// A report row stores `core_uid` and a coin, and NEITHER the venue nor the exchange-native
    /// market name is durable: the platform ordinal is reported by a live core and never written
    /// to `servers.enc`, and the coin is resolved into a market against that core's live catalog.
    /// So a trade whose core is offline, disabled, or since removed cannot be replayed at all —
    /// the same boundary the existing coin-cell click already stops at, which is why this is a
    /// NAMED outcome rather than a silent blank. Reconnecting the core is what fixes it, so the
    /// caller offers a retry.
    CoreNotConnected,
    /// The trade's own stamps cannot describe a window.
    ///
    /// A close at or before the open, or a non-positive stamp. There is nothing to fetch and
    /// nothing to retry.
    DegenerateWindow,
}

/// Why a replay could not be fetched, as opposed to having come back empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TradeReplayFailure {
    /// The venue's send permit is not due yet; the window may retry in this many seconds.
    RateLimited { retry_in_s: u32 },
    /// Transport, service, or malformed-response failure that may recover.
    ///
    /// `diagnostic` is for `log::warn!` ONLY. It is an English fragment from a transport library
    /// and must never be rendered as the user's sentence — that is precisely the pre-built string
    /// this module exists to avoid.
    Transient { diagnostic: String },
    /// The venue says the symbol does not exist there.
    ///
    /// Distinct from [`TradeReplayEmpty::NoDataInWindow`]: the market is wrong, not the window.
    UnknownSymbol,
}

/// The complete answer to one replay request.
#[derive(Clone, Debug)]
pub enum TradeReplayOutcome {
    /// Rows to draw.
    Ready(TradeReplaySeries),
    /// Nothing to draw, for a reason the window states.
    Empty(TradeReplayEmpty),
    /// The fetch itself did not produce an answer.
    Failed(TradeReplayFailure),
}

/// The inclusive millisecond window a replay covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayWindow {
    /// First millisecond the replay covers.
    pub from_ms: i64,
    /// Last millisecond the replay covers.
    pub to_ms: i64,
    /// The trade's own open, in milliseconds — the position's real entry stamp, not [`Self::from_ms`].
    pub open_ms: i64,
    /// The trade's own close, in milliseconds — the position's real exit stamp, not [`Self::to_ms`].
    pub close_ms: i64,
    /// Whether this window is WIDER than [`MAX_SPAN_MS`] because its floors demanded it.
    ///
    /// Renamed from `clipped`, and the rename is the point: the field used to mean "half the
    /// position was thrown away to fit the budget", which is no longer a thing that can happen —
    /// [`LEAD_FLOOR_MS`] and [`TRAIL_FLOOR_MS`] are honoured whatever the budget says. What is
    /// worth telling a caller now is the opposite fact: this fetch is expensive and may come back
    /// as a retryable failure rather than a chart.
    ///
    /// Stated plainly: NOTHING reads this yet. It is carried because the outcome it names is real
    /// and the caller is the only layer that can word it.
    pub over_budget: bool,
}

impl ReplayWindow {
    /// Span of the window in milliseconds, always at least one.
    ///
    /// Returns:
    ///     Inclusive width.
    pub const fn span_ms(self) -> i64 {
        match self.to_ms - self.from_ms {
            n if n < 1 => 1,
            n => n,
        }
    }

    /// The sub-window closest to the trade itself, for ordering a tick fetch around it.
    ///
    /// [`tick_plan`] fetches this region FIRST and orders every other tile by distance to it, so
    /// a fetch cut short by budget or deadline still lands the trade's own span rather than an
    /// hour of lead context nobody asked to see before it.
    ///
    /// Returns:
    ///     `(left, right)` inclusive, clamped into `[Self::from_ms, Self::to_ms]` on both ends —
    ///     independently. Every constructor of this type preserves `open_ms <= close_ms`; a
    ///     hand-built window that violates it is out of this function's contract and can yield an
    ///     inverted `(left, right)` rather than a usable focus (no guard here — that state is
    ///     unreachable today, per house style).
    pub(crate) fn focus(self) -> (i64, i64) {
        let left = (self.open_ms - FOCUS_MARGIN_MS)
            .max(self.from_ms)
            .min(self.to_ms);
        let right = (self.close_ms + FOCUS_MARGIN_MS)
            .min(self.to_ms)
            .max(self.from_ms);
        (left, right)
    }
}

/// Compute the window to fetch around one trade.
///
/// The window is the position padded by [`CONTEXT_FRACTION`] of its own duration on each side, so
/// a long trade gets proportionally more context than a short one — but never less than
/// [`LEAD_FLOOR_MS`] before the entry and [`TRAIL_FLOOR_MS`] after the exit, which is what makes
/// a forty-second scalp a picture of a market rather than a picture of two candles. The result is
/// then trimmed back toward those floors — never past them — when the result outruns
/// [`MAX_SPAN_MS`]. The budget spends the CONTEXT, so an exit with no bars after it is not a
/// state this function can produce at any position length.
///
/// Args:
///     buy_date_s: Position open, in Unix SECONDS, as the report row stores it.
///     close_date_s: Position close, in Unix seconds.
///
/// Returns:
///     The window to fetch, or `None` when the stamps cannot describe one.
pub fn replay_window(buy_date_s: i64, close_date_s: i64) -> Option<ReplayWindow> {
    // A close in the SAME second as the open is a real trade, not a bad stamp. These stamps carry
    // whole seconds, so a position that filled and closed inside one of them is recorded with two
    // identical numbers - a scalp, which is exactly the kind of trade a reader most wants replayed
    // and the kind this guard used to refuse outright. Only a close BEFORE the open, or a
    // non-positive stamp, is unusable. A zero-length position needs no special handling
    // downstream: its proportional context is zero, so the floors below decide the whole window,
    // which is what they exist for.
    if buy_date_s <= 0 || close_date_s <= 0 || close_date_s < buy_date_s {
        return None;
    }
    let open_ms = buy_date_s.checked_mul(1_000)?;
    let close_ms = close_date_s.checked_mul(1_000)?;
    let held_ms = close_ms - open_ms;
    let pad_ms = (held_ms as f64 * CONTEXT_FRACTION).round() as i64;
    // The floors are a MAXIMUM against the proportional context, never a sum with it: a long trade
    // keeps its own, wider margin, and a short one is lifted to the floor. Adding them instead
    // would double the fetch for every long position to buy context it already had.
    let mut from_ms = open_ms.saturating_sub(pad_ms.max(LEAD_FLOOR_MS));
    let mut to_ms = close_ms.saturating_add(pad_ms.max(TRAIL_FLOOR_MS));
    // The budget trims CONTEXT and never the floors. The centred clip this replaces trimmed both
    // ends inwards until, on a long enough position, the entry and the exit fell OUTSIDE the
    // window — a picture of a trade with no trade in it, which is the defect this module exists
    // to remove rather than one it may reintroduce at the top of its range.
    if to_ms - from_ms > MAX_SPAN_MS {
        from_ms = from_ms.max(open_ms.saturating_sub(LEAD_FLOOR_MS));
        to_ms = to_ms.min(close_ms.saturating_add(TRAIL_FLOOR_MS));
    }
    // A position held so long that its FLOORS alone outrun the budget keeps them anyway. The
    // floors are the requirement; the budget is this module's own judgement call, and it is not
    // the only one in force — `worker::JOB_DEADLINE` bounds the fetch in TIME and answers an
    // over-long job with a retryable failure. So the worst case here is an honest "could not
    // fetch it, retry", never a chart quietly missing its own entry and exit.
    let over_budget = to_ms - from_ms > MAX_SPAN_MS;
    // A pre-epoch left edge is meaningless to every venue and would be sent as a negative
    // `startTime`; pull it forward instead of asking for it. `open_ms`/`close_ms` are left alone
    // by this shift: they are the trade's own REAL stamps, not a fetch bound, and a pre-epoch
    // trade is already rejected above by the `close_date_s < buy_date_s` / non-positive guard
    // long before this shift ever runs, so nothing here has reason to move them.
    if from_ms < 0 {
        to_ms = to_ms.saturating_add(-from_ms);
        from_ms = 0;
    }
    Some(ReplayWindow {
        from_ms,
        to_ms,
        open_ms,
        close_ms,
        over_budget,
    })
}

/// Split one window into requests no larger than a route's documented row cap.
///
/// The pages tile the window with no gap and no overlap: a gap would draw a hole in the middle of
/// a trade, and an overlap would double-count volume once the rows are merged. The last page is
/// short rather than the first, so the earliest bar of the window is always the window's own left
/// edge and the series cannot start late.
///
/// Args:
///     window: The window to cover.
///     bar_ms: Milliseconds per bar.
///     max_rows: Largest number of bars one request may ask for.
///
/// Returns:
///     Inclusive `(from_ms, to_ms)` pairs in ascending order; empty when `max_rows` or `bar_ms`
///     is zero, which no route reports and which would otherwise loop forever.
pub fn pages(window: ReplayWindow, bar_ms: i64, max_rows: usize) -> Vec<(i64, i64)> {
    if bar_ms <= 0 || max_rows == 0 {
        return Vec::new();
    }
    let step = bar_ms.saturating_mul(max_rows as i64);
    let mut out = Vec::new();
    let mut cursor = window.from_ms;
    while cursor <= window.to_ms {
        let end = cursor.saturating_add(step - 1).min(window.to_ms);
        out.push((cursor, end));
        if end == window.to_ms {
            break;
        }
        cursor = end + 1;
    }
    out
}

/// Split one window into requests no larger than a trade route's documented query span.
///
/// The frozen shape [`super::worker`] and every `rest::<venue>::fetch_trades` build against:
/// `None` answers one slice covering the whole window; `Some(span)` tiles the window into
/// requests of at most `span` ms with no gap and no overlap, the same tiling discipline [`pages`]
/// uses for the kline pager, just keyed on a request's time SPAN rather than its row count —
/// several trade routes ([`super::venue_caps::TradeRoute::max_query_ms`]) cap a request's window
/// rather than its row count.
///
/// A non-positive span returns no slices: unlike [`pages`], which derives its step from a
/// `bar_ms * max_rows` product that route constants keep positive, a caller here supplies the
/// span directly. [`super::venue_caps::TradeRoute::max_query_ms`] represents an unbounded route
/// as `None`, so every `Some` value remains a finite vendor-imposed request window.
///
/// Args:
///     window: The window to cover.
///     max_span_ms: Widest span one request may cover, or `None` when the route documents no
///         cap.
///
/// Returns:
///     Inclusive `(from_ms, to_ms)` pairs in ascending order; empty when `max_span_ms` is
///     `Some(n)` with `n <= 0`, which no route reports.
pub fn time_slices(window: ReplayWindow, max_span_ms: Option<i64>) -> Vec<(i64, i64)> {
    let Some(span) = max_span_ms else {
        return vec![(window.from_ms, window.to_ms)];
    };
    if span <= 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cursor = window.from_ms;
    while cursor <= window.to_ms {
        let end = cursor.saturating_add(span - 1).min(window.to_ms);
        out.push((cursor, end));
        if end == window.to_ms {
            break;
        }
        cursor = end + 1;
    }
    out
}

/// One ordered decomposition of a [`ReplayWindow`] into tick-fetch tiles.
///
/// [`Self::slices`] is ordered so that ANY PREFIX is a CONTIGUOUS span with no gap between the
/// sorted first-k slices, for every k — [`worker::paginate_ticks`] leans on exactly this to
/// report a fetch truncated by budget or a deadline as ONE covered interval rather than a comb of
/// holes. A later "tidy" that resorts these tiles purely by `from_ms` would keep them contiguous
/// too, but back in CLOCK order — which throws away the whole reason this type exists, so preserve
/// the ORDER here, not merely the contiguity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TickPlan {
    /// Every tile to fetch, in fetch-priority order: the trade's own focus first, then outward.
    pub slices: Vec<(i64, i64)>,
    /// How many leading entries of [`Self::slices`] cover [`ReplayWindow::focus`].
    pub focus_len: usize,
}

/// Tile a window into tick-fetch requests ordered around the TRADE instead of around the clock.
///
/// The window is chopped into three independent regions — before the focus, the focus itself,
/// after the focus — each tiled by [`time_slices`] at `min(TICK_SLICE_MS, max_query_ms)`. The
/// focus tiles are placed first, ascending; every remaining tile then follows by distance to the
/// focus, nearest first, tied-break by `from_ms` (so the tile just before the focus outranks the
/// tile just after it, since it sits at a smaller time). This is what lets a walk that runs out of
/// budget or time abandon the FARTHEST tiles rather than the nearest ones — the defect this module
/// exists to fix in the first place.
///
/// A route's documented retention makes the LEFT edge of every region a moving floor: rows older
/// than `now_ms - retention_ms` cannot be fetched regardless of what the window asks for. Judging
/// retention against the padded window (as the naive check does) refuses a trade whose own span
/// is well inside retention the moment its OPTIONAL lead context crosses the boundary — the ticks
/// the user actually wants are available and get skipped anyway. So this clips instead of
/// refusing outright: every region drops the part of itself older than `earliest_ms`, and if the
/// FOCUS itself — the trade, not its context — falls entirely before it, the whole plan is empty
/// rather than a lead-less scrap of trail. The caller reads an empty plan as "nothing worth
/// fetching" and reports `OutOfRetention`.
///
/// Args:
///     window: The window to cover.
///     max_query_ms: The route's own cap on one request's span, or `None`/non-positive when it
///         documents none, in which case [`TICK_SLICE_MS`] alone tiles the window.
///     earliest_ms: The oldest millisecond the route's retention can still answer for, or `None`
///         when the route documents no retention limit.
///
/// Returns:
///     A [`TickPlan`] whose prefix-contiguity invariant (see [`TickPlan`]) holds for every k, even
///     after retention clipping — clipping only ever shrinks the LEAD region from the near edge
///     inward, and whenever it shrinks the focus's own start too, the entire lead region (being
///     strictly older) is guaranteed to fall before `earliest_ms` as well and is dropped whole, so
///     no clipped region can end up separated from its neighbour by a gap. A region that ends up
///     empty or inverted contributes no tiles — [`time_slices`] already returns none for an
///     inverted span, so no extra per-region check is needed here.
pub(crate) fn tick_plan(
    window: ReplayWindow,
    max_query_ms: Option<i64>,
    earliest_ms: Option<i64>,
) -> TickPlan {
    let span = match max_query_ms {
        Some(cap) if cap > 0 => TICK_SLICE_MS.min(cap),
        _ => TICK_SLICE_MS,
    };
    let (focus_from, focus_to) = window.focus();

    // The trade itself is the one thing worth fetching; a route whose retention does not even
    // reach the trade has nothing this plan can usefully prioritise.
    if let Some(earliest) = earliest_ms {
        if focus_to < earliest {
            return TickPlan {
                slices: Vec::new(),
                focus_len: 0,
            };
        }
    }
    let clip_from = |from: i64| match earliest_ms {
        Some(earliest) => from.max(earliest),
        None => from,
    };

    let focus_slices = time_slices(
        ReplayWindow {
            from_ms: clip_from(focus_from),
            to_ms: focus_to,
            ..window
        },
        Some(span),
    );
    let lead_slices = time_slices(
        ReplayWindow {
            from_ms: clip_from(window.from_ms),
            to_ms: focus_from - 1,
            ..window
        },
        Some(span),
    );
    let trail_slices = time_slices(
        ReplayWindow {
            from_ms: clip_from(focus_to + 1),
            to_ms: window.to_ms,
            ..window
        },
        Some(span),
    );

    let focus_len = focus_slices.len();
    let mut rest: Vec<(i64, i64)> = lead_slices.into_iter().chain(trail_slices).collect();
    // Ascending distance first; `from_ms` breaks the tie between the one lead tile and the one
    // trail tile that can sit exactly as close on either side of the focus.
    rest.sort_by_key(|&(from, to)| {
        let distance = if to < focus_from {
            focus_from - to
        } else {
            from - focus_to
        };
        (distance, from)
    });

    let mut slices = focus_slices;
    slices.extend(rest);
    TickPlan { slices, focus_len }
}

/// Thin a tick run down to a render/remember budget, coarsening only as far as needed.
///
/// Walks [`THIN_LADDER_MS`] in order and takes the FIRST bucket width whose thinned output fits
/// `budget` via [`super::candles::thin_ticks`], so a run that already fits pays no thinning at
/// all. A position held long enough makes even the coarsest rung's 300-second buckets outnumber
/// the budget — `THIN_LADDER_MS`'s terminal rung is a RATE, not a ceiling — so past it a final
/// uniform stride picks `budget` points evenly spaced across the coarsest rung's output,
/// including its first and last tick. Either path keeps every point a REAL tick, never a
/// synthesised one.
///
/// **Contract:** `result.len() <= budget`, unconditionally — this is the one property every
/// caller relies on ([`worker::TICK_BUDGET`] bounds both the composed series and the GPU point
/// ring), not a best effort.
///
/// Args:
///     ticks: Ascending by time (the caller sorts).
///     budget: Largest tick count the caller will draw or remember; `0` always returns nothing.
///
/// Returns:
///     `(ticks, 0)` unchanged when `ticks.len() <= budget`; otherwise `(thinned, bucket_ms)`,
///     `bucket_ms` being the ladder rung that produced it — the coarsest rung when the final
///     stride also had to run.
pub(crate) fn fit_ticks(ticks: Vec<Tick>, budget: usize) -> (Vec<Tick>, i64) {
    if ticks.len() <= budget {
        return (ticks, 0);
    }
    if budget == 0 {
        return (
            Vec::new(),
            *THIN_LADDER_MS.last().expect("non-empty ladder"),
        );
    }
    let mut out = Vec::new();
    let mut bucket_ms = 0;
    for &rung in THIN_LADDER_MS.iter() {
        bucket_ms = rung;
        crate::market::candles::thin_ticks(&ticks, rung, &mut out);
        if out.len() <= budget {
            return (out, bucket_ms);
        }
    }
    // The coarsest rung still overflows. A stride of `ceil(last_idx / (budget - 1))`, walked from
    // index 0 and always closed off by the true last index, is what keeps the bound UNCONDITIONAL:
    // re-walking forward in fixed `ceil(len / budget)` steps and appending the last tick
    // afterwards — the naive reading — can land `budget + 1` points whenever the true last index
    // is not itself a multiple of that step (e.g. 10 points into a budget of 3: steps of 4 land
    // 0/4/8, none of which is index 9, so appending it makes four).
    let last_idx = out.len() - 1;
    if budget == 1 {
        return (vec![out[last_idx]], bucket_ms);
    }
    let stride = (last_idx as f64 / (budget - 1) as f64).ceil() as usize;
    let mut strided = Vec::with_capacity(budget);
    let mut i = 0usize;
    while i < last_idx {
        strided.push(out[i]);
        i += stride;
    }
    strided.push(out[last_idx]);
    (strided, bucket_ms)
}

/// Whether cached rows already cover a window densely enough to skip the network.
///
/// COVERAGE, not presence, is the question. A partial prefix is exactly what a previously
/// interrupted fetch leaves behind, and treating it as a hit would pin a half-drawn trade forever.
/// A market can legitimately have no trades in a given minute, so a gap is tolerated up to
/// `max_gap_bars` bars; anything wider is a hole rather than a quiet market.
///
/// Args:
///     rows: Cached bars, in any order.
///     window: The window that must be covered.
///     bar_ms: Milliseconds per bar.
///     max_gap_bars: Largest run of missing bars still counted as covered.
///
/// Returns:
///     `true` when the rows span the window with no gap wider than the allowance.
pub fn cache_covers(
    rows: &[ChartCandle],
    window: ReplayWindow,
    bar_ms: i64,
    max_gap_bars: i64,
) -> bool {
    if rows.is_empty() || bar_ms <= 0 {
        return false;
    }
    let mut opens: Vec<i64> = rows
        .iter()
        .filter(|c| c.t_open_ms.is_finite())
        .map(|c| c.t_open_ms as i64)
        .filter(|t| *t >= window.from_ms - bar_ms && *t <= window.to_ms)
        .collect();
    if opens.is_empty() {
        return false;
    }
    opens.sort_unstable();
    let allowance = bar_ms.saturating_mul(max_gap_bars.max(0) + 1);
    // Both edges must be reached, or the series would start late or end early and the entry or the
    // exit arrow would sit outside the drawn rows.
    if opens[0] > window.from_ms + allowance {
        return false;
    }
    if *opens.last().expect("checked non-empty") + allowance < window.to_ms {
        return false;
    }
    opens.windows(2).all(|w| w[1] - w[0] <= allowance)
}

/// Stable identity of one replay, so a re-read of the same series is recognised as unchanged.
///
/// The chart asks for a series every frame and ships the revision it already holds; answering with
/// a CONSTANT would make a fresh pane — which arrives with a zero revision, and after a device-loss
/// reset with `u64::MAX` — be told "nothing changed" and draw nothing at all. Answering with
/// something that moves every frame would re-upload the whole candle layer at frame rate. So the
/// revision is a hash of the ASK, bucketed to the timeframe grid: it survives sub-bar camera
/// jitter and changes the moment the window or the timeframe genuinely does.
///
/// Args:
///     identity: Stable per-series discriminator, so two replays never share a revision.
///     tf_ms: Timeframe of the requested series in milliseconds.
///     from_bucket: Left edge of the ask, floored to the timeframe grid.
///     to_bucket: Right edge of the ask, floored to the timeframe grid.
///
/// Returns:
///     A non-zero revision; zero is reserved for "never served".
pub fn replay_revision(identity: u64, tf_ms: i64, from_bucket: i64, to_bucket: i64) -> u64 {
    // FNV-1a: a few instructions on the frame path, and mixing the four inputs is all that is
    // required of it. Nothing here is adversarial, so collision resistance is not the property
    // being bought.
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for value in [identity, tf_ms as u64, from_bucket as u64, to_bucket as u64] {
        for byte in value.to_le_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(PRIME);
        }
    }
    hash.max(1)
}

/// Per-source salt for [`replay_revision`], so a tick series and its sibling candle series of the
/// same identity, timeframe and window never share a revision.
///
/// The bug this exists to prevent: [`TradeReplaySeries::read_into`] derives `revision` from
/// `(identity, tf_ms, from_bucket, to_bucket)`, and a tick upgrade shares every one of those four
/// with the kline series it replaces — same `identity` ([`super::worker`] never changes it
/// between the two outcomes), same window, same `tf_ms == 60_000`. Unsalted, the upgrade's
/// revision would equal the one the pane already shipped, `read_into`'s `candles_changed` would
/// stay `false`, and the pane would keep drawing exchange klines forever under the new tick
/// points.
///
/// Args:
///     source: Which kind of series is being read.
///
/// Returns:
///     `0` for [`TradeReplaySource::Klines1m`], so every existing revision stays bit-identical to
///     today's; a fixed non-zero constant for [`TradeReplaySource::Ticks`].
pub fn tick_identity_salt(source: TradeReplaySource) -> u64 {
    match source {
        TradeReplaySource::Klines1m => 0,
        TradeReplaySource::Ticks => 0x9E37_79B9_7F4A_7C15,
    }
}

/// One trade's frozen market history, ready to be drawn.
#[derive(Clone, Debug)]
pub struct TradeReplaySeries {
    /// Which of the two kinds of data this actually carries.
    pub source: TradeReplaySource,
    /// The venue the rows came from, so the window can caption them.
    pub venue: Venue,
    /// The window the rows cover.
    pub window: ReplayWindow,
    /// Timeframe of [`Self::candles`] in milliseconds; one minute for every current route.
    pub tf_ms: i64,
    /// Bars in ascending open time. A [`TradeReplaySource::Klines1m`] series carries these alone;
    /// a [`TradeReplaySource::Ticks`] series carries these TOO — the EXCHANGE's own one-minute
    /// klines, not bars aggregated from [`Self::ticks`], so the bar layer covers the WHOLE window
    /// even where the points, per [`Self::partial`], do not.
    ///
    /// Stored whole; DRAWN only where the points are not. [`Self::read_into`] withholds every bar
    /// lying wholly inside the span [`Self::ticks`] covers, so the two layers never overlay each
    /// other and the bars are left holding exactly the edges the points never reached.
    pub candles: Vec<ChartCandle>,
    /// Trade points in ascending time. Empty when [`Self::source`] is
    /// [`TradeReplaySource::Klines1m`]; carried alongside [`Self::candles`] for
    /// [`TradeReplaySource::Ticks`] — never in place of them, and per [`Self::partial`] possibly
    /// covering only part of [`Self::window`] while the bars cover all of it.
    pub ticks: Vec<Tick>,
    /// Stable discriminator feeding [`replay_revision`], so two open windows never collide.
    pub identity: u64,
    /// How the tick attempt for this window ended. `Served` on a [`TradeReplaySource::Ticks`]
    /// series; every other variant is a reason the bar layer is all the window has, and the
    /// window PRINTS it.
    pub tick_status: TickStatus,
    /// Bucket the points were thinned to, in ms; `0` means raw, untouched ticks. Meaningless (and
    /// always `0`) on a [`TradeReplaySource::Klines1m`] series.
    pub bucket_ms: i64,
    /// Whether [`Self::ticks`] covers only PART of [`Self::window`] — the bars always cover all
    /// of it. Always `false` on a [`TradeReplaySource::Klines1m`] series.
    pub partial: bool,
    /// The inclusive span [`Self::ticks`] is guaranteed EXHAUSTIVE over, or `None` when there was
    /// no tick walk at all ([`TradeReplaySource::Klines1m`]).
    ///
    /// Carried straight from `worker::TickHarvest::covered`, the walk's own answer, and NOT
    /// re-derived from the rows: clipping proves every row is inside the span, never that the
    /// first and last rows ARE its edges. A completed boundary slice whose opening minute simply
    /// saw no trade is exhaustively covered while carrying no point there, and only this field
    /// knows it — which is what lets [`Self::read_into`] withhold that minute's bar instead of
    /// leaving one stray candle floating inside the tick trace.
    ///
    /// [`Self::partial`] is the BOOLEAN read of this same span against [`Self::window`]; this is
    /// the span itself.
    pub covered: Option<(i64, i64)>,
}

impl TradeReplaySeries {
    /// Whether this series carries no row at all.
    ///
    /// Returns:
    ///     `true` when there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.candles.is_empty() && self.ticks.is_empty()
    }

    /// Serve one chart read from the frozen series.
    ///
    /// This is the replacement for `MarketDataSource::read_chart_history_into` on a pane that owns
    /// a replay, and it answers the same protocol. Four fields of the answer are load-bearing in
    /// ways that are invisible from the signature, so each is set deliberately:
    ///
    /// - `combo_reset` is ALWAYS true. The caller stamps its `resident_left_rel` coverage mark
    ///   only inside its own combo-reset branch; left unstamped it stays NaN, the caller reads
    ///   that as "coverage unknown", and a full history re-read is forced on every single frame.
    ///   Frozen data has no live edge, so there is no incremental drain that an unconditional
    ///   reset could damage.
    /// - `tick_price_range` is never left empty. The chart's automatic Y fit is built from the
    ///   TICK range alone — candles do not feed it — so a bars-only replay must synthesise the
    ///   range from bar lows and highs. Omitting it collapses the scale onto the last price and
    ///   puts the whole series off screen, which reads as a broken window.
    /// - `combo_capacity` is never zero. The caller sizes its GPU point ring from it, and a zero
    ///   leaves the points nowhere to land.
    /// - `candles_changed` is gated on the caller's own shipped revision, or the entire bar layer
    ///   is re-uploaded every frame.
    ///
    /// Args:
    ///     epoch_ms: The pane's time origin.
    ///     from_rel_ms: Left edge of the ask, relative to the epoch.
    ///     to_rel_ms: Right edge of the ask, relative to the epoch.
    ///     candle_params: The caller's bar request, or `None` while bars are switched off.
    ///     out: Buffers to fill.
    ///
    /// Returns:
    ///     The read answer, in the same protocol the live path uses.
    pub fn read_into(
        &self,
        epoch_ms: f64,
        from_rel_ms: f32,
        to_rel_ms: f32,
        candle_params: Option<&CandleReadParams>,
        out: &mut ChartHistoryBuffers,
    ) -> ChartHistoryRead {
        // The live path clears these before every read, and the caller relies on that: this read
        // re-emits the whole window rather than draining a live edge, so extending without
        // clearing would duplicate every point on the second frame and leave rows from a previous
        // window behind when the ask moves. Clearing FIRST is also what makes an early return
        // below mean "nothing to draw" instead of "whatever was there last time".
        out.ticks.clear();
        out.liquidations.clear();
        out.last_points.clear();
        out.mark_points.clear();
        out.candles.clear();
        out.candle_tf_ms.clear();
        let mut read = ChartHistoryRead {
            caught_up: true,
            ..ChartHistoryRead::default()
        };
        // A non-finite bound converts to a saturated or zero timestamp and would silently ask for
        // the wrong window; there is nothing sensible to draw for one.
        if !epoch_ms.is_finite() || !from_rel_ms.is_finite() || !to_rel_ms.is_finite() {
            return read;
        }
        let from_ms = (epoch_ms + f64::from(from_rel_ms)).round() as i64;
        let to_ms = ((epoch_ms + f64::from(to_rel_ms.max(from_rel_ms))).round() as i64)
            .max(from_ms.saturating_add(1));
        let tf_ms = candle_params.map_or(self.tf_ms, |p| p.tf_ms).max(1);
        // Salted by source: a tick series and its sibling candle series otherwise share
        // `(identity, tf_ms, from_bucket, to_bucket)` bit-for-bit (§4 of the tick-replay plan),
        // so the tick upgrade's revision would equal the one the pane already shipped and
        // `candles_changed` below would stay false forever. See `tick_identity_salt`.
        let salted_identity = self.identity ^ tick_identity_salt(self.source);
        let revision = replay_revision(
            salted_identity,
            tf_ms,
            from_ms.div_euclid(tf_ms),
            to_ms.div_euclid(tf_ms),
        );
        read.revision = revision;
        read.candles_revision = revision;

        // Points first: they are re-emitted whole on every read, because a frozen series has no
        // live edge to drain incrementally and the whole window is bounded — a few hundred rows
        // for a candle-only series, or up to `worker::TICK_BUDGET` for a tick one.
        out.ticks.extend(
            self.ticks
                .iter()
                .filter(|t| {
                    t.time_ms.is_finite()
                        && t.price.is_finite()
                        && t.price > 0.0
                        && (t.time_ms as i64) >= from_ms
                        && (t.time_ms as i64) <= to_ms
                })
                .copied(),
        );
        read.combo_left_rel_ms = out.ticks.first().map(|t| (t.time_ms - epoch_ms) as f32);
        read.combo_capacity = out.ticks.len().max(1);
        read.combo_reset = true;
        read.tick_price_range = price_range_of_ticks(&out.ticks);
        read.last_price = out.ticks.last().map(|t| t.price);

        // Bars, only when the caller both wants them and does not already hold this exact series.
        if let Some(params) = candle_params {
            if params.shipped_revision != revision {
                let clipped: Vec<ChartCandle> = self
                    .candles
                    .iter()
                    .filter(|c| {
                        c.t_open_ms.is_finite()
                            && (c.t_open_ms as i64) >= from_ms.saturating_sub(tf_ms)
                            && (c.t_open_ms as i64) <= to_ms
                    })
                    .copied()
                    .collect();
                // The rows are fetched at one minute, but the caller draws them at the timeframe
                // IT asked for and sets no per-candle width. Shipping raw minutes under a coarser
                // request draws every body at the wider timeframe, so adjacent bars overlap and
                // the series reads as corrupt rather than as wrong. Aggregating is the same
                // answer the live path gives, through the same helper.
                match tf_ms > self.tf_ms {
                    true => crate::market::candles::resample(&clipped, tf_ms, &mut out.candles),
                    false => out.candles.extend(clipped),
                }
                // WHERE THE POINTS ARE, THE BARS STEP ASIDE. A tick series carries the exchange's
                // own one-minute klines for the WHOLE window (see `Self::candles`), which is what
                // keeps the edges the points never reached drawn — but inside the covered span
                // the two layers are the same trades told twice, drawn on top of each other.
                //
                // Decided from the WALK's own interval, never from pixels and never from the rows:
                // `Self::covered` is what the tick stage proved exhaustive, while the extrema of
                // the points are merely a subset of it — a covered minute the venue happened to
                // publish no trade in would keep its bar under a row-derived rule and read as a
                // stray candle floating inside the trace. `None` there is a `Klines1m` series,
                // which is what keeps a still-loading window whole: the bar-only stage walked no
                // ticks, so nothing is hidden until the upgrade lands.
                //
                // Applied AFTER the aggregation above so one rule covers both paths, and to the
                // OUTPUT timeframe, which is the width the caller actually draws. `candle_tf_ms`
                // is never filled on this path, so there is no parallel array to desync.
                if let Some(covered) = self.covered {
                    out.candles
                        .retain(|c| !bar_inside(c.t_open_ms, tf_ms, covered));
                }
                read.candles_changed = true;
            }
        }
        // The Y fit reads the tick range and nothing else, so a bars-only replay has to answer
        // with the range those bars actually span — and it has to answer even on a repeat read
        // whose bars were suppressed above, or the scale collapses the moment the series is
        // recognised as already shipped.
        if read.tick_price_range.is_none() {
            read.tick_price_range = price_range_of_candles(&self.candles, from_ms, to_ms);
        }
        if read.last_price.is_none() {
            read.last_price = self
                .candles
                .iter()
                .rfind(|c| c.t_open_ms.is_finite() && (c.t_open_ms as i64) <= to_ms)
                .map(|c| c.close);
        }
        read
    }
}

/// Whether one bar lies WHOLLY inside a covered span.
///
/// A bar that STRADDLES an edge stays drawn: half of it is over ground the points never reached,
/// so it is context rather than an overlay, and dropping it would leave a gap the user reads as
/// missing data. That is also what makes the window's own caption honest — the edges really are
/// the part still closed by candles.
///
/// Args:
///     t_open_ms: The bar's opening stamp; a non-finite one is never inside anything.
///     tf_ms: The bar's width, at the timeframe it is DRAWN at.
///     covered: Inclusive span from [`TradeReplaySeries::covered`].
///
/// Returns:
///     `true` when the whole bar sits inside the span.
fn bar_inside(t_open_ms: f64, tf_ms: i64, covered: (i64, i64)) -> bool {
    if !t_open_ms.is_finite() {
        return false;
    }
    let open = t_open_ms as i64;
    open >= covered.0 && open.saturating_add(tf_ms.max(1)) - 1 <= covered.1
}

/// Lowest and highest finite positive price across a run of trade points.
///
/// Args:
///     ticks: Points already clipped to the window.
///
/// Returns:
///     `(low, high)`, or `None` when no point carries a usable price.
fn price_range_of_ticks(ticks: &[Tick]) -> Option<(f32, f32)> {
    ticks
        .iter()
        .filter(|t| t.price.is_finite() && t.price > 0.0)
        .fold(None, |acc: Option<(f32, f32)>, t| {
            Some(match acc {
                None => (t.price, t.price),
                Some((lo, hi)) => (lo.min(t.price), hi.max(t.price)),
            })
        })
}

/// Lowest low and highest high across the bars inside a window.
///
/// Args:
///     candles: The whole series; bars outside the window are ignored here rather than by the
///         caller, so a repeat read whose bars were suppressed still gets a range.
///     from_ms: Left edge of the ask.
///     to_ms: Right edge of the ask.
///
/// Returns:
///     `(low, high)`, or `None` when no bar inside the window carries usable prices.
fn price_range_of_candles(candles: &[ChartCandle], from_ms: i64, to_ms: i64) -> Option<(f32, f32)> {
    candles
        .iter()
        .filter(|c| {
            c.t_open_ms.is_finite()
                && (c.t_open_ms as i64) >= from_ms
                && (c.t_open_ms as i64) <= to_ms
                && c.low.is_finite()
                && c.high.is_finite()
                && c.high > 0.0
        })
        .fold(None, |acc: Option<(f32, f32)>, c| {
            Some(match acc {
                None => (c.low, c.high),
                Some((lo, hi)) => (lo.min(c.low), hi.max(c.high)),
            })
        })
}

#[cfg(test)]
mod tests;
