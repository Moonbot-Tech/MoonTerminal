//! Which default a chart tab follows, and where those defaults live.
//!
//! A chart tab draws its captions, candles and graphics from its OWN setting when it has one, and
//! from a default when it does not. There used to be exactly one default per setting, shared by
//! every tab in the application — which made "make this the default" an all-or-nothing press: the
//! reader could not keep the main chart dense and the torn-off windows sparse.
//!
//! So the default is split by what a chart IS ([`ChartTabKind`]) — three kinds of TAB, plus the
//! trade-detail window, which is not a tab at all and needs its own set most of all. The base field
//! on `WindowLayout` stays the [`ChartTabKind::Main`] default — an old profile keeps working, and a
//! reader who never touches the feature keeps one default for everything — and the other kinds hold
//! [`ChartTabDefaults`], which is empty until the first time a default is set FOR that kind. Empty
//! means "follow Main", except for the CAPTIONS of the two kinds that ship their own set — the
//! trade window ([`crate::config::ChartLabelsCfg::trade_default`]) and a comparison
//! ([`crate::config::ChartLabelsCfg::compare_default`]); see
//! [`WindowLayout::set_chart_labels_default`](super::layout::WindowLayout::set_chart_labels_default)
//! for what the first press does about that.

use serde::{Deserialize, Serialize};

use super::chart_labels::ChartLabelsCfg;
use super::layout::ChartGraphicsCfg;
use crate::market::candles::CandleViewCfg;

/// What a chart tab is, for the purpose of choosing which default it follows.
///
/// Deliberately three KINDS OF TAB, not the five the tab bookkeeping can tell apart: a reader
/// groups tabs by what they are FOR, and the strip's own tabs — the main chart, the numbered
/// AddToChart tabs, a hand-assembled multi-coin tab — are all "the tabs I am looking at right now".
/// [`Self::Trade`] joins them as the one chart that is a WINDOW rather than a tab; a walk over tabs
/// iterates [`Self::TAB_KINDS`] instead of [`Self::ALL`].
///
/// [`Self::Compare`] wins over the other tab kinds, and it is a RUNTIME state rather than an
/// identity:
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
    ///
    /// Ships its own captions ([`crate::config::ChartLabelsCfg::compare_default`]): several panes
    /// of one coin, each a third of the usual width, are read by what tells them APART, and the
    /// live default's per-market blocks are printed over and over on such a tab.
    Compare,
    /// The trade-detail window: one closed trade, drawn from a frozen replay.
    ///
    /// Not a tab at all, and the only kind that is a WINDOW rather than a state a tab can be in —
    /// which is exactly why it needs its own defaults. It shows a market that stopped moving hours
    /// ago, so the captions a live chart is read with describe something that is not on the screen.
    /// Its caption default is its own rather than Main's, as [`Self::Compare`]'s is: see
    /// [`crate::config::ChartLabelsCfg::trade_default`].
    Trade,
}

impl ChartTabKind {
    /// Every kind, in the order the settings popup lists them.
    pub const ALL: [ChartTabKind; 4] = [
        ChartTabKind::Main,
        ChartTabKind::AddTo,
        ChartTabKind::Compare,
        ChartTabKind::Trade,
    ];

    /// Every kind, in the order a RESET must visit them: Main first.
    ///
    /// A reset of Main goes through the setter, whose separation pass freezes every kind that still
    /// FOLLOWS Main — which is what stops a press on the main chart from moving a kind the reader
    /// never ticked. A kind the same press also addresses is emptied again on its own turn, so it
    /// ends up following the reset Main rather than frozen at the value just discarded — and that
    /// only holds while Main is visited first.
    ///
    /// Beside [`Self::ALL`] rather than as a rule written at the caller: the ordering is a property
    /// of what these kinds ARE to each other, and the caller is in another crate where nothing
    /// tests it.
    pub const RESET_ORDER: [ChartTabKind; 4] = [
        ChartTabKind::Main,
        ChartTabKind::AddTo,
        ChartTabKind::Compare,
        ChartTabKind::Trade,
    ];

    /// The kinds that are TABS, for the walks that visit tabs.
    ///
    /// [`Self::Trade`] is absent: its windows are opened from the Report and live outside the tab
    /// strip and `charts.json` alike, so a walk that applies a setting to every matching tab has
    /// nothing to visit for it. A set rather than a predicate asked per element — the walks iterate
    /// one list or the other, and cannot forget to ask.
    pub const TAB_KINDS: [ChartTabKind; 3] = [
        ChartTabKind::Main,
        ChartTabKind::AddTo,
        ChartTabKind::Compare,
    ];

    /// The captions this kind opens with when NOTHING has been stored for it, or `None` to follow
    /// the Main default like every other kind.
    ///
    /// A property of the KIND rather than a branch inside the getter, so the mechanism reads "a
    /// kind may ship its own set" instead of "one kind is special" — and so a second kind that
    /// ever needs one costs an arm here and nothing anywhere else.
    ///
    /// Returns:
    ///     The built-in set, shared and already repaired, or `None`.
    pub fn builtin_labels(self) -> Option<&'static ChartLabelsCfg> {
        match self {
            // A trade-detail window draws a market that stopped moving, so Main's live captions —
            // funding, the last minute's volume, what is open right now — would describe something
            // that is not on the screen.
            ChartTabKind::Trade => {
                static TRADE: std::sync::OnceLock<ChartLabelsCfg> = std::sync::OnceLock::new();
                Some(TRADE.get_or_init(ChartLabelsCfg::trade_default))
            }
            // A comparison draws the SAME market several times over, in panes a third the width, so
            // Main's per-market blocks are printed once per pane and the pane's own identity — its
            // venue, its scale, its spread against the anchor — is what the eye is there for.
            ChartTabKind::Compare => {
                static COMPARE: std::sync::OnceLock<ChartLabelsCfg> = std::sync::OnceLock::new();
                Some(COMPARE.get_or_init(ChartLabelsCfg::compare_default))
            }
            ChartTabKind::Main | ChartTabKind::AddTo => None,
        }
    }

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
            ChartTabKind::Trade => "chart.defaults.kind.trade",
        }
    }
}

/// The defaults one non-Main kind holds, each absent until a default is set for that kind.
///
/// `None` means "follow the Main default", which is the state a profile starts in and stays in for
/// a reader who never splits the defaults apart. It is per SETTING rather than per kind: setting
/// the caption default for windows must not freeze their candles as a side effect.
///
/// The FIRST press for a setting fills this in for every other non-Main kind that FOLLOWS Main, not
/// only the one addressed: separating the defaults is the moment they stop moving together, and an
/// AddTo that still followed Main would jump the next time the main chart's default was set — which
/// is the surprise the split exists to remove. Each such kind is frozen at what IT was showing.
///
/// A kind that ships its own set for a setting is skipped instead: it does not follow Main, so
/// there is nothing to separate it from, and a copy taken today would outlive every later
/// improvement to that set. For the captions that is both the trade window and a comparison; see
/// [`ChartTabKind::builtin_labels`].
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
    /// The lenient read, BOXED — the shape `WindowLayout` stores these in.
    ///
    /// A `ChartTabDefaults` carries a whole caption configuration, which is a fixed array of
    /// sixteen rows of eight captions: over six kilobytes, by value. `WindowLayout` holds one per
    /// non-Main kind AND is moved around on the stack (it is loaded, cloned for a snapshot, and
    /// handed to the persistence pass), so keeping them inline put four such blocks in one frame —
    /// and the fourth is what overflowed the main thread's stack at startup. Behind a `Box` the
    /// layout carries a pointer per kind and the blocks live on the heap.
    ///
    /// See the size ceiling pinned by `config::layout::tests`.
    pub(super) fn de_lenient_boxed<'de, D>(d: D) -> Result<Box<Self>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::de_lenient(d).map(Box::new)
    }

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
