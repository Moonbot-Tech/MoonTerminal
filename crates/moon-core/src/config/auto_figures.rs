//! Auto figures: levels, sloped lines, squeezes and two-line formations the chart finds by itself
//! and draws with the ordinary figure layer.
//!
//! Configured like the chart captions (`chart_labels`): a table of rows, one row per figure "by a
//! characteristic" — which detector, its parameters, the timeframe, how many lines to keep, and how
//! they look. Rows are not figures: nothing here is drawn, saved to `figures.json` or sent to a
//! core. The detectors themselves live in `moon_chart::auto_figures`; this module is the model they
//! read and the file it is stored in. Specification: `docs-internal/AUTO_FIGURES.md`.
//!
//! The pieces, one file each:
//!
//! - `params` — the detector-facing contract (`DetectorParams`, `TfSource`, `Take`, `EventMarks`);
//! - `style` — how a row's lines and their captions look;
//! - `presets` — the ready-made rows and the shipped set;
//! - `wire` — the on-disk shape: a variable-length list that states only what differs from the
//!   defaults, keeps a detector this build does not know verbatim, and drops one broken row rather
//!   than the layout it sits in;
//! - `sanitize` — the idempotent repair every read runs.
//!
//! A `Vec` of rows rather than a fixed array, unlike `chart_labels`: nothing here addresses a
//! retained GPU text run by row index, so there is no index to keep stable.

mod params;
mod presets;
mod sanitize;
mod style;
mod wire;

pub use params::*;
pub use presets::AutoFigurePreset;
pub use sanitize::*;
pub use style::*;

use serde::{Deserialize, Deserializer, Serialize};

// Imported by name so the `#[serde(from = …)]` targets below are bare identifiers: the
// `settings_enum_tolerance` contract resolves a `from` target by its literal name.
use wire::{CfgRead, CfgWrite, RowWire};

/// Every auto-figure row of one chart kind, plus the master switch.
///
/// Read and written through `wire`: a file without the key, or with the key but no `rows`, is the
/// shipped set; an empty `rows` list is the reader's own choice and stays empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "CfgRead", into = "CfgWrite")]
pub struct AutoFiguresCfg {
    /// Master switch. Off keeps every row and its settings; it only stops the detectors.
    pub enabled: bool,
    /// The rows, in draw order: a later row draws over an earlier one.
    pub rows: Vec<AutoFigureRow>,
    /// One opacity for the whole block, `AF_OPACITY_MIN..=AF_OPACITY_MAX`: a factor on the alpha of
    /// every row's lines, their captions, their event marks and their zones.
    ///
    /// One factor rather than an alpha per row: what the reader turns down is "how loud the auto
    /// figures are against the chart", and a row's own colour, dash and zone keep telling the rows
    /// apart at any opacity. Absent from a file reads as [`AF_OPACITY_DEFAULT`].
    pub opacity: f32,
}

impl Default for AutoFiguresCfg {
    /// The shipped set, switched ON — CONTRIBUTING's "a new display feature ships ON": a profile
    /// that never stated this setting gets the full behaviour.
    fn default() -> Self {
        Self::shipped()
    }
}

impl AutoFiguresCfg {
    /// The Main chart's shipped configuration: on, with [`AutoFigurePreset::DEFAULT_SET`].
    pub fn shipped() -> Self {
        Self {
            enabled: true,
            rows: presets::shipped_rows(),
            opacity: AF_OPACITY_DEFAULT,
        }
    }

    /// The configuration a kind that does not draw auto figures yet ships: the same rows, switched
    /// OFF.
    ///
    /// The rows are there so that turning the switch on in that kind's popup shows the familiar
    /// set rather than an empty table. LinKvo, 18.11.2026: auto figures show on Main only for now;
    /// see `ChartTabKind::builtin_auto_figures`.
    pub fn shipped_off() -> Self {
        Self {
            enabled: false,
            ..Self::shipped()
        }
    }

    /// Whether a chart pane draws these auto figures: the pane's own eye override where the reader
    /// pressed it (`Some`), else this configuration's switch — and either way only while at least
    /// one row is visible and has a detector this build knows.
    ///
    /// The ONE copy of this rule: the terminal's pane sync asks it with the pane's eye, so the
    /// switch and the eye can never disagree about what "showing" means. The eye overrides the
    /// switch, not the rows: it cannot draw a row the popup has hidden.
    pub fn shows(&self, eye: Option<bool>) -> bool {
        eye.unwrap_or(self.enabled) && self.rows.iter().any(AutoFigureRow::is_drawn)
    }
}

/// One auto figure "by a characteristic": what to find, on which bars, how many, how it looks.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(into = "RowWire")]
pub struct AutoFigureRow {
    /// User-assigned name, which OVERRIDES [`Self::preset`]'s. Empty means the row is named by its
    /// preset — translated by the terminal through [`Self::title_key`] — or, with no preset, by its
    /// detector kind.
    ///
    /// Stored verbatim, in whatever language it was typed: a preset's name is NOT copied here, or
    /// it would freeze in the language the row was created in (the same reasoning as
    /// `ChartLabelRow::preset`).
    pub name: String,
    /// Ready-made row this one was created from, if any. A label: no detector, take or style treats
    /// a preset row differently, and editing its parameters does not clear it. What it decides is
    /// the row's NAME while [`Self::name`] is empty, read together with the row's current
    /// parameters ([`Self::title_key`]).
    pub preset: Option<AutoFigurePreset>,
    /// Whether the row's lines are drawn. A hidden row keeps everything, and its detector does not
    /// run.
    pub visible: bool,
    /// What the row looks for.
    pub detector: RowDetector,
    /// Which bars it looks at.
    pub tf: TfSource,
    /// Which of the found lines it keeps.
    pub take: Take,
    /// How the kept lines are drawn.
    pub style: LineStyle,
    /// What is printed on each line, and where.
    pub caption: LineCaption,
    /// Which events get a mark on the line.
    pub events: EventMarks,
}

impl AutoFigureRow {
    /// A visible, unnamed row running `detector`, with every other setting at its default.
    pub fn new(detector: DetectorParams) -> Self {
        Self {
            name: String::new(),
            preset: None,
            visible: true,
            detector: RowDetector::Known(detector),
            tf: TfSource::default(),
            take: Take::default(),
            style: LineStyle::default(),
            caption: LineCaption::default(),
            events: EventMarks::default(),
        }
    }

    /// Locale key of the name this row takes when [`Self::name`] is empty, if it has one: its
    /// preset's, read against the detector it holds now ([`AutoFigurePreset::title_key`]), so a
    /// calendar row named by its period follows the period the reader picked.
    ///
    /// The model carries no dictionary, so the key is all it can hand over; the terminal turns it
    /// into words with `t!()`.
    pub fn title_key(&self) -> Option<&'static str> {
        self.name
            .is_empty()
            .then(|| {
                self.preset
                    .and_then(|preset| preset.title_key(&self.detector, self.tf))
            })
            .flatten()
    }

    /// Whether the row puts anything on the chart: visible, and running a detector this build has.
    pub fn is_drawn(&self) -> bool {
        self.visible && self.detector.known().is_some()
    }
}

impl<'de> Deserialize<'de> for AutoFigureRow {
    /// Read the row as a raw value first and hand it to `wire`'s `TryFrom<Value>`, which keeps an
    /// unknown detector verbatim and repairs a known one field by field.
    ///
    /// Written by hand rather than as `#[serde(try_from = "serde_json::Value")]`: the
    /// `settings_enum_tolerance` contract reads every `from = "X"` in the crate as "this struct is
    /// read through the struct named `X`", and a path to a foreign type is not one it can resolve.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = serde_json::Value::deserialize(d)?;
        Self::try_from(raw).map_err(serde::de::Error::custom)
    }
}

/// A row's detector: one this build runs, or one a newer build wrote that this one keeps.
///
/// An unknown kind is not an error. Stepping back a version is routine, and a row the older build
/// cannot run must still be there — with every parameter — when the user steps forward again, the
/// way `figures::store` keeps a figure it cannot parse. Such a row is listed and never drawn.
#[derive(Debug, Clone, PartialEq)]
pub enum RowDetector {
    Known(DetectorParams),
    Unknown(UnknownDetector),
}

impl Default for RowDetector {
    fn default() -> Self {
        RowDetector::Known(DetectorParams::default())
    }
}

impl RowDetector {
    /// The parameters, when this build knows the kind.
    pub fn known(&self) -> Option<&DetectorParams> {
        match self {
            RowDetector::Known(params) => Some(params),
            RowDetector::Unknown(_) => None,
        }
    }
}

/// A detector table whose `kind` this build has no module for, kept exactly as it was read.
///
/// The map is private: nothing in this build may edit what it cannot interpret, so the only way to
/// make one is to read it from a file, and the only thing to do with it is write it back.
#[derive(Debug, Clone, PartialEq)]
pub struct UnknownDetector {
    raw: serde_json::Map<String, serde_json::Value>,
}

impl UnknownDetector {
    /// The `kind` the newer build wrote, for the table to print beside "not supported here".
    pub fn kind(&self) -> &str {
        self.raw
            .get(wire::KIND_KEY)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
