//! The all-markets scan: one market's 5-minute candles + the enabled rows of Main's auto-figure set
//! → that market's best event, its nearest line and what the screener's filter reads.
//!
//! The screener's "Calculate" button (`AUTO_FIGURES.md` §13.4) runs this once per market on a
//! background thread; nothing here runs on a timer or per frame. It calls exactly the functions the
//! chart calls — [`super::detect`], [`super::take::select`], [`super::events::line_events`] and
//! [`super::confluence::confluence`] — on the rows of Main's set (LinKvo, 19.11.2026: the same rows
//! and parameters, configured in one place), so a coin the screener flags shows the same lines and
//! marks when it is opened on a chart. The chart's cross-row rule is applied here too, in the
//! chart's order: a formation two rows both FOUND is one line, under its more precise name, before
//! either row's `take` runs ([`super::twins`]).
//!
//! Pure like every detector: no clock (`now_ms` is an argument), no I/O, no market lookup. The
//! caller reads the market's rows from the local 5-minute kline cache, which the background
//! recorder keeps for every market (15 days, written only while the terminal runs — holes are
//! normal); this module never asks a core for anything.
//!
//! # From 5-minute rows to a row's bars
//!
//! Only CLOSED bars are read, as on the chart: a 5-minute row counts once `t_open + 5 min <= now`,
//! a bucket of the row's timeframe once `t_open + tf <= now`. A bucket the input does not cover from
//! its start (the first one, when the input begins mid-bucket) is dropped, because a partial high
//! or low places a level where the market never turned. The newest closed bucket is also dropped
//! while it is short of 5-minute rows and its close is less than [`LAG_CAP_MS`] old: the recorder
//! may not have written the bucket's last rows yet — the same rule, with the same cap, as the
//! chart's own job (`chartdx/auto_figures_sync/job.rs::cache_bars`), so the two see the same bars.
//! Every timeframe — `D1` included — is resampled from the 5-minute rows; the scanner has no other
//! source.
//!
//! # Cost
//!
//! One market is O(rows × (resample of the input + the row's detector)), plus O(P²) for the twin
//! pass over the P formations the rows found (every other found line is skipped in one pass), plus
//! O(L log L) for the confluence pass over the L kept lines. Fifteen days of 5-minute rows is
//! 4 320 rows, so a few hundred markets per button press is background work, not a stall.
//! [`ScanResult`] keeps its scratch buffers between calls; what still allocates per row is each
//! detector's own result and the copy of its events tail, held until the twin pass has seen every
//! row.

use moon_core::config::auto_figures::{
    AutoEvent, AutoFigureRow, DetectorParams, EventMarks, Take, TfId, TfSource,
};
use moon_core::market::candles::{ChartCandle, resample};

use super::confluence::{PlacedLine, confluence};
use super::events::{EventSet, bars_read, line_events};
use super::history::history_full;
use super::twins::{Twin, twin_winners, union_marks};
use super::{DetectCtx, Found, bar_ok, detect, take};

/// The timeframe a `TfSource::Auto` row is scanned on.
///
/// On the chart `Auto` follows the visible time span; a screener has no span. H1 is what the
/// chart's own table (`tf.rs`, `AUTO_TF_TABLE`) picks for 4–8 days on screen, the middle of its
/// range, and the default 200-bar lookback of a pivot row is 8.3 days of H1 bars, inside the
/// cache's 15 days — so a default `Auto` row fills its history instead of always reporting it
/// short (on H4 it would need 33 days), and it is never M1, which 5-minute rows cannot make. It
/// is also no coarser than any detector's [`DetectorParams::coarsest_tf`], so the cap applied to
/// every row never moves it.
pub const AUTO_TF: TfId = TfId::H1;

/// How long past a bucket's close the newest bucket waits for its missing 5-minute rows, ms.
///
/// Mirrors `LAG_CAP_MS` of the chart's job: inside the cap an incomplete newest bucket is left out
/// (its high and low would be wrong); past it the bucket is taken as it is — an illiquid market may
/// simply have no row for a quiet five minutes.
pub const LAG_CAP_MS: i64 = 5 * 60_000;

/// The bar of the local cache this module reads, ms.
const BASE_MS: i64 = 5 * 60_000;

/// Longest a market's newest closed 5-minute row may have closed before `now_ms` for its answer to
/// be CURRENT, ms; past it the answer is marked [`ScanResult::cache_lag_ms`].
///
/// The recorder writes the cache only while the terminal runs, so after a restart (or a stretch the
/// recorder did not reach the market) the cache ends hours before now. The scan still finds the
/// lines in those old bars, but an event on them — a breakout, a squeeze — is an event of the
/// moment the cache stops, not of now, and the column must not present it as today's. Fifteen
/// minutes is three bars: a live recorder's newest row closed at most five minutes ago plus its
/// write delay, and an illiquid market can miss a row or two, but not an hour of them.
pub const STALE_CACHE_MS: i64 = 15 * 60_000;

/// Events from most to least useful to a trader looking down the screener's column — the order
/// the market's best event is picked by (see [`event_rank`]).
///
/// A pressed squeeze is a setup about to resolve; a false break and a breakout are something that
/// just happened at a level; a role flip and a stop hunt are a level that held in a telling way;
/// an approach is "price is near a line"; a bounce is the most common thing a line shows; a
/// confluence is a property of the place, not of what price did there.
pub const BEST_ORDER: [AutoEvent; 8] = [
    AutoEvent::Squeeze,
    AutoEvent::FalseBreak,
    AutoEvent::Breakout,
    AutoEvent::RoleFlip,
    AutoEvent::StopHunt,
    AutoEvent::Approach,
    AutoEvent::Bounce,
    AutoEvent::Confluence,
];

/// Position of `event` in [`BEST_ORDER`]: 0 is the best. What the screener's column can sort by.
pub fn event_rank(event: AutoEvent) -> usize {
    BEST_ORDER
        .iter()
        .position(|e| *e == event)
        .unwrap_or(BEST_ORDER.len())
}

/// What the scan needs from one row of the auto-figure set, with its timeframe already resolved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScanRow {
    /// The row's index in the set — what [`ScanLine::row_ix`] reports, and what tells two rows
    /// apart in the confluence pass.
    pub row_ix: usize,
    pub params: DetectorParams,
    /// The timeframe the row is scanned on: `Fixed` as is, `Auto` as [`AUTO_TF`], either capped by
    /// [`DetectorParams::coarsest_tf`].
    pub tf: TfId,
    pub take: Take,
    pub marks: EventMarks,
}

impl ScanRow {
    /// The scan row of `row`, the `row_ix`-th of the set; `None` for a row the scan skips.
    ///
    /// Skipped: a row that is not drawn (hidden, or a detector this build does not know —
    /// [`AutoFigureRow::is_drawn`]), and a row whose timeframe 5-minute rows cannot make (`M1`).
    /// The cap by [`DetectorParams::coarsest_tf`] is the one the chart applies, so a `Day` extreme
    /// pinned to D1 by a hand-edited file is scanned on H4 here too.
    pub fn from_row(row_ix: usize, row: &AutoFigureRow) -> Option<Self> {
        if !row.is_drawn() {
            return None;
        }
        let params = *row.detector.known()?;
        let tf = match row.tf {
            TfSource::Fixed(tf) => tf,
            TfSource::Auto => AUTO_TF,
        }
        .min(params.coarsest_tf());
        if tf.ms() % BASE_MS != 0 {
            return None;
        }
        Some(Self {
            row_ix,
            params,
            tf,
            take: row.take,
            marks: row.events,
        })
    }

    /// The scan rows of a whole set, in set order — what the UI builds once per button press from
    /// Main's rows, so the skip rules live here and not in the caller.
    pub fn from_rows(rows: &[AutoFigureRow]) -> Vec<Self> {
        rows.iter()
            .enumerate()
            .filter_map(|(ix, row)| Self::from_row(ix, row))
            .collect()
    }
}

/// One kept line, placed against the market's last price.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScanLine {
    /// The row that found it ([`ScanRow::row_ix`]).
    pub row_ix: usize,
    /// The timeframe that row was scanned on.
    pub tf: TfId,
    /// The line's price at `now_ms` (a sloped line projected there; for a two-line formation its
    /// side nearer the last price, `FoundShape::price_near`).
    pub price: f64,
    /// `(price - last price) / last price × 100`: positive when the line is ABOVE the last price,
    /// negative below.
    pub distance_pct: f64,
}

/// The market's best event and the line it is on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScanHit {
    pub event: AutoEvent,
    pub line: ScanLine,
}

/// One market's answer, and the scratch the next call reuses.
///
/// Build it once with `Default` and hand the same value to [`scan_market`] for every market: each
/// call overwrites every public field, and the scratch keeps its capacity. Keep a market's answer
/// with [`Self::answer`], which copies the public fields out.
#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    /// The best event over every kept line of every scanned row: the lowest [`event_rank`], then
    /// the line nearest to the last price, then the lowest row index. `None` when no line shows
    /// any event the rows mark.
    pub best: Option<ScanHit>,
    /// The kept line nearest to the last price, events or not (ties: the lowest row index).
    pub nearest: Option<ScanLine>,
    /// Kept lines over all scanned rows — what the rows would draw on this market's chart.
    pub lines: u32,
    /// Rows that had enough history and ran their detector.
    pub rows_scanned: u32,
    /// Rows short of their history ([`history_full`]: 90 % of their bars for a row that samples
    /// them, all of them for a calendar row), which did not run.
    pub rows_short: u32,
    /// No row ran, and at least one was short of history: the market has too little cache to say
    /// anything — "not enough history" in the column, rather than "nothing found".
    pub short_history: bool,
    /// The close of the newest valid 5-minute row opening at or before `now_ms` — the freshest
    /// price the cache holds (the recorder writes a row once it has closed, so this is normally
    /// the newest closed row's; a row out of order is not "newest"). What sides and distances are
    /// measured from.
    pub last_price: Option<f64>,
    /// How long before `now_ms` the newest closed 5-minute row closed, ms — set only when that is
    /// more than [`STALE_CACHE_MS`]: the cache is behind, and the events found are of the moment
    /// it stops, not current. `None` for a cache that is current, and for one with no closed row
    /// at all (that market is [`Self::short_history`]). Such an answer passes no event filter
    /// ([`ScanAnswer::matches`]).
    pub cache_lag_ms: Option<i64>,
    scratch: Scratch,
}

/// Buffers reused from one [`scan_market`] call to the next.
#[derive(Debug, Clone, Default)]
struct Scratch {
    /// The input's valid closed 5-minute rows.
    m5: Vec<ChartCandle>,
    /// One row's closed bars of its timeframe.
    bars: Vec<ChartCandle>,
    /// `line_events` output for one line.
    events: Vec<EventSet>,
    /// Every scanned row's detector run, before any selection.
    runs: Vec<RowRun>,
    /// Every found line of every run, flattened in row order: its twin identity ([`Twin`]), the
    /// survivor it loses to, whether it is out of its row's selection, and the marks it shows.
    twins: Vec<Option<Twin>>,
    twin_scratch: Vec<usize>,
    winner_of: Vec<Option<usize>>,
    lost: Vec<bool>,
    marks: Vec<EventMarks>,
    /// Every kept line of every scanned row.
    kept: Vec<Kept>,
    /// The kept lines as the confluence pass reads them, and its answer.
    placed: Vec<PlacedLine>,
    near: Vec<bool>,
}

/// A kept line with its events and the marks the confluence pass reads — its row's, joined with
/// those of the rows whose twin of it was dropped ([`union_marks`]).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Kept {
    line: ScanLine,
    events: EventSet,
    marks: EventMarks,
}

/// One scanned row's detector run, kept until the twin pass has seen every row.
#[derive(Debug, Clone)]
struct RowRun {
    /// Index into the scan's `rows`.
    row: usize,
    found: Vec<Found>,
    /// The newest closed bars the row's events read (`events::bars_read` of its policy).
    recent: Vec<ChartCandle>,
}

/// The public half of a [`ScanResult`] — every answer field, none of the scratch.
///
/// What a caller keeps per market once the scan of the next one overwrites the result: the
/// screener holds one per market for the column and the filter, and a whole `ScanResult` per market
/// would carry its 5-minute buffers along (thousands of bars each).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScanAnswer {
    /// [`ScanResult::best`].
    pub best: Option<ScanHit>,
    /// [`ScanResult::nearest`].
    pub nearest: Option<ScanLine>,
    /// [`ScanResult::lines`].
    pub lines: u32,
    /// [`ScanResult::rows_scanned`].
    pub rows_scanned: u32,
    /// [`ScanResult::rows_short`].
    pub rows_short: u32,
    /// [`ScanResult::short_history`].
    pub short_history: bool,
    /// [`ScanResult::last_price`].
    pub last_price: Option<f64>,
    /// [`ScanResult::cache_lag_ms`].
    pub cache_lag_ms: Option<i64>,
}

impl ScanAnswer {
    /// Whether this answer passes `filter` — the rule [`matches`] documents.
    pub fn matches(&self, filter: &ScanFilter) -> bool {
        let best_is = |e: AutoEvent| self.best.is_some_and(|b| b.event == e);
        match *filter {
            ScanFilter::Any => true,
            // A lagging cache's events and distances are of the moment it stops: not current.
            _ if self.cache_lag_ms.is_some() => false,
            ScanFilter::Squeeze => best_is(AutoEvent::Squeeze),
            ScanFilter::Breakout => best_is(AutoEvent::Breakout),
            ScanFilter::FalseBreak => best_is(AutoEvent::FalseBreak),
            ScanFilter::Approach { max_pct } => self
                .nearest
                .is_some_and(|l| l.distance_pct.abs() <= f64::from(max_pct)),
        }
    }
}

impl ScanResult {
    /// The answer fields, copied out, for a caller that keeps them past the next scan.
    pub fn answer(&self) -> ScanAnswer {
        ScanAnswer {
            best: self.best,
            nearest: self.nearest,
            lines: self.lines,
            rows_scanned: self.rows_scanned,
            rows_short: self.rows_short,
            short_history: self.short_history,
            last_price: self.last_price,
            cache_lag_ms: self.cache_lag_ms,
        }
    }

    /// Every public field back to "nothing"; the scratch keeps its capacity.
    fn reset(&mut self) {
        self.best = None;
        self.nearest = None;
        self.lines = 0;
        self.rows_scanned = 0;
        self.rows_short = 0;
        self.short_history = false;
        self.last_price = None;
        self.cache_lag_ms = None;
    }
}

/// What the screener's filter can ask of a market.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScanFilter {
    /// No filter: every market passes, those short of history included.
    Any,
    /// The best event is a pressed squeeze.
    Squeeze,
    /// The nearest kept line is within `max_pct` % of the last price, either side.
    Approach { max_pct: f32 },
    /// The best event is a breakout.
    Breakout,
    /// The best event is a false break.
    FalseBreak,
}

/// Whether `result` passes `filter`.
///
/// The event filters read the market's BEST event — the one its column shows (LinKvo, 19.11.2026:
/// "the column shows the coin's best event; the filter goes by it"), so a filtered list never shows
/// a column that disagrees with the filter. An answer whose cache lags ([`ScanResult::cache_lag_ms`])
/// passes none of them, `Approach` included: what it found is of the moment the cache stops.
/// `Approach` is the exception on purpose: it reads the
/// distance to the nearest kept line against the filter's own `max_pct`, not the rows' `Approach`
/// event, because each row marks an approach at its own `approach_pct` (0.3 % by default) while the
/// screener's question is "which coins are within X % of a level", with X chosen in the screener. A
/// `max_pct` that is negative or not a number passes nothing.
///
/// The rule itself lives on [`ScanAnswer::matches`], so a caller holding only the copied-out answer
/// filters by the same code.
pub fn matches(result: &ScanResult, filter: &ScanFilter) -> bool {
    result.answer().matches(filter)
}

/// Scan one market: every row of `rows` over `bars_5m`, written into `out`.
///
/// Per row: its closed bars resampled from `bars_5m` (see the module doc); fewer than
/// [`history_full`] of `bars_needed` → counted short and skipped (the chart's own rule, so a row
/// the chart calls short is never scanned here); otherwise the newest `bars_needed` of them go to
/// `detect`. Then, over every row's FOUND lines, a formation two rows both found is one line
/// ([`super::twins`]): the twin under the less precise name is out of its row's selection, and the
/// survivor shows the union of both rows' marks. Then `take::select_skipping` keeps each row's
/// lines against the last price at `now_ms`, and `line_events` marks each kept line from the
/// row's closed bars (it reads only the newest `events::bars_read(policy)` valid ones). Then
/// confluence across every row's kept lines, set where a line's marks ask for it; then the best
/// event and the nearest line.
///
/// Args:
///     rows: The rows to run ([`ScanRow::from_rows`] of Main's set).
///     bars_5m: The market's 5-minute rows, sorted by open time ascending, as the kline cache
///         returns them. Unreadable rows (non-finite or non-positive prices, `high < low`) and rows
///         out of order are skipped; nothing in them can panic the scan.
///     now_ms: The reference time, Unix ms — what decides which bars are closed and where sloped
///         lines are projected to. Not a finite number: no bar is closed and every row is short.
///     out: Overwritten with the answer; its buffers are reused.
pub fn scan_market(rows: &[ScanRow], bars_5m: &[ChartCandle], now_ms: f64, out: &mut ScanResult) {
    out.reset();
    let Scratch {
        m5,
        bars,
        events,
        runs,
        twins,
        twin_scratch,
        winner_of,
        lost,
        marks,
        kept,
        placed,
        near,
    } = &mut out.scratch;
    let last_price = closed_5m(bars_5m, now_ms, m5);
    runs.clear();
    kept.clear();
    let (mut scanned, mut short) = (0u32, 0u32);
    for (at, row) in rows.iter().enumerate() {
        let needed = row.params.bars_needed(row.tf);
        let tf_ms = row.tf.ms();
        closed_bars(m5, tf_ms, now_ms, bars);
        let read = tail(bars, needed);
        if read.is_empty() || !history_full(read.len(), needed, &row.params) {
            short += 1;
            continue;
        }
        scanned += 1;
        if last_price.is_none() {
            continue;
        }
        let found = detect(&row.params, read, &DetectCtx { tf_ms, now_ms });
        // The events get their OWN tail, not the detector's: a row needing one bar (round prices,
        // a period level) still judges a breakout by the closes before it. Every bar here is
        // valid (the 5-minute rows were filtered), so `bars_read` of them is exactly what
        // `line_events` reads.
        let events_tail = u32::try_from(bars_read(row.params.broken_policy())).unwrap_or(u32::MAX);
        runs.push(RowRun {
            row: at,
            found,
            recent: tail(bars, events_tail).to_vec(),
        });
    }

    // One formation found by two rows is one line, as on the chart, and BEFORE either row selects:
    // the twin under the less precise name is neither counted, nor a confluence partner, nor a
    // candidate for the best event — so the column never names an event of a line the chart does
    // not draw — and it does not take a slot of its own row's `count` either.
    twins.clear();
    marks.clear();
    for run in runs.iter() {
        let row = &rows[run.row];
        twins.extend(run.found.iter().map(|f| Twin::of(f, row.tf)));
        marks.extend(run.found.iter().map(|_| row.marks));
    }
    twin_winners(twins, twin_scratch, winner_of);
    lost.clear();
    lost.extend(winner_of.iter().map(Option::is_some));
    let mut flat = 0;
    for run in runs.iter() {
        let row_marks = rows[run.row].marks;
        for _ in &run.found {
            if let Some(winner) = winner_of[flat] {
                marks[winner] = union_marks(&marks[winner], &row_marks);
            }
            flat += 1;
        }
    }

    let mut offset = 0;
    for run in runs.iter() {
        let row = &rows[run.row];
        let span = offset..offset + run.found.len();
        offset = span.end;
        let Some(price) = last_price else {
            continue;
        };
        let policy = row.params.broken_policy();
        for ix in take::select_skipping(&run.found, &row.take, price, now_ms, &lost[span.clone()]) {
            let Some(f) = run.found.get(ix) else {
                continue;
            };
            // A formation's distance is to its nearer side, the one `take` kept it by.
            let Some(line_price) = f.shape.price_near(now_ms, price) else {
                continue;
            };
            let line_marks = marks[span.start + ix];
            line_events(
                std::slice::from_ref(f),
                &run.recent,
                price,
                now_ms,
                &line_marks,
                policy,
                events,
            );
            kept.push(Kept {
                line: ScanLine {
                    row_ix: row.row_ix,
                    tf: row.tf,
                    price: line_price,
                    distance_pct: (line_price - price) / price * 100.0,
                },
                events: events.first().copied().unwrap_or_default(),
                marks: line_marks,
            });
        }
    }

    placed.clear();
    placed.extend(kept.iter().map(|k| PlacedLine {
        row: k.line.row_ix,
        tf: k.line.tf,
        price: k.line.price,
        pct: k.marks.confluence_pct,
    }));
    confluence(placed, near);
    for (k, &flag) in kept.iter_mut().zip(near.iter()) {
        if flag && k.marks.marks(AutoEvent::Confluence) {
            k.events.insert(AutoEvent::Confluence);
        }
    }

    let nearer = |a: &ScanLine, b: &ScanLine| {
        a.distance_pct
            .abs()
            .total_cmp(&b.distance_pct.abs())
            .then(a.row_ix.cmp(&b.row_ix))
    };
    let mut best: Option<ScanHit> = None;
    let mut nearest: Option<ScanLine> = None;
    for k in kept.iter() {
        if nearest.is_none_or(|n| nearer(&k.line, &n).is_lt()) {
            nearest = Some(k.line);
        }
        for event in k.events.iter() {
            let better = best.is_none_or(|b| {
                event_rank(event)
                    .cmp(&event_rank(b.event))
                    .then_with(|| nearer(&k.line, &b.line))
                    .is_lt()
            });
            if better {
                best = Some(ScanHit {
                    event,
                    line: k.line,
                });
            }
        }
    }
    out.best = best;
    out.nearest = nearest;
    out.lines = u32::try_from(kept.len()).unwrap_or(u32::MAX);
    out.rows_scanned = scanned;
    out.rows_short = short;
    out.short_history = scanned == 0 && short > 0;
    out.last_price = last_price;
    out.cache_lag_ms = m5.last().and_then(|b| {
        let lag = now_ms - (b.t_open_ms + BASE_MS as f64);
        (lag > STALE_CACHE_MS as f64).then_some(lag as i64)
    });
}

/// The valid 5-minute rows of `bars` that have closed by `now_ms`, in strictly ascending open
/// time, into `out`; returns the last price — the close of the newest valid row opening at or
/// before `now_ms`. The recorder writes a row only once it has closed, so that is normally the
/// newest closed row; a row still forming at `now_ms` (a cache filled by another writer, or a
/// clock a little behind the recorder's) is read for the price but never resampled into a bar.
///
/// A row out of order or repeating an open time is dropped, from both answers, rather than letting
/// `resample` fold it into the wrong bucket or an older row at the end of the input pass for the
/// latest price. `None` when no row qualifies or `now_ms` is not a number.
fn closed_5m(bars: &[ChartCandle], now_ms: f64, out: &mut Vec<ChartCandle>) -> Option<f64> {
    out.clear();
    let mut after = f64::NEG_INFINITY;
    let mut last = None;
    for b in bars {
        if !(bar_ok(b) && b.t_open_ms > after && b.t_open_ms <= now_ms) {
            continue;
        }
        after = b.t_open_ms;
        last = Some(f64::from(b.close));
        if b.t_open_ms + BASE_MS as f64 <= now_ms {
            out.push(*b);
        }
    }
    last
}

/// The closed bars of `tf_ms` that `m5` (closed 5-minute rows, ascending) yields at `now_ms`, into
/// `out`: the partial first bucket, every bucket still forming, and a lagging newest bucket left
/// out (see the module doc).
fn closed_bars(m5: &[ChartCandle], tf_ms: i64, now_ms: f64, out: &mut Vec<ChartCandle>) {
    out.clear();
    let Some(first) = m5.first() else {
        return;
    };
    if tf_ms == BASE_MS {
        out.extend_from_slice(m5);
        return;
    }
    if tf_ms <= 0 {
        return;
    }
    resample(m5, tf_ms, out);
    if out.first().is_some_and(|b| first.t_open_ms > b.t_open_ms) {
        out.remove(0);
    }
    let tf = tf_ms as f64;
    while out.last().is_some_and(|b| b.t_open_ms + tf > now_ms) {
        out.pop();
    }
    if let Some(open) = out.last().map(|b| b.t_open_ms) {
        let expected = usize::try_from(tf_ms / BASE_MS)
            .unwrap_or(usize::MAX)
            .max(1);
        let have = m5
            .iter()
            .rev()
            .take_while(|r| r.t_open_ms >= open)
            .filter(|r| r.t_open_ms < open + tf)
            .count();
        if have < expected && now_ms < open + tf + LAG_CAP_MS as f64 {
            out.pop();
        }
    }
}

/// The newest `n` bars of `bars`.
fn tail(bars: &[ChartCandle], n: u32) -> &[ChartCandle] {
    let n = usize::try_from(n).unwrap_or(usize::MAX).min(bars.len());
    &bars[bars.len() - n..]
}

#[cfg(test)]
mod tests;
