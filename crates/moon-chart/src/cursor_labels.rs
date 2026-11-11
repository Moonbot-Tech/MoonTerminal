//! Which crosshair figure prints at which home, and how far out it stacks (#1118).
//!
//! Backend-free on purpose: the text pass of every GPU backend and the compare-mode ghost turn the
//! same stored places into the same slots here, then only resolve pixels themselves.

use moon_core::config::{CursorLabel, CursorLabelAnchor, CursorLabelsCfg};

/// One figure's resolved place for this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorLabelSlot {
    /// The figure.
    pub label: CursorLabel,
    /// Its home after the frame's fallbacks.
    pub anchor: CursorLabelAnchor,
    /// Whether it prints below the line.
    pub below: bool,
    /// How many figures sit between it and the line in the same column and on the same side.
    pub rank: usize,
}

/// What the frame can offer the figures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorLabelHomes {
    /// The ladder printed row volumes this frame, so "past the volumes" has a width to clear.
    /// Without them that home is the book's left edge.
    pub row_volumes: bool,
    /// The crosshair's vertical line is on this pane. The compare-mode ghost has none, so a figure
    /// placed beside the crosshair prints at the book's left edge, where the ghost always printed.
    pub cross: bool,
}

/// The column a home stacks in. The two homes beside the crosshair share one: at a pane edge the
/// text pass pulls a figure back inside the pane, which can carry a left one past the crosshair
/// onto the right one's spot, and one shared column keeps them a line apart instead of on top of
/// each other.
fn stack_of(anchor: CursorLabelAnchor) -> CursorLabelAnchor {
    match anchor {
        CursorLabelAnchor::CrossLeft => CursorLabelAnchor::CrossRight,
        other => other,
    }
}

/// Resolve each present figure's home, side and rank.
///
/// Figures stack in the stored order, nearest the line first, and only against figures that land
/// in the same column ([`stack_of`]) and on the same side AFTER the fallbacks — a figure the frame
/// moved onto the book edge stacks with the ones already there instead of printing over them. A
/// hidden or absent figure takes no rank, so the rest close up.
///
/// Args:
///     cfg: The tab's stored places and order.
///     present: Whether a figure has a value this frame (no armed size, no book level near the
///         cursor, no reference price).
///     homes: What the frame can offer.
///
/// Returns:
///     One entry per figure in stacking order; `None` for a figure that does not print.
pub fn resolve(
    cfg: &CursorLabelsCfg,
    present: impl Fn(CursorLabel) -> bool,
    homes: CursorLabelHomes,
) -> [Option<CursorLabelSlot>; 3] {
    let mut out = [None; 3];
    for (i, label) in cfg.order.iter().enumerate() {
        if !present(*label) {
            continue;
        }
        let Some((anchor, below)) = cfg.spot(*label).place() else {
            continue;
        };
        let anchor = match anchor {
            CursorLabelAnchor::AfterVolumes if !homes.row_volumes => CursorLabelAnchor::Book,
            CursorLabelAnchor::CrossLeft | CursorLabelAnchor::CrossRight if !homes.cross => {
                CursorLabelAnchor::Book
            }
            other => other,
        };
        let rank = out
            .iter()
            .flatten()
            .filter(|s: &&CursorLabelSlot| {
                stack_of(s.anchor) == stack_of(anchor) && s.below == below
            })
            .count();
        out[i] = Some(CursorLabelSlot {
            label: *label,
            anchor,
            below,
            rank,
        });
    }
    out
}

#[cfg(test)]
mod tests;
