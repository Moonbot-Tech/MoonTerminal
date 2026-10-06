//! Custom spans, span anchors and volume units of a label part.

use super::*;

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
    pub(super) fn is_default(&self) -> bool {
        *self == LabelSpan::Window
    }

    /// Repair a hand-edited span: a zero count is no span at all, and an unbounded one is a scan
    /// with no end.
    pub(super) fn sanitize(&mut self) {
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
    pub(super) fn is_default(&self) -> bool {
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
    pub(super) fn is_default(&self) -> bool {
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
