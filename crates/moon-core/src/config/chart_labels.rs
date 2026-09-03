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

/// First run index a module's ARBITRAGE rows occupy, past its captions and its name.
///
/// An arbitrage caption prints a whole COLUMN — one line per venue — from a single configured
/// caption, so its lines cannot be addressed as parts: there are more of them than a module holds,
/// and how many depends on what the core reports rather than on anything saved. They get their own
/// range of the same per-row stride instead, which keeps one addressing rule for every retained
/// run and costs nothing while no chart prints one (the pool grows by index, on demand).
pub const ARB_PART_BASE: usize = ROW_NAME_PART + 1;

/// First run index reserved for a caption's PREFIX.
///
/// A caption whose colour applies to the value alone is two runs, not one: the prefix in the
/// theme's colour and the figure in the sign's. They cannot share an index — a run holds one string
/// — and the prefix cannot borrow a neighbour's, so the whole prefix range mirrors the value range
/// above it. The pool grows by index on demand, so a chart that colours whole captions never
/// allocates any of this.
pub const PREFIX_PART_BASE: usize = ARB_PART_BASE + super::arb_view::ARB_MAX_ROWS;

/// Retained text runs reserved per row: one per part, one for the row's printed name, the
/// arbitrage column's own range, a prefix run mirroring each of them, and the continuation lines
/// of the one caption in the row that may wrap.
pub const ROW_RUN_STRIDE: usize = WRAP_PART_BASE + (LABEL_WRAP_LINES - 1);

/// How many lines a caption that WRAPS may take, its first line included.
///
/// Only prose wraps — see [`ChartLabelField::wraps`] — and only this far: a detect line is worth
/// two or three lines of the plot's width, and a caption that could take ten would push whatever
/// the module prints under it off the pane.
pub const LABEL_WRAP_LINES: usize = 3;

/// First run slot of the continuation lines a wrapped caption draws.
///
/// A retained text run is addressed by `row * ROW_RUN_STRIDE + part`, so a second line of the same
/// caption needs a part of its own or it would overwrite the first. Continuation `k` (counted from
/// one) takes `WRAP_PART_BASE + k - 1`.
///
/// Per ROW rather than per caption, because a retained run is some three kilobytes and the pool is
/// kept dense to its highest index: a slot per caption would reserve sixteen of them per row, on
/// every pane, to serve the one caption that is prose. The cost of that choice is the rule the
/// drawing pass enforces — only the FIRST prose caption of a module wraps, and a second one is cut
/// as it was before.
pub const WRAP_PART_BASE: usize = PREFIX_PART_BASE * 2;

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

/// Multiplier a caption draws at while nothing overrides it — every caption, of every field.
///
/// One number rather than a per-field ladder: a chart is read from across a desk, and the size the
/// captions were legible at is the same for the coin, the badge and the position figures alike. The
/// hierarchy the ladder used to state — the coin a step over the core, the badge a step under the
/// comparison delta — is still available per caption, on the popup's own size strip, which is where
/// a reader who wants one puts it.
///
/// It is a DEFAULT, not a value: a part that overrides nothing writes nothing to the file, so a
/// profile follows this number when it moves rather than freezing the one it was created under.
pub const LABEL_SIZE_MULT_DEFAULT: f32 = 1.5;

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

/// Retained-history window a caption reads its figure over.
///
/// A PARAMETER of the caption rather than a field of its own, and that is the whole point: the
/// history carries the same two figures — how far it moved, how much traded — over eight windows,
/// so spelling each pair as a field would put sixteen entries in the catalogue that differ only by
/// a number. One "Дельта" with a window control beside it is the same power in one menu line, and
/// it is also how the PnL basis already works.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelWindow {
    M1,
    /// Three minutes, which MoonProto's own rolling buckets keep beside the one and the five.
    M3,
    M5,
    M15,
    M30,
    /// The hour, which is what a glance at a chart is usually read against.
    #[default]
    H1,
    /// Two hours, which the retained candles state outright — both its movement and its volume.
    H2,
    H3,
    H24,
    H72,
}

/// How many windows a caption can choose from. The readout that fills them is indexed by
/// [`LabelWindow::ALL`]'s order, so the two must not drift.
pub const LABEL_WINDOW_COUNT: usize = 10;

impl LabelWindow {
    /// Every window, shortest first. THIS order indexes the market readout.
    pub const ALL: [LabelWindow; LABEL_WINDOW_COUNT] = [
        LabelWindow::M1,
        LabelWindow::M3,
        LabelWindow::M5,
        LabelWindow::M15,
        LabelWindow::M30,
        LabelWindow::H1,
        LabelWindow::H2,
        LabelWindow::H3,
        LabelWindow::H24,
        LabelWindow::H72,
    ];

    /// How long this window is, in milliseconds.
    ///
    /// The figure a raw scan needs: the readout serves the fixed windows off pre-aggregated
    /// buckets, but the completeness check compares this span against what the retained history
    /// actually reaches back to.
    pub fn millis(self) -> i64 {
        match self {
            LabelWindow::M1 => 60_000,
            LabelWindow::M3 => 3 * 60_000,
            LabelWindow::M5 => 5 * 60_000,
            LabelWindow::M15 => 15 * 60_000,
            LabelWindow::M30 => 30 * 60_000,
            LabelWindow::H1 => 3_600_000,
            LabelWindow::H2 => 2 * 3_600_000,
            LabelWindow::H3 => 3 * 3_600_000,
            LabelWindow::H24 => 24 * 3_600_000,
            LabelWindow::H72 => 72 * 3_600_000,
        }
    }

    /// Position in [`Self::ALL`], which is the index the readout is addressed by.
    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|w| *w == self)
            .unwrap_or_default()
    }

    /// The window as a caption spells it: `1м`, `24ч`. Short because it rides INSIDE a caption
    /// prefix over candles, where the field name has already been shortened for the same reason.
    pub fn locale_key(self) -> &'static str {
        match self {
            LabelWindow::M1 => "chart_labels.window.m1",
            LabelWindow::M3 => "chart_labels.window.m3",
            LabelWindow::M5 => "chart_labels.window.m5",
            LabelWindow::M15 => "chart_labels.window.m15",
            LabelWindow::M30 => "chart_labels.window.m30",
            LabelWindow::H1 => "chart_labels.window.h1",
            LabelWindow::H2 => "chart_labels.window.h2",
            LabelWindow::H3 => "chart_labels.window.h3",
            LabelWindow::H24 => "chart_labels.window.h24",
            LabelWindow::H72 => "chart_labels.window.h72",
        }
    }

    /// Whether this is the window a part carries when it says nothing, for the file that then does
    /// not state it.
    fn is_default(&self) -> bool {
        *self == LabelWindow::H1
    }
}

/// Timeframe whose candle a countdown caption is counting down to.
///
/// A PARAMETER of the caption for the same reason [`LabelWindow`] is one: the figure is identical
/// over every timeframe and differs only by which one, so six fields would be one field spelled six
/// times. [`Self::Auto`] follows the chart's own candle setting, which is what a reader wants on the
/// chart they are watching; a fixed one is what lets a minute chart carry the hour's and the day's
/// countdowns beside it, which is the case the feature was asked for.
///
/// The set mirrors [`crate::market::candles::CANDLE_TF_CHOICES_MIN`] — the timeframes the chart
/// itself can be set to — so a reader never picks a period the terminal cannot draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelTf {
    /// Follow the chart's own candle timeframe, whatever it is switched to next.
    #[default]
    Auto,
    M1,
    M5,
    M30,
    H1,
    H4,
    D1,
}

impl LabelTf {
    /// Every choice, in the order the editor lists them: `Авто` first, then shortest to longest.
    pub const ALL: [LabelTf; 7] = [
        LabelTf::Auto,
        LabelTf::M1,
        LabelTf::M5,
        LabelTf::M30,
        LabelTf::H1,
        LabelTf::H4,
        LabelTf::D1,
    ];

    /// Length of this timeframe in MINUTES, or `None` for [`Self::Auto`], which has none of its own.
    fn minutes(self) -> Option<u32> {
        match self {
            LabelTf::Auto => None,
            LabelTf::M1 => Some(1),
            LabelTf::M5 => Some(5),
            LabelTf::M30 => Some(30),
            LabelTf::H1 => Some(60),
            LabelTf::H4 => Some(240),
            LabelTf::D1 => Some(1440),
        }
    }

    /// This choice with [`Self::Auto`] replaced by the timeframe it currently means. Never `Auto`.
    ///
    /// Resolving to a NAMED choice rather than to a bare length is what lets the caption's prefix
    /// print the period a reader is looking at: printing `Авто` there would name the setting, and
    /// two `Авто` captions on two charts would read identically.
    ///
    /// A length this enum cannot name resolves to five minutes — the same period
    /// [`crate::market::candles::CandleViewCfg::tf_ms`] answers for a timeframe IT cannot name, so
    /// the two agree on the one case neither can express. No chart reaches it: that function is the
    /// only producer of the value, and it already answers within this set.
    pub fn resolved(self, chart_tf_ms: i64) -> LabelTf {
        if self != LabelTf::Auto {
            return self;
        }
        LabelTf::ALL
            .into_iter()
            .find(|tf| {
                tf.minutes()
                    .is_some_and(|min| i64::from(min) * 60_000 == chart_tf_ms)
            })
            .unwrap_or(LabelTf::M5)
    }

    /// Length in milliseconds, resolving [`Self::Auto`] against the chart's own timeframe.
    pub fn resolve_ms(self, chart_tf_ms: i64) -> i64 {
        i64::from(self.resolved(chart_tf_ms).minutes().unwrap_or(5)) * 60_000
    }

    /// Milliseconds left in the CURRENT candle of this timeframe, at the moment `now_ms`.
    ///
    /// Candle buckets are floored on the Unix epoch — see
    /// [`crate::market::candles::bucket_open_ms`] — so this reads the clock and nothing else: the
    /// answer is the same on every coin, every venue and every window, and no market data is
    /// consulted to produce it.
    ///
    /// It lives on the parameter rather than beside either caller because BOTH callers need it —
    /// the caption that prints the figure, and the clock that decides how often to re-print it —
    /// and they are in different crates. Two copies of one grid rule is how the two drift apart.
    ///
    /// `rem_euclid` rather than `%`: a clock before the epoch is not reachable, but the remainder
    /// of a negative dividend is negative in Rust and would report a remaining time LONGER than the
    /// timeframe. The result is in `(0, tf]` — exactly on a boundary the new candle has just
    /// opened, so the full period is what remains, and a zero is never reported.
    pub fn remaining_ms(self, chart_tf_ms: i64, now_ms: i64) -> i64 {
        let tf = self.resolve_ms(chart_tf_ms);
        tf - now_ms.rem_euclid(tf)
    }

    pub fn locale_key(self) -> &'static str {
        match self {
            LabelTf::Auto => "chart_labels.tf.auto",
            LabelTf::M1 => "chart_labels.tf.m1",
            LabelTf::M5 => "chart_labels.tf.m5",
            LabelTf::M30 => "chart_labels.tf.m30",
            LabelTf::H1 => "chart_labels.tf.h1",
            LabelTf::H4 => "chart_labels.tf.h4",
            LabelTf::D1 => "chart_labels.tf.d1",
        }
    }

    /// Whether this is the timeframe a part carries when it says nothing, for the file that then
    /// does not state it.
    fn is_default(&self) -> bool {
        *self == LabelTf::Auto
    }
}

/// Remaining time under which a candle countdown is clocked SECOND by second rather than by the
/// minute: the last hour, plus one minute of slack so the step changes before the display does.
///
/// The hour is where the caption's own format changes: past it the figure is hours and minutes,
/// which moves once a minute, so a second-by-second clock there would re-format the caption sixty
/// times for one printed change. Inside it the caption prints seconds and needs every one of them.
///
/// The minute of slack is what makes the switch land BEFORE the format needs it: a coarse clock
/// cannot express a sub-minute remainder at all, so arriving late would print `1ч 00м` where the
/// caption should already be reading `59м 50с`.
const COUNTDOWN_SECOND_STEP_BELOW_MS: i64 = 3_600_000 + 60_000;

/// Smallest and largest custom span a caption may ask for.
///
/// A minute span is bounded by what the retained history can ever cover — three days is already
/// past every ring — and a trade span by the deepest trade ring MoonProto allocates (98 000 rows on
/// the busiest venue). Past either the caption would state a figure the terminal cannot have.
pub const LABEL_SPAN_MINUTES_MAX: u16 = 4320;
/// A second span past a few minutes is a minute span spelled the long way, and the raw scan it
/// costs grows with it — the aggregates take over from there.
pub const LABEL_SPAN_SECONDS_MAX: u16 = 600;
pub const LABEL_SPAN_TRADES_MAX: u32 = 100_000;

/// The period a volume caption is read over.
///
/// A LAYER over [`LabelWindow`] rather than a replacement for it: the fixed windows are what every
/// other caption uses, they are served from readouts that are already aggregated, and they stay the
/// default. This adds the two spans the reference terminal offers beside them — an arbitrary number
/// of minutes, and an arbitrary number of TRADES, which is not a period at all and cannot be
/// expressed as one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelSpan {
    /// Read over the caption's own [`ChartLabelPart::window`].
    #[default]
    Window,
    /// Read over this many SECONDS.
    ///
    /// Below what the retained aggregates can express — they are five seconds wide — so a period
    /// this short is answered from raw trades, which is cheap precisely because it is short.
    Seconds(u16),
    /// Read over this many minutes, whatever the window says.
    Minutes(u16),
    /// Read over the last this many trades — the reference terminal's `N Trades`.
    Trades(u32),
}

impl LabelSpan {
    /// Whether this is the span a part carries when it says nothing, for the file that then does
    /// not state it.
    fn is_default(&self) -> bool {
        *self == LabelSpan::Window
    }

    /// Repair a hand-edited span: a zero count is no span at all, and an unbounded one is a scan
    /// with no end.
    fn sanitize(&mut self) {
        match self {
            LabelSpan::Window => {}
            LabelSpan::Seconds(n) => *n = (*n).clamp(1, LABEL_SPAN_SECONDS_MAX),
            LabelSpan::Minutes(n) => *n = (*n).clamp(1, LABEL_SPAN_MINUTES_MAX),
            LabelSpan::Trades(n) => *n = (*n).clamp(1, LABEL_SPAN_TRADES_MAX),
        }
    }
}

/// WHERE a caption's period sits on the time axis.
///
/// The reference terminal has this as a measuring tool: the same figures, read around the point the
/// pointer is on rather than at the live edge. It answers a different question — "what happened
/// HERE" instead of "what is happening now" — and it is the same figure either way, so it is an
/// axis of the caption rather than a second set of fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanAnchor {
    /// The live edge: the period ends now.
    #[default]
    Now,
    /// CENTRED on the pointer: half the period before it, half after.
    ///
    /// Centred rather than trailing because the pointer is placed ON something — a spike, a wick —
    /// and the question is what surrounded it. A trailing window would answer for the run-up and
    /// leave out the move itself.
    ///
    /// A caption anchored here prints nothing while the pointer is off the plot: there is no point
    /// to measure around, and holding the last one would keep stating a place the reader has left.
    Cursor,
}

impl SpanAnchor {
    pub const ALL: [SpanAnchor; 2] = [SpanAnchor::Now, SpanAnchor::Cursor];

    /// Whether this is the anchor a part carries when it says nothing.
    fn is_default(&self) -> bool {
        *self == SpanAnchor::Now
    }

    pub fn locale_key(self) -> &'static str {
        match self {
            SpanAnchor::Now => "chart_labels.anchor.now",
            SpanAnchor::Cursor => "chart_labels.anchor.cursor",
        }
    }
}

/// Which currency a volume caption states its amount in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeUnits {
    /// The market's quote money — what the reference terminal prints, and what compares across
    /// coins.
    #[default]
    Quote,
    /// The base coin itself.
    ///
    /// Available only as far back as the raw trade ring reaches: the mini-candles that serve the
    /// long windows carry `price × quantity` and no quantity of their own, so a coin figure over a
    /// window they cover would be an estimate. It is reported as incomplete instead.
    Base,
}

impl VolumeUnits {
    pub const ALL: [VolumeUnits; 2] = [VolumeUnits::Quote, VolumeUnits::Base];

    /// Whether this is the unit a part carries when it says nothing.
    fn is_default(&self) -> bool {
        *self == VolumeUnits::Quote
    }

    pub fn locale_key(self) -> &'static str {
        match self {
            VolumeUnits::Quote => "chart_labels.units.quote",
            VolumeUnits::Base => "chart_labels.units.base",
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
    fn default() -> Self {
        Self {
            name: String::new(),
            preset: None,
            plate: true,
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

/// Every label one chart draws.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartLabelsCfg {
    pub rows: [ChartLabelRow; CHART_LABEL_ROWS],
}

/// The instrument block both shipped sets open with: the coin, optionally its core, the venue.
///
/// One builder rather than two copies, so the band, the alignment and the stacking cannot drift
/// apart between the live default and the trade window's — which is exactly what a reader compares
/// when they look at the two charts side by side.
///
/// Args:
///     with_core: Whether to name the core between the coin and the venue.
///
/// Returns:
///     The row, ready to place.
fn instrument_row(with_core: bool) -> ChartLabelRow {
    let mut row = ChartLabelRow::new(LabelZone::ZoneTop, LabelAlign::Right);
    row.preset = Some(LabelPreset::Instrument);
    row.flow = LabelFlow::Column;
    row.push_part(ChartLabelField::Coin);
    if with_core {
        row.push_part(ChartLabelField::Core);
    }
    row.push_part(ChartLabelField::Venue);
    row
}

impl Default for ChartLabelsCfg {
    /// The working set the terminal ships with, and what the popup's Reset returns to.
    ///
    /// Not a designer's guess: this is the developer's own Main tab, transcribed from its
    /// `charts.json` entry on 2026-08-25 — ten modules, each placed and spaced by hand, and adopted
    /// as the shipped set so a fresh profile opens on a chart that has been USED rather than
    /// assembled.
    ///
    /// SIZES are deliberately absent from it: every caption here overrides nothing and draws at
    /// [`LABEL_SIZE_MULT_DEFAULT`], which is both what the shipped chart is meant to look like and
    /// what keeps a profile created today following that number if it ever moves again.
    ///
    /// Named through [`ChartLabelRow::preset`] rather than by a literal, so the popup speaks the
    /// reader's language: this set ships to everyone, and typed names would have shipped the
    /// developer's Russian to every locale. Three modules needed presets of their own for that —
    /// the badge, the measuring block and the session counters.
    ///
    /// Every optional figure disappears on its own when it has nothing to report, so a chart with
    /// no position shows the instrument, the badge and the volumes and nothing else.
    fn default() -> Self {
        let mut cfg = Self::empty();

        // The instrument, in the control strip pushed right: coin, core, venue stacked as a block.
        cfg.rows[0] = instrument_row(true);

        // The Y-scale badge on the plot's top-right corner.
        let mut scale = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Right);
        scale.preset = Some(LabelPreset::Scale);
        scale.push_part(ChartLabelField::ScaleBadge);
        cfg.rows[1] = scale;

        // The coin's own movement: a block of two, standing BESIDE the badge rather than under it,
        // with room between them.
        let mut deltas = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Right);
        deltas.preset = Some(LabelPreset::CoinDeltas);
        deltas.flow = LabelFlow::Column;
        deltas.placement = LabelFlow::Row;
        deltas.gap = 24;
        deltas.push_part(ChartLabelField::Delta1h);
        deltas.push_part(ChartLabelField::Delta24h);
        cfg.rows[2] = deltas;

        // What traded over the last minute, under the badge: the period, then the two sides. The
        // sides print bare — the heading above already names the period, and repeating it on every
        // line is the same word three times in a block three lines tall.
        let mut volumes = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Right);
        volumes.preset = Some(LabelPreset::Volumes);
        volumes.flow = LabelFlow::Column;
        volumes.gap = 6;
        volumes.push_part(ChartLabelField::WindowSpanName);
        volumes.push_part(ChartLabelField::WindowBuyVolume);
        volumes.push_part(ChartLabelField::WindowSellVolume);
        for part in volumes.parts.iter_mut().filter(|p| p.is_used()) {
            part.window = LabelWindow::M1;
        }
        volumes.parts[1].style.caption = Some(false);
        volumes.parts[2].style.caption = Some(false);
        cfg.rows[3] = volumes;

        // The same figures MEASURED around the pointer, over ten seconds, with the liquidations
        // that landed there. No bars: this block answers "what happened right HERE", and a bar is
        // read by comparing it against the line above it.
        let mut cursor = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Right);
        cursor.preset = Some(LabelPreset::CursorVolumes);
        cursor.flow = LabelFlow::Column;
        cursor.gap = 6;
        cursor.push_part(ChartLabelField::WindowSpanName);
        cursor.push_part(ChartLabelField::WindowBuyVolume);
        cursor.push_part(ChartLabelField::WindowSellVolume);
        cursor.push_part(ChartLabelField::WindowLiquidations);
        for part in cursor.parts.iter_mut().filter(|p| p.is_used()) {
            part.span = LabelSpan::Seconds(10);
            part.anchor = SpanAnchor::Cursor;
            part.bar = false;
        }
        cursor.parts[1].style.caption = Some(false);
        cursor.parts[2].style.caption = Some(false);
        cursor.parts[3].style.caption = Some(false);
        cfg.rows[4] = cursor;

        // What is open, as one line along the plot's top-left edge.
        let mut orders = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        orders.preset = Some(LabelPreset::Position);
        orders.placement = LabelFlow::Row;
        orders.push_part(ChartLabelField::OpenOrders);
        orders.push_part(ChartLabelField::OpenPnlMoney);
        orders.push_part(ChartLabelField::OpenPnlPct);
        orders.push_part(ChartLabelField::Exposure);
        orders.parts[2].style.caption = Some(false);
        cfg.rows[5] = orders;

        // The two session counters under it: this core's own, and the one MoonBot prints.
        let mut session = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        session.preset = Some(LabelPreset::Session);
        session.gap = 4;
        session.push_part(ChartLabelField::SessionPnl);
        session.push_part(ChartLabelField::SessionProfit);
        cfg.rows[6] = session;

        // Funding under that, spaced off the line; the countdown prints bare, beside the rate that
        // names itself.
        let mut funding = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        funding.preset = Some(LabelPreset::Funding);
        funding.gap = 8;
        funding.push_part(ChartLabelField::Funding);
        funding.push_part(ChartLabelField::FundingIn);
        funding.parts[1].style.caption = Some(false);
        cfg.rows[7] = funding;

        // The venue roster down the plot's left edge. Only a spread worth acting on is coloured;
        // below half a percent the column would be a wall of green and red with nothing to find.
        let mut arbitrage = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        arbitrage.preset = Some(LabelPreset::Arbitrage);
        arbitrage.flow = LabelFlow::Column;
        arbitrage.gap = 8;
        arbitrage.push_part(ChartLabelField::ArbColumn);
        arbitrage.parts[0].style.color = Some(LabelColor::BySign);
        arbitrage.parts[0].style.color_min_pct = Some(0.5);
        cfg.rows[8] = arbitrage;

        // What fired, and what is trading: centred over the plot, where a line of the core's own
        // prose has the width to be read. It is the only caption that WRAPS, so the modules beside
        // it yield to it — as much as it asks for, and never more than a share of the plot.
        let mut detect = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Center);
        detect.preset = Some(LabelPreset::Detect);
        detect.flow = LabelFlow::Column;
        detect.push_part(ChartLabelField::DetectStrategy);
        detect.push_part(ChartLabelField::DetectMsg);
        detect.push_part(ChartLabelField::OrderStrategy);
        cfg.rows[9] = detect;

        cfg
    }
}

impl ChartLabelsCfg {
    /// The working set a TRADE-DETAIL window opens with.
    ///
    /// Its own value rather than [`Self::default`] because that set is built for a LIVE chart:
    /// funding, the coin's deltas, what traded in the last minute, what is open right now. Printed
    /// over a trade that closed hours ago those figures are not stale, they are about a different
    /// thing entirely — and a caption is read as describing the picture under it.
    ///
    /// So this set states what the picture IS: which coin on which venue, and what the trade was —
    /// the strategy that opened it, the line it fired on, and why it closed. Everything else the
    /// window has to say is already in its own figures rail beside the chart, which is where the
    /// prices, the size and the profit live.
    ///
    /// It is a DEFAULT, not a fixture: the reader owns this view's captions like any other's, and
    /// the moment they set a default for it, theirs is what opens. See
    /// [`super::chart_defaults::ChartTabKind::Trade`].
    pub fn trade_default() -> Self {
        let mut cfg = Self::empty();

        // The Y-scale badge FIRST, so it takes the plot's top-right corner and the instrument
        // block stacks under it. This window fits each trade on its own — nothing pins its scale —
        // and the badge is what states how far the pane reaches while it does, which is what a
        // reader comparing two trades needs. Pinning a step from the window's own control hides it
        // by design: the step is then written on that control instead.
        cfg.push_preset(LabelPreset::Scale);

        // The same block the live default opens with, minus the core: "what am I looking at" is
        // the same question on a frozen chart, and which core recorded the trade is already the
        // first thing the window's own header states.
        //
        // Over the PLOT rather than in the control strip, which is the one thing this set changes
        // about it. Width is shared inside a ZONE: the strip and the plot are two of them, so a
        // block in the strip and the detect line over the plot cannot see each other and neither
        // yields — they simply overlap. In one zone the figures draw first and hand the prose what
        // is left (`chartdx::text::captions::widths`).
        let mut instrument = instrument_row(false);
        instrument.zone = LabelZone::ChartTop;
        cfg.push_prepared(instrument);

        // What the trade was, centred over the plot: the detect line is the widest thing this
        // window prints, and either edge would put it under the block above.
        cfg.push_preset(LabelPreset::Trade);

        // Repaired HERE, so the built-in set has one shape wherever it is read or compared —
        // rather than at each reader, where one of them would eventually forget.
        cfg.sanitize();
        cfg
    }

    /// The working set a COMPARISON opens with.
    ///
    /// Its own value rather than [`Self::default`] for the reason the trade window has one: a
    /// comparison is READ differently. Several panes of the same coin stand side by side, and the
    /// question asked of them is where this venue is against that one — so the figures that
    /// describe ONE market in depth are what has to go. The live default's volume block, its
    /// measuring block and its session counters are printed three or four times over on such a
    /// tab, in panes a third the width, and none of them is what the eye is there for.
    ///
    /// What stays is what a comparison is read by: which venue this pane is, how far its scale
    /// reaches, what is open on it, the venue roster, and the spread against the anchor. The last
    /// one — [`ChartLabelField::CompareDelta`] — is fed only on a book-only broom follower
    /// (`chartdx::text::captions`), so on any other pane it prints nothing and takes no room, the
    /// way every optional figure here behaves. It appears in no other shipped set for the same
    /// reason.
    ///
    /// Transcribed from the developer's own comparison tab on 2026-09-03, the way
    /// [`Self::default`] was transcribed from their main chart: a set that has been USED rather
    /// than assembled. Two SIZES come with it, which is where this set parts company with
    /// `default`'s "no sizes at all", and they go in OPPOSITE directions on purpose: against
    /// [`LABEL_SIZE_MULT_DEFAULT`] the badge is a step up and the venue roster a step down. In a
    /// pane a third of the usual width the badge is what a glance checks first, and the roster is
    /// a dozen lines that have to fit beside the plot at all. The cost is the one `default`
    /// documents — these two captions no longer follow that number if it moves — and it is
    /// accepted here because the sizes ARE the layout on a narrow pane.
    ///
    /// It follows the LOCK rather than the pane's width, and that is the intended reading: the
    /// anchor lock is a state the reader puts on and takes off, and while it is on the question
    /// being asked of the chart is "this venue against that one" whether the pane is a third of the
    /// screen or all of it. Locking a single full-width main chart therefore re-dresses it too, and
    /// unlocking hands it back the set of the kind its place gives it — nothing is written to the
    /// profile either way.
    ///
    /// It is a DEFAULT, not a fixture: the moment the reader sets one for comparisons, theirs is
    /// what opens. See [`super::chart_defaults::ChartTabKind::Compare`].
    pub fn compare_default() -> Self {
        // Stated as STEPS from the shared size rather than as absolutes, so this pair keeps its
        // relationship to that number if it ever moves — which is the property `default`'s "no
        // sizes at all" protects, kept here at the one place a size is worth spending.
        const BADGE_STEP: f32 = 0.2;
        const ROSTER_STEP: f32 = 0.25;

        let mut cfg = Self::empty();

        // The same block the live default opens with, in the control strip: on a tab where every
        // pane is the same coin, the venue under it is the pane's whole identity.
        cfg.push_prepared(instrument_row(true));

        // The Y-scale badge, a step ABOVE the shared size: two panes are only comparable while
        // their scales are, and this is the caption that states one. Through the preset rather than
        // hand-built, so its band and alignment cannot drift from the catalogue's.
        if let Some(ix) = cfg.push_preset(LabelPreset::Scale) {
            cfg.rows[ix].parts[0].style.size_mult = Some(LABEL_SIZE_MULT_DEFAULT + BADGE_STEP);
        }

        // What is open on THIS venue, as one line along the plot's top-left edge: the reason a
        // comparison is usually being looked at. Every figure keeps its caption, unlike the live
        // default's copy of this module — on a tab of near-identical panes the bare percentage
        // beside a bare amount is the one place a reader has to guess which is which.
        let mut orders = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        orders.preset = Some(LabelPreset::Position);
        orders.placement = LabelFlow::Row;
        orders.push_part(ChartLabelField::OpenOrders);
        orders.push_part(ChartLabelField::OpenPnlMoney);
        orders.push_part(ChartLabelField::OpenPnlPct);
        orders.push_part(ChartLabelField::Exposure);
        cfg.push_prepared(orders);

        // The venue roster down the left edge, a step BELOW the shared size: it is the tallest
        // module the chart prints and a narrow pane has to hold all of it. Only a spread worth
        // acting on is coloured, exactly as the live default sets it.
        if let Some(ix) = cfg.push_preset(LabelPreset::Arbitrage) {
            let row = &mut cfg.rows[ix];
            row.gap = 8;
            row.parts[0].style.color = Some(LabelColor::BySign);
            row.parts[0].style.color_min_pct = Some(0.5);
            row.parts[0].style.size_mult = Some(LABEL_SIZE_MULT_DEFAULT - ROSTER_STEP);
        }

        // The spread against the anchor, in the strip's bottom band where the pane's own numbers
        // end. No preset and no name: the field names itself, and there is no module for it to be
        // one of.
        cfg.push_row(
            ChartLabelField::CompareDelta,
            LabelZone::ZoneBottom,
            LabelAlign::Right,
        );

        // Repaired here for [`Self::trade_default`]'s reason: one shape wherever it is compared.
        cfg.sanitize();
        cfg
    }

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
                // A window the field cannot be read over is repaired to one it can: switching a
                // caption's field must not leave it asking for a figure that never arrives, and a
                // hand-edited file must not either.
                let choices = part.field.window_choices();
                if !choices.contains(&part.window) {
                    part.window = choices.first().copied().unwrap_or_default();
                }
                // A zero or unbounded custom span is repaired rather than dropped: the reader asked
                // for a custom period, and falling back to the window would silently answer a
                // different question than the one on screen.
                part.span.sanitize();
                // A timeframe on a caption that counts nothing down, dropped for the reason the
                // span below it is: switching a caption's field must not leave a parameter behind
                // that nothing reads and a later field change would suddenly obey.
                if !part.field.uses_tf() {
                    part.tf = LabelTf::Auto;
                }
                // A span on a caption that reads no period at all is dropped, like the window
                // repair above: switching a caption's field must not leave a period behind that
                // nothing reads but a later field would suddenly obey.
                if !part.field.uses_window() {
                    part.span = LabelSpan::Window;
                    // Same repair as the span: an anchor left on a caption that reads no period is
                    // a setting nothing honours, waiting to surprise a later field change.
                    part.anchor = SpanAnchor::Now;
                }
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

    /// Append a row that is already built, returning its index, or `None` when there is no room.
    ///
    /// The door the module EDITOR comes back through when it was opened on a module that does not
    /// exist yet: a new module is only worth a slot once it holds something, and a slot taken
    /// before the editor opened would be a module the user then cancelled out of. A blank row is
    /// refused here rather than pushed and swept away by [`Self::sanitize`], so the caller can tell
    /// "no room" from "nothing to add".
    pub fn push_prepared(&mut self, row: ChartLabelRow) -> Option<usize> {
        if row.is_blank() {
            return None;
        }
        let ix = self.first_free_row()?;
        self.rows[ix] = row;
        Some(ix)
    }

    /// Append a row built from a preset: its fields, in its band, under its name.
    ///
    /// The NAME is not stored — the preset is, and the terminal looks it up in the dictionary every
    /// time it prints it. A localized string baked into a saved profile keeps speaking the language
    /// it was created in, which is what the shipped default did before this.
    pub fn push_preset(&mut self, preset: LabelPreset) -> Option<usize> {
        let ix = self.first_free_row()?;
        let mut row = ChartLabelRow::new(preset.zone(), preset.align());
        row.preset = Some(preset);
        row.flow = preset.flow();
        for field in preset.fields() {
            if !row.push_part(*field) {
                break;
            }
        }
        for part in row.parts.iter_mut().filter(|p| p.field.uses_window()) {
            if let Some(window) = preset.window() {
                part.window = window;
            }
            part.span = preset.span();
            part.anchor = preset.anchor();
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

    /// The wall clock the drawn countdown captions should be formatted against, QUANTIZED to the
    /// coarsest step they can live with — or `None` when none of them is drawn.
    ///
    /// The quantum is the whole cost control for these captions. A countdown that re-formatted on
    /// every market revision would reshape a pane's whole caption set several times a second on a
    /// busy coin to print the same string; quantizing makes the caption cache answer "unchanged"
    /// until the figure actually moves. A minute is enough while every countdown is far out, and
    /// only a countdown inside its last hour — which is when the caption starts printing seconds —
    /// buys the second-by-second step.
    ///
    /// The threshold carries a minute of SLACK past the hour so the step changes just BEFORE the
    /// display needs it. Without it the switch waits for the next minute tick, and the caption
    /// spends up to a minute printing an hour figure while the seconds it should be showing run.
    ///
    /// Args:
    ///     chart_tf_ms: The chart's own candle timeframe, which an `Авто` caption resolves to.
    ///     now_ms: Unix milliseconds.
    ///
    /// Returns:
    ///     The quantized clock, or `None` when no caption counts anything down.
    ///
    /// An `Option` rather than a zero: zero is a legal clock — a machine whose system time cannot
    /// be read reports exactly that — and a caller comparing against a sentinel would then take
    /// "the epoch" for "nothing to do" and freeze the countdown for good.
    pub fn countdown_clock_ms(&self, chart_tf_ms: i64, now_ms: i64) -> Option<i64> {
        let mut quantum: Option<i64> = None;
        for part in self.drawn_parts() {
            let step = match part.field {
                ChartLabelField::FundingIn => 60_000,
                ChartLabelField::TfCloseIn => {
                    match part.tf.remaining_ms(chart_tf_ms, now_ms) < COUNTDOWN_SECOND_STEP_BELOW_MS
                    {
                        true => 1_000,
                        false => 60_000,
                    }
                }
                _ => continue,
            };
            // The FINEST step any of them asks for: a clock coarser than one caption needs would
            // freeze that caption, while a finer one merely re-formats the others for nothing.
            quantum = Some(quantum.map_or(step, |held: i64| held.min(step)));
        }
        quantum.map(|q| now_ms.div_euclid(q) * q)
    }

    /// Every distinct PERIOD the drawn volume captions ask for, in first-seen order.
    ///
    /// The sync path turns each of these into one history read, so the deduplication is the point:
    /// a module printing the buying, the selling and their total over one minute is one read, not
    /// three. Returned as the configuration's own pair rather than the market layer's span so this
    /// crate's model stays independent of what reads it.
    pub fn volume_spans(&self) -> Vec<VolumeSpanKey> {
        let mut out: Vec<VolumeSpanKey> = Vec::new();
        for part in self.drawn_parts().filter(|p| p.field.reads_volume()) {
            // A trade-count span ignores the window, so two captions asking for the same count must
            // not read twice just because their unused windows differ.
            let (span, window) = match part.span {
                LabelSpan::Window => (LabelSpan::Window, part.window),
                other => (other, LabelWindow::default()),
            };
            let key = VolumeSpanKey {
                span,
                window,
                anchor: part.anchor,
                // Liquidations come off their own ring, so a period that nothing prints `L` over
                // must not pay for reading it.
                liquidations: part.field == ChartLabelField::WindowLiquidations,
            };
            match out.iter_mut().find(|held| held.same_period(&key)) {
                // One read serves both figures over one period: the flags are unioned rather than
                // the period being listed twice.
                Some(held) => held.liquidations |= key.liquidations,
                None => out.push(key),
            }
        }
        out
    }

    /// Whether anything drawn measures around the POINTER rather than at the live edge.
    ///
    /// The gate for the extra work a measuring caption costs: its figures move with the mouse, not
    /// with the market, so they are refreshed on a path the ordinary captions never touch.
    pub fn any_cursor_anchored(&self) -> bool {
        self.drawn_parts()
            .any(|p| p.anchor == SpanAnchor::Cursor && p.field.reads_volume())
    }

    /// Whether any part uses this field, for the "already added" mark in the add menu.
    pub fn contains(&self, field: ChartLabelField) -> bool {
        self.rows
            .iter()
            .take_while(|r| !r.is_blank())
            .any(|r| r.parts[..r.used_parts()].iter().any(|p| p.field == field))
    }
}

/// One PERIOD a configuration asks the retained history for, and what it wants out of it.
///
/// The sync path turns each of these into at most one read of the trades and one of the
/// liquidations, so it is deduplicated by the period itself — the two figures over one minute are
/// one entry with both flags, not two entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolumeSpanKey {
    pub span: LabelSpan,
    /// The window the span defers to; meaningless unless `span` is [`LabelSpan::Window`].
    pub window: LabelWindow,
    /// Where the period sits: at the live edge, or around the pointer.
    pub anchor: SpanAnchor,
    /// Whether anything printed over this period reads the LIQUIDATION ring.
    pub liquidations: bool,
}

impl VolumeSpanKey {
    /// Whether two keys describe the same stretch of time, ignoring which figures want it.
    fn same_period(&self, other: &Self) -> bool {
        self.span == other.span && self.window == other.window && self.anchor == other.anchor
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
