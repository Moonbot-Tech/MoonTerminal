//! Part style overrides, parts and rows of the label configuration.

use super::*;

/// A part's style override. Every field is optional and absent means "whatever the FIELD defaults
/// to", so a user who only changed the color does not freeze the size against a later default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LabelStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<LabelColor>,
    /// Whether only the VALUE takes the colour, leaving the caption's prefix in the theme's.
    ///
    /// "Фандинг: +3.90%" reads as a label and a figure, and only the figure is positive — colouring
    /// the word with it makes the row a block of green that the eye has to re-parse to find the
    /// number. On by default for exactly that reason; a caption with no prefix is unaffected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_only: Option<bool>,
    /// Smallest magnitude, in percent, that is worth colouring at all.
    ///
    /// A by-sign caption paints every hundredth of a percent as a gain or a loss, and a column of
    /// arbitrage spreads then reads as noise where only one row matters. Below this the caption
    /// keeps the theme colour and still prints its value. `0` — the default — colours everything,
    /// which is what every caption did before this existed.
    ///
    /// Percent, so it applies to the fields that print one; see [`ChartLabelField::is_percent`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_min_pct: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_mult: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<bool>,
}

/// A style with every question answered, produced by laying a [`LabelStyle`] over its field's
/// default. This is what the drawing pass consumes; it never sees an `Option`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedLabelStyle {
    pub color: LabelColor,
    /// Whether the colour applies to the value alone, leaving the prefix in the theme's colour.
    pub value_only: bool,
    /// Magnitude below which a by-sign caption stays in the theme colour, in percent.
    pub color_min_pct: f32,
    /// Multiplier on the chart's label font size, already clamped to the drawable range.
    pub size_mult: f32,
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
    /// Read leniently: a field this build does not know empties this ONE part instead of failing
    /// the whole configuration. See [`wire::de_lenient_field`].
    #[serde(deserialize_with = "wire::de_lenient_field")]
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
    /// Which retained-history window a movement or volume figure is read over; meaningless for
    /// other fields.
    ///
    /// Not written while it is the default, like every other flag here: a file states what was
    /// CHANGED, and a window on a caption that ignores one is noise in a diff.
    #[serde(skip_serializing_if = "LabelWindow::is_default")]
    pub window: LabelWindow,
    /// Which timeframe a countdown caption counts down to; meaningless for other fields.
    ///
    /// Not written while it is `Авто`, like every other parameter here: a file states what was
    /// CHANGED, and a timeframe on a caption that ignores one is noise in a diff.
    #[serde(skip_serializing_if = "LabelTf::is_default")]
    pub tf: LabelTf,
    /// A custom period that OVERRIDES [`Self::window`], for the volume captions that offer one.
    ///
    /// Beside the window rather than instead of it, so switching back to a fixed window returns to
    /// the one that was set rather than to a default.
    #[serde(skip_serializing_if = "LabelSpan::is_default")]
    pub span: LabelSpan,
    /// Which currency a volume figure is stated in; meaningless for other fields.
    #[serde(skip_serializing_if = "VolumeUnits::is_default")]
    pub units: VolumeUnits,
    /// Where this caption's period sits: at the live edge, or around the pointer.
    #[serde(skip_serializing_if = "SpanAnchor::is_default")]
    pub anchor: SpanAnchor,
    /// Whether a proportion bar is drawn beside a buy/sell figure.
    ///
    /// On by default, and only where [`ChartLabelField::uses_volume_bar`] allows one: the bar is
    /// what makes the pair readable at a glance, which is the reason the block is printed as a pair
    /// at all.
    #[serde(skip_serializing_if = "is_true")]
    pub bar: bool,
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
                value_only: None,
                color_min_pct: None,
                size_mult: None,
                caption: None,
            },
            pnl_basis: PnlBasis::All,
            tf: LabelTf::Auto,
            window: LabelWindow::H1,
            span: LabelSpan::Window,
            units: VolumeUnits::Quote,
            anchor: SpanAnchor::Now,
            bar: true,
        }
    }

    /// How far back this caption reaches, in milliseconds — `None` for a trade-count span, which
    /// has no length until the trades are read.
    pub fn span_millis(&self) -> Option<i64> {
        match self.span {
            LabelSpan::Window => Some(self.window.millis()),
            LabelSpan::Seconds(n) => Some(i64::from(n) * 1_000),
            LabelSpan::Minutes(n) => Some(i64::from(n) * 60_000),
            LabelSpan::Trades(_) => None,
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
            value_only: self.style.value_only.unwrap_or(base.value_only),
            // A hand-edited negative or non-finite threshold is treated as ABSENT rather than
            // clamped: it means "colour everything", which is the default.
            color_min_pct: self
                .style
                .color_min_pct
                .filter(|v| v.is_finite() && *v >= 0.0)
                .unwrap_or(base.color_min_pct),
            // A non-finite multiplier is treated as ABSENT rather than clamped: `f32::clamp`
            // passes NaN straight through, and a NaN size reaches the shaper as a caption of no
            // size at all.
            size_mult: self
                .style
                .size_mult
                .filter(|m| m.is_finite())
                .unwrap_or(base.size_mult)
                .clamp(LABEL_SIZE_MULT_MIN, LABEL_SIZE_MULT_MAX),
            caption: self.style.caption.unwrap_or(base.caption),
        }
    }
}

/// One row of captions: where it is printed, what it is called, and what it prints.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartLabelRow {
    /// User-assigned name, which OVERRIDES [`Self::preset`]'s own. Empty means the row is named by
    /// its preset, or — with no preset either — by the fields it prints.
    ///
    /// A name typed here is the user's own words and is stored verbatim, in whatever language they
    /// typed it. That is exactly why it is not where a preset's name goes: see [`Self::preset`].
    pub name: String,
    /// Ready-made module this row was created from, if any.
    ///
    /// Held so the row can be NAMED in the reader's language: the alternative — storing the
    /// preset's localized name as [`Self::name`] at creation time — freezes that name in the
    /// language the row was created in, and the shipped default then names its modules in the
    /// developer's. The model carries no dictionary, so the lookup key is all it can hold; the
    /// terminal turns [`LabelPreset::locale_key`] into words.
    ///
    /// Purely a LABEL: nothing downstream treats a preset row differently, and editing the row's
    /// captions does not clear it — a user who renames "Position" and adds a caption to it still
    /// gets their own name, and one who clears the name gets the translated one back.
    pub preset: Option<LabelPreset>,
    /// Band this row's captions are printed in.
    pub zone: LabelZone,
    /// Where in that band the row sits.
    pub align: LabelAlign,
    /// Whether the name is printed on the chart as the row's leading caption.
    pub show_name: bool,
    /// Whether the strategy-filter module shows only its clickable header.
    /// Other modules ignore this flag; absent persisted values keep the filters expanded.
    pub collapsed: bool,
    /// Whether a translucent plate is drawn under this module.
    ///
    /// The MODULE's, not a caption's, and that is what the switch means to a reader: the plate is
    /// one rectangle behind a block of figures, so "put a backing under this" is a question about
    /// the block. Held per caption it could not be answered at all — half a plate under half a
    /// line is not a thing the chart can draw — and the switch appeared to do nothing, because a
    /// caption's neighbours kept growing the same rectangle.
    ///
    /// On by default: every caption drew a plate before this moved.
    pub plate: bool,
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
    /// Keep new rows visible and their strategy filters expanded.
    fn default() -> Self {
        Self {
            name: String::new(),
            preset: None,
            plate: true,
            zone: LabelZone::ZoneTop,
            align: LabelAlign::Center,
            show_name: false,
            collapsed: false,
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
    ///
    /// A preset does NOT keep a row alive: it is a name for what the row prints, and a row that
    /// prints nothing has nothing to name. Counting it here would leave a module behind after its
    /// last caption was deleted — the one thing `sanitize` exists to prevent.
    pub fn is_blank(&self) -> bool {
        self.name.is_empty() && !self.parts.iter().any(ChartLabelPart::is_used)
    }

    /// Locale key of the name this row takes when [`Self::name`] is empty, if it has one.
    ///
    /// The other half of [`Self::name`]: together they answer "what is this module called" without
    /// the model holding a dictionary. A row with neither is named by its captions, which only the
    /// terminal can spell.
    pub fn title_key(&self) -> Option<&'static str> {
        self.name
            .is_empty()
            .then(|| self.preset.map(LabelPreset::locale_key))
            .flatten()
    }

    /// Whether the row puts anything on the chart.
    pub fn is_drawn(&self) -> bool {
        self.visible && (self.parts.iter().any(ChartLabelPart::is_drawn) || self.prints_name())
    }

    /// Whether this module places a BUTTON — a caption that acts rather than reports.
    ///
    /// Every part is asked, hidden ones included, and that is the point of having one spelling of
    /// it: the migration must not add a second button to a module that already holds one the reader
    /// switched off, and a test asking only about the first part would answer a narrower question
    /// than the code does.
    pub fn holds_action(&self) -> bool {
        self.parts.iter().any(|part| part.field.action().is_some())
    }

    /// Whether this module places the strategy-filter column.
    ///
    /// Hidden parts count, for the same reason [`Self::holds_action`] asks every part: a one-shot
    /// insert must not add a second column to a module the reader already placed and switched off.
    pub fn holds_strategy_filters(&self) -> bool {
        self.parts
            .iter()
            .any(|part| part.field == ChartLabelField::StrategyFilters)
    }

    /// Whether the first drawn column is strategy filters and therefore owns a collapse header.
    /// A hidden column or a second column cannot claim the first column's shared run range.
    pub fn draws_strategy_filters(&self) -> bool {
        self.visible
            && self.parts[..self.used_parts()]
                .iter()
                .find(|part| part.is_drawn() && part.field.is_column())
                .is_some_and(|part| part.field == ChartLabelField::StrategyFilters)
    }

    /// Whether the row prints its own name as a caption.
    ///
    /// A preset row counts as named: the switch prints "Позиция" without the user having to type
    /// it, and it follows the language like every other caption.
    pub fn prints_name(&self) -> bool {
        self.show_name && (!self.name.is_empty() || self.preset.is_some())
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
