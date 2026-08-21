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

use gpui::{Hsla, point, px};
use moon_core::config::{
    ARB_PART_BASE, CHART_LABEL_ROWS, ChartLabelRow, ChartLabelsCfg, LabelAlign, LabelColor,
    LabelZone, PREFIX_PART_BASE, ROW_NAME_PART, ROW_RUN_STRIDE, ResolvedLabelStyle,
};
use moon_core::util::fmt::DeltaSign;

use super::caption::{CaptionBox, CaptionGeom, caption_geom};
use super::labels::LabelText;
use super::{CAPTION_PAD_X, CAPTION_PAD_Y};
use crate::chartdx::RenderState;

/// Horizontal gap between two captions on the same row.
const CAPTION_GAP: f32 = 8.0;
/// Inset of a band from the plot's edges.
const ZONE_PAD: f32 = 6.0;
/// Smallest width a caption is truncated to before it is dropped instead.
const MIN_LEGIBLE_W: f32 = 12.0;
/// Number of backing plates a pane publishes: one per band and alignment.
///
/// Rows that share a band but not an alignment are independent stacks — left, centre and right do
/// not overlap horizontally, so each starts at the band's own edge — and each needs its own plate,
/// or one plate would span the gap between two of them and darken the candles in between.
pub(in crate::chartdx) const CAPTION_PLATES: usize = LabelZone::ALL.len() * LabelAlign::ALL.len();

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
    /// Row this caption is printed on, and its index inside that row: together they address the
    /// caption's retained text run.
    row: usize,
    part: usize,
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

/// One column of a printed line: a module's block, or a single caption.
///
/// This is what lets a module that runs DOWN a column still sit beside its neighbour: the module
/// contributes one cell, the cell stacks its own captions, and the line places cells across.
struct Cell {
    /// Space before this column, from the module that owns it. Spent only when the column JOINS a
    /// line; a module that opened the line spends its gap above the line instead.
    gap: f32,
    items: Vec<Item>,
}

impl Cell {
    /// How tall the cell is: its captions stack, so their heights add up.
    fn height(&self) -> f32 {
        self.items.iter().map(|it| it.line_h()).sum()
    }
}

/// One printed line: the columns on it, left to right.
///
/// Which MODULE opened it matters only while the lines are being grouped, and that lives in
/// [`group_lines`]; by the time a line is drawn it is just cells.
struct Row {
    /// Space before this line, from the module that opened it.
    gap: f32,
    cells: Vec<Cell>,
}

impl Row {
    /// Height of the line: the tallest cell on it.
    ///
    /// Known from the STYLES, before anything is measured or drawn — which is what lets a
    /// bottom-anchored band place its first line without having to draw it first.
    fn height(&self) -> f32 {
        self.cells.iter().map(Cell::height).fold(0.0_f32, f32::max)
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
        for (zone_ix, zone) in LabelZone::ALL.into_iter().enumerate() {
            for (align_ix, align) in LabelAlign::ALL.into_iter().enumerate() {
                let rows = self.collect_rows(cfg, texts, zone, align);
                if rows.is_empty() {
                    continue;
                }
                let column = zone_column(zone, align, &geom, &corner);
                let start_y = zone_start_y(zone, &geom, &corner);
                // Rows fill toward the far edge of the band and stop there.
                let downward = zone.is_top();
                let limit_y = if downward {
                    geom.plot_bottom
                } else {
                    geom.plot_top
                };
                let plate = self.draw_stack(
                    ctx, idx, texts, &rows, column, start_y, limit_y, downward, caption_fg,
                )?;
                plates[zone_ix * LabelAlign::ALL.len() + align_ix] = plate.plate(geom.scale_factor);
            }
        }
        Ok(())
    }

    /// Gather this band's captions into the LINES they are printed on, styled and sized.
    ///
    /// The grouping itself is [`group_lines`] — the rule that decides what the chart looks like, and
    /// the only part of this pass that can be checked without a device. What is left here needs the
    /// pane: the font size the styles are measured against.
    fn collect_rows(
        &self,
        cfg: &ChartLabelsCfg,
        texts: &[LabelText],
        zone: LabelZone,
        align: LabelAlign,
    ) -> Vec<Row> {
        let base = self.label_font_px();
        let item = |pos: usize| -> Option<Item> {
            let text = texts.get(pos)?;
            let row_cfg = cfg.rows.get(text.row)?;
            let style = caption_style(row_cfg, text.part)?;
            Some(Item {
                pos,
                row: text.row,
                part: text.part,
                style,
                size: (base * style.size_mult).clamp(6.0, 60.0),
            })
        };
        // A module's gap in the chart's own logical pixels, as the configuration states it.
        let gap_of = |positions: &[usize]| -> f32 {
            positions
                .first()
                .and_then(|pos| texts.get(*pos))
                .and_then(|text| cfg.rows.get(text.row))
                .map_or(0.0, |row| f32::from(row.gap))
        };
        group_lines(cfg, texts, zone, align)
            .into_iter()
            .map(|line| Row {
                // The line is spaced by the module that OPENED it; the gaps of modules that join it
                // space their own columns instead.
                gap: line.first().map_or(0.0, |cell| gap_of(cell)),
                cells: line
                    .into_iter()
                    .map(|cell| Cell {
                        gap: gap_of(&cell),
                        items: cell.into_iter().filter_map(item).collect(),
                    })
                    .filter(|cell: &Cell| !cell.items.is_empty())
                    .collect(),
            })
            .collect()
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
        limit_y: f32,
        downward: bool,
        caption_fg: Hsla,
    ) -> anyhow::Result<CaptionBox> {
        let mut plate = CaptionBox::default();
        let mut y = start_y;
        // Whether anything has actually been PUT on the pane. Not the loop index: the first line
        // of a band is exempt from the clamp below, and "first" means first DRAWN.
        let mut any_drawn = false;
        for row in rows {
            let row_h = row.height();
            // The module's own spacing, in the direction the band runs: below the previous line in
            // a band that fills downward, above it in one that fills upward. On the FIRST line it
            // is the indent from the band's own edge, which is the case an "after this module"
            // reading cannot express at all.
            let gap = row.gap;
            // The band stacks until it runs out of pane. Lines past that are dropped rather than
            // drawn: a caption over the time axis — or outside the pane entirely — reads as a
            // glitch, and the horizontal budget already drops what does not fit the same way.
            //
            // The FIRST line of a band is exempt. A pane can be shorter than one line of text — a
            // broom follower, a compressed stack slot — and the coin caption drew there before this
            // clamp existed; suppressing a band entirely because it is cramped would take the
            // chart's only identification with it.
            let overflows = if downward {
                y + gap + row_h > limit_y
            } else {
                y - gap - row_h < limit_y
            };
            if overflows && any_drawn {
                break;
            }
            y += if downward { gap } else { -gap };
            // Runs are drawn top-anchored, so an upward stack subtracts the row's height BEFORE
            // drawing it. Taking that height from the styles rather than from a measurement is what
            // makes this possible without drawing the row twice.
            let top = if downward { y } else { y - row_h };
            self.draw_row(
                ctx, idx, texts, &row.cells, column, top, limit_y, downward, caption_fg,
                &mut plate,
            )?;
            any_drawn = true;
            y += if downward { row_h } else { -row_h };
        }
        Ok(plate)
    }

    /// Draw one row of captions, returning the row's LEFT edge.
    ///
    /// Only captions that ask for a plate grow `plate`: the backing is a per-caption setting, and a
    /// box grown by every drawn run would put a plate under a caption that switched it off.
    #[allow(clippy::too_many_arguments)]
    fn draw_row(
        &mut self,
        ctx: &mut gpui::GpuCanvasTextContext<'_>,
        idx: usize,
        texts: &[LabelText],
        cells: &[Cell],
        column: Column,
        top: f32,
        // Edge of the band this line lives in, and which way the band fills. A CELL can be taller
        // than the pane on its own — an arbitrage column is one line per venue — and the caller's
        // per-line guard exempts the first line of a band, so the stack is clipped here too.
        limit_y: f32,
        downward: bool,
        caption_fg: Hsla,
        plate: &mut CaptionBox,
    ) -> anyhow::Result<f32> {
        if cells.is_empty() {
            return Ok(column.x);
        }
        // A centred line must know its own width before it can be placed; the other two directions
        // walk out from their edge and never need the total.
        let start_x = if column.align > 0.0 && column.align < 1.0 {
            column.x - self.measure_row(ctx, texts, cells, column.max_w) * 0.5
        } else {
            column.x
        };
        // Right-anchored lines draw at their right edge and walk LEFT; the other two draw at their
        // left edge and walk right. One `ax` per direction, the same arithmetic mirrored.
        let rightwards = column.align < 1.0;
        let ax = if rightwards { 0.0 } else { 1.0 };
        let mut cursor = start_x;
        let mut left_edge = start_x;
        let mut budget = column.max_w;
        for (n, cell) in cells.iter().enumerate() {
            // The base spacing keeps two columns from touching; the module's own gap is ADDED to
            // it, so `0` means "as before" and any value means "and this much more".
            //
            // The FIRST column takes none of it. Its module is the one that OPENED this line, and
            // that gap has already been spent on the space above the line — spending it twice
            // would move the line diagonally instead of spacing it.
            let gap = if n == 0 { 0.0 } else { CAPTION_GAP + cell.gap };
            budget -= gap;
            // No room left on this line. Dropping the rest is deliberate: a caption clipped
            // mid-number reads as a plausible WRONG number.
            if budget < MIN_LEGIBLE_W {
                break;
            }
            // The column is as wide as its widest caption, and every caption in it is anchored to
            // the same edge — which is what makes a block read as a block.
            let cell_w = self.measure_cell(ctx, texts, cell, budget);
            let anchor_x = if rightwards {
                cursor + gap
            } else {
                cursor - gap
            };
            let mut y = top;
            let mut drawn_w = 0.0_f32;
            for (n_item, item) in cell.items.iter().enumerate() {
                // Out of pane. Which END of the stack is lost depends on which way the band fills,
                // and getting that backwards is how an over-tall arbitrage column printed one venue
                // outside the plot and dropped the two dozen that would have fitted:
                //
                // - a band filling DOWNWARD keeps its top lines and drops the tail;
                // - a band filling UPWARD is anchored at its BOTTOM edge, so the lines that fall
                //   off are the ones at the top — skipped, not a reason to stop.
                //
                // One line always survives, mirroring the first LINE of a band above: a pane can be
                // shorter than one line of text, and a stack that printed nothing there would take
                // the chart's only identification with it.
                let past_edge = if downward {
                    y + item.line_h() > limit_y
                } else {
                    y < limit_y
                };
                let last = n_item + 1 == cell.items.len();
                if past_edge {
                    if downward {
                        if n_item > 0 {
                            break;
                        }
                    } else if !last {
                        y += item.line_h();
                        continue;
                    }
                }
                let Some(entry) = texts.get(item.pos) else {
                    continue;
                };
                // A line's own colour wins over the caption's style: an arbitrage row is coloured
                // by its VENUE, which one style cannot say for a dozen lines.
                let value_color = match entry.color {
                    Some(rgb) => gpui::rgb(rgb).into(),
                    None => self.caption_color(item.style.color, entry.sign, caption_fg),
                };
                // A prefix that takes the same colour as its value is not a second run: gluing them
                // makes ONE string, which shapes once and cannot drift apart. Only "colour the
                // value alone" needs the split, and then the prefix keeps the theme's colour.
                // The prefix is measured FIRST, through its OWN retained run: it is drawn with
                // the value and spends the same budget, and measuring it through a throwaway run
                // would re-shape it on every measure and every frame — the cost `caption_runs`
                // exists to avoid.
                let prefix_w = match item.style.value_only && !entry.prefix.is_empty() {
                    true => self.measure_caption_run(
                        ctx,
                        idx,
                        item.row,
                        PREFIX_PART_BASE + item.part,
                        &entry.prefix,
                        item.size,
                    ),
                    false => 0.0,
                };
                // Too little room for the pair: the caption falls back to ONE run holding both,
                // truncated as a whole. A split that kept the full-width prefix would paint it past
                // the band's edge, and truncating the prefix instead would leave a caption naming
                // nothing.
                let split = prefix_w > 0.0 && budget - prefix_w >= MIN_LEGIBLE_W;
                let prefix_w = if split { prefix_w } else { 0.0 };
                let glued = match split {
                    true => entry.text.clone(),
                    false => format!("{}{}", entry.prefix, entry.text),
                };
                let (text, value_w) = fit_caption(ctx, &glued, item, budget - prefix_w);
                if text.is_empty() {
                    continue;
                }
                if split {
                    // A right-anchored caption ends at `anchor_x`, so BOTH runs are placed from
                    // that edge backwards: the value takes the last `value_w`, and the prefix the
                    // `prefix_w` before it. Placing the prefix at the value's own left edge — one
                    // subtraction short — draws the two on top of each other.
                    let prefix_x = if rightwards {
                        anchor_x
                    } else {
                        anchor_x - value_w - prefix_w
                    };
                    self.draw_caption_run(
                        ctx,
                        idx,
                        item.row,
                        PREFIX_PART_BASE + item.part,
                        &entry.prefix,
                        item.size,
                        prefix_x,
                        y,
                        // The prefix always draws LEFT-anchored from the point computed above:
                        // right-anchoring it would place its right edge where its left edge belongs.
                        0.0,
                        caption_fg,
                    )?;
                    crate::diag::bump(&crate::diag::CHART_CAPTION_DRAW);
                }
                let value_x = match (split, rightwards) {
                    (true, true) => anchor_x + prefix_w,
                    _ => anchor_x,
                };
                let _ = value_w;
                let metrics = self.draw_caption_run(
                    ctx, idx, item.row, item.part, &text, item.size, value_x, y, ax, value_color,
                )?;
                crate::diag::bump(&crate::diag::CHART_CAPTION_DRAW);
                let w = metrics.width.as_f32() + prefix_w;
                drawn_w = drawn_w.max(w);
                let box_left = if rightwards { anchor_x } else { anchor_x - w };
                left_edge = left_edge.min(box_left);
                if item.style.plate {
                    plate.add(box_left, w, y, metrics.line_height.as_f32());
                }
                y += item.line_h();
            }
            let advance = drawn_w.max(cell_w);
            cursor = if rightwards {
                anchor_x + advance
            } else {
                anchor_x - advance
            };
            budget -= advance;
        }
        Ok(left_edge)
    }

    /// Width of a line once every column is truncated to the budget it would actually get.
    fn measure_row(
        &self,
        ctx: &gpui::GpuCanvasTextContext<'_>,
        texts: &[LabelText],
        cells: &[Cell],
        max_w: f32,
    ) -> f32 {
        let mut total = 0.0;
        let mut budget = max_w;
        for (n, cell) in cells.iter().enumerate() {
            let gap = if n == 0 { 0.0 } else { CAPTION_GAP + cell.gap };
            budget -= gap;
            if budget < MIN_LEGIBLE_W {
                break;
            }
            let w = self.measure_cell(ctx, texts, cell, budget);
            total += gap + w;
            budget -= w;
        }
        total
    }

    /// Width of one column: its widest caption, each truncated to the same budget.
    fn measure_cell(
        &self,
        ctx: &gpui::GpuCanvasTextContext<'_>,
        texts: &[LabelText],
        cell: &Cell,
        budget: f32,
    ) -> f32 {
        cell.items
            .iter()
            .filter_map(|item| {
                let entry = texts.get(item.pos)?;
                // Same rule as the drawing pass: a split caption is a prefix plus a value, and its
                // width is the sum. Measuring only the glued form would misplace every centred line
                // holding one.
                // The whole caption, prefix and value as one string. Deliberately NOT measured
                // as two: the column's width is the same either way in a monospaced face, and a
                // second measurement here would shape the prefix again on every frame.
                let glued = format!("{}{}", entry.prefix, entry.text);
                Some(fit_caption(ctx, &glued, item, budget).1)
            })
            .fold(0.0_f32, f32::max)
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

    /// Measure a caption's text through the retained run it will be DRAWN with.
    ///
    /// The run keeps its shaping, so measuring and then drawing the same string costs one shaping
    /// rather than two — which is the whole reason these runs are addressed by index instead of
    /// taken from a cursor.
    fn measure_caption_run(
        &mut self,
        ctx: &gpui::GpuCanvasTextContext<'_>,
        pane_ix: usize,
        row_ix: usize,
        part_ix: usize,
        text: &str,
        size: f32,
    ) -> f32 {
        let run_ix = (pane_ix * CHART_LABEL_ROWS + row_ix) * ROW_RUN_STRIDE + part_ix;
        if self.caption_runs.len() <= run_ix {
            self.caption_runs
                .resize_with(run_ix + 1, gpui::GpuCanvasTextRun::default);
        }
        self.caption_runs[run_ix]
            .measure(
                ctx,
                text,
                gpui::font(crate::design::mono()),
                px(size),
                px(size + 4.0),
            )
            .width
            .as_f32()
    }

    /// Draw one caption through its OWN retained run.
    ///
    /// The run is addressed by `(pane * CHART_LABEL_ROWS + row) * ROW_RUN_STRIDE + part`, never by
    /// a running cursor: a caption that stops resolving must not hand its run to the next one,
    /// which would reshape both strings on every frame for as long as the pane stays that way. The
    /// stride reserves one index past the captions for the row's printed name, so switching that on
    /// renumbers nothing either.
    #[allow(clippy::too_many_arguments)]
    fn draw_caption_run(
        &mut self,
        ctx: &mut gpui::GpuCanvasTextContext<'_>,
        pane_ix: usize,
        row_ix: usize,
        part_ix: usize,
        text: &str,
        size: f32,
        x: f32,
        y: f32,
        ax: f32,
        color: Hsla,
    ) -> anyhow::Result<gpui::GpuCanvasTextMetrics> {
        let run_ix = (pane_ix * CHART_LABEL_ROWS + row_ix) * ROW_RUN_STRIDE + part_ix;
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
        let cfg = self.chart_labels.clone();
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
        // A shot is in flight: this pane's core-name caption names the EXCHANGE instead. The
        // substitution happens HERE, at the one place the caption's inputs are assembled, so that
        // nothing below — the caption resolution, the truncation, the measuring, the plate geometry —
        // learns that a shot is happening. Only which string arrives changes.
        //
        // The core name is the user's own free text, an account label such as `SUB ACC No 38`, and
        // these pictures get shared publicly. `venue` is never empty while a shot is armed: the
        // order sync resolves it through the shared label helper, which answers with the "not
        // identified" wording for a core that cannot be named.
        let shot = self.shot_caption_active();
        let inputs = super::LabelInputs {
            ticker: pr.ticker.clone(),
            core_name: match shot {
                true => pr.venue.clone(),
                false => pr.core_name.clone(),
            },
            venue: pr.venue.clone(),
            quote: pr.quote.clone(),
            strategy: pr.label_strategy.clone(),
            detect_strategy: pr.label_detect_strategy.clone(),
            detect_msg: pr.label_detect_msg.clone(),
            last_price: pr.cached_last_price,
            scale_badge: pr.scale_badge,
            compare_pct,
            delta_1h: pr.delta_1h,
            delta_24h: pr.delta_24h,
            context: pr.label_context,
            figures: pr.label_figures.clone(),
            windows: pr.label_windows,
            arb: pr.label_arb.clone(),
            now_ms: pr.label_now_ms,
            basis: pr.label_basis,
        };
        let arb_view = self.arb_view.clone();
        let changed = self.panes[idx].labels.update(&cfg, &arb_view, inputs);
        // Recorded whether or not the texts changed: `update` is a cache and answers "nothing
        // moved" when the substitution happens to produce the same string, but the shot's proof
        // asks what the CURRENT labels were built from, not whether they differ from last time.
        self.panes[idx].labels_shot_substituted = shot;
        if changed {
            crate::diag::bump(&crate::diag::CHART_CAPTION_REBUILD);
        }
        changed
    }
}

/// Style one caption of a module draws with, or `None` when the module holds no such caption.
///
/// The row's own NAME is not a configured caption and carries no style of its own; any other index
/// the module does not hold is not a caption at all — a hand-edited file can state one — and is
/// dropped rather than drawn with a guessed style.
fn caption_style(row: &ChartLabelRow, part: usize) -> Option<ResolvedLabelStyle> {
    if part == ROW_NAME_PART {
        return Some(ChartLabelRow::name_style());
    }
    // An arbitrage line is drawn in its OWN run range, past every part index, and takes the style
    // of the caption that produced it — the whole column is one configured caption, so its size and
    // its plate are set once. Only the colour differs per line, and that rides on the line itself.
    if part >= ARB_PART_BASE {
        let column = row.parts[..row.used_parts()]
            .iter()
            .find(|p| p.field.is_column() && p.visible)?;
        return Some(column.resolved_style());
    }
    Some(row.parts.get(part)?.resolved_style())
}

/// Which captions of a band land on which LINE, and in which column of it.
///
/// The grouping rule alone, with no styling and no geometry: the drawing pass needs the same answer
/// but cannot be run without a device, and this is the part that decides what the chart looks like.
/// Two questions, asked in this order:
///
/// 1. Does this module open a LINE? Only its placement decides — [`LabelFlow::Column`] starts one
///    under the previous line, [`LabelFlow::Row`] continues it. A module that runs down a column is
///    NOT excluded: it joins as a block, which is the whole point of the two axes being separate.
/// 2. Does this caption open a COLUMN inside that line? A module whose captions run across the line
///    gives each of them its own column; a module that runs down a column keeps them in one.
///
/// Args:
///     cfg: The pane's caption configuration.
///     texts: Every resolved caption of the pane, in draw order.
///     zone: Band being collected.
///     align: Edge of that band.
///
/// Returns:
///     One vector per printed line, holding one vector of `texts` indices per column.
fn group_lines(
    cfg: &ChartLabelsCfg,
    texts: &[LabelText],
    zone: LabelZone,
    align: LabelAlign,
) -> Vec<Vec<Vec<usize>>> {
    let mut lines: Vec<Vec<Vec<usize>>> = Vec::new();
    let mut current: Option<usize> = None;
    for (pos, text) in texts.iter().enumerate() {
        let Some(row_cfg) = cfg.rows.get(text.row) else {
            continue;
        };
        if row_cfg.zone != zone || row_cfg.align != align {
            continue;
        }
        if caption_style(row_cfg, text.part).is_none() {
            continue;
        }
        let same_module = current == Some(text.row);
        if !same_module && (lines.is_empty() || !row_cfg.placement.is_row()) {
            lines.push(Vec::new());
        }
        let line = lines.last_mut().expect("a line was opened above");
        // Inside a module that runs down a column, every caption after the first joins the column
        // the module opened. Everything else opens one of its own.
        match line.last_mut() {
            Some(cell) if same_module && !row_cfg.flow.is_row() => cell.push(pos),
            _ => line.push(vec![pos]),
        }
        current = Some(text.row);
    }
    lines
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

/// Anchor and width budget for one band at one alignment.
fn zone_column(
    zone: LabelZone,
    align: LabelAlign,
    geom: &CaptionGeomInput,
    corner: &CaptionGeom,
) -> Column {
    let plot_w = (geom.plot_right - geom.plot_left - 2.0 * ZONE_PAD).max(0.0);
    // The control strip's own width, which is what its two zones are measured against — never the
    // plot's, or a caption there would run out over the candles.
    let zone_w = (corner.right_x - corner.zone_left).max(0.0);
    // Each band knows its own edges; the alignment picks which of them the row anchors to. The
    // strip's right edge is already inset clear of the pane's close button by `caption_geom`.
    let (left, right, max_w) = if zone.is_control_zone() {
        (corner.zone_left, corner.right_x, zone_w)
    } else {
        (
            geom.plot_left + ZONE_PAD,
            geom.plot_right - ZONE_PAD,
            plot_w,
        )
    };
    let x = match align {
        LabelAlign::Left => left,
        LabelAlign::Center => (left + right) * 0.5,
        LabelAlign::Right => right,
    };
    Column {
        x,
        align: align.fraction(),
        max_w,
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

#[cfg(test)]
mod tests;
