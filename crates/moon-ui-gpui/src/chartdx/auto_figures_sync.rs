//! Auto figures in the own-pass drawing layer: the levels, sloped lines and squeezes the chart
//! finds by itself (`moon_chart::auto_figures`), drawn through the ordinary figure geometry.
//!
//! ## Who does what
//!
//! - The PANEL (`panels::chart::auto_figures`) owns the recompute policy: it evaluates each row's
//!   [`job::RowKey`], and when one moved it snapshots the pane's closed candles here
//!   ([`ChartDataState::auto_pane_candles`]) and runs [`job::run`] on the background executor —
//!   cache read, resample and detector all off the UI thread. The result comes back through the
//!   task's own completion, already selected (`take::select`), and is published here with
//!   [`ChartDataState::set_auto_figures`].
//! - THIS module owns the published set and its geometry. Publishing bumps one revision folded
//!   into [`ChartDataState::figures_sig`] and stamps the panes dirty — the same gate the user's
//!   figures ride — so the next order sync rebuilds the userdata buffers ONCE. Nothing here runs
//!   per frame: [`ChartDataState::append_auto_figure_geometry`] is called only from that rebuild.
//! - The PRICE PASS ([`ChartDataState::auto_figures_on_price`]) follows the current price: the
//!   kept set and each line's side (its `BySign` colour) and events depend on it ([`select`]). It
//!   runs at the end of a market sync, gated on the price having moved, and replaces a published
//!   set only when its drawn lines changed; the panes showing it then redraw their auto segment
//!   alone ([`pane`]), without the order sync.
//! - Captions are strings built on those same revisions ([`pane::refresh_auto_captions`]) and only
//!   placed by the text pass; hover and right-click hit-test the drawn lines through
//!   [`pane::AutoPaneDrawn::line_at`].
//!
//! ## Why a second, passive figure list rather than the store
//!
//! An auto figure must not be saved, selected, dragged, listed as an alert or upserted to a core.
//! Every one of those paths reads `FigureStore` — persistence, `pick_figure`, the Alerts panel, the
//! alert reconcile — so keeping auto figures OUT of it makes those guarantees structural instead of
//! a flag each path has to remember (`from_server` shows how many places a flag has to reach). The
//! one interaction they do have — hover and right-click — hit-tests this module's own record of
//! what was drawn, never the store; "pin as figure" is how a line becomes an ordinary figure. They still draw as figures: each found shape becomes a figure-tool value
//! (`moon_chart::auto_figures::draw`) and goes through `build_passive_figure_geometry`, the same
//! sink and the same tools as the store's figures, so the three GPU backends need no change.
//!
//! The figures are rebuilt from the published data on each rebuild rather than cached, because
//! their colour is resolved against THIS engine's theme at that moment: a theme switch forces the
//! rebuild anyway, and then the lines follow it with nothing extra to invalidate.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use moon_core::config::ChartTheme;
use moon_core::config::LabelColor;
use moon_core::config::auto_figures::{BrokenPolicy, EventMarks, LineCaption, LineStyle, Take};
use moon_core::market::ChartCandle;
use moon_core::session::CoreId;

use super::ChartDataState;
use super::types::CandleGpu;

pub(crate) mod caption;
pub(crate) mod job;
pub(crate) mod pane;
pub(crate) mod select;

#[cfg(test)]
mod tests;

pub(crate) use caption::RowTitle;
pub(crate) use select::{KeptLine, LineKey};

/// One drawn row of one pane: its detector's outcome, the figures the row keeps, and the settings
/// the price pass re-selects them by.
#[derive(Debug, Clone)]
pub(crate) struct AutoRowDrawn {
    /// The row's index in its kind's configuration — what the caption, the hover card and the
    /// right-click menu name the row by.
    pub row_ix: usize,
    /// What the row's lines are called.
    pub title: RowTitle,
    /// How the row draws, copied at publication.
    pub style: LineStyle,
    /// What the row prints on its lines.
    pub caption: LineCaption,
    /// Which of the found lines the row keeps, re-applied on every price change.
    pub take: Take,
    /// Which events the row marks.
    pub marks: EventMarks,
    /// The row's broken-line policy, for the false-break window of its events.
    pub policy: BrokenPolicy,
    /// Everything the detector run produced, shared with the panel that holds it.
    pub outcome: Arc<job::AutoRowOutcome>,
    /// The drawn lines, ranked: indices into `outcome.found` with their side and events.
    pub kept: Vec<KeptLine>,
    /// What the twin pass of the selection decided for this row's kept lines
    /// ([`select::select_pane`]), re-derived with `kept`; empty for a row whose kept lines no other
    /// row keeps too.
    pub twins: RowTwins,
    /// The moment the selection was made, Unix ms: where sloped lines were projected for it, and
    /// where a squeeze's sloped half stops when its apex lies further ahead.
    pub ref_ms: f64,
}

/// One row's share of the selection's cross-row twin pass ([`select::select_pane`]), re-derived
/// with every selection — a twin exists only between lines two rows both keep.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RowTwins {
    /// The kept lines that survived a set of twins and show more marks than the row's own:
    /// `(index into found, the union of the marks of every row of the set)`.
    pub marks: Vec<(usize, EventMarks)>,
}

impl RowTwins {
    /// The joined marks of found line `ix`, when it is the survivor of a set of twins.
    pub(crate) fn marks_of(&self, ix: usize) -> Option<&EventMarks> {
        self.marks.iter().find(|(i, _)| *i == ix).map(|(_, m)| m)
    }
}

/// Everything one pane's auto figures draw.
#[derive(Debug, Clone, Default)]
pub(crate) struct AutoPaneFigures {
    pub rows: Vec<AutoRowDrawn>,
    /// Lines the user hid on this pane target until it changes market. Owned by the panel,
    /// carried here so the price pass drops them too.
    pub hidden: Vec<LineKey>,
    /// The block's one opacity (`AutoFiguresCfg::opacity`), copied at publication: the factor on
    /// the alpha of every line, zone, event mark and caption this set draws ([`pane`]). A change
    /// of it is a configuration edit, which republishes the set — nothing per frame reads the
    /// configuration.
    pub opacity: f32,
    /// The price (`f32` bits) the price pass last evaluated this set at; `None` until it has.
    /// Interior: the pass runs over a shared set and replaces it only when the selection moved.
    eval_price: Cell<Option<u32>>,
    /// The price pass's working buffers, handed on to the set that replaces this one, so a price
    /// change allocates nothing for them once they have grown. Scratch, not state.
    bufs: RefCell<PassBufs>,
}

/// The price pass's reused buffers: the events scratch of [`select::select_pane`], the rows' new
/// selections before they are compared with the drawn ones, and the confluence pass's.
#[derive(Debug, Default)]
pub(crate) struct PassBufs {
    events: Vec<moon_chart::auto_figures::events::EventSet>,
    kept: Vec<Vec<KeptLine>>,
    confluence: select::ConfluenceBufs,
}

impl Clone for PassBufs {
    /// A clone starts empty: the buffers hold nothing between passes, and the set that replaces a
    /// published one takes the old one's buffers explicitly ([`AutoPaneFigures::reselect`]).
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl AutoPaneFigures {
    /// A set of `rows`, with `hidden` lines left out by every later re-selection, drawn opaque
    /// until the publisher sets [`Self::opacity`]. The rows' `kept` lines and `twins` are taken as
    /// they are; [`Self::select_all`] selects them.
    pub(crate) fn new(rows: Vec<AutoRowDrawn>, hidden: Vec<LineKey>) -> Self {
        Self {
            rows,
            hidden,
            opacity: 1.0,
            eval_price: Cell::new(None),
            bufs: RefCell::new(PassBufs::default()),
        }
    }

    /// Select every row at `at_ms`, row `i` against `ref_prices[i]` (`NaN`, keeping nothing, past
    /// its end): the pane's selection with its twins ([`select::select_pane`]), then the
    /// confluences between the selections ([`select::mark_confluence`]) — the same two steps the
    /// price pass ([`Self::reselect`]) repeats with one price for every row.
    ///
    /// For the panel's publication of a finished run, where a row without the chart's last price
    /// is measured from its own last close.
    pub(crate) fn select_all(&mut self, ref_prices: &[f64], at_ms: f64) {
        let bufs = self.bufs.get_mut();
        let price = |i: usize| ref_prices.get(i).copied().unwrap_or(f64::NAN);
        let twins = select::select_pane(
            &self.rows,
            &self.hidden,
            &price,
            at_ms,
            &mut bufs.events,
            &mut bufs.kept,
        );
        select::mark_confluence(
            &self.rows,
            &mut bufs.kept,
            &twins,
            at_ms,
            &mut bufs.confluence,
        );
        for ((row, kept), twins) in self.rows.iter_mut().zip(bufs.kept.drain(..)).zip(twins) {
            row.kept = kept;
            row.twins = twins;
        }
    }

    /// One line describing every row for the app log: its index, timeframe, bar source, bars read
    /// against bars needed (`short` when fewer, `lagging` when the cache had not filled the newest
    /// bucket yet), figures found and lines kept.
    pub(crate) fn summary(&self) -> String {
        if self.rows.is_empty() {
            return "no rows".to_owned();
        }
        self.rows
            .iter()
            .map(|r| {
                let o = &r.outcome;
                format!(
                    "row {} {:?} {:?} {}/{} bars{}{} found {} kept {}",
                    r.row_ix,
                    o.key.tf,
                    o.source,
                    o.bars,
                    o.needed,
                    if o.short_history() { " short" } else { "" },
                    if o.lagging { " lagging" } else { "" },
                    o.found.len(),
                    r.kept.len(),
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// The same set re-selected against `price` at `now_ms`, or `None` when no row's drawn lines
    /// — which ones, their sides, their events — moved.
    ///
    /// What makes the price pass cheap to run on every tick: an unchanged answer replaces nothing,
    /// so it rebuilds no geometry and uploads nothing.
    ///
    /// The cross-row passes are part of "drawn": every row is re-selected together — a formation
    /// two rows keep drawn once, by the more precise row, the other row taking its next line
    /// ([`select::select_pane`], O(K²) in the few formations kept) — and the confluences between
    /// the new selections marked across the rows, and only then is the result compared, so a line
    /// that merely moved into or out of another timeframe's band, or a twin that changed hands,
    /// replaces the set too.
    pub(crate) fn reselect(&self, price: f64, now_ms: f64) -> Option<Self> {
        let mut guard = self.bufs.borrow_mut();
        let PassBufs {
            events,
            kept,
            confluence,
        } = &mut *guard;
        let twins = select::select_pane(&self.rows, &self.hidden, &|_| price, now_ms, events, kept);
        select::mark_confluence(&self.rows, kept, &twins, now_ms, confluence);
        let same = self
            .rows
            .iter()
            .zip(kept.iter().zip(&twins))
            .all(|(r, (k, t))| r.kept == *k && r.twins == *t);
        if same {
            return None;
        }
        let next = std::mem::take(kept);
        // Released before the clone: cloning the set borrows its buffers' cell.
        drop(guard);
        let mut set = self.clone();
        for ((row, kept), twins) in set.rows.iter_mut().zip(next).zip(twins) {
            row.kept = kept;
            row.twins = twins;
            row.ref_ms = now_ms;
        }
        set.bufs = RefCell::new(self.bufs.take());
        Some(set)
    }
}

/// What the panel needs to know about one pane to evaluate its rows' keys, read without copying
/// any candles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AutoPaneView {
    /// The chart series' bar length, ms.
    pub chart_tf_ms: i64,
    /// Whether the pane holds any candles of the chart's own timeframe yet.
    pub chart_ready: bool,
    /// Time the plot spans, ms, once a market sync measured it.
    pub visible_ms: Option<i64>,
    /// The pane's last traded price, once known.
    pub last_price: Option<f64>,
    /// Whether the pane's series has CoinCard deep history (`ChartHistoryRead::deep_present`) —
    /// part of a row's recompute key.
    pub deep_history: bool,
}

impl ChartDataState {
    /// Publish one pane target's auto figures, or remove them with `None`.
    ///
    /// Always a change when it is called: the panel publishes only when a detector run completed
    /// or the configuration moved, both rare. The revision bump reaches every pane's figure gate,
    /// as [`Self::set_figure_visual`] does, because the panes are keyed by index while the set is
    /// keyed by target.
    pub(crate) fn set_auto_figures(
        &mut self,
        core: CoreId,
        market: &str,
        set: Option<Rc<AutoPaneFigures>>,
    ) -> bool {
        let at = self
            .auto_figures
            .iter()
            .position(|((c, m), _)| *c == core && m == market);
        match (at, set) {
            (None, None) => return false,
            (Some(i), None) => {
                self.auto_figures.swap_remove(i);
            }
            (Some(i), Some(set)) => self.auto_figures[i].1 = set,
            (None, Some(set)) => self.auto_figures.push(((core, market.to_owned()), set)),
        }
        self.auto_figures_rev = self.auto_figures_rev.wrapping_add(1);
        let mut st = self.render.borrow_mut();
        for pr in &mut st.panes {
            pr.last_figures_sig = u64::MAX;
            pr.gpu_prepare_dirty = true;
        }
        st.needs_present = true;
        true
    }

    /// Appends one pane target's auto figures to the userdata buffers, recording what was drawn
    /// in `drawn`.
    ///
    /// Called only from the userdata rebuild in `orders::sync`, gated on `figures_sig`, and LAST
    /// among the layers, so the price pass can redraw this segment alone ([`pane`]). Bands go into
    /// the zone buffer BEFORE that rebuild hashes it, so a zone that appears or moves re-bakes the
    /// base cache exactly like an order zone or a figure fill.
    pub(super) fn append_auto_figure_geometry(
        &self,
        core: CoreId,
        market: &str,
        epoch_ms: f64,
        out: &mut moon_chart::figures::FigureBuffers<'_>,
        drawn: &mut pane::AutoPaneDrawn,
    ) {
        drawn.segment = Some(pane::SegmentBase {
            zones: out.zones.len(),
            hlines: out.hlines.len(),
            segs: out.segs.len(),
            markers: out.markers.len(),
        });
        drawn.epoch_ms = epoch_ms;
        drawn.target = Some((core, market.to_owned()));
        // Book-only mode leaves a 1 px plot with no time axis; the news layer skips it too.
        let set = self
            .auto_figures_rc(core, market)
            .filter(|_| !self.orderbook_only);
        pane::append_auto_geometry(set, &self.theme, self.last_ppp, epoch_ms, out, drawn);
    }

    /// The published set of one pane target, shared.
    fn auto_figures_rc(&self, core: CoreId, market: &str) -> Option<&Rc<AutoPaneFigures>> {
        self.auto_figures
            .iter()
            .find(|((c, m), _)| *c == core && m == market)
            .map(|(_, set)| set)
    }

    /// The price pass: re-select every published set against its pane's current price, redraw the
    /// auto segment of every pane whose drawn set is no longer the published one, and refresh the
    /// captions whose printed values moved.
    ///
    /// Called at the end of a market sync — where a price change is first seen. A mouse move, a
    /// pan or a zoom reaches it only as a camera sync with an UNCHANGED price, which the price gate
    /// turns away. Cost when the price did not move: one comparison per published target and one
    /// caption hash per pane. When it moved: `take::select_skipping` and the events of the kept
    /// lines, O(found + kept) per row, then the confluence pass, O(kept log kept) over the pane.
    /// The twin pass, O(P²) in the found formations, is NOT part of it: it runs once per
    /// published result ([`AutoPaneFigures::new`]), and the price pass only reads its answer.
    /// Geometry is rebuilt only for a pane whose drawn lines actually changed, and only its auto
    /// segment.
    pub(super) fn auto_figures_on_price(&mut self) {
        if !self.scene_visible {
            return;
        }
        let now_ms = super::now_unix_ms();
        let mut st = self.render.borrow_mut();
        for (target, set) in &mut self.auto_figures {
            let price = st
                .panes
                .iter()
                .find(|pr| pr.active && pr.core == Some(target.0) && pr.market == target.1)
                .and_then(|pr| pr.cached_last_price)
                .filter(|p| p.is_finite() && *p > 0.0);
            if let Some(price) = price {
                price_step(set, price, now_ms);
            }
        }
        let mut base_changed = false;
        let mut changed = false;
        for pr in &mut st.panes {
            if !pr.active {
                continue;
            }
            let current = pr.core.and_then(|core| {
                self.auto_figures
                    .iter()
                    .find(|((c, m), _)| *c == core && *m == pr.market)
                    .map(|(_, set)| set)
            });
            let current = current.filter(|_| !self.orderbook_only);
            if pr.auto.redrawable_for(pr.core, &pr.market) && !pr.auto.draws(current) {
                base_changed |= redraw_auto_segment(pr, current, &self.theme, self.last_ppp);
                changed = true;
            }
            changed |= pane::refresh_auto_captions(
                &mut pr.auto,
                pr.cached_last_price.map(f64::from),
                now_ms,
                pr.price_tick.tick(),
            );
        }
        if base_changed {
            st.base_dirty = true;
        }
        if changed {
            st.needs_present = true;
        }
    }

    /// Re-print the age part of the captions as the clock moves it, at most once a second.
    ///
    /// The one caption input that changes with nothing but time. Without this a quiet market would
    /// show "5m" for an hour; with it the frame path pays one hash per pane per second, and builds
    /// a string only when an age's printed count moved.
    pub(super) fn tick_auto_captions(&mut self, now: std::time::Instant) {
        if !self.scene_visible
            || self
                .last_auto_caption_tick
                .is_some_and(|last| now.duration_since(last) < AUTO_CAPTION_TICK)
        {
            return;
        }
        self.last_auto_caption_tick = Some(now);
        let now_ms = super::now_unix_ms();
        let mut st = self.render.borrow_mut();
        let mut changed = false;
        for pr in &mut st.panes {
            if pr.active {
                changed |= pane::refresh_auto_captions(
                    &mut pr.auto,
                    pr.cached_last_price.map(f64::from),
                    now_ms,
                    pr.price_tick.tick(),
                );
            }
        }
        if changed {
            st.needs_present = true;
        }
    }

    /// What the panel reads to evaluate pane `idx`'s row keys, or `None` while the pane is not
    /// the target named (a slot reused for another market answers for the new one only after the
    /// next sync re-stamped it).
    pub(crate) fn auto_pane_view(
        &self,
        idx: usize,
        core: CoreId,
        market: &str,
    ) -> Option<AutoPaneView> {
        let st = self.render.borrow();
        let pr = st.panes.get(idx)?;
        if pr.core != Some(core) || pr.market != market {
            return None;
        }
        let chart_tf_ms = self.candle_view.tf_ms();
        Some(AutoPaneView {
            chart_tf_ms,
            chart_ready: pr
                .candle_rows
                .iter()
                .any(|r| is_native(r, chart_tf_ms as f32)),
            visible_ms: pr.time_scale_ms,
            last_price: pr.cached_last_price.map(f64::from),
            deep_history: pr.deep_history,
        })
    }

    /// The pane's CLOSED candles of the chart's own timeframe, in absolute time — the snapshot a
    /// detector job takes with it. Copied only when a job is about to run.
    pub(crate) fn auto_pane_candles(
        &self,
        idx: usize,
        core: CoreId,
        market: &str,
        now_ms: f64,
    ) -> Vec<ChartCandle> {
        let st = self.render.borrow();
        let Some(pr) = st.panes.get(idx) else {
            return Vec::new();
        };
        if pr.core != Some(core) || pr.market != market {
            return Vec::new();
        }
        let mut candles = native_closed_candles(
            &pr.candle_rows,
            pr.candle_rows_epoch,
            self.candle_view.tf_ms(),
            now_ms,
        );
        mark_range_only(&mut candles, &pr.range_only_opens);
        candles
    }
}

/// What the price pass did with one published set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PriceStep {
    /// The set was already evaluated at this price: nothing was selected or derived.
    Skipped,
    /// Re-selected at a new price; the drawn lines came out the same, so nothing was replaced.
    Same,
    /// Re-selected, and the drawn lines (which, their sides or their events) changed: the set was
    /// replaced, and every pane showing it redraws its auto segment.
    Replaced,
}

/// One published set's step of the price pass at `price` (the pane's last price, `f32` as the
/// pane holds it).
///
/// The gate is the price itself: the set remembers the price it was last evaluated at, so a
/// market sync with an unchanged price — every camera sync of a mouse move, a pan or a zoom —
/// costs one comparison and derives nothing.
pub(super) fn price_step(set: &mut Rc<AutoPaneFigures>, price: f32, now_ms: f64) -> PriceStep {
    if set.eval_price.get() == Some(price.to_bits()) {
        return PriceStep::Skipped;
    }
    set.eval_price.set(Some(price.to_bits()));
    match set.reselect(f64::from(price), now_ms) {
        Some(next) => {
            next.eval_price.set(Some(price.to_bits()));
            *set = Rc::new(next);
            PriceStep::Replaced
        }
        None => PriceStep::Same,
    }
}

/// How often the age captions are re-checked against the clock.
const AUTO_CAPTION_TICK: std::time::Duration = std::time::Duration::from_secs(1);

/// The buffer half of [`redraw_auto_segment`]: cut `scratch` back to `base` and append `set`'s auto
/// geometry again. Kept apart so it can be checked against a full rebuild without a GPU.
pub(super) fn resegment(
    scratch: &mut super::UdScratch,
    base: pane::SegmentBase,
    set: Option<&Rc<AutoPaneFigures>>,
    theme: &ChartTheme,
    ppp: f32,
    epoch_ms: f64,
    drawn: &mut pane::AutoPaneDrawn,
) {
    scratch.zones.truncate(base.zones);
    scratch.hlines.truncate(base.hlines);
    scratch.segs.truncate(base.segs);
    scratch.markers.truncate(base.markers);
    // Passive figures emit no readouts; the buffer exists only to satisfy the sink.
    let mut no_labels = Vec::new();
    pane::append_auto_geometry(
        set,
        theme,
        ppp,
        epoch_ms,
        &mut moon_chart::figures::FigureBuffers {
            zones: &mut scratch.zones,
            hlines: &mut scratch.hlines,
            segs: &mut scratch.segs,
            markers: &mut scratch.markers,
            labels: &mut no_labels,
        },
        drawn,
    );
}

/// Redraw ONE pane's auto segment from `set` without the order sync: truncate the retained
/// userdata to the segment's base, append the auto geometry again, re-upload. Returns whether the
/// zone buffer changed, which re-bakes the base cache (a zone band lives in it).
///
/// Everything in front of the segment — orders, figures, news, trade arrows, warning badges — is
/// left exactly as the last full rebuild made it; see [`pane`] for why that is sound.
fn redraw_auto_segment(
    pr: &mut super::PaneRender,
    set: Option<&Rc<AutoPaneFigures>>,
    theme: &ChartTheme,
    ppp: f32,
) -> bool {
    let Some(base) = pr.auto.segment else {
        return false;
    };
    let mut scratch = std::mem::take(&mut pr.ud_scratch);
    let epoch_ms = pr.auto.epoch_ms;
    resegment(&mut scratch, base, set, theme, ppp, epoch_ms, &mut pr.auto);
    let zone_sig = super::data_state::orders::hash_order_zones(&scratch.zones);
    let base_changed = pr.last_order_zone_sig != zone_sig;
    pr.last_order_zone_sig = zone_sig;
    super::trade_history_sync::set(
        &mut pr.layers,
        &scratch.zones,
        &scratch.hlines,
        &scratch.segs,
        &scratch.markers,
    );
    pr.ud_scratch = scratch;
    pr.gpu_prepare_dirty = true;
    base_changed
}

/// Whether a GPU candle row is one of the series' own timeframe rather than a coarser prefix row.
fn is_native(row: &CandleGpu, tf_ms: f32) -> bool {
    row.tf_rel == 0.0 || (row.tf_rel - tf_ms).abs() < 0.5
}

/// The closed candles of the chart's own timeframe among `rows`, converted back to absolute time.
///
/// `rows` keep their times relative to `epoch_ms` in `f32`, which over two weeks of history is
/// off by up to a few hundred milliseconds; each open time is snapped back to its bar grid, which
/// is exact because a bar opens on a multiple of its length. A row still forming at `now_ms`, a
/// coarser prefix row, and anything on an unusable epoch are dropped.
pub(crate) fn native_closed_candles(
    rows: &[CandleGpu],
    epoch_ms: f64,
    tf_ms: i64,
    now_ms: f64,
) -> Vec<ChartCandle> {
    if tf_ms <= 0 || !epoch_ms.is_finite() {
        return Vec::new();
    }
    let tf = tf_ms as f64;
    let mut out: Vec<ChartCandle> = rows
        .iter()
        .filter(|r| is_native(r, tf as f32))
        .filter_map(|r| {
            let t = ((epoch_ms + f64::from(r.t_open_rel)) / tf).round() * tf;
            (t.is_finite() && t + tf <= now_ms).then_some(ChartCandle {
                t_open_ms: t,
                open: r.open,
                high: r.high,
                low: r.low,
                close: r.close,
                // The GPU row keeps turnover only; no detector reads volume.
                volume: 0.0,
                quote_volume: r.volume,
            })
        })
        .collect();
    // The composed list is sorted, but a snapped duplicate would read as two touches of one bar.
    out.dedup_by(|b, a| a.t_open_ms == b.t_open_ms);
    out
}

/// Give the chart candles that are the core's range-only ring rows an UNKNOWN open and close
/// (`NaN`), the way the kline cache's ring rows reach the detectors
/// (`moon_chart::auto_figures::with_ring_rows`): `range_only_opens` are their bucket opens
/// (ascending), as the history layer reports them by SOURCE
/// (`ChartHistoryBuffers::range_only_opens`) — never read from a candle's shape, and never
/// covering a bucket a cached kline, deep history or the trades filled.
///
/// The chart draws those candles with an open and a close oriented from their range
/// (`moon_core::market::candles::orient_range_rows`), right for the eye and wrong for a detector:
/// read as closes they stretched the ATR and moved a level by a pivot (AKEUSDT 21.11.2026,
/// `logs/auto_figures_diag.log`: the chart's first run read 9 of 40 hour bars closing at their
/// low, the cache none).
pub(crate) fn mark_range_only(candles: &mut [ChartCandle], range_only_opens: &[i64]) {
    if range_only_opens.is_empty() {
        return;
    }
    for c in candles.iter_mut() {
        if range_only_opens
            .binary_search(&(c.t_open_ms as i64))
            .is_ok()
        {
            c.open = f32::NAN;
            c.close = f32::NAN;
        }
    }
}

/// A row's line colour against the theme: the caption colour, the theme's sign colours by the
/// line's side of price (support below in the positive colour, resistance above in the negative),
/// or the user's own pick.
pub(crate) fn resolve_rgb(color: LabelColor, above: bool, theme: &ChartTheme) -> [u8; 3] {
    match color {
        LabelColor::Theme => theme.caption_label,
        LabelColor::BySign if above => theme.label_negative,
        LabelColor::BySign => theme.label_positive,
        LabelColor::Fixed(rgb) => [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8],
    }
}
