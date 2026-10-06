//! Laying the configured captions out around one pane's plot and drawing them.
//!
//! The layout rule, stated once because two spellings of it drift: a row's ALIGNMENT owns the
//! direction it fills. A left-aligned row lays its captions out from the left edge rightwards, a
//! right-aligned one from the right edge leftwards, and a centred one is measured first and then
//! centred. The FIRST caption of a row is therefore the outermost one either way — "first" would
//! otherwise mean opposite things on opposite sides of the same chart.
//!
//! A pane is two columns, and the bands follow that: `ChartTop`/`ChartBottom` are edges of the
//! PLOT, while `ZoneTop`/`ZoneBottom` live in the CONTROL ZONE down the right side. The zone is
//! reserved whether or not an order book is drawn — [`super::caption::book_zone_left`] falls back
//! to a book-sized strip — which is why the chart still captions there with the book switched off.
//!
//! Everything a band holds is drawn INSIDE that band. The control strip measures its rows against
//! its own width; the plot's bands measure against the plot. The old caption hung its second line
//! off the strip's left edge, out over the candles — that is gone: WHICH caption ended up out there
//! depended on its position in the list, so "I chose the strip and it drew on the chart" was the
//! honest reading of it.
//!
//! A zone holding a band that WRAPS also divides its width rather than handing all of it to each
//! band in turn: the bands printing figures are drawn first, and the wrapping one is drawn last,
//! into what they left. A COLUMN — strategy-filter skip lines, the venue roster — is drawn between
//! those two: a line of it yields to figures that share its Y, and spends the rest of the zone
//! once those figures have ended, rather than wrapping at half the plot beside empty candles. See
//! [`widths`]. Before the wrap split,
//! a long detect line printed straight through whatever was pinned to the left and the right of it.
//! Two different zones are never divided against each other — each stays inside its own band, which
//! is the paragraph above.

use gpui::{Hsla, point};

use super::{chart_metrics, content_px};
use moon_core::config::{
    ARB_PART_BASE, CHART_LABEL_ROWS, ChartLabelField, ChartLabelRow, ChartLabelsCfg,
    FILTER_HEADER_PART, LABEL_WRAP_LINES, LabelAlign, LabelColor, LabelZone, PREFIX_PART_BASE,
    ROW_NAME_PART, ROW_RUN_STRIDE, ResolvedLabelStyle, WRAP_PART_BASE,
};
use moon_core::util::fmt::DeltaSign;
use rust_i18n::t;

use super::caption::{CaptionBox, CaptionGeom, caption_geom};
use super::labels::{LabelAction, LabelText};
use super::{CAPTION_PAD_X, CAPTION_PAD_Y};
use crate::chartdx::RenderState;
use crate::chartdx::{ActionPlacement, ArbHit, VolumeHit};

mod fit;
mod fit_memo;
mod measure;
mod model;
mod plan;
mod refresh;
mod stack;
mod zone;

pub(in crate::chartdx) use fit_memo::FitMemo;
pub(in crate::chartdx) use model::{ActionDraw, CAPTION_PLATES, CaptionBar, CaptionGeomInput};
use model::{Band, Column, HungryBudget};
pub(in crate::chartdx) use plan::RowPlanCache;
use zone::{elastic_band, zone_anchor, zone_bounds, zone_start_y, zone_width};

impl RenderState {
    /// Draw every configured caption for one pane and publish its backing plates.
    ///
    /// Returns whether the plates moved, which the caller folds into its readout-metrics flag.
    pub(in crate::chartdx) fn draw_pane_captions(
        &mut self,
        ctx: &mut gpui::GpuCanvasTextContext<'_>,
        idx: usize,
        geom: CaptionGeomInput,
        caption_fg: Hsla,
    ) -> anyhow::Result<bool> {
        let cfg = self.chart_labels.clone();
        let mut plates = [[0.0f32; 4]; CAPTION_PLATES];
        // The corner the order book shares. `None` means the pane is too small to caption at all,
        // which suppresses the WHOLE pass — exactly as it did before captions were configurable.
        let corner = caption_geom(
            geom.pane_left,
            geom.pane_right,
            geom.plot_left,
            geom.plot_right,
            geom.plot_top,
            geom.orderbook_enabled,
            geom.orderbook_left,
            self.order_book_width_px,
            CAPTION_PAD_X,
            CAPTION_PAD_Y,
        );
        // Taken out for the duration of the pass rather than cloned per caption: the texts live on
        // the pane and the runs live on `self`, and this pass runs on every presented frame.
        // `label_placed` beside it uses the same take-and-return.
        let texts = std::mem::take(&mut self.panes[idx].labels.texts);
        // Taken and returned like the texts above: the hit rectangles are rebuilt every frame and
        // the buffer is reused, so a chart with an arbitrage column allocates nothing per frame.
        let mut hits = std::mem::take(&mut self.panes[idx].arb_hits);
        hits.clear();
        // The SCRATCH buffer, not the published one: what was published has to stay in place until
        // the comparison below, or "did the geometry move" is answered against an empty vector.
        let mut bars = std::mem::take(&mut self.panes[idx].caption_bars_scratch);
        bars.clear();
        let mut vol_hits = std::mem::take(&mut self.panes[idx].volume_boxes);
        vol_hits.clear();
        // Rebuilt every frame like the arbitrage rectangles above: a press has to hit what the last
        // frame drew, and a button moves with the pane it stands in. Taken and returned, so a chart
        // carrying buttons allocates nothing per frame.
        let mut act_draws = std::mem::take(&mut self.panes[idx].action_draws);
        act_draws.clear();
        // Cleared before the pass, so a frame that fails leaves no rectangle behind: the panel
        // would otherwise keep a button standing where nothing was drawn.
        self.panes[idx].action_rects.clear();
        self.panes[idx].filter_header_hits.clear();
        self.panes[idx].column_bands.clear();
        self.panes[idx].filter_strip = None;
        // The wrapped lines belong to THIS pane's pass. Cleared rather than dropped so the
        // allocation is reused, and cleared HERE because the indices `Item` holds are handed out
        // during the pass: carrying entries across panes would leak a Vec per frame and let a
        // stale index draw the previous pane's sentence.
        self.caption_wraps.clear();
        let result = self.draw_all_zones(
            ctx,
            idx,
            &cfg,
            &texts,
            geom,
            corner,
            caption_fg,
            &mut plates,
            &mut hits,
            &mut bars,
            &mut vol_hits,
            &mut act_draws,
        );
        self.panes[idx].labels.texts = texts;
        self.panes[idx].arb_hits = hits;
        if let Err(error) = result {
            self.panes[idx].filter_header_hits.clear();
            self.panes[idx].column_bands.clear();
            // Nothing was drawn: the rectangles cleared before the pass stay cleared, so a press
            // cannot land on a button from a frame that was thrown away, and the build buffer goes
            // back rather than being dropped.
            act_draws.clear();
            self.panes[idx].action_draws = act_draws;
            return Err(error);
        }
        // A column's header joins its band only once the column has lines: a collapsed column keeps
        // no band, so the wheel over its header still pans the chart.
        {
            let pane = &mut self.panes[idx];
            for hit in &pane.filter_header_hits {
                if pane.column_bands.iter().any(|band| band.row == hit.row) {
                    crate::chartdx::ColumnBand::grow(&mut pane.column_bands, hit.row, hit.rect);
                }
            }
            pane.filter_strip = crate::chartdx::ColumnBand::filter_strip(
                &pane.column_bands,
                &cfg,
                [
                    geom.plot_left,
                    geom.plot_top,
                    geom.plot_right,
                    geom.plot_bottom,
                ],
            );
        }
        for bar in &mut bars {
            let sf = geom.scale_factor;
            bar.dst = [
                bar.dst[0] * sf,
                bar.dst[1] * sf,
                bar.dst[2] * sf,
                bar.dst[3] * sf,
            ];
        }
        // The bars ride the plates' own "did the geometry move" flag: both are published to the
        // readout batch, and a bar whose fill changed has to reach it exactly like a plate that
        // moved.
        // Where each button GOES, in the pane's own logical pixels. The panel reads these on its
        // next render and places one of the application's own buttons in each — the chart's pass
        // cannot host a GPUI element, so the two halves meet on this rectangle and nothing else.
        let pane = &mut self.panes[idx];
        for draw in &act_draws {
            let [x, y, w, h] = draw.rect;
            if w <= 0.0 || h <= 0.0 {
                continue;
            }
            pane.action_rects.push(ActionPlacement {
                x,
                y,
                w,
                h,
                size: draw.size,
                mark: draw.mark,
                row: draw.row,
                part: draw.part,
            });
        }
        act_draws.clear();
        pane.action_draws = act_draws;
        let changed =
            self.panes[idx].caption_plates != plates || self.panes[idx].caption_bars != bars;
        if changed {
            self.panes[idx].caption_plates = plates;
            // Swapped rather than assigned: the published bars become the next pass's scratch, so
            // neither buffer is reallocated and what is published is replaced only when it moved.
            std::mem::swap(&mut self.panes[idx].caption_bars, &mut bars);
        }
        self.panes[idx].caption_bars_scratch = bars;
        // Rebuilt in place so a chart with a volume block allocates nothing per frame.
        let hits = &mut self.panes[idx].volume_hits;
        hits.clear();
        hits.extend(vol_hits.iter().filter_map(|(row, box_)| {
            let (x, y, w, h) = box_.bounds()?;
            Some(VolumeHit {
                x,
                y,
                w,
                h,
                row: *row,
            })
        }));
        self.panes[idx].volume_boxes = vol_hits;
        Ok(changed)
    }

    /// Draw every zone that has captions in it.
    #[allow(clippy::too_many_arguments)]
    fn draw_all_zones(
        &mut self,
        ctx: &mut gpui::GpuCanvasTextContext<'_>,
        idx: usize,
        cfg: &std::rc::Rc<ChartLabelsCfg>,
        texts: &[LabelText],
        geom: CaptionGeomInput,
        corner: Option<CaptionGeom>,
        caption_fg: Hsla,
        plates: &mut [[f32; 4]; CAPTION_PLATES],
        hits: &mut Vec<ArbHit>,
        bars: &mut Vec<CaptionBar>,
        vol_hits: &mut Vec<(usize, CaptionBox)>,
        act_draws: &mut Vec<ActionDraw>,
    ) -> anyhow::Result<()> {
        let Some(corner) = corner else {
            return Ok(());
        };
        let mut cache = self.panes[idx].caption_rows.take();
        let working = RowPlanCache::working_rows(
            &mut cache,
            self.panes[idx].labels.generation,
            cfg,
            self.label_font_px(),
            |zone, align| self.collect_rows(cfg, texts, zone, align),
        );
        // Restore before drawing: even a failed draw cannot mutate or lose the pristine plan.
        self.panes[idx].caption_rows = cache;
        for (zone, rows) in LabelZone::ALL.into_iter().zip(working) {
            // Every band of the zone is gathered before ANY of them is drawn. Three passes that
            // each spent the whole zone are exactly what printed a centred detect line over the
            // modules pinned to either edge — neither pass could see the other.
            let mut rows = rows.into_iter();
            let mut bands = LabelAlign::ALL
                .map(|align| Band::new(align, rows.next().expect("one plan per alignment")));
            if bands.iter().all(Band::is_empty) {
                continue;
            }
            let total = zone_width(zone, &geom, &corner);
            // How much the elastic band may hold its neighbours to — see [`widths`]. `None` leaves
            // the zone undivided, which is what a cramped zone and a zone holding two elastic bands
            // both get: captions that touch beat a caption that vanished.
            let cap = match elastic_band(&bands) {
                Some(band) => widths::edge_cap(total, self.prose_width(ctx, texts, &band.rows)),
                None => None,
            };
            // Rows fill toward the far edge of the band and stop there.
            let downward = zone.is_top();
            let limit_y = if downward {
                geom.plot_bottom
            } else {
                geom.plot_top
            };
            // FIGURES first, COLUMNS next, wrapping prose last. A column has a natural width, but
            // that width is the longest skip line — routinely the whole plot — so it is drawn after
            // the figures and spends only what they left. Stable sort keeps `LabelAlign::ALL`'s
            // order inside each rank.
            let mut order = [0usize, 1, 2];
            order.sort_by_key(|&n| widths::band_rank(bands[n].elastic, bands[n].hungry));
            let mut taken = widths::Taken::default();
            for n in order {
                if bands[n].is_empty() {
                    continue;
                }
                let (align, elastic, hungry) = (bands[n].align, bands[n].elastic, bands[n].hungry);
                let start_y = zone_start_y(zone, align, &geom, &corner);
                let (x, fraction) = zone_anchor(zone, align, &geom, &corner);
                let base_left = zone_bounds(zone, &geom, &corner).0;
                // A special left anchor spends part of the zone before its text begins, so remove
                // that inset from the text budget instead of letting a long caption cross the far
                // edge by the same amount.
                let anchor_inset = if align == LabelAlign::Left {
                    (x - base_left).max(0.0)
                } else {
                    0.0
                };
                let max_w = (widths::band_max_w(total, cap, elastic, hungry, align, taken)
                    - anchor_inset)
                    .max(0.0);
                let column = Column {
                    x,
                    align: fraction,
                    max_w,
                };
                // Height of a wrapped caption is not known from its style. Prose is measured here
                // at one budget. A hungry column wraps per line inside `draw_stack`, because a
                // line that has cleared the neighbours may spend the rest of the zone.
                if elastic {
                    self.plan_wraps(ctx, texts, &mut bands[n].rows, column);
                }
                // Per-line widening only when nothing wrapping is still waiting to be drawn: a
                // detect line in the centre is laid out last, so its height is not in `taken` yet
                // and the figure cap has to keep the column off it. Without one, a short core name
                // is already in `taken` and lines below it can run the width of the plot.
                let hungry_budget = (hungry && !elastic && cap.is_none()).then_some(HungryBudget {
                    total,
                    align,
                    taken,
                    inset: anchor_inset,
                });
                let (module_plates, used_w, used_h) = self.draw_stack(
                    ctx,
                    idx,
                    texts,
                    &mut bands[n].rows,
                    column,
                    start_y,
                    limit_y,
                    downward,
                    caption_fg,
                    hits,
                    bars,
                    vol_hits,
                    act_draws,
                    hungry_budget,
                )?;
                taken.set_extent(align, used_w, used_h);
                // A module lives in exactly one band, so a band writes only its own slots and
                // cannot overwrite another's.
                for (module_ix, box_) in module_plates {
                    let Some(slot) = plates.get_mut(module_ix) else {
                        continue;
                    };
                    *slot = box_.plate(geom.scale_factor);
                }
            }
        }
        Ok(())
    }
}

mod widths;

#[cfg(test)]
mod tests;
