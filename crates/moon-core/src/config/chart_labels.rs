//! Chart caption labels: WHAT a chart prints beside its plot, WHERE, and in WHICH style.
//!
//! The chart used to print a fixed roster — the coin, the core name that qualifies it, the
//! comparison delta and the Y-scale badge — hard-coded into the text pass. That roster became DATA
//! in PR #281: a flat list of slots, one caption each, joined into rows by an `inline` flag.
//!
//! This module is that model's second shape. A caption is no longer the unit of configuration; a
//! ROW is. A row carries its own name, the band it lives in and where in that band it sits, and
//! holds up to [`CHART_LABEL_PARTS`] captions — each with its own field, colour, size, prefix and
//! PnL basis. What used to cost four slots (the open-order figures chained with `inline`) is one
//! row with four parts, and the ceiling that used to be "sixteen captions on the whole chart" is
//! now "sixteen rows", each of them a family.
//!
//! Fixed-length rather than a `Vec`, like [`super::detect_view::DetectSizeCfg`] beside it, for the
//! reason the retained text pass needs: a part's GPU run is addressed by its INDEX
//! (`row * ROW_RUN_STRIDE + part`), and an index that shifts when a neighbour appears hands a run a
//! different string and reshapes it. The FILE, however, is written as a variable-length list —
//! see `wire` — so neither ceiling is baked into a saved profile and both can be raised without
//! costing anybody their tabs.

use serde::{Deserialize, Serialize};

mod fields;
mod presets;
mod wire;

pub use fields::{ChartLabelField, ChartLabelGroup};
pub use presets::LabelPreset;

/// Number of caption rows one chart configuration holds.
///
/// Rows past the last used one are blank and are skipped while drawing. The count also sizes the
/// terminal's per-pane text-run pool together with [`ROW_RUN_STRIDE`], so raising it costs retained
/// runs on every pane and must be done deliberately. It costs nothing in a saved file.
pub const CHART_LABEL_ROWS: usize = 16;

/// Number of captions one row holds.
///
/// Eight is what a row can print before the horizontal budget truncates it anyway: the control
/// strip is only as wide as the order book, and even a full-width row over the plot runs out of
/// room around there.
pub const CHART_LABEL_PARTS: usize = 8;

/// Retained text runs reserved per row: one per part, plus one for the row's printed name.
pub const ROW_RUN_STRIDE: usize = CHART_LABEL_PARTS + 1;

/// Run index — and part index — of a row's printed NAME.
///
/// Past every caption, so switching the name on renumbers none of them. Declared here rather than
/// in the drawing pass because it is the other half of [`ROW_RUN_STRIDE`]: two crates agreeing on
/// the stride but not on which index it reserves is a silent overlap.
pub const ROW_NAME_PART: usize = CHART_LABEL_PARTS;

/// Largest gap a module may ask for, in the chart's own logical pixels.
///
/// A gap past this is not spacing any more, it is a second band — and a hand-edited file asking for
/// one would push everything after it off the pane.
pub const LABEL_GAP_MAX: u8 = 64;

/// Longest row name kept; anything longer is cut on write.
///
/// A name is an identifier in a list and, when the row prints it, a caption over candles. Both stop
/// being readable well before this, and an unbounded string in a per-tab config is a file-size
/// question nobody wants to answer later.
pub const LABEL_ROW_NAME_MAX: usize = 48;

/// Smallest and largest font multiplier a part may carry.
///
/// The multiplier scales the chart's own label size, which already follows the Settings font
/// slider, so these bounds are relative to whatever the user picked there.
pub const LABEL_SIZE_MULT_MIN: f32 = 0.5;
pub const LABEL_SIZE_MULT_MAX: f32 = 3.0;

/// Which band of the pane a row lives in.
///
/// A chart pane is two columns, and a row belongs to one of them: `Chart*` bands lie over the
/// PLOT — the candles — while `Zone*` bands lie in the CONTROL STRIP down the right side. The strip
/// is reserved whether or not an order book is drawn, which is why a row keeps its place there with
/// the book switched off.
///
/// WHERE in the band a row sits is [`LabelAlign`], a separate axis. Folding the two together is
/// what made "right" mean the plot's edge on one pane and the strip's on another.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelZone {
    /// Along the plot's top edge.
    ///
    /// The three legacy `top_*` spellings map here: they used to carry the alignment, which is now
    /// [`ChartLabelRow::align`]'s job.
    #[serde(alias = "top_left", alias = "top_center", alias = "top_right")]
    ChartTop,
    /// Along the plot's bottom edge, filling upward.
    #[serde(alias = "bottom_left", alias = "bottom_center", alias = "bottom_right")]
    ChartBottom,
    /// Top of the control strip: the chart's traditional caption spot, and the default.
    #[default]
    #[serde(alias = "zone_top")]
    ZoneTop,
    /// Bottom of that same strip, filling upward.
    #[serde(alias = "zone_bottom")]
    ZoneBottom,
}

impl LabelZone {
    /// Every band, in popup order: the plot's, then the strip's.
    pub const ALL: [LabelZone; 4] = [
        LabelZone::ChartTop,
        LabelZone::ChartBottom,
        LabelZone::ZoneTop,
        LabelZone::ZoneBottom,
    ];

    /// Whether rows in this band stack DOWNWARD from its top edge.
    pub fn is_top(self) -> bool {
        matches!(self, LabelZone::ChartTop | LabelZone::ZoneTop)
    }

    /// Whether this band lives in the control strip rather than over the plot.
    pub fn is_control_zone(self) -> bool {
        matches!(self, LabelZone::ZoneTop | LabelZone::ZoneBottom)
    }

    pub fn locale_key(self) -> &'static str {
        match self {
            LabelZone::ChartTop => "chart_labels.zone.chart_top",
            LabelZone::ChartBottom => "chart_labels.zone.chart_bottom",
            LabelZone::ZoneTop => "chart_labels.zone.zone_top",
            LabelZone::ZoneBottom => "chart_labels.zone.zone_bottom",
        }
    }
}

/// Where in its band a row sits.
///
/// Its own axis rather than part of the band, so "push it off the close button" is one control and
/// not a different zone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelAlign {
    Left,
    #[default]
    Center,
    Right,
}

impl LabelAlign {
    pub const ALL: [LabelAlign; 3] = [LabelAlign::Left, LabelAlign::Center, LabelAlign::Right];

    /// Alignment fraction the text pass takes: 0 anchors a row's left edge, 1 its right, 0.5 its
    /// centre.
    pub fn fraction(self) -> f32 {
        match self {
            LabelAlign::Left => 0.0,
            LabelAlign::Center => 0.5,
            LabelAlign::Right => 1.0,
        }
    }

    /// Glyph for the popup's three-state control.
    pub fn glyph(self) -> &'static str {
        match self {
            LabelAlign::Left => "⇤",
            LabelAlign::Center => "≡",
            LabelAlign::Right => "⇥",
        }
    }

    pub fn locale_key(self) -> &'static str {
        match self {
            LabelAlign::Left => "chart_labels.align.left",
            LabelAlign::Center => "chart_labels.align.center",
            LabelAlign::Right => "chart_labels.align.right",
        }
    }
}

/// Which way things run: side by side, or one under another.
///
/// The SAME question is asked twice, at two levels, and that is deliberate — it is what lets one
/// chart print the position figures as one dense line and the deltas as a stacked block, without
/// either choice being a special case of the other:
///
/// - a module's own captions ([`ChartLabelRow::flow`]) run across a line or down a column;
/// - a module ([`ChartLabelRow::placement`]) either continues the previous module's line or starts
///   a new one under it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelFlow {
    /// One under another.
    #[default]
    Column,
    /// Side by side, on one line.
    Row,
}

impl LabelFlow {
    pub const ALL: [LabelFlow; 2] = [LabelFlow::Row, LabelFlow::Column];

    /// Whether this is the side-by-side direction.
    pub fn is_row(self) -> bool {
        matches!(self, LabelFlow::Row)
    }

    pub fn locale_key(self) -> &'static str {
        match self {
            LabelFlow::Row => "chart_labels.flow.row",
            LabelFlow::Column => "chart_labels.flow.column",
        }
    }

    /// Glyph for the popup's two-state control.
    pub fn glyph(self) -> &'static str {
        match self {
            LabelFlow::Row => "→",
            LabelFlow::Column => "↵",
        }
    }
}

/// How a part picks its color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode", content = "rgb")]
pub enum LabelColor {
    /// The chart theme's caption color, shared with everything else in the corner.
    #[default]
    Theme,
    /// The theme's positive or negative color, chosen by the value's own sign. A field with no
    /// sign to read falls back to the theme color rather than picking one at random.
    BySign,
    /// A fixed `0xRRGGBB` the user picked.
    Fixed(u32),
}

/// Which open orders a position figure counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PnlBasis {
    /// Every open order, live and emulated alike.
    #[default]
    All,
    /// Only orders a real (non-emulator) strategy placed.
    Real,
    /// Only emulator orders.
    Emulator,
}

impl PnlBasis {
    pub const ALL: [PnlBasis; 3] = [PnlBasis::All, PnlBasis::Real, PnlBasis::Emulator];

    pub fn locale_key(self) -> &'static str {
        match self {
            PnlBasis::All => "chart_labels.basis.all",
            PnlBasis::Real => "chart_labels.basis.real",
            PnlBasis::Emulator => "chart_labels.basis.emulator",
        }
    }

    /// Whether an order with this emulator flag counts toward the figure.
    pub fn accepts(self, emulator: bool) -> bool {
        match self {
            PnlBasis::All => true,
            PnlBasis::Real => !emulator,
            PnlBasis::Emulator => emulator,
        }
    }
}

/// A part's style override. Every field is optional and absent means "whatever the FIELD defaults
/// to", so a user who only changed the color does not freeze the size against a later default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LabelStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<LabelColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_mult: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plate: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<bool>,
}

/// A style with every question answered, produced by laying a [`LabelStyle`] over its field's
/// default. This is what the drawing pass consumes; it never sees an `Option`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedLabelStyle {
    pub color: LabelColor,
    /// Multiplier on the chart's label font size, already clamped to the drawable range.
    pub size_mult: f32,
    /// Whether a translucent plate is drawn under the row this part belongs to.
    pub plate: bool,
    /// Whether the printed text carries the field's short caption ("Δ1ч 0.8%" rather than "0.8%").
    pub caption: bool,
}

/// One configured caption: a field and how it looks.
///
/// Everything about WHERE it goes lives on the row that holds it — a part cannot be in a different
/// band from the row it is printed on, and giving it its own would be a setting with no effect.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChartLabelPart {
    pub field: ChartLabelField,
    /// Whether the caption is drawn at all. A hidden part keeps its position and style, which is
    /// the difference between this and deleting it.
    ///
    /// Not written while it is true, like the row's own switch: a file states what was turned OFF,
    /// and the default above answers everything it leaves out.
    #[serde(skip_serializing_if = "is_true")]
    pub visible: bool,
    pub style: LabelStyle,
    /// Which orders a position figure counts; meaningless for other fields.
    pub pnl_basis: PnlBasis,
}

/// Whether a flag is at its default, for the fields a file only states when they are turned off.
fn is_true(v: &bool) -> bool {
    *v
}

impl Default for ChartLabelPart {
    /// An empty, VISIBLE part.
    ///
    /// Hand-written because the derive would answer `visible: false`, and this default is what
    /// `#[serde(default)]` above hands a file that omits the flag — a file written before the flag
    /// existed, whose captions were all drawn.
    fn default() -> Self {
        Self::new(ChartLabelField::None)
    }
}

impl ChartLabelPart {
    /// A part in its simplest form: a field, fully default-styled.
    pub const fn new(field: ChartLabelField) -> Self {
        Self {
            field,
            visible: true,
            style: LabelStyle {
                color: None,
                size_mult: None,
                plate: None,
                caption: None,
            },
            pnl_basis: PnlBasis::All,
        }
    }

    /// Whether this part carries a field at all, occupied or not by a hidden flag.
    pub fn is_used(&self) -> bool {
        self.field != ChartLabelField::None
    }

    /// Whether this part contributes anything to the chart.
    pub fn is_drawn(&self) -> bool {
        self.visible && self.is_used()
    }

    /// This part's style with every question answered.
    pub fn resolved_style(&self) -> ResolvedLabelStyle {
        let base = self.field.default_style();
        ResolvedLabelStyle {
            color: self.style.color.unwrap_or(base.color),
            // A non-finite multiplier is treated as ABSENT rather than clamped: `f32::clamp`
            // passes NaN straight through, and a NaN size reaches the shaper as a caption of no
            // size at all.
            size_mult: self
                .style
                .size_mult
                .filter(|m| m.is_finite())
                .unwrap_or(base.size_mult)
                .clamp(LABEL_SIZE_MULT_MIN, LABEL_SIZE_MULT_MAX),
            plate: self.style.plate.unwrap_or(base.plate),
            caption: self.style.caption.unwrap_or(base.caption),
        }
    }
}

/// One row of captions: where it is printed, what it is called, and what it prints.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartLabelRow {
    /// User-assigned name. Empty means the popup shows the row's fields instead, and
    /// [`Self::show_name`] has nothing to print.
    pub name: String,
    /// Band this row's captions are printed in.
    pub zone: LabelZone,
    /// Where in that band the row sits.
    pub align: LabelAlign,
    /// Whether the name is printed on the chart as the row's leading caption.
    pub show_name: bool,
    /// Whether the row is drawn at all.
    ///
    /// One switch for the whole family, which is what "hide this for a moment" asks for; a row
    /// hidden here keeps its captions, its styles and its place in the order.
    pub visible: bool,
    /// Which way this module's own captions run.
    ///
    /// [`LabelFlow::Row`] is the shape the chart has always drawn — figures side by side — and the
    /// default. [`LabelFlow::Column`] makes the module a BLOCK: its captions stack, and the block
    /// takes one column of whatever line it lands on. Where that line is remains
    /// [`Self::placement`]'s question — a block can stand beside the module above it just as well
    /// as under it.
    pub flow: LabelFlow,
    /// Space before this module, in the chart's own logical pixels.
    ///
    /// ONE number for what would otherwise be four settings, because the direction is never the
    /// user's question — it is whichever way the band already runs, and the gap always goes on the
    /// side the module CAME FROM:
    ///
    /// - a module continuing a line is pushed away from the column before it — leftwards in a
    ///   right-aligned band, rightwards in a left-aligned one;
    /// - a module opening a line is pushed away from the line above it, or below it in a band that
    ///   stacks upward;
    /// - the FIRST module of a band has no neighbour, so the same number indents its line from the
    ///   band's own edge. That case has no spelling at all under an "after this module" reading,
    ///   which is why the gap is stated before rather than after.
    ///
    /// Exactly ONE direction is spent per module — the one it was placed in. A module that opens a
    /// line spends its gap above that line; a module that joins one spends it beside the column
    /// before it. Spending both would move a line diagonally rather than space it.
    pub gap: u8,
    /// Where this module goes relative to the PREVIOUS one in the same band.
    ///
    /// [`LabelFlow::Column`] — under it, which is what a list of modules does by default.
    /// [`LabelFlow::Row`] — on the same line, continuing it: two short modules that belong together
    /// read as one line without being one module, and a module that stacks its own captions stands
    /// there as a block.
    pub placement: LabelFlow,
    /// The captions, in print order. Used parts are contiguous from the front; `sanitize` closes
    /// any hole a hand-edited file states.
    pub parts: [ChartLabelPart; CHART_LABEL_PARTS],
}

impl Default for ChartLabelRow {
    fn default() -> Self {
        Self {
            name: String::new(),
            zone: LabelZone::ZoneTop,
            align: LabelAlign::Center,
            show_name: false,
            visible: true,
            // The chart's own shape before either axis existed: captions across a line, each module
            // on a line of its own.
            flow: LabelFlow::Row,
            placement: LabelFlow::Column,
            // No gap: modules sit exactly as tightly as the chart drew them before this existed.
            gap: 0,
            parts: [ChartLabelPart::new(ChartLabelField::None); CHART_LABEL_PARTS],
        }
    }
}

impl ChartLabelRow {
    /// Band a row created from the field catalogue lands in, and where in it.
    ///
    /// The control strip, pushed right, where the chart's other captions live: a new row appears
    /// somewhere the reader is already looking, and the band control moves it from there. Beside
    /// the model rather than in the popup, because [`LabelPreset`] answers the same question here.
    pub const DEFAULT_ZONE: LabelZone = LabelZone::ZoneTop;
    pub const DEFAULT_ALIGN: LabelAlign = LabelAlign::Right;

    /// Style the row's printed NAME draws with.
    ///
    /// A name is not a configured caption and carries no style of its own: it draws like a plain
    /// field, on the row's plate, without a prefix.
    pub fn name_style() -> ResolvedLabelStyle {
        ChartLabelField::None.default_style()
    }

    /// An empty row in a band, with the alignment that band is usually read at.
    pub fn new(zone: LabelZone, align: LabelAlign) -> Self {
        Self {
            zone,
            align,
            ..Default::default()
        }
    }

    /// Whether the row holds nothing at all: no caption and no name.
    ///
    /// A blank row is not a row — `sanitize` drops it, which is what makes removing a row's last
    /// caption remove the row.
    pub fn is_blank(&self) -> bool {
        self.name.is_empty() && !self.parts.iter().any(ChartLabelPart::is_used)
    }

    /// Whether the row puts anything on the chart.
    pub fn is_drawn(&self) -> bool {
        self.visible && (self.parts.iter().any(ChartLabelPart::is_drawn) || self.prints_name())
    }

    /// Whether the row prints its own name as a caption.
    pub fn prints_name(&self) -> bool {
        self.show_name && !self.name.is_empty()
    }

    /// How many leading parts carry a field.
    pub fn used_parts(&self) -> usize {
        self.first_free_part().unwrap_or(CHART_LABEL_PARTS)
    }

    /// Index of the first part holding no field, or `None` when the row is full.
    pub fn first_free_part(&self) -> Option<usize> {
        self.parts.iter().position(|p| !p.is_used())
    }

    /// Append a caption, returning whether there was room.
    pub fn push_part(&mut self, field: ChartLabelField) -> bool {
        let Some(ix) = self.first_free_part() else {
            return false;
        };
        self.parts[ix] = ChartLabelPart::new(field);
        true
    }

    /// Remove one caption, closing the gap so the remaining print order is preserved.
    pub fn remove_part(&mut self, ix: usize) {
        remove_at(&mut self.parts, ix);
    }

    /// Swap a caption with its neighbour, moving it earlier (`up`) or later in the print order.
    ///
    /// Returns whether anything moved: the ends refuse rather than wrapping around.
    pub fn move_part(&mut self, ix: usize, up: bool) -> bool {
        let used = self.used_parts();
        move_at(&mut self.parts, used, ix, up)
    }
}

/// Every label one chart draws.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartLabelsCfg {
    pub rows: [ChartLabelRow; CHART_LABEL_ROWS],
}

impl Default for ChartLabelsCfg {
    /// The working set the terminal ships with, and what the popup's Reset returns to.
    ///
    /// Not a designer's guess: this is the developer's own Main tab, transcribed from its
    /// `charts.json` entry on 2026-08-21 — five named modules, each placed and spaced by hand.
    /// The module names are theirs and travel with the layout; nothing prints them (`show_name` is
    /// off), they only name the rows in the settings popup.
    ///
    /// Every optional figure disappears on its own when it has nothing to report, so a chart with
    /// no position shows the instrument and the scale badge and nothing else.
    fn default() -> Self {
        let mut cfg = Self::empty();

        // The instrument, in the control strip pushed right: coin, core, venue stacked as a block.
        let mut instrument = ChartLabelRow::new(LabelZone::ZoneTop, LabelAlign::Right);
        instrument.name = "Инструмент".to_string();
        instrument.flow = LabelFlow::Column;
        instrument.push_part(ChartLabelField::Coin);
        instrument.push_part(ChartLabelField::Core);
        instrument.push_part(ChartLabelField::Venue);
        instrument.parts[2].style.size_mult = Some(1.0);
        cfg.rows[0] = instrument;

        // The Y-scale badge rides the plot's own top-right corner, one size up.
        let mut scale = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Right);
        scale.name = "Масштаб".to_string();
        scale.push_part(ChartLabelField::ScaleBadge);
        scale.parts[0].style.size_mult = Some(1.7);
        cfg.rows[1] = scale;

        // The coin's own movement: a block of two, standing BESIDE the badge rather than under it,
        // with room between them.
        let mut deltas = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Right);
        deltas.name = "Дельты монеты".to_string();
        deltas.flow = LabelFlow::Column;
        deltas.placement = LabelFlow::Row;
        deltas.gap = 24;
        deltas.push_part(ChartLabelField::Delta1h);
        deltas.push_part(ChartLabelField::Delta24h);
        cfg.rows[2] = deltas;

        // What is open, as one line along the plot's top-left edge.
        let mut orders = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        orders.name = "Открытые ордера".to_string();
        orders.placement = LabelFlow::Row;
        orders.push_part(ChartLabelField::OpenOrders);
        orders.push_part(ChartLabelField::OpenPnlMoney);
        orders.push_part(ChartLabelField::OpenPnlPct);
        orders.push_part(ChartLabelField::Exposure);
        cfg.rows[3] = orders;

        // Funding under it, spaced off that line; the countdown prints bare, with no prefix.
        let mut funding = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        funding.name = "Фандинг".to_string();
        funding.gap = 8;
        funding.push_part(ChartLabelField::Funding);
        funding.push_part(ChartLabelField::FundingIn);
        funding.parts[1].style.caption = Some(false);
        cfg.rows[4] = funding;

        cfg
    }
}

impl ChartLabelsCfg {
    /// A configuration with no rows at all.
    ///
    /// Public because "print nothing" is a legitimate choice a user can reach by removing every
    /// row, and the popup's reset needs the same value the loader produces for an empty list.
    pub fn empty() -> Self {
        Self {
            rows: std::array::from_fn(|_| ChartLabelRow::default()),
        }
    }

    /// Repair anything a hand-edited file — or the popup — could state that the layout cannot
    /// honour.
    ///
    /// Three repairs, and the last one is what keeps indices meaningful:
    ///
    /// 1. A size multiplier outside the drawable range is clamped into it.
    /// 2. A name longer than [`LABEL_ROW_NAME_MAX`] is cut, on a character boundary.
    /// 3. Holes are closed — captions inside a row, and blank rows in the list — so that "the
    ///    leading N are the used ones" holds everywhere, which is what the popup, the draw order
    ///    and the run pool all read.
    ///
    /// `layout.toml` and `charts.json` are both hand-editable and this configuration is
    /// materialized into specs by ⧉, so an unrepaired value would outlive the file it came from.
    pub fn sanitize(&mut self) {
        for row in &mut self.rows {
            // Trimmed before anything reads it, so "is this row named?" has ONE answer: the
            // popup's list, `is_blank` and the caption that prints the name all ask it separately.
            //
            // Trim, CUT, trim again — in that order and not the other one. Cutting a trimmed name
            // can land the boundary on a space, and a repair that leaves work for its own next run
            // is a value that never equals itself: the panel's settings signature would then report
            // a change on every notification, which is exactly what the `nan` guard below prevents.
            row.gap = row.gap.min(LABEL_GAP_MAX);
            let repaired = {
                let cut: String = row.name.trim().chars().take(LABEL_ROW_NAME_MAX).collect();
                cut.trim_end().to_string()
            };
            if row.name != repaired {
                row.name = repaired;
            }
            compact(&mut row.parts, ChartLabelPart::is_used);
            for part in &mut row.parts {
                // A hand-edited `nan` is dropped, not clamped: it would survive the clamp, and a
                // configuration that does not equal ITSELF turns every comparison downstream — the
                // panel's settings signature, the engine's change check — into a false change on
                // every notification.
                part.style.size_mult = part.style.size_mult.and_then(|mult| {
                    mult.is_finite()
                        .then(|| mult.clamp(LABEL_SIZE_MULT_MIN, LABEL_SIZE_MULT_MAX))
                });
            }
        }
        // Close holes between rows, keeping their order. A row that lost its last caption and was
        // never named is blank, and drops out here.
        compact(&mut self.rows, |row| !row.is_blank());
    }

    /// Index of the first blank row, or `None` when every row is taken.
    pub fn first_free_row(&self) -> Option<usize> {
        self.rows.iter().position(ChartLabelRow::is_blank)
    }

    /// How many leading rows hold something.
    pub fn used_rows(&self) -> usize {
        self.first_free_row().unwrap_or(CHART_LABEL_ROWS)
    }

    /// Append a row holding one caption, returning its index.
    ///
    /// A row is never created empty: an empty row is blank, and a blank row does not survive
    /// [`Self::sanitize`] — which every write goes through.
    pub fn push_row(
        &mut self,
        field: ChartLabelField,
        zone: LabelZone,
        align: LabelAlign,
    ) -> Option<usize> {
        let ix = self.first_free_row()?;
        let mut row = ChartLabelRow::new(zone, align);
        row.push_part(field);
        self.rows[ix] = row;
        Some(ix)
    }

    /// Append a row built from a preset: its fields, in its band, under its name.
    ///
    /// The name is the caller's, already localized — the model holds no dictionary, and a key baked
    /// into a saved profile would keep speaking the language it was created in.
    pub fn push_preset(&mut self, preset: LabelPreset, name: String) -> Option<usize> {
        let ix = self.first_free_row()?;
        let mut row = ChartLabelRow::new(preset.zone(), preset.align());
        row.name = name;
        for field in preset.fields() {
            if !row.push_part(*field) {
                break;
            }
        }
        self.rows[ix] = row;
        Some(ix)
    }

    /// Remove one row, closing the gap so the remaining draw order is preserved.
    pub fn remove_row(&mut self, ix: usize) {
        remove_at(&mut self.rows, ix);
    }

    /// Swap a row with its neighbour, moving it earlier (`up`) or later in the draw order.
    ///
    /// Rows in the same band stack in this order, so it is the only thing that decides which of two
    /// rows sits closer to the plot's edge. Returns whether anything moved.
    pub fn move_row(&mut self, ix: usize, up: bool) -> bool {
        let used = self.used_rows();
        move_at(&mut self.rows, used, ix, up)
    }

    /// Whether any DRAWN caption's field satisfies `pred`.
    ///
    /// The sync paths gate their work on this: collecting open-position figures walks a core's
    /// whole order array, and reading the delta snapshot takes the market-source lock. Neither is
    /// worth doing for a configuration that prints none of it — which is the default.
    pub fn any_drawn(&self, pred: impl Fn(ChartLabelField) -> bool) -> bool {
        self.drawn_parts().any(|p| pred(p.field))
    }

    /// Every caption that reaches the chart, in draw order.
    ///
    /// Stops at the first blank row rather than walking all sixteen: `sanitize` packs the used rows
    /// to the front, and the gates below run several times per market revision, per pane.
    fn drawn_parts(&self) -> impl Iterator<Item = &ChartLabelPart> {
        self.rows
            .iter()
            .take_while(|r| !r.is_blank())
            // A hidden ROW takes its captions with it: the gates below decide whether the sync
            // paths do work for them, and a row nobody sees must not order any.
            .filter(|r| r.visible)
            .flat_map(|r| r.parts[..r.used_parts()].iter())
            .filter(|p| p.is_drawn())
    }

    /// Whether any part uses this field, for the "already added" mark in the add menu.
    pub fn contains(&self, field: ChartLabelField) -> bool {
        self.rows
            .iter()
            .take_while(|r| !r.is_blank())
            .any(|r| r.parts[..r.used_parts()].iter().any(|p| p.field == field))
    }
}

/// Move the used items of a fixed-length list to the front, blanking the tail.
///
/// The same repair at both levels — captions inside a row, rows inside a configuration — because
/// both read "the leading N are the used ones" everywhere: the popup's list, the draw order and the
/// retained-run pool.
fn compact<T: Default>(items: &mut [T], is_used: impl Fn(&T) -> bool) {
    let mut write = 0;
    for read in 0..items.len() {
        if is_used(&items[read]) {
            items.swap(write, read);
            write += 1;
        }
    }
    for item in &mut items[write..] {
        *item = T::default();
    }
}

/// Remove one item, closing the gap and blanking the freed tail slot.
fn remove_at<T: Default>(items: &mut [T], ix: usize) {
    if ix >= items.len() {
        return;
    }
    items[ix..].rotate_left(1);
    if let Some(last) = items.last_mut() {
        *last = T::default();
    }
}

/// Swap one item with its neighbour, refusing at the ends of the USED run rather than wrapping.
fn move_at<T>(items: &mut [T], used: usize, ix: usize, up: bool) -> bool {
    if ix >= used {
        return false;
    }
    // `wrapping_sub` turns "up from the first" into an index past the end, which the same bound
    // rejects — one check instead of a nested pair.
    let other = if up { ix.wrapping_sub(1) } else { ix + 1 };
    if other >= used {
        return false;
    }
    items.swap(ix, other);
    true
}

#[cfg(test)]
mod tests;
