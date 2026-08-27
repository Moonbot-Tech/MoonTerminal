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
//! into what they left. See [`widths`], which also states what is deliberately NOT divided. Before
//! that, a long detect line printed straight through whatever was pinned to the left and the right
//! of it. Two different zones are never divided against each other — each stays inside its own
//! band, which is the paragraph above.

use gpui::{Hsla, point, px};
use moon_core::config::{
    ARB_PART_BASE, CHART_LABEL_ROWS, ChartLabelRow, ChartLabelsCfg, LABEL_WRAP_LINES, LabelAlign,
    LabelColor, LabelZone, PREFIX_PART_BASE, ROW_NAME_PART, ROW_RUN_STRIDE, ResolvedLabelStyle,
    WRAP_PART_BASE,
};
use moon_core::util::fmt::DeltaSign;

use super::caption::{CaptionBox, CaptionGeom, caption_geom};
use super::labels::LabelText;
use super::{CAPTION_PAD_X, CAPTION_PAD_Y};
use crate::chartdx::RenderState;
use crate::chartdx::{ArbHit, VolumeHit};

/// Horizontal gap between two captions on the same row.
const CAPTION_GAP: f32 = 8.0;
/// Inset of a band from the plot's edges.
const ZONE_PAD: f32 = 6.0;
/// Smallest width a caption is truncated to before it is dropped instead.
const MIN_LEGIBLE_W: f32 = 12.0;

/// Width of a buy/sell proportion bar, in the chart's own logical pixels.
///
/// Fixed rather than proportional to the figure beside it: the bar is read by comparing it with the
/// bar on the line above, and two tracks of different lengths cannot be compared at a glance.
const BAR_W: f32 = 42.0;

/// What one bar costs its column: the track plus the gap that separates it from the figure.
///
/// Charged ONCE per module rather than per line — every bar in a module shares one vertical, so the
/// column reserves one track's worth and every line draws into it.
const BAR_ZONE: f32 = CAPTION_GAP + BAR_W;

/// Height of that bar as a share of the caption's line height, and its floor in logical pixels.
const BAR_H_RATIO: f32 = 0.34;
const BAR_H_MIN: f32 = 2.0;

/// One proportion bar, placed and ready for the readout batch.
///
/// Published by this pass the way the backing plates are — geometry decided where the text was
/// actually drawn — and coloured where the rectangles are built, which is the only place that holds
/// the order book's own bid/ask colours.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::chartdx) struct CaptionBar {
    /// `[x, y, width, height]` of the whole track.
    ///
    /// LOGICAL pixels while the pass builds it, DEVICE pixels once it is published — converted in
    /// one place, the way a module's plate is, so the two cannot end up in different spaces.
    pub dst: [f32; 4],
    /// Share of that track the filled part takes, `0.0..=1.0`.
    pub fill: f32,
    /// Whether this is the selling side.
    pub sell: bool,
}
/// How much room this column reserves for proportion bars: one track, or none.
///
/// ONE per column, not one per line. Every bar in a module is drawn on the same vertical — that is
/// what makes two of them comparable at a glance — so the column reserves a single track and each
/// line draws into it. Charging it per line made the reserve depend on which line happened to be
/// widest, and the tracks then sat wherever their own figure ended.
fn bar_zone(texts: &[LabelText], cell: &Cell) -> f32 {
    let any = cell
        .items
        .iter()
        .filter_map(|item| texts.get(item.pos))
        .any(|entry| entry.bar.is_some());
    match any {
        true => BAR_ZONE,
        false => 0.0,
    }
}

/// Number of backing plates a pane publishes: one per MODULE.
///
/// Per module, because that is whose switch it is — see `ChartLabelRow::plate`. One plate for a
/// whole band used to span every line in it, so switching the backing off on one thing changed
/// nothing visible while switching it off on the tallest thing in the band looked like switching it
/// off everywhere.
///
/// Indexed BY the module, not by draw order: a module's plate then keeps its slot whatever its
/// neighbours do, which is the same reason its captions address their runs by index.
pub(in crate::chartdx) const CAPTION_PLATES: usize = moon_core::config::CHART_LABEL_ROWS;

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
    /// Whether the MODULE this caption belongs to draws a backing plate. Copied onto the item
    /// because the drawing pass has the items and not the configuration.
    plate: bool,
    /// Font size in logical pixels, already resolved and clamped.
    size: f32,
    /// Whether this caption is prose and may be WRAPPED rather than cut. See
    /// [`ChartLabelField::wraps`].
    wraps: bool,
    /// How many lines it actually takes at the width it was given, filled by `plan_wraps` before
    /// anything is stacked. Always at least one.
    lines: u8,
    /// Index of this caption's wrapped lines in `RenderState::caption_wraps`, or `usize::MAX` when
    /// it is not prose and takes the single-line path.
    wrap_ix: usize,
}

impl Item {
    /// Height of one line at this caption's size, matching what `draw_aligned` is given.
    fn line_h(self) -> f32 {
        self.size + 4.0
    }

    /// Height of the whole caption: one line, or the lines a wrapped one took.
    fn block_h(self) -> f32 {
        self.line_h() * f32::from(self.lines.max(1))
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
    /// Whether this column holds a caption that WRAPS rather than being truncated.
    fn has_prose(&self) -> bool {
        self.items.iter().any(|item| item.wraps)
    }

    /// How tall the cell is: its captions stack, so their heights add up — and a wrapped caption
    /// counts every line it took, or the module below it would be drawn over its tail.
    fn height(&self) -> f32 {
        self.items.iter().map(|it| it.block_h()).sum()
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
    /// Known before anything is DRAWN — which is what lets a bottom-anchored band place its first
    /// line without drawing it first. From the styles alone for every caption but one: a prose
    /// caption's line count comes from `plan_wraps`, which measures it beforehand for this reason.
    fn height(&self) -> f32 {
        self.cells.iter().map(Cell::height).fold(0.0_f32, f32::max)
    }
}

/// Everything drawn at one alignment of one zone.
///
/// A type rather than three parallel arrays because `elastic` is the property the layout turns on,
/// and re-deriving it by walking every item — which is what the drawing pass did until this
/// existed — costs that walk on every presented frame.
struct Band {
    align: LabelAlign,
    rows: Vec<Row>,
    /// Whether this band WRAPS, and so has no width of its own: a wrapped caption fills whatever
    /// budget it is handed, which is why it is drawn after the bands that do.
    elastic: bool,
}

impl Band {
    fn new(align: LabelAlign, rows: Vec<Row>) -> Self {
        let elastic = rows.iter().any(|row| row.cells.iter().any(Cell::has_prose));
        Self {
            align,
            rows,
            elastic,
        }
    }

    /// Whether the band prints nothing at all.
    ///
    /// Not `rows.is_empty()`: a line whose captions all resolved to nothing keeps its `Row`, and
    /// walking a band of those is work with no pixel behind it.
    fn is_empty(&self) -> bool {
        self.rows.iter().all(|row| row.cells.is_empty())
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
        );
        self.panes[idx].labels.texts = texts;
        self.panes[idx].arb_hits = hits;
        result?;
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
        cfg: &ChartLabelsCfg,
        texts: &[LabelText],
        geom: CaptionGeomInput,
        corner: Option<CaptionGeom>,
        caption_fg: Hsla,
        plates: &mut [[f32; 4]; CAPTION_PLATES],
        hits: &mut Vec<ArbHit>,
        bars: &mut Vec<CaptionBar>,
        vol_hits: &mut Vec<(usize, CaptionBox)>,
    ) -> anyhow::Result<()> {
        let Some(corner) = corner else {
            return Ok(());
        };
        for zone in LabelZone::ALL {
            // Every band of the zone is gathered before ANY of them is drawn. Three passes that
            // each spent the whole zone are exactly what printed a centred detect line over the
            // modules pinned to either edge — neither pass could see the other.
            let mut bands = LabelAlign::ALL
                .map(|align| Band::new(align, self.collect_rows(cfg, texts, zone, align)));
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
            let start_y = zone_start_y(zone, &geom, &corner);
            // Rows fill toward the far edge of the band and stop there.
            let downward = zone.is_top();
            let limit_y = if downward {
                geom.plot_bottom
            } else {
                geom.plot_top
            };
            // FIGURES first, elastic last: a band that wraps is the only one whose width cannot be
            // read off its own content, so it takes what the others left. Stable sort — the figure
            // bands keep `LabelAlign::ALL`'s order between themselves.
            let mut order = [0usize, 1, 2];
            order.sort_by_key(|&n| bands[n].elastic);
            let mut taken = widths::Taken::default();
            for n in order {
                if bands[n].is_empty() {
                    continue;
                }
                let (align, elastic) = (bands[n].align, bands[n].elastic);
                let max_w = match (cap, elastic) {
                    (None, _) => total,
                    (Some(cap), false) => cap,
                    (Some(_), true) => widths::free_width(total, align, taken),
                };
                let (x, fraction) = zone_anchor(zone, align, &geom, &corner);
                let column = Column {
                    x,
                    align: fraction,
                    max_w,
                };
                // A prose caption's height is not known from its style, so it is measured here —
                // before `draw_stack` places anything from those heights.
                if elastic {
                    self.plan_wraps(ctx, texts, &mut bands[n].rows, column);
                }
                let (module_plates, used_w) = self.draw_stack(
                    ctx,
                    idx,
                    texts,
                    &bands[n].rows,
                    column,
                    start_y,
                    limit_y,
                    downward,
                    caption_fg,
                    hits,
                    bars,
                    vol_hits,
                )?;
                taken.set(align, used_w);
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
                plate: row_cfg.plate,
                size: (base * style.size_mult).clamp(6.0, 60.0),
                wraps: row_cfg
                    .parts
                    .get(text.part)
                    .is_some_and(|part| part.field.wraps()),
                lines: 1,
                wrap_ix: usize::MAX,
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

    /// How wide this band's prose would be on ONE line, or `0` when it holds none.
    ///
    /// The only thing the width split has to ASK for rather than read back from a drawn band: how
    /// wide a wrapped caption ends up is decided by the budget it is given, so it cannot also be
    /// what decides that budget. One shaping pass for one line — the figure bands are never
    /// measured here, they are drawn first and report what they took.
    fn prose_width(
        &self,
        ctx: &gpui::GpuCanvasTextContext<'_>,
        texts: &[LabelText],
        rows: &[Row],
    ) -> f32 {
        rows.iter()
            .flat_map(|row| row.cells.iter())
            .flat_map(|cell| cell.items.iter())
            .filter(|item| item.wraps)
            .filter_map(|item| {
                let entry = texts.get(item.pos)?;
                // The prefix is empty on almost every caption, and on every prose one so far:
                // measuring the text as it stands saves the glue allocation on the frame path.
                Some(match entry.prefix.is_empty() {
                    true => super::measure_run_width(ctx, &entry.text, item.size),
                    false => super::measure_run_width(ctx, &entry.glued(), item.size),
                })
            })
            .fold(0.0_f32, f32::max)
    }

    /// Work out how many lines each PROSE caption takes at the width it will be drawn at.
    ///
    /// Before anything is stacked, because a band places its lines from their HEIGHTS — and a
    /// bottom-anchored one subtracts a line's height before drawing it. A wrapped caption that
    /// still reported one line would have its tail drawn over whatever the band placed next.
    ///
    /// The budget walk follows `draw_row`'s, on the MEASURED width of each column. The drawn width
    /// can exceed it — a caption split into a prefix run and a value run shapes as two strings — so
    /// a later column can be drawn at a slightly narrower budget than it was planned at. The line
    /// COUNT cannot drift with it: the lines planned here are the lines drawn, read back from
    /// `caption_wraps` rather than wrapped again.
    fn plan_wraps(
        &mut self,
        ctx: &gpui::GpuCanvasTextContext<'_>,
        texts: &[LabelText],
        rows: &mut [Row],
        column: Column,
    ) {
        // Nothing on this pane is prose: the whole walk — a measurement per cell of every band —
        // is skipped, which is the case on every chart that prints no detect line.
        if !rows.iter().any(|row| row.cells.iter().any(Cell::has_prose)) {
            return;
        }
        // Modules that already have a wrapped caption: the continuation runs are per module, so
        // the second prose caption in one module is cut instead. Nothing ships two.
        let mut wrapped_modules: Vec<usize> = Vec::new();
        for row in rows.iter_mut() {
            // The budget walk exists to reach the prose captions; the cells after the last one on
            // this line cost a measurement each and change nothing.
            let Some(last_prose) = row.cells.iter().rposition(Cell::has_prose) else {
                continue;
            };
            let mut budget = column.max_w;
            for (n, cell) in row.cells.iter_mut().enumerate().take(last_prose + 1) {
                let gap = if n == 0 { 0.0 } else { CAPTION_GAP + cell.gap };
                budget -= gap;
                if budget < MIN_LEGIBLE_W {
                    break;
                }
                for item in cell.items.iter_mut() {
                    if !item.wraps || wrapped_modules.contains(&item.row) {
                        continue;
                    }
                    wrapped_modules.push(item.row);
                    let Some(entry) = texts.get(item.pos) else {
                        continue;
                    };
                    let lines = wrap_caption(ctx, &entry.glued(), item, budget);
                    item.lines = lines.len().clamp(1, LABEL_WRAP_LINES) as u8;
                    item.wrap_ix = self.caption_wraps.len();
                    self.caption_wraps.push(lines);
                }
                // AFTER the wrap, never before: `measure_cell` reads a prose caption's width off
                // the wrap, and asking it first would send the whole sentence through the
                // truncation walk — which measures one character at a time — every frame.
                budget -= self.measure_cell(ctx, texts, cell, budget);
            }
        }
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
        hits: &mut Vec<ArbHit>,
        bars: &mut Vec<CaptionBar>,
        vol_hits: &mut Vec<(usize, CaptionBox)>,
    ) -> anyhow::Result<(Vec<(usize, CaptionBox)>, f32)> {
        let mut plates: Vec<(usize, CaptionBox)> = Vec::new();
        // The band is as wide as its widest LINE — what the bands drawn after it have to clear.
        let mut used_w = 0.0_f32;
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
            // One box PER MODULE on this line: two modules can share a line — that is what the
            // placement axis is for — and one rectangle behind both would put a backing under the
            // module that switched it off.
            let row_w = self.draw_row(
                ctx,
                idx,
                texts,
                &row.cells,
                column,
                top,
                limit_y,
                downward,
                caption_fg,
                &mut plates,
                hits,
                bars,
                vol_hits,
            )?;
            used_w = used_w.max(row_w);
            any_drawn = true;
            y += if downward { row_h } else { -row_h };
        }
        Ok((plates, used_w))
    }

    /// Draw one row of captions, returning the width it actually took.
    ///
    /// That width is what the bands drawn after this one are placed against — see [`widths`] — so
    /// it is the DRAWN width, not the measured one: a caption shaped as a prefix run plus a value
    /// run can come out a little wider than it measured, and a neighbour placed from the
    /// measurement would be the one to pay for it.
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
        plates: &mut Vec<(usize, CaptionBox)>,
        hits: &mut Vec<ArbHit>,
        bars: &mut Vec<CaptionBar>,
        vol_hits: &mut Vec<(usize, CaptionBox)>,
    ) -> anyhow::Result<f32> {
        if cells.is_empty() {
            return Ok(0.0);
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
            // the same edge — which is what makes a block read as a block. A CENTRED band is the
            // exception: there each caption is centred inside that width.
            let cell_w = self.measure_cell(ctx, texts, cell, budget);
            // The bar column, decided ONCE for the module: every track in it starts on the same
            // vertical, which is what lets two of them be compared at a glance. Placed after the
            // figures in reading order either way — a track before the number it belongs to reads
            // as belonging to the line above.
            let reserve = bar_zone(texts, cell);
            let text_w = (cell_w - reserve).max(0.0);
            let anchor_x = if rightwards {
                cursor + gap
            } else {
                cursor - gap
            };
            // Where every bar of this module starts, and where its text ends.
            //
            // Filling rightwards the text runs from the anchor and the track follows it; filling
            // leftwards the module is pinned by its RIGHT edge, so the track takes that edge and
            // the text ends before it. Both put the figure first and the track second.
            let (bar_x, text_anchor_x) = match (reserve > 0.0, rightwards) {
                (false, _) => (0.0, anchor_x),
                (true, true) => (anchor_x + text_w + CAPTION_GAP, anchor_x),
                // Pinned by its RIGHT edge: the track takes that edge, and the text column starts a
                // whole track and gap before it.
                (true, false) => (anchor_x - BAR_W, anchor_x - reserve - text_w),
            };
            // Inside a module with bars the figures are LEFT-aligned against each other, whatever
            // edge the module itself is pinned to. Right-aligning them would line up their last
            // digits and leave `Bv` and `Sv` starting in different places — the block reads as a
            // pair of labelled figures, and a label that moves is not one.
            let cell_ax = match reserve > 0.0 {
                true => 0.0,
                false => ax,
            };
            // A CENTRED band centres its captions against each other too, not just the line as a
            // whole: a module whose captions stack — a detect line under its strategy — reads as a
            // ragged left edge otherwise, which is the one thing centring was asked for.
            let centred = column.align > 0.0 && column.align < 1.0;
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
                        // Its whole block, not one line: an upward band reserved `block_h` for it,
                        // and stepping over a wrapped caption by a single line would put every
                        // caption below it two lines out of place.
                        y += item.block_h();
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
                let prefix_w =
                    match !item.wraps && item.style.value_only && !entry.prefix.is_empty() {
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
                let split = !item.wraps && prefix_w > 0.0 && budget - prefix_w >= MIN_LEGIBLE_W;
                let prefix_w = if split { prefix_w } else { 0.0 };
                let glued = match split {
                    true => entry.text.clone(),
                    false => entry.glued(),
                };
                // Prose is broken across lines instead of being cut, and every line but the
                // first draws through a run slot of its own: a retained run is addressed by its
                // part, so a second line drawn through the first line's run would replace it.
                // Taken out for the duration of the draw and put back after, like the texts and
                // the hit rectangles above: the runs live on `self`, so a borrow cannot span the
                // draw — and cloning a sentence per caption per frame is what the cache exists to
                // avoid.
                if item.wrap_ix < self.caption_wraps.len() {
                    let lines = std::mem::take(&mut self.caption_wraps[item.wrap_ix]);
                    for (k, (line, line_w)) in lines.iter().enumerate() {
                        // Re-asked for every line, at the y that line will be drawn at: the guard
                        // above was answered for the caption's FIRST line, and a block that passed
                        // it there would otherwise paint its tail over the time axis.
                        //
                        // Which lines are lost depends on the direction, exactly as it does for
                        // whole captions: a band filling DOWNWARD keeps its head and drops the
                        // tail, while one filling UPWARD is anchored at its bottom, so the lines
                        // that fall off are at the TOP and the ones after them come back on-pane.
                        if k > 0 {
                            match downward {
                                true if y + item.line_h() > limit_y => break,
                                false if y < limit_y => {
                                    y += item.line_h();
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        let line_x = match centred {
                            true => anchor_x + (cell_w - line_w).max(0.0) * 0.5,
                            false => anchor_x,
                        };
                        let part = match k {
                            0 => item.part,
                            k => WRAP_PART_BASE + k - 1,
                        };
                        let metrics = self.draw_caption_run(
                            ctx,
                            idx,
                            item.row,
                            part,
                            line,
                            item.size,
                            line_x,
                            y,
                            ax,
                            value_color,
                        )?;
                        crate::diag::bump(&crate::diag::CHART_CAPTION_DRAW);
                        let w = metrics.width.as_f32();
                        drawn_w = drawn_w.max(w);
                        let box_left = if rightwards { line_x } else { line_x - w };
                        if item.plate {
                            match plates.iter_mut().find(|(row, _)| *row == item.row) {
                                Some((_, box_)) => {
                                    box_.add(box_left, w, y, metrics.line_height.as_f32())
                                }
                                None => {
                                    let mut box_ = CaptionBox::default();
                                    box_.add(box_left, w, y, metrics.line_height.as_f32());
                                    plates.push((item.row, box_));
                                }
                            }
                        }
                        y += item.line_h();
                    }
                    self.caption_wraps[item.wrap_ix] = lines;
                    continue;
                }
                // The bar's track is part of what this caption occupies, so its room is taken out
                // of the budget BEFORE the text is fitted to what is left. Added afterwards, the
                // track would be drawn past the width the band actually allotted — over whatever
                // the layout put beside it — and the plate grown from the same width with it.
                let (text, value_w) = fit_caption(ctx, &glued, item, budget - prefix_w - reserve);
                if text.is_empty() {
                    continue;
                }
                // Where this caption sits inside its column. Only a centred band moves it: the
                // other two anchor every caption to the same edge, which is what makes a block
                // read as a block there. The bar column is excluded from that centring — it is a
                // reserve, not text, and centring against it would push every figure off-centre by
                // half a track.
                let item_x = match (centred, reserve > 0.0) {
                    // A module with bars is a column of its own: its figures start where its text
                    // column starts, centred band or not.
                    (_, true) => text_anchor_x,
                    (true, false) => text_anchor_x + (text_w - (prefix_w + value_w)).max(0.0) * 0.5,
                    (false, false) => text_anchor_x,
                };
                if split {
                    // A right-anchored caption ends at `anchor_x`, so BOTH runs are placed from
                    // that edge backwards: the value takes the last `value_w`, and the prefix the
                    // `prefix_w` before it. Placing the prefix at the value's own left edge — one
                    // subtraction short — draws the two on top of each other.
                    let prefix_x = if cell_ax < 0.5 {
                        item_x
                    } else {
                        item_x - value_w - prefix_w
                    };
                    // An arbitrage line's prefix is the VENUE's name, and clicking it opens this
                    // coin there. The rectangle is recorded from the placement above rather than
                    // recomputed later: a click has to hit what was actually drawn.
                    if let Some((code, dex)) = entry.venue.clone() {
                        hits.push(ArbHit {
                            x: prefix_x,
                            y,
                            w: prefix_w,
                            h: item.line_h(),
                            code,
                            dex,
                            reachable: entry.reachable,
                        });
                    }
                    // A venue this terminal cannot open is drawn faded: the column still states
                    // its price — that is what the column is for — but the name is not a target,
                    // and a reader should see which ones are.
                    let prefix_color = match entry.venue.is_some() && !entry.reachable {
                        true => caption_fg.opacity(0.45),
                        false => caption_fg,
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
                        prefix_color,
                    )?;
                    crate::diag::bump(&crate::diag::CHART_CAPTION_DRAW);
                }
                let value_x = match (split, cell_ax < 0.5) {
                    (true, true) => item_x + prefix_w,
                    _ => item_x,
                };
                let _ = value_w;
                let metrics = self.draw_caption_run(
                    ctx,
                    idx,
                    item.row,
                    item.part,
                    &text,
                    item.size,
                    value_x,
                    y,
                    cell_ax,
                    value_color,
                )?;
                crate::diag::bump(&crate::diag::CHART_CAPTION_DRAW);
                let w = metrics.width.as_f32() + prefix_w;
                // The proportion bar, placed from the SAME measurement the text was drawn at, so it
                // cannot drift from the figure it belongs to. It extends the caption's own width,
                // which is what keeps the module beside it from being drawn over the track.
                if let Some(bar) = entry.bar {
                    let line_h = metrics.line_height.as_f32();
                    let bar_h = (line_h * BAR_H_RATIO).max(BAR_H_MIN);
                    let bar_y = y + (line_h - bar_h) * 0.5;
                    // The module's OWN vertical, computed once above — not this line's right edge.
                    // Following the figure is what made the tracks jump between lines: `Bv 1.36K`
                    // and `Sv 917.36` are different widths, so their bars started in different
                    // places and the pair stopped being comparable.
                    bars.push(CaptionBar {
                        dst: [bar_x, bar_y, BAR_W, bar_h],
                        fill: bar.fill.clamp(0.0, 1.0),
                        sell: bar.sell,
                    });
                }
                // What this line OCCUPIES includes the module's bar column: the plate behind it and
                // the right-click target are grown from this, and a module whose tracks fell
                // outside both would be drawn over by its neighbour.
                let occupied = w + reserve;
                drawn_w = drawn_w.max(occupied);
                let box_left = match (reserve > 0.0, rightwards) {
                    // With bars the text column starts at `item_x` either way, and the track sits
                    // after it — so the block runs from there.
                    (true, _) => item_x,
                    (false, true) => item_x,
                    (false, false) => item_x - w,
                };
                // The module's right-click target grows with every line of it — the heading, the
                // figures and the bars beside them — so the menu opens from anywhere on the block.
                // Independent of the plate: a module with its backing switched off is still a
                // target, and tying the two would make the menu unreachable for it.
                if entry.volume_menu {
                    let line_h = metrics.line_height.as_f32();
                    match vol_hits.iter_mut().find(|(row, _)| *row == item.row) {
                        Some((_, box_)) => box_.add(box_left, w, y, line_h),
                        None => {
                            let mut box_ = CaptionBox::default();
                            box_.add(box_left, w, y, line_h);
                            vol_hits.push((item.row, box_));
                        }
                    }
                }
                // Grown into the box of the MODULE this caption belongs to. A module whose
                // plate is switched off never opens one, so its captions grow nothing.
                if item.plate {
                    match plates.iter_mut().find(|(row, _)| *row == item.row) {
                        Some((_, box_)) => box_.add(box_left, w, y, metrics.line_height.as_f32()),
                        None => {
                            let mut box_ = CaptionBox::default();
                            box_.add(box_left, w, y, metrics.line_height.as_f32());
                            plates.push((item.row, box_));
                        }
                    }
                }
                y += item.line_h();
            }
            // The cursor moves by whichever is wider: what the column was measured at, or what it
            // actually drew. A centred caption sits INSIDE that width and never adds to it.
            let advance = drawn_w.max(cell_w);
            cursor = if rightwards {
                anchor_x + advance
            } else {
                anchor_x - advance
            };
            budget -= advance;
        }
        // Whichever way the line walked, this is how much of the band it spent.
        Ok((cursor - start_x).abs())
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
        let reserve = bar_zone(texts, cell);
        let text_w = cell
            .items
            .iter()
            .filter_map(|item| {
                let entry = texts.get(item.pos)?;
                // Same rule as the drawing pass: a split caption is a prefix plus a value, and its
                // width is the sum. Measuring only the glued form would misplace every centred line
                // holding one.
                // The whole caption, prefix and value as one string. Deliberately NOT measured
                // as two: the column's width is the same either way in a monospaced face, and a
                // second measurement here would shape the prefix again on every frame.
                let glued = entry.glued();
                // A wrapped caption is as wide as its WIDEST line, not as wide as its first: the
                // column it sits in is what centres the captions above and below it. Read from the
                // plan rather than wrapped again — measuring is what the plan exists to do once.
                // Measured WITHOUT the bar: the track is a column-wide reserve, added once below.
                let bar_reserve = reserve;
                match self.wrapped(item) {
                    Some(lines) => Some(lines.iter().map(|(_, w)| *w).fold(0.0_f32, f32::max)),
                    None => Some(fit_caption(ctx, &glued, item, budget - bar_reserve).1),
                }
            })
            .fold(0.0_f32, f32::max);
        text_w + reserve
    }

    /// The lines `plan_wraps` broke this caption into, or `None` when it is not prose.
    fn wrapped(&self, item: &Item) -> Option<&Vec<(String, f32)>> {
        self.caption_wraps.get(item.wrap_ix)
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
    /// Called from the SYNC paths — a market revision or an order revision — and, for the
    /// countdown captions alone, from the frame path when their quantized clock moves
    /// (`ChartDataState::tick_countdown_captions`). This is where strings are BUILT, which is why
    /// the frame path calls it once per quantum rather than per frame: `prepare_text` runs on
    /// every presented frame and must find the strings already made.
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
            // Off the ENGINE rather than the pane: a handed trade is a property of the whole
            // window — one chart, one trade — and copying it per pane would be the same three
            // strings stored as many times as the stack has panes.
            trade: self.trade_labels.clone(),
            last_price: pr.cached_last_price,
            scale_badge: pr.scale_badge,
            compare_pct,
            delta_1h: pr.delta_1h,
            delta_24h: pr.delta_24h,
            context: pr.label_context,
            figures: pr.label_figures.clone(),
            windows: pr.label_windows,
            volumes: pr.label_volumes.clone(),
            liquidations: pr.label_liquidations.clone(),
            cursor_ms: pr.label_cursor_ms,
            arb: pr.label_arb.clone(),
            arb_reachable: pr.label_arb_reachable.clone(),
            now_ms: pr.label_now_ms,
            chart_tf_ms: self.chart_tf_ms,
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
    // Whether the caption placed last was an arbitrage line.
    let mut was_column_line = false;
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
        // An arbitrage line is part of a COLUMN by its own nature — one venue under another — and
        // the module's flow does not apply to it: that switch decides how the module's ordinary
        // captions run, and a module can hold both. So arbitrage lines join each other and nothing
        // else joins them.
        let is_column_line = text.part >= ARB_PART_BASE;
        let joins = match (is_column_line, was_column_line) {
            (true, true) => same_module,
            (true, false) | (false, true) => false,
            (false, false) => same_module && !row_cfg.flow.is_row(),
        };
        match line.last_mut() {
            Some(cell) if joins => cell.push(pos),
            _ => line.push(vec![pos]),
        }
        current = Some(text.row);
        was_column_line = is_column_line;
    }
    lines
}

/// Split a caption into the lines it is actually drawn on.
///
/// A caption that is not prose answers with the one line it always had, cut to its budget. A prose
/// one is broken on WORD boundaries into at most [`LABEL_WRAP_LINES`] lines, and whatever is still
/// left over is cut into the last one — so the ellipsis lands at the end of the block rather than
/// in the middle of the first line, which is the whole point of wrapping it.
///
/// The rule itself is [`crate::design::wrap_text`], which is pure and tested there; what is here is
/// only the measurement this pass draws with.
fn wrap_caption(
    ctx: &gpui::GpuCanvasTextContext<'_>,
    text: &str,
    item: &Item,
    budget: f32,
) -> Vec<(String, f32)> {
    let measure = |s: &str| super::measure_run_width(ctx, s, item.size);
    match item.wraps {
        true => crate::design::wrap_text(text, budget, LABEL_WRAP_LINES, measure),
        false => vec![crate::design::fit_text(text, budget, measure)],
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

/// The one band of a zone whose width has to be decided from the others, or `None`.
///
/// `None` for a zone with no elastic band, for one where nothing else is drawn — there is nothing
/// to divide against — and for one holding TWO of them: dividing those against each other leaves
/// the loser under `MIN_LEGIBLE_W`, where a band is dropped rather than truncated. Nothing ships
/// two; the detect module is the only prose there is.
fn elastic_band(bands: &[Band; 3]) -> Option<&Band> {
    if bands.iter().filter(|band| !band.is_empty()).count() < 2 {
        return None;
    }
    let mut elastic = bands.iter().filter(|band| band.elastic);
    match (elastic.next(), elastic.next()) {
        (Some(band), None) => Some(band),
        _ => None,
    }
}

/// Left and right edge of one zone, in logical pixels.
///
/// THE one place a zone's bounds are stated: the anchor and the width budget are both read off it,
/// and two copies of this arithmetic would be free to disagree about where a band ends. The control
/// strip is bounded by ITS OWN edges, never the plot's, or a caption there would run out over the
/// candles; its right edge is already inset clear of the pane's close button by `caption_geom`.
fn zone_bounds(zone: LabelZone, geom: &CaptionGeomInput, corner: &CaptionGeom) -> (f32, f32) {
    if zone.is_control_zone() {
        return (corner.zone_left, corner.right_x);
    }
    // The plot's own edges, EXCEPT that the top band also clears the pane's close button. That
    // button sits in the pane's top-right corner and is drawn over whatever is beneath it, so a
    // right-aligned caption on this band hides under it — `ZONE_PAD` is six pixels and the button
    // takes twenty-six. `corner.right_x` is the same inset the control strip already keeps
    // (`caption_geom`), and taking the SMALLER of the two changes nothing while a book is drawn:
    // the plot ends well before the strip does. It matters exactly where the book is off and the
    // plot runs to the pane's edge — the trade-detail window, which draws no book at all.
    let right = match zone {
        LabelZone::ChartTop => (geom.plot_right - ZONE_PAD).min(corner.right_x),
        _ => geom.plot_right - ZONE_PAD,
    };
    (geom.plot_left + ZONE_PAD, right)
}

/// Width the bands of one zone share, in logical pixels.
fn zone_width(zone: LabelZone, geom: &CaptionGeomInput, corner: &CaptionGeom) -> f32 {
    let (left, right) = zone_bounds(zone, geom, corner);
    (right - left).max(0.0)
}

/// Where one band anchors, and at which fraction of its own width.
///
/// Only the anchor: what a band may SPEND is what its neighbours in the same zone left it, which
/// only `draw_all_zones` knows.
fn zone_anchor(
    zone: LabelZone,
    align: LabelAlign,
    geom: &CaptionGeomInput,
    corner: &CaptionGeom,
) -> (f32, f32) {
    // Each band knows its own edges; the alignment picks which of them the row anchors to.
    let (left, right) = zone_bounds(zone, geom, corner);
    let x = match align {
        LabelAlign::Left => left,
        LabelAlign::Center => (left + right) * 0.5,
        LabelAlign::Right => right,
    };
    (x, align.fraction())
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

mod widths;

#[cfg(test)]
mod tests;
