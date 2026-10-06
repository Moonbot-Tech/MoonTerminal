use super::*;

/// Horizontal gap between two captions on the same row.
pub(super) const CAPTION_GAP: f32 = 8.0;
/// Inset of a band from the plot's edges.
pub(super) const ZONE_PAD: f32 = 6.0;
/// Smallest width a caption is truncated to before it is dropped instead.
pub(super) const MIN_LEGIBLE_W: f32 = 12.0;
/// Largest size one caption is ever resolved to — the ceiling [`Item::size`] clamps against.
pub(super) const CAPTION_SIZE_MAX: f32 = 60.0;
/// Tallest one caption LINE can be: the largest resolved size plus what [`Item::line_h`] adds.
///
/// The corner-strip reservation is spent only where a line this tall would still fit beneath the
/// strip. The caption size is a user setting — the interface's label-font slider and a
/// per-caption size multiplier both feed it — so the strip's own height says nothing about how
/// tall the line it is pushing down will be.
pub(super) const MAX_CAPTION_LINE_H: f32 = CAPTION_SIZE_MAX + 4.0;

/// Opacity a caption is drawn at when it is SHOWN but cannot be acted on: an arbitrage venue with
/// no core behind it, a button the workspace rail has closed.
///
/// The THEME's own fade step, so the chart says "here, and not a target" in the same voice every
/// panel does rather than in a number of its own.
pub(super) const DISABLED_OPACITY: f32 = crate::design::STALE_ALPHA;

/// Width of a buy/sell proportion bar, in the chart's own logical pixels.
///
/// Fixed rather than proportional to the figure beside it: the bar is read by comparing it with the
/// bar on the line above, and two tracks of different lengths cannot be compared at a glance.
pub(super) const BAR_W: f32 = 42.0;

/// What one bar costs its column: the track plus the gap that separates it from the figure.
///
/// Charged ONCE per module rather than per line — every bar in a module shares one vertical, so the
/// column reserves one track's worth and every line draws into it.
pub(super) const BAR_ZONE: f32 = CAPTION_GAP + BAR_W;

/// Height of that bar as a share of the caption's line height, and its floor in logical pixels.
pub(super) const BAR_H_RATIO: f32 = 0.34;
pub(super) const BAR_H_MIN: f32 = 2.0;

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
/// One pressable caption as it was drawn, before the rectangle is resolved.
///
/// The box is grown exactly like a backing plate's, because that is the room the layout gave the
/// button — see [`CaptionBox::button`] — so what the panel places and what the neighbouring module
/// cleared are the same rectangle.
pub(in crate::chartdx) struct ActionDraw {
    pub(in crate::chartdx) mark: super::super::labels::ActionMark,
    /// The caption's own font size, in LOGICAL pixels, which is what the rectangle was measured at.
    /// The control placed in it is told to draw at the same number, or the box grows with the
    /// caption's size while the words in it do not.
    pub(in crate::chartdx) size: f32,
    /// `[x, y, width, height]` in the pane's own LOGICAL pixels.
    pub(in crate::chartdx) rect: [f32; 4],
    /// Which caption this is, by IDENTITY rather than by position: the resolved list is rebuilt and
    /// compacted on every revision, so an index taken from one frame names a different caption in
    /// the next — the label would then come off a neighbour.
    pub(in crate::chartdx) row: usize,
    pub(in crate::chartdx) part: usize,
}

/// How much room a pressable caption reserves beyond its text: the control drawn over it.
///
/// A button occupies its whole PLATE, not its glyphs. Without this the module beside it is placed
/// against the text width and the control is drawn over it — which is exactly how far the padding
/// reaches.
pub(super) const ACTION_PLATE_W: f32 = super::super::caption::ACTION_PAD_X * 2.0;

/// How much room this column reserves for proportion bars: one track, or none.
///
/// ONE per column, not one per line. Every bar in a module is drawn on the same vertical — that is
/// what makes two of them comparable at a glance — so the column reserves a single track and each
/// line draws into it. Charging it per line made the reserve depend on which line happened to be
/// widest, and the tracks then sat wherever their own figure ended.
pub(super) fn bar_zone(texts: &[LabelText], cell: &Cell) -> f32 {
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
    /// Height of the bottom volume band, in logical pixels. Zero when the band is off — or when
    /// the tab prints its captions OVER the bars (`candle_volume_labels_over`): the caller passes
    /// zero for a band that is drawn, and the captions never learn it is there.
    ///
    /// [`LabelZone::ChartBottom`] sits above whatever height arrives here, so every module in
    /// that zone clears the bars unless the caller chose to hide them; the control strip's floor
    /// is the plot's, and the bars never reach it.
    pub volume_band_h: f32,
    /// Height of the chart's top-left button strip — pin, compare lock, broom — measured down from
    /// `plot_top`, in logical pixels. Zero when no button is drawn on this pane. A LEFT-ALIGNED
    /// [`LabelZone::ChartTop`] band starts below it; every other zone and every other alignment
    /// ignores it, because the strip stands only in that one corner.
    pub corner_strip_h: f32,
}

/// One caption prepared for drawing: where its text lives and how it is styled.
#[derive(Clone, Copy)]
pub(super) struct Item {
    /// Index into the pane's resolved caption list.
    pub(super) pos: usize,
    /// Row this caption is printed on, and its index inside that row: together they address the
    /// caption's retained text run.
    pub(super) row: usize,
    pub(super) part: usize,
    pub(super) style: ResolvedLabelStyle,
    /// Whether the MODULE this caption belongs to draws a backing plate. Copied onto the item
    /// because the drawing pass has the items and not the configuration.
    pub(super) plate: bool,
    /// Font size in logical pixels, already resolved and clamped.
    pub(super) size: f32,
    /// Whether this caption is prose and may be WRAPPED rather than cut. See
    /// [`ChartLabelField::wraps`].
    pub(super) wraps: bool,
    /// How many lines it actually takes at the width it was given, filled by `plan_wraps` before
    /// anything is stacked. Always at least one.
    pub(super) lines: u8,
    /// Index of this caption's wrapped lines in `RenderState::caption_wraps`, or `usize::MAX` when
    /// it is not prose and takes the single-line path.
    pub(super) wrap_ix: usize,
}

impl Item {
    /// Height of one line at this caption's size, matching what `draw_aligned` is given.
    pub(super) fn line_h(self) -> f32 {
        self.size + 4.0
    }

    /// Height of the whole caption: one line, or the lines a wrapped one took.
    pub(super) fn block_h(self) -> f32 {
        self.line_h() * f32::from(self.lines.max(1))
    }
}

/// One column of a printed line: a module's block, or a single caption.
///
/// This is what lets a module that runs DOWN a column still sit beside its neighbour: the module
/// contributes one cell, the cell stacks its own captions, and the line places cells across.
#[derive(Clone)]
pub(super) struct Cell {
    /// Space before this column, from the module that owns it. Spent only when the column JOINS a
    /// line; a module that opened the line spends its gap above the line instead.
    pub(super) gap: f32,
    pub(super) items: Vec<Item>,
}

impl Cell {
    /// Keep a filters header and the complete entries that fit below it in the available height.
    /// Trimming before bottom anchoring prevents a tall list from placing its control offscreen.
    /// Returns whether entries were removed and their remaining wrap positions need recalculation.
    pub(super) fn fit_filter_header(&mut self, texts: &[LabelText], max_h: f32) -> bool {
        let Some(first) = self.items.first() else {
            return false;
        };
        if texts.get(first.pos).and_then(|text| text.action)
            != Some(LabelAction::ToggleStrategyFilters)
        {
            return false;
        }
        let mut height = first.block_h();
        let mut keep = 1;
        for item in self.items.iter().skip(1) {
            if height + item.block_h() > max_h {
                break;
            }
            height += item.block_h();
            keep += 1;
        }
        let trimmed = keep < self.items.len();
        self.items.truncate(keep);
        trimmed
    }

    /// Whether this column holds wrapping PROSE — a detect line — as opposed to a stacked column
    /// that also wraps. Prose is elastic (the zone is divided for it); a column is only hungry.
    pub(super) fn has_prose(&self) -> bool {
        self.items
            .iter()
            .any(|item| item.wraps && item.part < ARB_PART_BASE)
    }

    /// Whether any caption in this column wraps onto another line, prose or skip-reason alike.
    pub(super) fn has_wrap(&self) -> bool {
        self.items.iter().any(|item| item.wraps)
    }

    /// Whether this column is a stacked field — filter skip lines, the venue roster — whose
    /// natural width is the longest line it holds, not a single figure.
    pub(super) fn has_column(&self) -> bool {
        self.items
            .iter()
            .any(|item| item.part >= FILTER_HEADER_PART)
    }

    /// How tall the cell is: its captions stack, so their heights add up — and a wrapped caption
    /// counts every line it took, or the module below it would be drawn over its tail.
    pub(super) fn height(&self) -> f32 {
        self.items.iter().map(|it| it.block_h()).sum()
    }
}

/// One printed line: the columns on it, left to right.
///
/// Which MODULE opened it matters only while the lines are being grouped, and that lives in
/// [`super::fit::group_lines`]; by the time a line is drawn it is just cells.
#[derive(Clone)]
pub(super) struct Row {
    /// Space before this line, from the module that opened it.
    pub(super) gap: f32,
    pub(super) cells: Vec<Cell>,
}

impl Row {
    /// Height of the line: the tallest cell on it.
    ///
    /// Known before anything is DRAWN — which is what lets a bottom-anchored band place its first
    /// line without drawing it first. From the styles alone for every caption but one: a prose
    /// caption's line count comes from `plan_wraps`, which measures it beforehand for this reason.
    pub(super) fn height(&self) -> f32 {
        self.cells.iter().map(Cell::height).fold(0.0_f32, f32::max)
    }
}

/// Everything drawn at one alignment of one zone.
///
/// A type rather than three parallel arrays because `elastic` is the property the layout turns on,
/// and re-deriving it by walking every item — which is what the drawing pass did until this
/// existed — costs that walk on every presented frame.
pub(super) struct Band {
    pub(super) align: LabelAlign,
    pub(super) rows: Vec<Row>,
    /// Whether this band WRAPS, and so has no width of its own: a wrapped caption fills whatever
    /// budget it is handed, which is why it is drawn after the bands that do.
    pub(super) elastic: bool,
    /// Whether this band holds a COLUMN whose natural width is the plot: drawn after the figures
    /// and before wrapping prose, into what the figures left. Not elastic — two wrapping bands
    /// disable the detect-line split, and a column is not prose.
    pub(super) hungry: bool,
}

impl Band {
    pub(super) fn new(align: LabelAlign, rows: Vec<Row>) -> Self {
        let elastic = rows.iter().any(|row| row.cells.iter().any(Cell::has_prose));
        let hungry = rows
            .iter()
            .any(|row| row.cells.iter().any(Cell::has_column));
        Self {
            align,
            rows,
            elastic,
            hungry,
        }
    }

    /// Whether the band prints nothing at all.
    ///
    /// Not `rows.is_empty()`: a line whose captions all resolved to nothing keeps its `Row`, and
    /// walking a band of those is work with no pixel behind it.
    pub(super) fn is_empty(&self) -> bool {
        self.rows.iter().all(|row| row.cells.is_empty())
    }
}

/// How one column of captions is anchored.
#[derive(Clone, Copy)]
pub(super) struct Column {
    /// The x every row anchors against.
    pub(super) x: f32,
    /// Alignment fraction at that x: 0 left, 0.5 centred, 1 right.
    pub(super) align: f32,
    /// Widest a row may become before its captions are truncated.
    pub(super) max_w: f32,
}

/// What a hungry column needs to pick a width per line: the neighbours already drawn, and the
/// inset a left anchor has already spent.
#[derive(Clone, Copy)]
pub(super) struct HungryBudget {
    pub(super) total: f32,
    pub(super) align: LabelAlign,
    pub(super) taken: widths::Taken,
    pub(super) inset: f32,
}
