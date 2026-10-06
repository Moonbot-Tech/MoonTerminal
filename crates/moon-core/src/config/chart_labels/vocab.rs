//! Row bands, alignment, windows and the other label vocabulary types.

use super::*;

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
    pub(super) fn is_default(&self) -> bool {
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
    pub(super) fn is_default(&self) -> bool {
        *self == LabelTf::Auto
    }
}

/// How long a temporary ban runs.
///
/// MoonBot's own menu — one hour, four hours, a day, three days — so a reader who bans a coin from
/// the chart and one who bans it from the core's own window choose between the same four spans.
/// Asked at the press, by the lock's own popup and by the coin menu's rows; nothing stores one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TempBanSpan {
    /// One hour, the span a chart button is pressed for most: "not this one, not right now".
    #[default]
    H1,
    H4,
    H24,
    D3,
}

impl TempBanSpan {
    /// Every span, shortest first — the order MoonBot's own menu lists them in.
    pub const ALL: [TempBanSpan; 4] = [
        TempBanSpan::H1,
        TempBanSpan::H4,
        TempBanSpan::H24,
        TempBanSpan::D3,
    ];

    /// How long the ban lasts, in whole hours.
    ///
    /// Hours rather than a `Duration` because that is the unit the whole path speaks: the core is
    /// told a span in hours, the caption prints one, and the menu offers them.
    pub fn hours(self) -> u64 {
        match self {
            TempBanSpan::H1 => 1,
            TempBanSpan::H4 => 4,
            TempBanSpan::H24 => 24,
            TempBanSpan::D3 => 72,
        }
    }

    /// The same span as a duration, for the command that carries one.
    pub fn duration(self) -> std::time::Duration {
        std::time::Duration::from_secs(self.hours() * 3_600)
    }
}
