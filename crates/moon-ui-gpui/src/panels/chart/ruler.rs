//! The percent ruler's gesture on the chart panel: press with the configured modifier over the
//! plot, drag, read the move, release — and nothing is left behind.
//!
//! The panel only holds the press and turns the pointer into data coordinates; what the ruler shows
//! and how it is drawn belongs to the engine (`chartdx/ruler.rs`).

use gpui::{App, Modifiers};
use moon_core::figures::FigNode;

use super::ChartPanel;
use crate::chartdx::RulerSpan;

/// A ruler held by the left button: the pane it measures on and the point its press fixed.
#[derive(Clone, Copy, Debug)]
pub(super) struct RulerHold {
    pane: usize,
    start: FigNode,
}

impl ChartPanel {
    /// Start the ruler when a left press carries its modifier over a pane's plot.
    ///
    /// The plot only. `chart_gesture_pane_at` excludes the order book, its strip, the horizontal
    /// volumes and a broom pane, where the same presses are trading gestures (Move Open is
    /// Shift+Left in the book by default) — but it answers for the WHOLE pane rectangle, so the
    /// price-axis gutter and the time axis are excluded here, against the plot itself: a press
    /// there keeps stretching the axis.
    ///
    /// Returns:
    ///     Whether the ruler took the press; the caller then starts no pan.
    pub(super) fn try_start_ruler(
        &mut self,
        pos: (f32, f32),
        modifiers: Modifiers,
        cx: &App,
    ) -> bool {
        let binding = {
            let b = self.backend.read(cx);
            b.preview.as_ref().unwrap_or(&b.config).hotkeys.ruler_drag
        };
        if !binding.matches(modifiers.control, modifiers.shift, modifiers.alt) {
            return false;
        }
        let Some(pane) = self.chart_gesture_pane_at(pos) else {
            return false;
        };
        let Some(map) = self.pane_map(pane) else {
            return false;
        };
        let plot = map.plot;
        if !(plot.x..=plot.x + plot.w).contains(&pos.0)
            || !(plot.y..=plot.y + plot.h).contains(&pos.1)
        {
            return false;
        }
        // The drawing magnet, on the same modifier as for figures: the ends snap to candle prices
        // and ticks, which is where a measured move actually starts and stops.
        let start = self.fig_pointer_node(pane, pos, &map, modifiers.secondary());
        self.ruler = Some(RulerHold { pane, start });
        self.update_ruler(pos, modifiers, cx);
        true
    }

    /// Move the ruler's far end to the pointer.
    pub(super) fn update_ruler(&mut self, pos: (f32, f32), modifiers: Modifiers, cx: &App) {
        let Some(hold) = self.ruler else {
            return;
        };
        let Some(map) = self.pane_map(hold.pane) else {
            return;
        };
        let end = self.fig_pointer_node(hold.pane, pos, &map, modifiers.secondary());
        let source = self.backend.read(cx).session.market_source();
        self.chart.set_ruler(
            Some(RulerSpan {
                pane: hold.pane,
                t0_ms: hold.start.time_ms,
                p0: hold.start.price,
                t1_ms: end.time_ms,
                p1: end.price,
            }),
            &source,
        );
    }

    /// Drop the ruler; nothing it measured is kept.
    ///
    /// Returns:
    ///     Whether a ruler was held, so the caller knows the release was its gesture.
    pub(super) fn end_ruler(&mut self, cx: &App) -> bool {
        if self.ruler.take().is_none() {
            return false;
        }
        let source = self.backend.read(cx).session.market_source();
        self.chart.set_ruler(None, &source);
        true
    }

    /// Whether the left button currently holds the ruler.
    pub(super) fn ruler_held(&self) -> bool {
        self.ruler.is_some()
    }
}
