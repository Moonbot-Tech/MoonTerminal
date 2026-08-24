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
    // `startTime`; pull it forward instead of asking for it.
    if from_ms < 0 {
        to_ms = to_ms.saturating_add(-from_ms);
        from_ms = 0;
    }
    Some(ReplayWindow {
        from_ms,
        to_ms,
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
    /// Bars in ascending open time; empty when [`Self::source`] is [`TradeReplaySource::Ticks`].
    pub candles: Vec<ChartCandle>,
    /// Trade points in ascending time; empty when the source is bars.
    pub ticks: Vec<Tick>,
    /// Stable discriminator feeding [`replay_revision`], so two open windows never collide.
    pub identity: u64,
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
        let revision = replay_revision(
            self.identity,
            tf_ms,
            from_ms.div_euclid(tf_ms),
            to_ms.div_euclid(tf_ms),
        );
        read.revision = revision;
        read.candles_revision = revision;

        // Points first: they are re-emitted whole on every read, because a frozen series has no
        // live edge to drain incrementally and the whole window is a few hundred rows at most.
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
