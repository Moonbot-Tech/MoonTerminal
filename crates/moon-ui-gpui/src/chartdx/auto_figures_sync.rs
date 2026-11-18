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
//!
//! ## Why a second, passive figure list rather than the store
//!
//! An auto figure must not be saved, hovered, selected, dragged, listed as an alert or upserted to
//! a core. Every one of those paths reads `FigureStore` — persistence, `pick_figure`, the Alerts
//! panel, the alert reconcile — so keeping auto figures OUT of it makes all six guarantees
//! structural instead of a flag each path has to remember (`from_server` shows how many places a
//! flag has to reach). They still draw as figures: each found shape becomes a figure-tool value
//! (`moon_chart::auto_figures::draw`) and goes through `build_passive_figure_geometry`, the same
//! sink and the same tools as the store's figures, so the three GPU backends need no change.
//!
//! The figures are rebuilt from the published data on each rebuild rather than cached, because
//! their colour is resolved against THIS engine's theme at that moment: a theme switch forces the
//! rebuild anyway, and then the lines follow it with nothing extra to invalidate.

use std::rc::Rc;
use std::sync::Arc;

use moon_chart::auto_figures::draw::{ResolvedStyle, append_shapes};
use moon_core::config::ChartTheme;
use moon_core::config::LabelColor;
use moon_core::config::auto_figures::LineStyle;
use moon_core::market::ChartCandle;
use moon_core::session::CoreId;

use super::ChartDataState;
use super::types::CandleGpu;

pub(crate) mod job;

#[cfg(test)]
mod tests;

/// One drawn row of one pane: its detector's outcome and the figures the row keeps.
#[derive(Debug, Clone)]
pub(crate) struct AutoRowDrawn {
    /// The row's index in its kind's configuration — what a later stage's popup, caption and
    /// right-click name the row by.
    pub row_ix: usize,
    /// How the row draws, copied at publication.
    pub style: LineStyle,
    /// The row's touch tolerance in % of price, for the zone band.
    pub tolerance_pct: f32,
    /// Everything the detector run produced, shared with the panel that holds it for later
    /// stages (touches, timeframe, break, short history).
    pub outcome: Arc<job::AutoRowOutcome>,
    /// Indices into `outcome.found` of the kept figures, each with whether it lies ABOVE the
    /// reference price it was selected against (what a `BySign` colour reads).
    pub kept: Vec<(usize, bool)>,
}

/// Everything one pane's auto figures draw.
#[derive(Debug, Clone, Default)]
pub(crate) struct AutoPaneFigures {
    pub rows: Vec<AutoRowDrawn>,
}

impl AutoPaneFigures {
    /// One line describing every row for the app log: its index, timeframe, bar source, bars read
    /// against bars needed (`short` when fewer), figures found and lines kept.
    pub(crate) fn summary(&self) -> String {
        if self.rows.is_empty() {
            return "no rows".to_owned();
        }
        self.rows
            .iter()
            .map(|r| {
                let o = &r.outcome;
                format!(
                    "row {} {:?} {:?} {}/{} bars{} found {} kept {}",
                    r.row_ix,
                    o.key.tf,
                    o.source,
                    o.bars,
                    o.needed,
                    if o.short_history() { " short" } else { "" },
                    o.found.len(),
                    r.kept.len(),
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
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

    /// The published auto figures of one pane target, if any.
    pub(crate) fn auto_figures_of(&self, core: CoreId, market: &str) -> Option<&AutoPaneFigures> {
        self.auto_figures
            .iter()
            .find(|((c, m), _)| *c == core && m == market)
            .map(|(_, set)| set.as_ref())
    }

    /// Appends one pane target's auto figures to the userdata buffers, after the user's figures.
    ///
    /// Called only from the userdata rebuild in `orders::sync`, gated on `figures_sig`. Bands go
    /// into the zone buffer BEFORE that rebuild hashes it, so a zone that appears or moves re-bakes
    /// the base cache exactly like an order zone or a figure fill.
    pub(super) fn append_auto_figure_geometry(
        &self,
        core: CoreId,
        market: &str,
        epoch_ms: f64,
        out: &mut moon_chart::figures::FigureBuffers<'_>,
    ) {
        // Book-only mode leaves a 1 px plot with no time axis; the news layer skips it too.
        if self.orderbook_only {
            return;
        }
        let Some(set) = self.auto_figures_of(core, market) else {
            return;
        };
        let mut figures = Vec::new();
        let mut bands = Vec::new();
        let mut lines = 0u64;
        for row in &set.rows {
            for &(ix, above) in &row.kept {
                let Some(found) = row.outcome.found.get(ix) else {
                    continue;
                };
                let rgb = resolve_rgb(row.style.color, above, &self.theme);
                append_shapes(
                    found,
                    &ResolvedStyle::new(&row.style, rgb),
                    row.tolerance_pct,
                    &mut figures,
                    &mut bands,
                );
                lines += 1;
            }
        }
        crate::diag::bump_by(&crate::diag::AUTOFIGURES_LINES, lines);
        if figures.is_empty() && bands.is_empty() {
            return;
        }
        moon_chart::figures::build_passive_figure_geometry(&figures, &bands, epoch_ms, out);
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
        native_closed_candles(
            &pr.candle_rows,
            pr.candle_rows_epoch,
            self.candle_view.tf_ms(),
            now_ms,
        )
    }
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
