//! Which default a chart tab follows, and where those defaults live.
//!
//! A chart tab draws its captions, candles and graphics from its OWN setting when it has one, and
//! from a default when it does not. There used to be exactly one default per setting, shared by
//! every tab in the application — which made "make this the default" an all-or-nothing press: the
//! reader could not keep the main chart dense and the torn-off windows sparse.
//!
//! So the default is split by what a tab IS, into the three kinds a reader actually distinguishes
//! ([`ChartTabKind`]). The base field on `WindowLayout` stays the [`ChartTabKind::Main`] default —
//! an old profile keeps working, and a reader who never touches the feature keeps one default for
//! everything — and the two other kinds hold [`ChartTabDefaults`], which is empty until the first
//! time a default is set FOR that kind. Empty means "follow Main"; see
//! [`WindowLayout::set_chart_labels_default`](super::layout::WindowLayout::set_chart_labels_default)
//! for what the first press does about that.

use serde::{Deserialize, Serialize};

use super::chart_labels::ChartLabelsCfg;
use super::layout::ChartGraphicsCfg;
use crate::market::candles::CandleViewCfg;

/// What a chart tab is, for the purpose of choosing which default it follows.
///
/// Deliberately three, not the five the tab bookkeeping can tell apart: a reader groups tabs by
/// what they are FOR, and the strip's own tabs — the main chart, the numbered AddToChart tabs, a
/// hand-assembled multi-coin tab — are all "the tabs I am looking at right now".
///
/// [`Self::Compare`] wins over the other two, and it is a RUNTIME state rather than an identity:
/// it is the anchor lock, which the reader puts on and takes off a live tab. Taking it off moves
/// the tab back to the kind its place gives it, and — its own setting having been cleared when a
/// default was applied — it adopts that kind's default at once. That is the intended behaviour and
/// not a side effect: a tab looks like what it currently IS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChartTabKind {
    /// The main chart and every tab in the strip — anything not detached and not comparing.
    Main,
    /// A tab torn off into its own window. Named for what is nearly always in one.
    AddTo,
    /// A tab under the anchor lock, wherever it lives: in the strip, in a window, or the main
    /// chart itself.
    Compare,
}

impl ChartTabKind {
    /// Every kind, in the order the settings popup lists them.
    pub const ALL: [ChartTabKind; 3] = [
        ChartTabKind::Main,
        ChartTabKind::AddTo,
        ChartTabKind::Compare,
    ];

    /// The kind of a tab that is `detached` and/or `comparing`.
    ///
    /// The lock is asked FIRST: a comparison torn off into its own window is a comparison, and
    /// answering "a window" would hand it the wrong default.
    pub fn of(detached: bool, comparing: bool) -> Self {
        match (comparing, detached) {
            (true, _) => ChartTabKind::Compare,
            (false, true) => ChartTabKind::AddTo,
            (false, false) => ChartTabKind::Main,
        }
    }

    /// Dictionary key naming this kind in the settings popup.
    pub fn locale_key(self) -> &'static str {
        match self {
            ChartTabKind::Main => "chart.defaults.kind.main",
            ChartTabKind::AddTo => "chart.defaults.kind.addto",
            ChartTabKind::Compare => "chart.defaults.kind.compare",
        }
    }
}

/// The defaults one non-Main kind holds, each absent until a default is set for that kind.
///
/// `None` means "follow the Main default", which is the state a profile starts in and stays in for
/// a reader who never splits the defaults apart. It is per SETTING rather than per kind: setting
/// the caption default for windows must not freeze their candles as a side effect.
///
/// The FIRST press for a setting fills this in for BOTH non-Main kinds, not only the one addressed:
/// separating the defaults is the moment they stop moving together, and a Compare that still
/// followed Main would jump the next time the main chart's default was set — which is the surprise
/// the split exists to remove.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChartTabDefaults {
    /// Candle and trade display, from the "Candles and Trades" popup.
    #[serde(deserialize_with = "de_lenient_field")]
    pub candle_view: Option<CandleViewCfg>,
    /// Chart drawing settings, from the graphics popup.
    #[serde(deserialize_with = "de_lenient_field")]
    pub chart_graphics: Option<ChartGraphicsCfg>,
    /// Chart captions, from the labels popup.
    #[serde(deserialize_with = "de_lenient_field")]
    pub chart_labels: Option<ChartLabelsCfg>,
}

/// Read one stored default, discarding only what cannot be read.
///
/// Per FIELD rather than per table: a hand-edited candle default must not take this kind's captions
/// and graphics down with it — the same reasoning that keeps a broken table from costing the whole
/// layout file.
fn de_lenient_field<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    /// Either the value, or anything else at all — accepted and discarded.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Shape<T> {
        Value(T),
        Other(serde::de::IgnoredAny),
    }

    Ok(match Option::<Shape<T>>::deserialize(d)? {
        Some(Shape::Value(v)) => Some(v),
        Some(Shape::Other(_)) | None => None,
    })
}

impl ChartTabDefaults {
    /// Read leniently, repairing what a hand-edited file can state.
    ///
    /// The whole layout document is one deserialization, so an unusable table here must cost these
    /// defaults only — never every window position in the file. A usable caption configuration is
    /// still sanitized: the drawing pass cannot honour a hole between captions or a size outside
    /// the drawable range, and this value is COMPARED, so an unrepaired one would look like a
    /// change on every notification.
    pub(super) fn de_lenient<'de, D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// Either the table, or anything else at all — accepted and discarded.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Shape {
            Value(Box<ChartTabDefaults>),
            Other(serde::de::IgnoredAny),
        }

        let mut out = match Option::<Shape>::deserialize(d)? {
            Some(Shape::Value(v)) => *v,
            Some(Shape::Other(_)) | None => Self::default(),
        };
        if let Some(labels) = out.chart_labels.as_mut() {
            labels.sanitize();
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
