//! Carry-over of the three line switches from `candle_view` into `chart_graphics`, on the
//! defaults `layout.toml` stores.
//!
//! `last_price_line`, `mark_price_line` and `moonshot_zone` used to be fields of
//! [`CandleViewCfg`](crate::market::candles::CandleViewCfg) and are fields of [`ChartGraphicsCfg`]
//! now, so the "Chart graphics" popup that shows them distributes them with its own ⧉ press. An
//! existing profile still names them under `candle_view` — on the base default and on any kind's
//! own — and without this pass every user who turned a line off would watch it come back.
//!
//! The pass writes what each KIND drew. A kind's line switches came from its own `candle_view` —
//! the table replaced Main's WHOLE, so a switch it did not name was the shipped default, not
//! Main's — or, with no table of its own, from Main's; its graphics come from its own
//! `chart_graphics` or from Main's. The two chains are independent, so a kind following Main's
//! candles but holding graphics of its own must have Main's switches stamped into those graphics,
//! and a kind with candles of its own but no graphics needs graphics of its own from now on — the
//! base copy, with its switches on top — whenever the two would differ.
//!
//! No marker: the source is consumed. `CandleViewCfg::carried_lines` is written back only while
//! it is carried, so the save that follows the pass drops the old keys and a later launch finds
//! nothing to carry — and a save that lands BEFORE the pass is committed keeps them for it.

use super::WindowLayout;
use crate::config::chart_defaults::ChartTabKind;
use crate::config::layout::ChartGraphicsCfg;
use crate::market::candles::CarriedLines;

/// Stamp the switches an old table carried into a graphics config, leaving absent ones alone.
///
/// Args:
///     cfg: The graphics to write into.
///     carried: What the old `candle_view` table named.
///
/// Returns:
///     Whether anything changed.
pub fn stamp_carried_lines(cfg: &mut ChartGraphicsCfg, carried: CarriedLines) -> bool {
    let before = *cfg;
    if let Some(v) = carried.last_price_line {
        cfg.last_price_line = v;
    }
    if let Some(v) = carried.mark_price_line {
        cfg.mark_price_line = v;
    }
    if let Some(v) = carried.moonshot_zone {
        cfg.moonshot_zone = v;
    }
    *cfg != before
}

impl WindowLayout {
    /// Whether any stored `candle_view` still carries the old keys — a profile written before
    /// the move. In such a profile a table that names none of them drew the shipped defaults.
    fn carries_old_lines(&self) -> bool {
        self.candle_view.carried_lines.is_some()
            || ChartTabKind::ALL.iter().any(|kind| {
                self.kind_defaults(*kind)
                    .and_then(|d| d.candle_view)
                    .is_some_and(|cv| cv.carried_lines.is_some())
            })
    }

    /// The line switches a tab of this kind DREW before the move: its own `candle_view`'s — the
    /// shipped default for every switch that table did not name, since it replaced Main's whole
    /// — else Main's. `None` in a profile that carries nothing anywhere: written after the move.
    pub fn carried_lines_for(&self, kind: ChartTabKind) -> Option<CarriedLines> {
        let old = self.carries_old_lines();
        match self.kind_defaults(kind).and_then(|d| d.candle_view) {
            Some(own) => own
                .carried_lines
                .or(old.then_some(CarriedLines::SHIPPED))
                .map(CarriedLines::or_shipped),
            None => self.candle_view.carried_lines,
        }
    }

    /// Move the line switches every stored `candle_view` still carries into the graphics the same
    /// kind opens with, then drop them from the candle tables.
    ///
    /// Returns:
    ///     Whether the layout changed and must be saved.
    pub fn carry_lines_into_graphics(&mut self) -> bool {
        let mut changed = false;
        for kind in ChartTabKind::ALL {
            let Some(carried) = self.carried_lines_for(kind) else {
                continue;
            };
            // Resolved BEFORE the kind's own graphics may be created below: a kind with no
            // graphics of its own draws the base copy, and that copy is what its switches go on
            // top of.
            let inherited = self.chart_graphics_for(kind);
            match self.kind_defaults_mut(kind) {
                None => changed |= stamp_carried_lines(&mut self.chart_graphics, carried),
                Some(d) => match d.chart_graphics.as_mut() {
                    Some(cfg) => changed |= stamp_carried_lines(cfg, carried),
                    // Graphics inherited: those carry MAIN's switches. Where the kind's own
                    // differ it takes a copy with its own on top; where they agree it has
                    // nothing to add and keeps following.
                    None => {
                        let mut cfg = inherited;
                        if stamp_carried_lines(&mut cfg, carried) {
                            d.chart_graphics = Some(cfg);
                            changed = true;
                        }
                    }
                },
            }
        }
        // Consumed: the tables stop carrying them, so the next save drops the old keys.
        if self.candle_view.carried_lines.take().is_some() {
            changed = true;
        }
        for kind in ChartTabKind::ALL {
            let carried = self
                .kind_defaults_mut(kind)
                .and_then(|d| d.candle_view.as_mut())
                .and_then(|cv| cv.carried_lines.take());
            changed |= carried.is_some();
        }
        changed
    }
}

#[cfg(test)]
mod tests;
