//! Chart caption labels: WHAT a chart prints beside its plot, WHERE, and in WHICH style.
//!
//! The chart used to print a fixed roster — the coin, the core name that qualifies it, the
//! comparison delta and the Y-scale badge — hard-coded into the text pass in that order and in that
//! corner. This module turns that roster into DATA: a fixed-length list of slots, each naming a
//! field, a zone, whether it shares the previous slot's row, and an optional style override.
//! [`ChartLabelsCfg::default`] reproduces the old roster exactly, so a profile that never touches
//! the popup keeps the caption it has always had.
//!
//! Fixed-length rather than a `Vec`, like [`super::detect_view::DetectSizeCfg`] beside it, for
//! three reasons that all point the same way: the whole configuration stays `Copy`, which is what
//! `StackSetting` in the terminal requires to travel through the shared ⧉ walk; the retained GPU
//! text run of a slot can be addressed by its INDEX, which is what keeps a hidden slot from
//! reshaping every run below it every frame; and a chart with more than [`CHART_LABEL_SLOTS`]
//! captions on it is not a chart any more.

use serde::{Deserialize, Serialize};

/// Number of label slots one chart configuration holds.
///
/// Slots past the last used one carry [`ChartLabelField::None`] and are skipped while drawing. The
/// count is also the size of the terminal's per-pane text-run pool, so raising it costs retained
/// runs on every pane and must be done deliberately.
pub const CHART_LABEL_SLOTS: usize = 16;

/// Smallest and largest font multiplier a slot may carry.
///
/// The multiplier scales the chart's own label size, which already follows the Settings font
/// slider, so these bounds are relative to whatever the user picked there.
pub const LABEL_SIZE_MULT_MIN: f32 = 0.5;
pub const LABEL_SIZE_MULT_MAX: f32 = 3.0;

/// What one slot prints.
///
/// The wire form is the serde name, so a variant may be REORDERED here freely but never RENAMED
/// without migrating `charts.json` and `layout.toml`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartLabelField {
    /// Empty slot: prints nothing and occupies no row.
    #[default]
    None,
    /// Coin ticker as the pane resolved it (`BEAT-USDT`, `@206`'s classic name).
    Coin,
    /// Name of the core the pane's market belongs to.
    Core,
    /// Venue the pane's core trades on, through the shared venue directory.
    Venue,
    /// Current Y-scale badge as a whole percentage of the visible range.
    ScaleBadge,
    /// Comparison-mode difference from the locked anchor's price, signed.
    CompareDelta,
    /// Last traded price.
    LastPrice,
    /// Signed one-hour price change, from the same readout the header ticker uses.
    Delta1h,
    /// Signed 24-hour price change, from the same readout the header ticker uses.
    Delta24h,
    /// Unrealized result of the orders open RIGHT NOW on this market, as a percentage of what they
    /// spent. See [`PnlBasis`] for which orders count.
    OpenPnlPct,
    /// The same result in the market's quote money.
    OpenPnlMoney,
    /// How many orders are open on this market.
    OpenOrders,
    /// Open position size in the base coin.
    PosSize,
    /// Current notional of what is open, in the market's quote currency: position size times the
    /// mark price.
    ///
    /// Counted over a WIDER set than the PnL figures: an order whose entry price has not arrived
    /// still has a size and a mark, and withholding it would understate what is actually at risk.
    Exposure,
    /// User-assigned name of the strategy that owns the newest open order on this market.
    OrderStrategy,
}

impl ChartLabelField {
    /// Every assignable field, in the order the "add label" menu offers them.
    pub const ALL: [ChartLabelField; 14] = [
        ChartLabelField::Coin,
        ChartLabelField::Core,
        ChartLabelField::Venue,
        ChartLabelField::LastPrice,
        ChartLabelField::Delta1h,
        ChartLabelField::Delta24h,
        ChartLabelField::ScaleBadge,
        ChartLabelField::CompareDelta,
        ChartLabelField::OpenPnlPct,
        ChartLabelField::OpenPnlMoney,
        ChartLabelField::OpenOrders,
        ChartLabelField::PosSize,
        ChartLabelField::Exposure,
        ChartLabelField::OrderStrategy,
    ];

    /// Menu section this field belongs to.
    pub fn group(self) -> ChartLabelGroup {
        match self {
            ChartLabelField::Coin
            | ChartLabelField::Core
            | ChartLabelField::Venue
            | ChartLabelField::None => ChartLabelGroup::Instrument,
            ChartLabelField::LastPrice
            | ChartLabelField::Delta1h
            | ChartLabelField::Delta24h
            | ChartLabelField::ScaleBadge
            | ChartLabelField::CompareDelta => ChartLabelGroup::Market,
            ChartLabelField::OpenPnlPct
            | ChartLabelField::OpenPnlMoney
            | ChartLabelField::OpenOrders
            | ChartLabelField::PosSize
            | ChartLabelField::Exposure => ChartLabelGroup::Position,
            ChartLabelField::OrderStrategy => ChartLabelGroup::Strategy,
        }
    }

    /// Locale key for this field's menu and list label.
    pub fn locale_key(self) -> &'static str {
        match self {
            ChartLabelField::None => "chart_labels.field.none",
            ChartLabelField::Coin => "chart_labels.field.coin",
            ChartLabelField::Core => "chart_labels.field.core",
            ChartLabelField::Venue => "chart_labels.field.venue",
            ChartLabelField::ScaleBadge => "chart_labels.field.scale_badge",
            ChartLabelField::CompareDelta => "chart_labels.field.compare_delta",
            ChartLabelField::LastPrice => "chart_labels.field.last_price",
            ChartLabelField::Delta1h => "chart_labels.field.delta_1h",
            ChartLabelField::Delta24h => "chart_labels.field.delta_24h",
            ChartLabelField::OpenPnlPct => "chart_labels.field.open_pnl_pct",
            ChartLabelField::OpenPnlMoney => "chart_labels.field.open_pnl_money",
            ChartLabelField::OpenOrders => "chart_labels.field.open_orders",
            ChartLabelField::PosSize => "chart_labels.field.pos_size",
            ChartLabelField::Exposure => "chart_labels.field.exposure",
            ChartLabelField::OrderStrategy => "chart_labels.field.order_strategy",
        }
    }

    /// Locale key of the SHORT prefix a caption prints when its style asks for one.
    ///
    /// Separate from [`Self::locale_key`]: the menu needs a name a reader can pick from a list
    /// ("PnL открытых, %"), while a caption drawn over candles needs the shortest thing that still
    /// identifies the figure ("PnL"). A field with nothing worth prefixing returns `None`.
    pub fn caption_key(self) -> Option<&'static str> {
        match self {
            ChartLabelField::Delta1h => Some("chart_labels.short.delta_1h"),
            ChartLabelField::Delta24h => Some("chart_labels.short.delta_24h"),
            ChartLabelField::OpenPnlPct | ChartLabelField::OpenPnlMoney => {
                Some("chart_labels.short.pnl")
            }
            ChartLabelField::OpenOrders => Some("chart_labels.short.orders"),
            ChartLabelField::PosSize => Some("chart_labels.short.position"),
            ChartLabelField::Exposure => Some("chart_labels.short.exposure"),
            _ => None,
        }
    }

    /// Whether this field takes a [`PnlBasis`], and therefore shows that control in the popup.
    ///
    /// Asked of the FIELD rather than stored per slot so a basis cannot linger on a slot whose
    /// field was changed to something that ignores it.
    pub fn uses_pnl_basis(self) -> bool {
        matches!(
            self,
            ChartLabelField::OpenPnlPct
                | ChartLabelField::OpenPnlMoney
                | ChartLabelField::OpenOrders
                | ChartLabelField::PosSize
                | ChartLabelField::Exposure
        )
    }

    /// Style this field draws with when its slot overrides nothing.
    ///
    /// These are the sizes and colors the hard-coded caption used, so the default configuration
    /// reproduces it without the popup restating them.
    pub fn default_style(self) -> ResolvedLabelStyle {
        match self {
            // The coin leads, one size up: it is the fact a glance needs.
            ChartLabelField::Coin => ResolvedLabelStyle {
                color: LabelColor::Theme,
                size_mult: 1.25,
                plate: true,
                caption: false,
            },
            // The comparison delta is the one figure a broom-mode pane exists to show.
            ChartLabelField::CompareDelta => ResolvedLabelStyle {
                color: LabelColor::BySign,
                size_mult: 1.7,
                plate: true,
                caption: false,
            },
            // Deliberately smaller than the comparison delta beside it: a secondary indicator must
            // not compete with the figure the pane is being read for.
            ChartLabelField::ScaleBadge => ResolvedLabelStyle {
                color: LabelColor::Theme,
                size_mult: 1.45,
                plate: true,
                caption: false,
            },
            ChartLabelField::Delta1h
            | ChartLabelField::Delta24h
            | ChartLabelField::OpenPnlPct
            | ChartLabelField::OpenPnlMoney => ResolvedLabelStyle {
                color: LabelColor::BySign,
                size_mult: 1.0,
                plate: true,
                caption: true,
            },
            // Counts and sizes carry their caption too: a bare "2" over the candles names nothing.
            ChartLabelField::OpenOrders | ChartLabelField::PosSize | ChartLabelField::Exposure => {
                ResolvedLabelStyle {
                    color: LabelColor::Theme,
                    size_mult: 1.0,
                    plate: true,
                    caption: true,
                }
            }
            _ => ResolvedLabelStyle {
                color: LabelColor::Theme,
                size_mult: 1.0,
                plate: true,
                caption: false,
            },
        }
    }
}

/// Section a field appears under in the "add label" menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartLabelGroup {
    Instrument,
    Market,
    Position,
    Strategy,
}

impl ChartLabelGroup {
    /// Sections in menu order.
    pub const ALL: [ChartLabelGroup; 4] = [
        ChartLabelGroup::Instrument,
        ChartLabelGroup::Market,
        ChartLabelGroup::Position,
        ChartLabelGroup::Strategy,
    ];

    pub fn locale_key(self) -> &'static str {
        match self {
            ChartLabelGroup::Instrument => "chart_labels.group.instrument",
            ChartLabelGroup::Market => "chart_labels.group.market",
            ChartLabelGroup::Position => "chart_labels.group.position",
            ChartLabelGroup::Strategy => "chart_labels.group.strategy",
        }
    }
}

/// Which band of the pane a slot's row lives in.
///
/// A chart pane is two columns, and a caption belongs to one of them: `Chart*` bands lie over the
/// PLOT — the candles — while `Zone*` bands lie in the CONTROL STRIP down the right side. The strip
/// is reserved whether or not an order book is drawn, which is why a caption keeps its place there
/// with the book switched off.
///
/// WHERE in the band a row sits is [`LabelAlign`], a separate axis. Folding the two together is
/// what made "right" mean the plot's edge on one pane and the strip's on another.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelZone {
    /// Along the plot's top edge.
    ///
    /// The three legacy `top_*` spellings map here: they used to carry the alignment, which is now
    /// [`ChartLabelSlot::align`]'s job.
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

/// How a slot picks its color.
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

/// A slot's style override. Every field is optional and absent means "whatever the FIELD defaults
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
    /// Whether a translucent plate is drawn under the row this slot belongs to.
    pub plate: bool,
    /// Whether the printed text carries the field's short caption ("Δ1ч 0.8%" rather than "0.8%").
    pub caption: bool,
}

/// One configured label.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChartLabelSlot {
    pub field: ChartLabelField,
    /// Band this slot's row lives in. Ignored while [`Self::inline`] is set: an inline slot joins
    /// the row of the slot before it and cannot be in a different band from it.
    pub zone: LabelZone,
    /// Where in that band the row sits. Like the band, an inline slot inherits it from the row it
    /// joins — a row has ONE alignment, or it is not a row.
    pub align: LabelAlign,
    /// Whether this slot shares the previous VISIBLE slot's row instead of opening a new one.
    ///
    /// The first slot of a zone can never be inline — there is no row to join — and
    /// [`ChartLabelsCfg::sanitize`] clears the flag rather than trusting a hand-edited file.
    pub inline: bool,
    /// Whether the slot is drawn at all. A hidden slot keeps its position and style, which is the
    /// difference between this and deleting it.
    #[serde(default = "def_true")]
    pub visible: bool,
    pub style: LabelStyle,
    /// Which orders a position figure counts; meaningless for other fields.
    pub pnl_basis: PnlBasis,
}

fn def_true() -> bool {
    true
}

impl ChartLabelSlot {
    /// A slot in its simplest form: a field in a zone, opening its own row, fully default-styled.
    pub const fn new(field: ChartLabelField, zone: LabelZone) -> Self {
        Self {
            field,
            zone,
            align: LabelAlign::Center,
            inline: false,
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

    /// The same, joined to the previous visible slot's row.
    pub const fn inline(field: ChartLabelField, zone: LabelZone) -> Self {
        let mut s = Self::new(field, zone);
        s.inline = true;
        s
    }

    /// Whether this slot contributes anything to the chart.
    pub fn is_drawn(&self) -> bool {
        self.visible && self.field != ChartLabelField::None
    }

    /// This slot's style with every question answered.
    pub fn resolved_style(&self) -> ResolvedLabelStyle {
        let base = self.field.default_style();
        ResolvedLabelStyle {
            color: self.style.color.unwrap_or(base.color),
            size_mult: self
                .style
                .size_mult
                .unwrap_or(base.size_mult)
                .clamp(LABEL_SIZE_MULT_MIN, LABEL_SIZE_MULT_MAX),
            plate: self.style.plate.unwrap_or(base.plate),
            caption: self.style.caption.unwrap_or(base.caption),
        }
    }
}

/// Every label one chart draws.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChartLabelsCfg {
    pub slots: [ChartLabelSlot; CHART_LABEL_SLOTS],
}

impl Default for ChartLabelsCfg {
    /// The working set the terminal ships with.
    ///
    /// Not a designer's guess: this is the layout the developer arrived at by hand and asked to
    /// become the default (2026-08-20), transcribed from the Main tab's own `charts.json` entry.
    /// The corner block sits in the control strip pushed right, the Y-scale badge rides the plot's
    /// top-right, and the open-order figures run as one row along the plot's top edge on the left —
    /// where the badge overlay they replaced used to draw them.
    ///
    /// Every optional figure disappears on its own when it has nothing to report, so a chart with
    /// no position shows the coin and the core name and nothing else.
    fn default() -> Self {
        const EMPTY: ChartLabelSlot =
            ChartLabelSlot::new(ChartLabelField::None, LabelZone::ZoneTop);
        let mut slots = [EMPTY; CHART_LABEL_SLOTS];
        slots[0] = ChartLabelSlot::new(ChartLabelField::Coin, LabelZone::ZoneTop);
        slots[0].align = LabelAlign::Right;
        slots[1] = ChartLabelSlot::new(ChartLabelField::ScaleBadge, LabelZone::ChartTop);
        slots[1].align = LabelAlign::Right;
        slots[2] = ChartLabelSlot::new(ChartLabelField::Core, LabelZone::ZoneTop);
        slots[2].align = LabelAlign::Right;
        // The open-order row. Muted grey on the count so the money beside it leads the eye.
        slots[3] = ChartLabelSlot::new(ChartLabelField::OpenOrders, LabelZone::ChartTop);
        slots[3].align = LabelAlign::Left;
        slots[3].style.color = Some(LabelColor::Fixed(0x8d99ae));
        slots[4] = ChartLabelSlot::inline(ChartLabelField::Exposure, LabelZone::ChartTop);
        slots[4].align = LabelAlign::Left;
        slots[5] = ChartLabelSlot::inline(ChartLabelField::OpenPnlMoney, LabelZone::ChartTop);
        slots[5].align = LabelAlign::Left;
        slots[6] = ChartLabelSlot::inline(ChartLabelField::OpenPnlPct, LabelZone::ChartTop);
        slots[6].align = LabelAlign::Left;
        Self { slots }
    }
}

impl ChartLabelsCfg {
    /// Repair anything a hand-edited file — or the popup — could state that the layout cannot
    /// honour.
    ///
    /// Two invariants, and the second one is load-bearing:
    ///
    /// 1. The FIRST drawn slot cannot be inline. There is no row before it to join, and the layout
    ///    pass would have to invent one.
    /// 2. An inline slot takes the BAND and the ALIGNMENT of the row it joins. A caption cannot be
    ///    in one band and on another band's row at the same time, and letting it keep a stale band
    ///    is what made a caption disappear from where the user put it: it drifted to the band its
    ///    own value named, became the first slot there, and lost the inline flag on arrival.
    ///
    /// `layout.toml` and `charts.json` are both hand-editable and this configuration is
    /// materialized into specs by ⧉, so an unrepaired value would outlive the file it came from.
    pub fn sanitize(&mut self) {
        let mut row_style: Option<(LabelZone, LabelAlign)> = None;
        for slot in &mut self.slots {
            if let Some(mult) = slot.style.size_mult {
                slot.style.size_mult = Some(mult.clamp(LABEL_SIZE_MULT_MIN, LABEL_SIZE_MULT_MAX));
            }
            if !slot.is_drawn() {
                continue;
            }
            match row_style {
                Some((zone, align)) if slot.inline => {
                    slot.zone = zone;
                    slot.align = align;
                }
                _ => slot.inline = false,
            }
            row_style = Some((slot.zone, slot.align));
        }
    }

    /// Index of the first slot holding no field, or `None` when every slot is taken.
    pub fn first_free(&self) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.field == ChartLabelField::None)
    }

    /// Append a field to a zone, returning whether there was room.
    pub fn push(&mut self, field: ChartLabelField, zone: LabelZone) -> bool {
        let Some(ix) = self.first_free() else {
            return false;
        };
        self.slots[ix] = ChartLabelSlot::new(field, zone);
        true
    }

    /// Remove one slot, closing the gap so the remaining order is preserved.
    ///
    /// The gap has to close: order IS the configuration here — it decides both which row a label
    /// lands on and where in that row it sits — and a hole would silently separate two slots the
    /// user put next to each other.
    pub fn remove(&mut self, ix: usize) {
        if ix >= CHART_LABEL_SLOTS {
            return;
        }
        for i in ix..CHART_LABEL_SLOTS - 1 {
            self.slots[i] = self.slots[i + 1];
        }
        self.slots[CHART_LABEL_SLOTS - 1] =
            ChartLabelSlot::new(ChartLabelField::None, LabelZone::ZoneTop);
        self.sanitize();
    }

    /// Swap a slot with its neighbour, moving it earlier (`up`) or later in the draw order.
    ///
    /// Returns whether anything moved: the ends of the list refuse rather than wrapping around.
    pub fn move_slot(&mut self, ix: usize, up: bool) -> bool {
        let used = self.used_len();
        if ix >= used {
            return false;
        }
        let other = if up {
            if ix == 0 {
                return false;
            }
            ix - 1
        } else {
            if ix + 1 >= used {
                return false;
            }
            ix + 1
        };
        self.slots.swap(ix, other);
        self.sanitize();
        true
    }

    /// How many leading slots carry a field.
    pub fn used_len(&self) -> usize {
        self.slots
            .iter()
            .position(|s| s.field == ChartLabelField::None)
            .unwrap_or(CHART_LABEL_SLOTS)
    }

    /// Whether any DRAWN slot's field satisfies `pred`.
    ///
    /// The sync paths gate their work on this: collecting open-position figures walks a core's
    /// whole order array, and reading the delta snapshot takes the market-source lock. Neither is
    /// worth doing for a configuration that prints none of it — which is the default.
    pub fn any_drawn(&self, pred: impl Fn(ChartLabelField) -> bool) -> bool {
        self.slots.iter().any(|s| s.is_drawn() && pred(s.field))
    }

    /// Whether any drawn slot uses this field, for the "already added" mark in the add menu.
    pub fn contains(&self, field: ChartLabelField) -> bool {
        self.slots.iter().any(|s| s.field == field)
    }
}

#[cfg(test)]
mod tests;
