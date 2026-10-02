//! The chart's percent ruler gesture: a modifier held with a left-button drag over the plot.

use serde::{Deserialize, Serialize};

/// Which modifier turns a left-button drag over the chart plot into the percent ruler.
///
/// Not a click gesture: a [`super::MouseGestureBinding`] names one press, while the ruler lives for
/// the whole drag and is gone on release. It acts only on the plot — the order book keeps its own
/// trading gestures on the same presses (Move Open is Shift+Left there by default), which is why
/// the two never meet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RulerDrag {
    /// No ruler: every modified left drag pans the chart, as before the ruler existed.
    None,
    #[default]
    Shift,
    Alt,
    Ctrl,
}

impl RulerDrag {
    pub const ALL: [Self; 4] = [Self::None, Self::Shift, Self::Alt, Self::Ctrl];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Shift => "shift",
            Self::Alt => "alt",
            Self::Ctrl => "ctrl",
        }
    }

    /// Whether a left press carrying these modifiers starts the ruler; `None` never does.
    ///
    /// Not exclusive, unlike the wheel binding: the drawing magnet rides the secondary modifier, so
    /// a ruler held on Shift must still start when Ctrl joins it to snap the ends to candles.
    pub fn matches(self, ctrl: bool, shift: bool, alt: bool) -> bool {
        match self {
            Self::None => false,
            Self::Shift => shift,
            Self::Alt => alt,
            Self::Ctrl => ctrl,
        }
    }
}

pub(super) fn default_ruler_drag() -> RulerDrag {
    RulerDrag::Shift
}

/// Field-default twin of [`default_ruler_drag`] for `tolerant::or_else`: a value a newer build
/// wrote falls back to the same binding an absent key gets.
pub(super) fn tolerant_ruler_drag<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<RulerDrag, D::Error> {
    super::super::tolerant::or_else(d, default_ruler_drag)
}
