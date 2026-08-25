//! How much was BOUGHT and how much was SOLD over a period, from the retained history.
//!
//! The chart prints this as a block — a period, a buying figure, a selling figure — and the reader
//! changes the period from the chart itself, so the period is not one of the fixed windows the
//! other captions use: it can be any number of minutes, or a number of TRADES, which is not a
//! period at all.
//!
//! # Where the numbers come from, cheapest first
//!
//! Three sources, and which one answers depends on the span:
//!
//! 1. **MoonProto's own rolling buckets** ([`RollingTradeVolumeSnapshot`]) — the 1, 3 and 5-minute
//!    figures, already accumulated incrementally in five-second buckets on the protocol's side. A
//!    read is a struct copy: no scan at all.
//! 2. **The retained MINI-CANDLES** — five-second aggregates carrying `buy_vol`/`sell_vol`, which
//!    is the only long-horizon source that keeps the two sides apart at all (the 5-minute candle
//!    ring carries a total and no split). Thirty minutes is at most 360 of them.
//! 3. **The raw trade ring** — the live tail, and the only source that can answer "the last N
//!    trades" or state an amount in the BASE coin, since a mini-candle carries `price × quantity`
//!    and no quantity of its own.
//!
//! Two and three are complementary by construction rather than by luck: a mini-candle is built from
//! trades EVICTED from the ring (`compact_evicted_futures`), so a busy market whose ring covers ten
//! minutes has mini-candles behind them, while a quiet one whose ring covers a day has few
//! mini-candles and does not need them. The two therefore do not overlap, and this module reads the
//! trades for the tail and the mini-candles strictly BEFORE the oldest trade it saw.
//!
//! # What is never guessed
//!
//! [`VolumeSpanReadout::complete`] states whether the retained history actually reached back as far
//! as the span asked, and [`VolumeSpanReadout::base_exact`] whether a coin-denominated figure came
//! from rows that carry a quantity. Neither is patched over with an estimate: a chart that says
//! `Bv 12.7k` over half the period it names is worse than one that says the period was not covered.
//!
//! # Cost
//!
//! Keyed by the PROVIDER, not the consumer core — cores on one exchange share retained history —
//! and cached for [`SPAN_TTL_MS`], so a stack of panes on one coin pays for one read. Same shape as
//! [`super::arb`] beside it, and for the same reason.

use std::collections::HashMap;

use moonproto::MoonTime;
use moonproto::state::{MiniCandle, TradeHistoryRow};

use crate::config::{LabelSpan, LabelWindow};
use crate::session::CoreId;
use crate::util::time::now_unix_ms_i64;

use super::MarketDataSource;

mod liq;
mod track;

pub use liq::LiqSpanReadout;

use track::{MarketTrack, TRACK_SPAN_MS};

/// How long one market's figures are reused before the history is read again.
///
/// Matched to the chart's own read period, like the arbitrage book's: the panes ask on that clock,
/// so one tick serves the whole stack and a second pane on the same coin costs nothing. A volume
/// figure is read by eye and the reference terminal repaints it no faster.
const SPAN_TTL_MS: i64 = 250;

/// How far past the period's start the oldest retained row may sit and still count as covering it.
///
/// The aggregates are five seconds wide, so the first one inside a period routinely begins a moment
/// after it does. Judging coverage to the millisecond would mark almost every long period
/// incomplete for a gap nobody can see.
const BUCKET_SLACK_MS: i64 = 5_000;

/// How long a figure that had to be ACCUMULATED from retained aggregates is reused.
///
/// Periods past the track's own hour cannot be answered incrementally — nothing keeps three days of
/// five-second buckets — so they are walked. Nobody watches a three-day volume tick, and pricing
/// that walk at four times a second is what turns a chart drag into a series of hitches.
const DEEP_TTL_MS: i64 = 5_000;

/// Longest a cached entry is kept once nothing reads it.
///
/// Without this the map grows by one entry per `(coin, span)` ever charted in the session and never
/// shrinks — a slow leak that looks like a cache. An entry past this would be rebuilt on its next
/// read anyway.
const SPAN_KEEP_MS: i64 = SPAN_TTL_MS * 40;

/// How many rows one drain of a windowed read takes at a time.
///
/// The overshoot a bounded window pays: the drain stops at the far end, but only after the chunk
/// carrying it. Large enough that an ordinary window is one or two calls, small enough that the
/// overshoot is not the read.
const WINDOW_CHUNK: usize = 4_096;

/// The period a volume figure covers.
///
/// Normalized: the caption's own model knows fixed windows, custom minutes and trade counts, and
/// all of that reduces to "this many milliseconds" or "this many trades" before it reaches the
/// history. Keeping the configuration's own spelling out of this layer is what lets the readout be
/// cached under a key that is `Copy` and `Hash`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VolumeSpan {
    /// The last this many milliseconds.
    Millis(i64),
    /// The last this many trades, however long they took.
    Trades(u32),
}

impl VolumeSpan {
    /// The span one configured caption asks for.
    ///
    /// The translation lives HERE rather than in the caption layer because it is the only place
    /// that knows what the history can be asked for: a fixed window and a custom minute count are
    /// both just a length once they reach a ring, and only a trade count is a different question.
    ///
    /// Args:
    ///     span: The caption's own custom span, or [`LabelSpan::Window`] to follow its window.
    ///     window: The caption's window, used when the span defers to it.
    pub fn from_label(span: LabelSpan, window: LabelWindow) -> Self {
        match span {
            LabelSpan::Window => VolumeSpan::Millis(window.millis()),
            LabelSpan::Seconds(n) => VolumeSpan::Millis(i64::from(n) * 1_000),
            LabelSpan::Minutes(n) => VolumeSpan::Millis(i64::from(n) * 60_000),
            LabelSpan::Trades(n) => VolumeSpan::Trades(n),
        }
    }

    /// Whether this span asks for something at all.
    ///
    /// A zero span is not a question the history can answer, and the caller's own `sanitize` has
    /// already repaired the configured value — this guards the hand-built one.
    fn is_useful(self) -> bool {
        match self {
            VolumeSpan::Millis(ms) => ms > 0,
            VolumeSpan::Trades(n) => n > 0,
        }
    }
}

/// WHERE the span sits on the time axis.
///
/// `Now` is the live edge, which is what a caption watching the market wants. `Around` is the
/// measuring anchor: the same period CENTRED on a moment the reader pointed at, which answers what
/// surrounded that moment rather than what is happening.
///
/// Part of the cache key, so the caller quantizes it before asking — a pointer moves with the mouse,
/// and a key that followed it exactly would miss on every pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VolumeAt {
    Now,
    /// Centre of the window, in unix milliseconds.
    Around(i64),
}

impl VolumeAt {
    /// The window this anchor and span describe, as `[from, to]` in unix milliseconds.
    ///
    /// `None` for a TRADE COUNT read around a point: the retained rings are read forward from a
    /// cursor, so "the N trades before this moment" is not a question they answer cheaply, and
    /// answering a different one silently is worse than printing nothing.
    fn bounds(self, span: VolumeSpan, now: i64) -> Option<(i64, i64)> {
        match (self, span) {
            (VolumeAt::Now, VolumeSpan::Millis(ms)) => Some((now.saturating_sub(ms), now)),
            (VolumeAt::Around(at), VolumeSpan::Millis(ms)) => {
                let half = ms / 2;
                Some((at.saturating_sub(half), at.saturating_add(ms - half)))
            }
            (_, VolumeSpan::Trades(_)) => None,
        }
    }
}

/// What was traded over one span, split by side.
///
/// Amounts rather than shares: the caption decides whether to print `Bv`, `Sv`, their sum or the
/// buying share, and deriving all four from one value is what keeps `Bv + Sv` equal to `Vol` on
/// every window. Zero is a legitimate answer — a coin that did not trade — and is told apart from
/// "no history" by the readout being absent altogether.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VolumeSpanReadout {
    /// Bought and sold, in the market's quote currency.
    pub buy_quote: f64,
    pub sell_quote: f64,
    /// The same, in the base coin. Meaningful only while [`Self::base_exact`] holds.
    pub buy_base: f64,
    pub sell_base: f64,
    /// How many trades printed over the span.
    pub trades: u32,
    /// Whether the retained history reached as far back as the span asked.
    ///
    /// `false` says the figures cover only part of the period — a market subscribed minutes ago, or
    /// a span longer than anything retained. The caption marks it rather than hiding it: a partial
    /// figure is still the answer to "is anybody trading right now", as long as it does not pretend
    /// to be whole.
    pub complete: bool,
    /// Whether the base-coin amounts came from rows that carry a quantity.
    ///
    /// `false` when a mini-candle contributed: those hold `price × quantity` only, so the coin
    /// amount over that stretch is unknown rather than approximate.
    pub base_exact: bool,
    /// The period's WHOLE traded value as the 5-minute candle ring states it, when it covers the
    /// period and the split-carrying sources do not.
    ///
    /// The one figure that survives past the mini-candles' reach: the candle ring holds a day and
    /// more, and it is where `Vol 24ч` came from before the sides existed. It carries no split at
    /// all, which is why it answers only the TOTAL — the sides over such a period stay marked
    /// incomplete rather than being back-filled from a number that cannot be halved.
    pub total_quote_candles: Option<f64>,
}

impl VolumeSpanReadout {
    /// Everything traded over the span, both sides together, in the quote currency.
    ///
    /// The two halves, always: this is what keeps `Bv + Sv` equal to what a `Vol` caption prints
    /// beside them. See [`Self::total_quote_stated`] for the figure a caption should print, which
    /// prefers the candle ring where the halves could not cover the period.
    pub fn total_quote(self) -> f64 {
        self.buy_quote + self.sell_quote
    }

    /// The total a caption prints, and whether it is whole.
    ///
    /// Prefers the candle ring over a partial sum of the sides: a day's volume is a figure the
    /// terminal HAS, and printing a marked fraction of it instead would be a worse answer to the
    /// same question. The sides keep their own mark either way — they really are incomplete there.
    pub fn total_quote_stated(self) -> (f64, bool) {
        match self.total_quote_candles {
            Some(total) if !self.complete => (total, true),
            _ => (self.total_quote(), self.complete),
        }
    }

    /// The same in the base coin.
    pub fn total_base(self) -> f64 {
        self.buy_base + self.sell_base
    }

    /// Share of the traded value that was BUYING, in percent, or `None` when nothing traded.
    pub fn buy_share_pct(self) -> Option<f64> {
        let total = self.total_quote();
        (total > 0.0).then(|| self.buy_quote / total * 100.0)
    }

    /// Add one trade row.
    fn add_trade(&mut self, row: TradeHistoryRow) {
        let qty = f64::from(row.quantity());
        let value = f64::from(row.price) * qty;
        if row.is_buy() {
            self.buy_quote += value;
            self.buy_base += qty;
        } else {
            self.sell_quote += value;
            self.sell_base += qty;
        }
        self.trades = self.trades.saturating_add(1);
    }

    /// Add one five-second aggregate.
    ///
    /// It carries no quantity, so the base amounts stay where they were and the figure is marked
    /// inexact — the caller decides whether that matters, because it only does when the caption is
    /// printing coins.
    fn add_mini(&mut self, candle: MiniCandle) {
        self.buy_quote += f64::from(candle.buy_vol);
        self.sell_quote += f64::from(candle.sell_vol);
        self.trades = self.trades.saturating_add(candle.cnt.max(0) as u32);
        self.base_exact = false;
    }
}

/// How long one span's answer is reused: a tracked period on the chart's own clock, a walked one on
/// a slower one.
fn span_ttl(span: VolumeSpan, at: VolumeAt) -> i64 {
    match (span, at) {
        // A window centred on a fixed moment stops changing once the market has moved past it — only
        // its own edge is still filling. Re-reading it on the live clock would pay for an answer
        // that cannot differ.
        (_, VolumeAt::Around(_)) => DEEP_TTL_MS,
        (VolumeSpan::Millis(ms), _) if ms > TRACK_SPAN_MS => DEEP_TTL_MS,
        _ => SPAN_TTL_MS,
    }
}

/// One market's liquidation figures for one span, and when they were read.
struct LiqEntry {
    read_ms: i64,
    readout: liq::LiqSpanReadout,
}

/// One market's figures for one span, and when they were read.
struct SpanEntry {
    read_ms: i64,
    readout: VolumeSpanReadout,
}

/// The read-once-per-`(market, span)` cache behind [`MarketDataSource::market_volume_span`].
#[derive(Default)]
pub(super) struct VolumeBook {
    /// Keyed by the PROVIDER and the span; the inner map by market name, so a lookup borrows the
    /// name instead of allocating one.
    spans: HashMap<(CoreId, VolumeSpan, VolumeAt), HashMap<String, SpanEntry>>,
    /// Liquidation figures by `(provider, span, anchor)` then market, mirroring `spans`.
    ///
    /// Its own map rather than a field on the traded readout: a block printing volume alone must not
    /// order the liquidation read, and the two are gated separately for exactly that reason.
    liqs: HashMap<(CoreId, VolumeSpan, VolumeAt), HashMap<String, LiqEntry>>,
    /// Five-second buckets per market, advanced by what arrived since the last read.
    ///
    /// One track serves every period a chart asks for — the buckets are summed per request — so a
    /// module printing a minute beside another printing the quarter hour is still one accumulator.
    tracks: HashMap<(CoreId, String), MarketTrack>,
}

impl VolumeBook {
    /// Drop entries nothing will read again; see [`SPAN_KEEP_MS`].
    ///
    fn prune(&mut self, now: i64) {
        for markets in self.spans.values_mut() {
            markets.retain(|_, entry| now.saturating_sub(entry.read_ms) < SPAN_KEEP_MS);
        }
        self.spans.retain(|_, markets| !markets.is_empty());
        for markets in self.liqs.values_mut() {
            markets.retain(|_, entry| now.saturating_sub(entry.read_ms) < SPAN_KEEP_MS);
        }
        self.liqs.retain(|_, markets| !markets.is_empty());
        // A track is some forty kilobytes, so a terminal that walks a hundred coins an hour must
        // not keep one per coin for the session.
        self.tracks
            .retain(|_, track| now.saturating_sub(track.used_ms) < SPAN_KEEP_MS);
    }

    /// Forget everything a core answered.
    ///
    /// Called when its client is replaced or removed: the figures describe retained history that
    /// went away with it.
    pub(super) fn forget_core(&mut self, core: CoreId) {
        self.spans.retain(|(provider, _, _), _| *provider != core);
        self.liqs.retain(|(provider, _, _), _| *provider != core);
        // The buckets describe a stream this client no longer has a cursor into: a new slot starts
        // its sequence again, and a stale cursor would fold the wrong rows in.
        self.tracks.retain(|(provider, _), _| *provider != core);
    }
}

impl MarketDataSource {
    /// Return what was bought and sold on one market over one span.
    ///
    /// Reads the cheapest source that can answer — see the module docs — and states whether the
    /// retained history actually covered the period.
    ///
    /// Args:
    ///     core: Consumer core whose pane is being captioned. Its PROVIDER owns the history, so
    ///         two cores on one exchange share both the read and its cache entry.
    ///     market: Data-key market name on that core.
    ///     span: Period or trade count to cover.
    ///
    /// Returns:
    ///     The figures, or `None` when the provider, its client, its snapshot or this market's
    ///     retained history is unavailable — which is NOT the same as a market that did not trade.
    pub fn market_volume_span(
        &self,
        core: CoreId,
        market: &str,
        span: VolumeSpan,
        at: VolumeAt,
    ) -> Option<VolumeSpanReadout> {
        if !span.is_useful() {
            return None;
        }
        let provider = self.provider_of(core)?;
        let now = now_unix_ms_i64();
        if let Some(hit) = self.volume_cached(provider, market, span, at, now) {
            return Some(hit);
        }
        let snapshot = self.core_client(provider)?.snapshot_versioned()?;
        let readers = snapshot.market_history_readers(market)?;
        // The futures ring first, then spot: the same order every other retained read in this crate
        // uses, so a market cannot be captioned from one ring and charted from the other.
        let trades = readers.futures_trades.or(readers.spot_trades);
        let readout = match (span, at) {
            (VolumeSpan::Trades(n), VolumeAt::Now) => read_trade_count(trades.as_ref(), n)?,
            // A trade COUNT around a point is not a question the rings answer; see `bounds`.
            (VolumeSpan::Trades(_), VolumeAt::Around(_)) => return None,
            (VolumeSpan::Millis(ms), at) => {
                let (from, to) = at.bounds(span, now)?;
                // The protocol's own buckets answer 1, 3 and 5 minutes for free — but only at the
                // LIVE EDGE, which is the only period they maintain.
                match rolling_totals(&snapshot, market, ms).filter(|_| at == VolumeAt::Now) {
                    Some(totals) => VolumeSpanReadout {
                        buy_quote: totals.buy_value,
                        sell_quote: totals.sell_value,
                        buy_base: totals.buy_qty,
                        sell_base: totals.sell_qty,
                        trades: totals.trade_count,
                        complete: true,
                        base_exact: true,
                        total_quote_candles: None,
                    },
                    // Everything inside the track's own hour is SUMMED from buckets it keeps up to
                    // date, so the cost is the bucket count and not the market's trade rate. Both
                    // ends have to fall inside it — a window centred on a pointer an hour back
                    // reaches further than the track holds.
                    None if from >= now - TRACK_SPAN_MS && from <= now => self.volume_tracked(
                        provider,
                        market,
                        trades.as_ref(),
                        readers.mini_candles.as_ref(),
                        (from, to),
                        now,
                    ),
                    // Past that, the retained aggregates are walked — on the slower clock below.
                    None => read_period(
                        trades.as_ref(),
                        readers.mini_candles.as_ref(),
                        from,
                        to,
                        &snapshot,
                        market,
                        ms,
                        at,
                        now,
                    )?,
                }
            }
        };
        self.volume_store(provider, market, span, at, now, readout);
        Some(readout)
    }

    /// A cached entry still inside its TTL.
    fn volume_cached(
        &self,
        provider: CoreId,
        market: &str,
        span: VolumeSpan,
        at: VolumeAt,
        now: i64,
    ) -> Option<VolumeSpanReadout> {
        let handle = self.volume_book();
        let book = handle.lock().ok()?;
        let entry = book.spans.get(&(provider, span, at))?.get(market)?;
        (now.saturating_sub(entry.read_ms) < span_ttl(span, at)).then_some(entry.readout)
    }

    /// Sum this market's buckets, bringing them up to date first.
    fn volume_tracked(
        &self,
        provider: CoreId,
        market: &str,
        trades: Option<&moonproto::state::SeqRingReader<TradeHistoryRow>>,
        minis: Option<&moonproto::state::SeqRingReader<MiniCandle>>,
        // The window as ONE argument: its two ends are never chosen apart, and splitting them put
        // this call past what a reader can hold at a glance.
        window: (i64, i64),
        now: i64,
    ) -> VolumeSpanReadout {
        let handle = self.volume_book();
        let Ok(mut book) = handle.lock() else {
            return VolumeSpanReadout::default();
        };
        let track = book
            .tracks
            .entry((provider, market.to_string()))
            .or_insert_with(|| MarketTrack::new(now));
        track.advance(trades, minis, now);
        track.read_range(window.0, window.1)
    }

    /// File a freshly read entry, pruning what nothing reads any more.
    fn volume_store(
        &self,
        provider: CoreId,
        market: &str,
        span: VolumeSpan,
        at: VolumeAt,
        now: i64,
        readout: VolumeSpanReadout,
    ) {
        let handle = self.volume_book();
        let Ok(mut book) = handle.lock() else {
            return;
        };
        book.prune(now);
        book.spans.entry((provider, span, at)).or_default().insert(
            market.to_string(),
            SpanEntry {
                read_ms: now,
                readout,
            },
        );
    }

    /// A cached liquidation entry still inside its TTL.
    pub(super) fn liq_cached(
        &self,
        provider: CoreId,
        market: &str,
        span: VolumeSpan,
        at: VolumeAt,
        now: i64,
    ) -> Option<liq::LiqSpanReadout> {
        let handle = self.volume_book();
        let book = handle.lock().ok()?;
        let entry = book.liqs.get(&(provider, span, at))?.get(market)?;
        (now.saturating_sub(entry.read_ms) < span_ttl(span, at)).then_some(entry.readout)
    }

    /// File a freshly read liquidation entry.
    pub(super) fn liq_store(
        &self,
        provider: CoreId,
        market: &str,
        span: VolumeSpan,
        at: VolumeAt,
        now: i64,
        readout: liq::LiqSpanReadout,
    ) {
        let handle = self.volume_book();
        let Ok(mut book) = handle.lock() else {
            return;
        };
        book.prune(now);
        book.liqs.entry((provider, span, at)).or_default().insert(
            market.to_string(),
            LiqEntry {
                read_ms: now,
                readout,
            },
        );
    }

    /// The shared book handle, taken without holding the source lock across the work behind it.
    fn volume_book(&self) -> std::sync::Arc<std::sync::Mutex<VolumeBook>> {
        self.inner
            .read()
            .expect("market source poisoned")
            .volume_book
            .clone()
    }
}

/// Copy exactly the rows of `[from, to)` out of a retained ring.
///
/// The obvious call — `copy_time_range_ms` — is bounded by the WINDOW in what it copies but not in
/// what it WALKS: its fold runs to the ring head, testing every row past the far end. On the pointer
/// path that is the whole retained tail per mouse move, under the ring's read lock.
///
/// So the window is drained in CHUNKS from its own start and stopped at its far end. The cost is the
/// rows inside the window plus at most one chunk, whatever the ring holds behind them. The sequence
/// arithmetic that would do it in one call is not available: the protocol keeps
/// `first_seq_at_or_after_time` and `copy_from_seq` behind its diagnostics feature.
///
/// Args:
///     reader: The ring to read.
///     from: Start of the window, unix milliseconds, inclusive.
///     to: End of the window, unix milliseconds, exclusive.
///     out: Reused buffer; cleared by the copy.
pub(super) fn copy_window<T>(
    reader: &moonproto::state::SeqRingReader<T>,
    from: i64,
    to: i64,
    out: &mut Vec<T>,
) where
    T: moonproto::state::SeqRingTimedRow,
{
    out.clear();
    if to <= from {
        return;
    }
    let mut cursor = reader.cursor_at_or_after_time(MoonTime::from_unix_millis(from));
    let mut chunk: Vec<T> = Vec::new();
    loop {
        chunk.clear();
        let meta = reader.drain_new_bounded(&mut cursor, WINDOW_CHUNK, &mut chunk);
        if meta.copied == 0 {
            return;
        }
        for row in &chunk {
            if row.seq_ring_time_ms() >= to {
                // Past the far end: rows are in sequence order, so nothing behind this one belongs
                // to the window either.
                return;
            }
            out.push(*row);
        }
        if meta.caught_up {
            return;
        }
    }
}

/// Sum the last `n` trades.
///
/// The one span the raw ring answers exactly and nothing else can: a count is not a period, so
/// neither the rolling buckets nor the mini-candles can be asked for it.
fn read_trade_count(
    trades: Option<&moonproto::state::SeqRingReader<TradeHistoryRow>>,
    n: u32,
) -> Option<VolumeSpanReadout> {
    let reader = trades?;
    let want = n as usize;
    let mut out = VolumeSpanReadout {
        base_exact: true,
        ..VolumeSpanReadout::default()
    };
    reader.with_last(want, |view| {
        view.for_each(|row| out.add_trade(*row));
    });
    // Fewer rows than asked for means the ring does not hold that many yet — the figures are real,
    // they just cover a shorter stretch than the caption names.
    out.complete = out.trades as usize >= want;
    Some(out)
}

/// Sum everything traded since `from_ms`.
///
/// Args:
///     trades: The market's raw trade ring, if it has one.
///     minis: Its five-second aggregates, if it has any.
///     from_ms: Start of the period, in unix milliseconds.
///     snapshot: Live snapshot, for the protocol's own rolling buckets.
///     market: Data-key market name, to address those buckets.
///     span_ms: Length of the period, which is what decides whether the buckets can serve it.
///
/// Returns:
///     The figures, or `None` when the market has no retained trade history at all.
#[allow(clippy::too_many_arguments)]
fn read_period(
    trades: Option<&moonproto::state::SeqRingReader<TradeHistoryRow>>,
    minis: Option<&moonproto::state::SeqRingReader<MiniCandle>>,
    from_ms: i64,
    to_ms: i64,
    snapshot: &moonproto::MoonStateSnapshot,
    market: &str,
    span_ms: i64,
    at: VolumeAt,
    now_ms: i64,
) -> Option<VolumeSpanReadout> {
    // AGGREGATES FIRST, raw rows only for the tail they do not cover. The other way round — walk the
    // trades over the whole period, then patch the front from mini-candles — is what this did, and
    // it is priced in the market's trade RATE: a busy coin keeps tens of thousands of rows inside
    // the ring, and the walk landed as a visible hitch while the chart was being dragged. A
    // mini-candle is a five-second aggregate, so the same period costs at most one row per five
    // seconds of it.
    let mut out = VolumeSpanReadout {
        base_exact: true,
        ..VolumeSpanReadout::default()
    };
    let mut earliest = i64::MAX;
    // Newest aggregate seen, which is where the raw tail begins. A mini-candle exists only because
    // its trades were EVICTED, so starting strictly after it cannot count a trade twice.
    let mut newest_mini = i64::MIN;
    if let Some(reader) = minis {
        let mut rows: Vec<MiniCandle> = Vec::new();
        copy_window(reader, from_ms, to_ms, &mut rows);
        for row in &rows {
            let at = row.time.unix_millis();
            out.add_mini(*row);
            earliest = earliest.min(at);
            newest_mini = newest_mini.max(at);
        }
    }
    // The live tail: the trades that have not been compacted yet.
    if let Some(reader) = trades {
        let tail_from = match newest_mini > i64::MIN {
            true => newest_mini + 1,
            false => from_ms,
        };
        let mut rows: Vec<TradeHistoryRow> = Vec::new();
        copy_window(reader, tail_from, to_ms, &mut rows);
        for row in &rows {
            out.add_trade(*row);
            earliest = earliest.min(row.time.unix_millis());
        }
    }
    if earliest == i64::MAX {
        // Neither ring held anything inside the period. That is not the same as a quiet market —
        // this market has no retained history to speak of at all.
        return None;
    }
    // Covered at BOTH ends. A window CENTRED on a point near the live edge reaches into the future,
    // and half of it has simply not happened yet — reporting that as whole prints half a period's
    // volume under a heading naming the whole one.
    out.complete = earliest <= from_ms + BUCKET_SLACK_MS && to_ms <= now_ms + BUCKET_SLACK_MS;
    // The 5-minute candle ring, which reaches a day and more where the split-carrying sources do
    // not. Read only when they fell short, and only for a period the ring states outright — which
    // is a period ending NOW. A window centred on a point in the past cannot borrow from it: those
    // totals are counted back from the live edge.
    if !out.complete && at == VolumeAt::Now {
        out.total_quote_candles = candle_total(snapshot, market, span_ms);
    }
    Some(out)
}

/// MoonProto's own rolling totals for this span, when it maintains one of exactly this length.
///
/// Only three lengths qualify, and they are the ones the protocol accumulates itself. Anything else
/// — including a custom "two minutes" — is read from the rings.
fn rolling_totals(
    snapshot: &moonproto::MoonStateSnapshot,
    market: &str,
    span_ms: i64,
) -> Option<moonproto::state::TradeVolumeTotals> {
    let rolling = snapshot.market_history_rolling_volumes_now(market)?;
    match span_ms {
        60_000 => Some(rolling.one_minute),
        180_000 => Some(rolling.three_minutes),
        300_000 => Some(rolling.five_minutes),
        _ => None,
    }
}

/// The whole traded value over this span from the retained 5-minute candles, when they state one
/// of exactly this length.
///
/// Candles carry no buy/sell split — which is why they cannot serve the sides — but they are the
/// deepest source of the TOTAL, and the one the window captions read before the sides existed.
fn candle_total(
    snapshot: &moonproto::MoonStateSnapshot,
    market: &str,
    span_ms: i64,
) -> Option<f64> {
    let candles = snapshot
        .market_history_derived_snapshot_now(market)?
        .candle_volumes;
    let total = match span_ms {
        300_000 => candles.five_minutes,
        900_000 => candles.fifteen_minutes,
        1_800_000 => candles.thirty_minutes,
        3_600_000 => candles.one_hour,
        7_200_000 => candles.two_hours,
        10_800_000 => candles.three_hours,
        86_400_000 => candles.twenty_four_hours,
        259_200_000 => candles.seventy_two_hours,
        _ => return None,
    };
    (total.is_finite() && total > 0.0).then_some(total)
}

#[cfg(test)]
mod tests;
