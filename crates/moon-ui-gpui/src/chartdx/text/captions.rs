//! Laying the configured captions out around one pane's plot and drawing them.
//!
//! The zone rule, stated once because two spellings of it drift: a zone owns a corner AND the
//! direction its rows fill. A `*Left` zone lays a row out from the left edge rightwards, a `*Right`
//! zone from the right edge leftwards, and a `*Center` zone measures the row first and centres it.
//! The FIRST slot of a row is therefore the outermost one in every zone — "first" would otherwise
//! mean opposite things on opposite sides of the same chart.
//!
//! A pane is two columns, and the zones follow that: `Top*`/`Bottom*` are corners of the PLOT,
//! while `ZoneTop`/`ZoneBottom` live in the CONTROL ZONE down the right side. The zone is reserved
//! whether or not an order book is drawn — [`super::caption::book_zone_left`] falls back to a
//! book-sized strip — which is why the chart still captions there with the book switched off.
//!
//! `ZoneTop` carries the adaptive behaviour the hard-coded caption always had: over a book with
//! chart to its left it SPLITS into two columns — the first row's HEAD centres on the zone, while
//! the second row and the first row's inline tail hang off the zone's left edge. That is a property
//! of the ZONE, not a setting. The two columns get separate plates on purpose: one plate spanning
//! both would darken the whole run of candles between them.

use gpui::{Hsla, point, px};
use moon_core::config::{
    CHART_LABEL_SLOTS, ChartLabelsCfg, LabelColor, LabelZone, ResolvedLabelStyle,
};
use moon_core::util::fmt::DeltaSign;

use super::caption::{CaptionBox, CaptionGeom, CaptionLayout, caption_geom, caption_layout};
use super::labels::LabelText;
use super::{CAPTION_PAD_X, CAPTION_PAD_Y};
use crate::chartdx::RenderState;

/// Horizontal gap between two captions sharing a row.
const INLINE_GAP: f32 = 8.0;
/// Gap between the split corner's two columns.
const SPLIT_GAP: f32 = 8.0;
/// Inset of a non-`TopRight` zone from the plot's edges.
const ZONE_PAD: f32 = 6.0;
/// Smallest width a caption is truncated to before it is dropped instead.
const MIN_LEGIBLE_W: f32 = 12.0;
/// Number of backing plates a pane publishes: one per zone, plus the split column that `TopRight`
/// grows when it shares the corner with an order book.
pub(in crate::chartdx) const CAPTION_PLATES: usize = LabelZone::ALL.len() + 1;
/// Index of the plate belonging to `TopRight`'s split column.
const SPLIT_PLATE: usize = LabelZone::ALL.len();

/// The pane rectangles a caption pass needs, in LOGICAL pixels.
#[derive(Clone, Copy)]
pub(in crate::chartdx) struct CaptionGeomInput {
    pub pane_left: f32,
    pub pane_right: f32,
    pub plot_left: f32,
    pub plot_right: f32,
    pub plot_top: f32,
    pub plot_bottom: f32,
    pub orderbook_enabled: bool,
    pub orderbook_left: f32,
    pub scale_factor: f32,
}

/// One caption prepared for drawing: where its text lives and how it is styled.
#[derive(Clone, Copy)]
struct Item {
    /// Index into the pane's resolved caption list.
    pos: usize,
    /// Slot index, which addresses this caption's retained text run.
    slot: usize,
    style: ResolvedLabelStyle,
    /// Font size in logical pixels, already resolved and clamped.
    size: f32,
}

impl Item {
    /// Height of one line at this caption's size, matching what `draw_aligned` is given.
    fn line_h(self) -> f32 {
        self.size + 4.0
    }
}

/// One row: the captions on it, in order.
struct Row {
    items: Vec<Item>,
}

impl Row {
    /// Tallest line on this row.
    ///
    /// Known from the STYLES, before anything is measured or drawn — which is what lets a
    /// bottom-anchored zone place its first row without having to draw it first.
    fn height(&self) -> f32 {
        self.items
            .iter()
            .map(|it| it.line_h())
            .fold(0.0_f32, f32::max)
    }
}

/// How one column of captions is anchored.
#[derive(Clone, Copy)]
struct Column {
    /// The x every row anchors against.
    x: f32,
    /// Alignment fraction at that x: 0 left, 0.5 centred, 1 right.
    align: f32,
    /// Widest a row may become before its captions are truncated.
    max_w: f32,
}

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
        let cfg = self.chart_labels;
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
            CAPTION_PAD_X,
            CAPTION_PAD_Y,
        );
        // Taken out for the duration of the pass rather than cloned per caption: the texts live on
        // the pane and the runs live on `self`, and this pass runs on every presented frame.
        // `label_placed` beside it uses the same take-and-return.
        let texts = std::mem::take(&mut self.panes[idx].labels.texts);
        let result = self.draw_all_zones(
            ctx,
            idx,
            &cfg,
            &texts,
            geom,
            corner,
            caption_fg,
            &mut plates,
        );
        self.panes[idx].labels.texts = texts;
        result?;
        let changed = self.panes[idx].caption_plates != plates;
        if changed {
            self.panes[idx].caption_plates = plates;
        }
        Ok(changed)
    }

    /// Draw every zone that has captions in it.
    #[allow(clippy::too_many_arguments)]
    fn draw_all_zones(
        &mut self,
        ctx: &mut gpui::GpuCanvasTextContext<'_>,
        idx: usize,
        cfg: &ChartLabelsCfg,
        texts: &[LabelText],
        geom: CaptionGeomInput,
        corner: Option<CaptionGeom>,
        caption_fg: Hsla,
        plates: &mut [[f32; 4]; CAPTION_PLATES],
    ) -> anyhow::Result<()> {
        let Some(corner) = corner else {
            return Ok(());
        };
        for zone in LabelZone::ALL {
            let rows = self.collect_rows(cfg, texts, zone);
            if rows.is_empty() {
                continue;
            }
            // Only the zone's TOP splits: that is the spot the caption has always occupied, and
            // the split exists to put the coin over the book with its qualifier beside it.
            let split = matches!(zone, LabelZone::ZoneTop)
                .then(|| caption_layout(&corner, geom.plot_left))
                .filter(|lay| lay.split);
            let zone_ix = LabelZone::ALL
                .iter()
                .position(|z| *z == zone)
                .unwrap_or_default();
            let (main_box, split_box) = match split {
                Some(lay) => {
                    self.draw_split_corner(ctx, idx, texts, &rows, &corner, &lay, caption_fg)?
                }
                None => {
                    let column = zone_column(zone, &geom, &corner);
                    let start_y = zone_start_y(zone, &geom, &corner);
                    let boxed = self.draw_stack(
                        ctx,
                        idx,
                        texts,
                        &rows,
                        column,
                        start_y,
                        zone.is_top(),
                        caption_fg,
                    )?;
                    (boxed, CaptionBox::default())
                }
            };
            plates[zone_ix] = main_box.plate(geom.scale_factor);
            if split.is_some() {
                plates[SPLIT_PLATE] = split_box.plate(geom.scale_factor);
            }
        }
        Ok(())
    }

    /// Group one zone's resolved captions into rows, honouring the inline flag.
    fn collect_rows(&self, cfg: &ChartLabelsCfg, texts: &[LabelText], zone: LabelZone) -> Vec<Row> {
        let base = self.label_font_px();
        let mut rows: Vec<Row> = Vec::new();
        for (pos, text) in texts.iter().enumerate() {
            let Some(slot) = cfg.slots.get(text.slot) else {
                continue;
            };
            if slot.zone != zone {
                continue;
            }
            let style = slot.resolved_style();
            let item = Item {
                pos,
                slot: text.slot,
                style,
                size: (base * style.size_mult).clamp(6.0, 60.0),
            };
            // An inline slot joins the row before it. `sanitize` guarantees the first DRAWN slot of
            // a zone is never inline, but a caption that resolved to nothing can still leave an
            // inline one first here — it has no row to join either, so it opens its own.
            if slot.inline && !rows.is_empty() {
                rows.last_mut().expect("checked non-empty").items.push(item);
            } else {
                rows.push(Row { items: vec![item] });
            }
        }
        rows
    }

    /// Draw rows stacked in one column, downward for a top zone and upward for a bottom one.
    #[allow(clippy::too_many_arguments)]
    fn draw_stack(
        &mut self,
        ctx: &mut gpui::GpuCanvasTextContext<'_>,
        idx: usize,
        texts: &[LabelText],
        rows: &[Row],
        column: Column,
        start_y: f32,
        downward: bool,
        caption_fg: Hsla,
    ) -> anyhow::Result<CaptionBox> {
        let mut plate = CaptionBox::default();
        let mut y = start_y;
        for row in rows {
            let row_h = row.height();
            // Runs are drawn top-anchored, so an upward stack subtracts the row's height BEFORE
            // drawing it. Taking that height from the styles rather than from a measurement is what
            // makes this possible without drawing the row twice.
            let top = if downward { y } else { y - row_h };
            self.draw_row(
                ctx, idx, texts, &row.items, column, top, caption_fg, &mut plate,
            )?;
            y += if downward { row_h } else { -row_h };
        }
        Ok(plate)
    }

    /// Draw `TopRight` the way it has always drawn over an order book.
    ///
    /// Returns the two plates: the column over the book, and the one hanging off its left edge.
    #[allow(clippy::too_many_arguments)]
    fn draw_split_corner(
        &mut self,
        ctx: &mut gpui::GpuCanvasTextContext<'_>,
        idx: usize,
        texts: &[LabelText],
        rows: &[Row],
        corner: &CaptionGeom,
        lay: &CaptionLayout,
        caption_fg: Hsla,
    ) -> anyhow::Result<(CaptionBox, CaptionBox)> {
        let mut main = CaptionBox::default();
        let mut left = CaptionBox::default();
        let cap_y = corner.top_y;
        let head_row = &rows[0];
        let head_h = head_row.height();
        let over_book = Column {
            x: lay.coin_x,
            align: lay.coin_ax,
            max_w: lay.coin_max_w,
        };
        // The row's HEAD centres over the book; anything else on that row belongs to the left
        // column, which is where the scale badge has always been drawn in this layout.
        let split_at = 1.min(head_row.items.len());
        let (head, tail) = head_row.items.split_at(split_at);
        self.draw_row(
            ctx, idx, texts, head, over_book, cap_y, caption_fg, &mut main,
        )?;
        // The second row hangs off the book's left edge, vertically centred against the taller head
        // row: aligned by their tops, the smaller one reads as having slipped downward.
        let mut left_edge = lay.core_x;
        if let Some(second) = rows.get(1) {
            let column = Column {
                x: lay.core_x,
                align: 1.0,
                max_w: lay.core_max_w,
            };
            let top = cap_y + ((head_h - second.height()) * 0.5).max(0.0);
            let drawn = self.draw_row(
                ctx,
                idx,
                texts,
                &second.items,
                column,
                top,
                caption_fg,
                &mut left,
            )?;
            left_edge = left_edge.min(drawn);
        }
        // The head row's tail sits further left still, on the head's own top edge — it is usually
        // the scale badge, the tallest run in the caption.
        if !tail.is_empty() {
            let x = left_edge - SPLIT_GAP;
            let column = Column {
                x,
                align: 1.0,
                max_w: (x - corner.zone_left).max(0.0),
            };
            self.draw_row(ctx, idx, texts, tail, column, cap_y, caption_fg, &mut left)?;
        }
        // Rows below the first follow the head's column, stacked under it.
        let mut y = cap_y + head_h;
        for row in rows.iter().skip(2) {
            self.draw_row(
                ctx, idx, texts, &row.items, over_book, y, caption_fg, &mut main,
            )?;
            y += row.height();
        }
        Ok((main, left))
    }

    /// Draw one row of captions, returning the row's LEFT edge.
    ///
    /// Only slots that ask for a plate grow `plate`: the backing is a per-caption setting, and a
    /// box grown by every drawn run would put a plate under a caption that switched it off.
    #[allow(clippy::too_many_arguments)]
    fn draw_row(
        &mut self,
        ctx: &mut gpui::GpuCanvasTextContext<'_>,
        idx: usize,
        texts: &[LabelText],
        items: &[Item],
        column: Column,
        top: f32,
        caption_fg: Hsla,
        plate: &mut CaptionBox,
    ) -> anyhow::Result<f32> {
        if items.is_empty() {
            return Ok(column.x);
        }
        // A centred row must know its own width before it can be placed; the other two directions
        // walk out from their edge and never need the total.
        let start_x = if column.align > 0.0 && column.align < 1.0 {
            column.x - self.measure_row(ctx, texts, items, column.max_w) * 0.5
        } else {
            column.x
        };
        // Right-anchored rows draw at their right edge and walk LEFT; the other two draw at their
        // left edge and walk right. One `ax` per direction, the same arithmetic mirrored.
        let rightwards = column.align < 1.0;
        let ax = if rightwards { 0.0 } else { 1.0 };
        let mut cursor = start_x;
        let mut left_edge = start_x;
        let mut budget = column.max_w;
        for (n, item) in items.iter().enumerate() {
            let Some(entry) = texts.get(item.pos) else {
                continue;
            };
            let gap = if n == 0 { 0.0 } else { INLINE_GAP };
            budget -= gap;
            // No room left on this row. Dropping the rest is deliberate: a caption clipped
            // mid-number reads as a plausible WRONG number.
            if budget < MIN_LEGIBLE_W {
                break;
            }
            let (text, w) = fit_caption(ctx, &entry.text, item, budget);
            if text.is_empty() {
                continue;
            }
            let color = self.caption_color(item.style.color, entry.sign, caption_fg);
            let draw_x = if rightwards {
                cursor + gap
            } else {
                cursor - gap
            };
            let metrics = self.draw_caption_run(
                ctx, idx, item.slot, &text, item.size, draw_x, top, ax, color,
            )?;
            crate::diag::bump(&crate::diag::CHART_CAPTION_DRAW);
            let drawn_w = metrics.width.as_f32().max(w);
            let box_left = if rightwards { draw_x } else { draw_x - drawn_w };
            left_edge = left_edge.min(box_left);
            if item.style.plate {
                plate.add(box_left, drawn_w, top, metrics.line_height.as_f32());
            }
            cursor = if rightwards {
                draw_x + drawn_w
            } else {
                draw_x - drawn_w
            };
            budget -= drawn_w;
        }
        Ok(left_edge)
    }

    /// Width of a row once every caption is truncated to the budget it would actually get.
    fn measure_row(
        &self,
        ctx: &gpui::GpuCanvasTextContext<'_>,
        texts: &[LabelText],
        items: &[Item],
        max_w: f32,
    ) -> f32 {
        let mut total = 0.0;
        let mut budget = max_w;
        for (n, item) in items.iter().enumerate() {
            let Some(entry) = texts.get(item.pos) else {
                continue;
            };
            let gap = if n == 0 { 0.0 } else { INLINE_GAP };
            budget -= gap;
            if budget < MIN_LEGIBLE_W {
                break;
            }
            let (_, w) = fit_caption(ctx, &entry.text, item, budget);
            total += gap + w;
            budget -= w;
        }
        total
    }

    /// Resolve one caption's color.
    fn caption_color(&self, mode: LabelColor, sign: Option<DeltaSign>, caption_fg: Hsla) -> Hsla {
        match mode {
            LabelColor::Theme => caption_fg,
            LabelColor::Fixed(rgb) => gpui::rgb(rgb).into(),
            LabelColor::BySign => match sign {
                Some(DeltaSign::Positive) => gpui::rgb(self.label_positive).into(),
                Some(DeltaSign::Negative) => gpui::rgb(self.label_negative).into(),
                // A figure that rounds to zero, or has no sign at all, keeps the caption color
                // rather than being coloured as a gain.
                _ => caption_fg,
            },
        }
    }

    /// Draw one caption through its OWN retained run.
    ///
    /// The run is addressed by `pane * CHART_LABEL_SLOTS + slot`, never by a running cursor: a
    /// caption that stops resolving must not hand its run to the next one, which would reshape both
    /// strings on every frame for as long as the pane stays that way.
    #[allow(clippy::too_many_arguments)]
    fn draw_caption_run(
        &mut self,
        ctx: &mut gpui::GpuCanvasTextContext<'_>,
        pane_ix: usize,
        slot_ix: usize,
        text: &str,
        size: f32,
        x: f32,
        y: f32,
        ax: f32,
        color: Hsla,
    ) -> anyhow::Result<gpui::GpuCanvasTextMetrics> {
        let run_ix = pane_ix * CHART_LABEL_SLOTS + slot_ix;
        if self.caption_runs.len() <= run_ix {
            self.caption_runs
                .resize_with(run_ix + 1, gpui::GpuCanvasTextRun::default);
        }
        self.caption_runs[run_ix].draw_aligned(
            ctx,
            point(px(x), px(y)),
            text,
            gpui::font(crate::design::mono()),
            px(size),
            px(size + 4.0),
            color,
            ax,
            0.0,
        )
    }

    /// Re-resolve one pane's captions from the values it currently holds.
    ///
    /// Called from the SYNC paths — a market revision or an order revision — never from the frame
    /// path: this is where strings are built, and `prepare_text` runs on every presented frame.
    ///
    /// Returns whether the drawn captions changed, so the caller can repaint only when they did.
    pub(in crate::chartdx) fn refresh_pane_labels(&mut self, idx: usize) -> bool {
        let cfg = self.chart_labels;
        let Some(pr) = self.panes.get(idx) else {
            return false;
        };
        // Comparison delta: this pane's own last price against the anchor's, and only while the
        // pane is a book-only broom follower. Either half missing means there is nothing to
        // compare — which prints no caption rather than a zero.
        let compare_pct = self
            .compare_ref_price
            .filter(|_| pr.orderbook_only)
            .zip(pr.cached_last_price)
            .filter(|(r, l)| *r > 0.0 && *l > 0.0)
            .map(|(r, l)| (l - r) / r * 100.0);
        let inputs = super::LabelInputs {
            ticker: pr.ticker.clone(),
            core_name: pr.core_name.clone(),
            venue: pr.venue.clone(),
            strategy: pr.label_strategy.clone(),
            last_price: pr.cached_last_price,
            scale_badge: pr.scale_badge,
            compare_pct,
            delta_1h: pr.delta_1h,
            delta_24h: pr.delta_24h,
            basis: pr.label_basis,
        };
        let changed = self.panes[idx].labels.update(&cfg, inputs);
        if changed {
            crate::diag::bump(&crate::diag::CHART_CAPTION_REBUILD);
        }
        changed
    }
}

/// Truncate one caption to the width it is allowed, returning the text and its measured width.
///
/// THE one place the truncation rule lives. The drawing pass and the measuring pass both go
/// through it because their answers have to agree: a centred row placed from one rule and drawn by
/// another walks off its own centre. Truncating at all is what the fixed caption always did to the
/// coin and the core name — without it a long core name runs past the plot's left edge.
fn fit_caption(
    ctx: &gpui::GpuCanvasTextContext<'_>,
    text: &str,
    item: &Item,
    budget: f32,
) -> (String, f32) {
    crate::design::fit_text(text, budget, |s| {
        super::measure_run_width(ctx, s, item.size)
    })
}

/// Anchor and width budget for an ordinary zone.
fn zone_column(zone: LabelZone, geom: &CaptionGeomInput, corner: &CaptionGeom) -> Column {
    let plot_w = (geom.plot_right - geom.plot_left - 2.0 * ZONE_PAD).max(0.0);
    // The control strip's own width, which is what its two zones are measured against — never the
    // plot's, or a caption there would run out over the candles.
    let zone_w = (corner.right_x - corner.zone_left).max(0.0);
    match zone {
        // The plot's own right edge, left of the control strip. This is what "right" means to a
        // reader looking at the candles.
        LabelZone::TopRight | LabelZone::BottomRight => Column {
            x: geom.plot_right - ZONE_PAD,
            align: 1.0,
            max_w: plot_w,
        },
        // The control strip. `caption_layout` is deferred to even when it does NOT split: on a
        // narrow pane, or in book-only broom mode, it LEFT-anchors inside the strip rather than
        // right-anchoring at the pane edge, and re-deriving that here is how the block ended up on
        // the opposite side of such panes.
        LabelZone::ZoneTop => {
            let lay = caption_layout(corner, geom.plot_left);
            Column {
                x: lay.coin_x,
                align: lay.coin_ax,
                max_w: lay.coin_max_w,
            }
        }
        LabelZone::ZoneBottom => Column {
            x: corner.right_x,
            align: 1.0,
            max_w: zone_w,
        },
        LabelZone::TopLeft | LabelZone::BottomLeft => Column {
            x: geom.plot_left + ZONE_PAD,
            align: 0.0,
            max_w: plot_w,
        },
        LabelZone::TopCenter | LabelZone::BottomCenter => Column {
            x: (geom.plot_left + geom.plot_right) * 0.5,
            align: 0.5,
            max_w: plot_w,
        },
    }
}

/// Y of a zone's first row: below the plot's top edge, or above its bottom one.
fn zone_start_y(zone: LabelZone, geom: &CaptionGeomInput, corner: &CaptionGeom) -> f32 {
    if zone.is_top() {
        match zone {
            // The control strip keeps the inset its own geometry resolved, which clears the pane's
            // close button; the plot's corners only clear the plot edge.
            LabelZone::ZoneTop => corner.top_y,
            _ => geom.plot_top + ZONE_PAD,
        }
    } else {
        // Both bottom families share the plot's lower edge: the control strip runs the full height
        // of the plot, so its floor is the same line.
        geom.plot_bottom - ZONE_PAD
    }
}
