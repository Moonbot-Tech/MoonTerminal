//! The captions on auto lines: placed here, built elsewhere.
//!
//! The strings are made on revisions by `auto_figures_sync::pane::refresh_auto_captions` — a
//! rebuild of the lines, a price change that moved a printed digit, the age clock — so this pass,
//! which runs on every presented frame, does arithmetic only: where the line's drawn stretch is on
//! screen, the line's price there, and which side of it the text goes.
//!
//! The placement itself is pure ([`caption_spot`]) so it is tested without a
//! text context; [`RenderState::draw_auto_captions`] only draws what it decides.

use moon_chart::auto_figures::{FoundShape, PairSide};
use moon_core::config::auto_figures::CaptionPlace;

use super::*;

#[cfg(test)]
mod tests;

/// One frame's mapping of a pane from time and price to logical pixels — the arithmetic the lines
/// are drawn by, so a caption lands on its line.
#[derive(Debug, Clone, Copy)]
pub(super) struct CaptionMap {
    /// The plot's `[left, top, right, bottom]`, logical pixels.
    pub plot: [f32; 4],
    /// Logical-to-physical factor of the text layer.
    pub sf: f32,
    /// The pane's epoch, Unix ms: what the view's times are relative to.
    pub epoch_ms: f64,
    /// The view's left edge, ms after the epoch.
    pub view_time0: f32,
    /// Logical pixels per millisecond.
    pub time_to_px: f32,
    /// The price at the plot's bottom edge.
    pub view_price0: f32,
    /// PHYSICAL pixels per unit of price (the view's own factor).
    pub price_to_px: f32,
}

impl CaptionMap {
    /// The logical x of the Unix time `t_ms`.
    fn x_of(&self, t_ms: f64) -> f32 {
        self.plot[0] + ((t_ms - self.epoch_ms) as f32 - self.view_time0) * self.time_to_px
    }

    /// The Unix time at logical x.
    fn t_at(&self, x: f32) -> f64 {
        self.epoch_ms + f64::from(self.view_time0) + f64::from((x - self.plot[0]) / self.time_to_px)
    }

    /// The logical y of `price`, snapped to a physical pixel like the lines.
    fn y_of(&self, price: f32) -> f32 {
        ((self.plot[3] * self.sf) - (price - self.view_price0) * self.price_to_px).round() / self.sf
    }
}

/// Where one caption goes, before its text is measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct CaptionSpot {
    /// The text anchor's x, logical pixels.
    pub x: f32,
    /// Horizontal alignment of the text about `x`: `0` left, `0.5` centre, `1` right.
    pub ax: f32,
    /// The y of the line at the point the caption names, logical pixels.
    pub line_y: f32,
}

/// Where a caption of `place` goes on a line of `shape` drawn over `span` (Unix ms, `±∞` = to
/// that plot edge), reading the price of `side` for a two-line formation; `None` when the drawn
/// stretch is off screen or the line's price there is outside the plot.
///
/// The price is read at the point the caption names — the visible left or right end of the drawn
/// stretch, or its middle — so a caption at the right end of a triangle's side sits at the end of
/// the drawn segment (its apex or the run's reference time, `draw::drawn_side`), not at the plot
/// edge where the line would be extrapolated.
///
/// Args:
///     map: The frame's pane mapping.
///     shape: The line's geometry.
///     side: The formation side the line was kept by; `None` for every other shape.
///     span: The drawn stretch.
///     place: The caption's place.
///     label_x: The right edge of the price-label column, where `AtPriceAxis` captions end.
pub(super) fn caption_spot(
    map: &CaptionMap,
    shape: &FoundShape,
    side: Option<PairSide>,
    span: (f64, f64),
    place: CaptionPlace,
    label_x: f32,
) -> Option<CaptionSpot> {
    let [plot_left, plot_top, plot_right, plot_bottom] = map.plot;
    // The part of the drawn stretch that is on screen.
    let x_lo = if span.0.is_finite() {
        map.x_of(span.0).max(plot_left)
    } else {
        plot_left
    };
    let x_hi = if span.1.is_finite() {
        map.x_of(span.1).min(plot_right)
    } else {
        plot_right
    };
    // `!(>)` also turns a NaN end away.
    if !(x_hi > x_lo) {
        return None;
    }
    // (text anchor x, horizontal alignment, where on the line the price is read).
    let (x, ax, at_x) = match place {
        CaptionPlace::LeftEnd => (x_lo + READOUT_PAD_X, 0.0, x_lo),
        CaptionPlace::RightEnd => (x_hi - READOUT_PAD_X, 1.0, x_hi),
        // The label column the order labels use, read where the drawn line ends on screen.
        CaptionPlace::AtPriceAxis => (label_x, 1.0, x_hi),
        CaptionPlace::AboveLine | CaptionPlace::BelowLine => {
            let mid = (x_lo + x_hi) * 0.5;
            (mid, 0.5, mid)
        }
    };
    // A formation's caption sits on the side it was kept by, the one it prices.
    let price = shape.side_price_at(map.t_at(at_x), side)?;
    let line_y = map.y_of(price as f32);
    if !(plot_top..=plot_bottom).contains(&line_y) {
        return None;
    }
    Some(CaptionSpot { x, ax, line_y })
}

impl RenderState {
    /// Draw pane `idx`'s auto-line captions.
    ///
    /// An `OnHover` caption is drawn only for the line under the pointer (`auto_hover`). A line
    /// whose drawn stretch is off screen, or whose price at the anchor is outside the plot, prints
    /// nothing.
    ///
    /// Args:
    ///     ctx: Text context of the frame.
    ///     idx: Pane index.
    ///     sf: Logical-to-physical factor of the text layer.
    ///     plot: The plot's `[left, top, right, bottom]` in logical pixels.
    ///     label_x: The right edge of the price-label column, where `AtPriceAxis` captions end.
    ///     placed: The frame's label layout.
    pub(super) fn draw_auto_captions(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        idx: usize,
        sf: f32,
        plot: [f32; 4],
        label_x: f32,
        placed: &mut Vec<PlacedLabel>,
    ) -> anyhow::Result<()> {
        let Some(pr) = self.panes.get(idx) else {
            return Ok(());
        };
        if pr.auto.lines.is_empty() {
            return Ok(());
        }
        let view = pr.view;
        let map = CaptionMap {
            plot,
            sf,
            epoch_ms: pr.epoch_ms,
            view_time0: view.view_time0,
            time_to_px: (view.time_to_px / sf).max(moon_chart::view::MIN_PX_PER_MS),
            view_price0: view.view_price0,
            price_to_px: view.price_to_px,
        };
        let hovered = self
            .auto_hover
            .filter(|(pane, _)| *pane == idx)
            .map(|(_, key)| key);
        let mut result = Ok(());
        for li in 0..self.panes[idx].auto.lines.len() {
            // Copied out: the text is an `Arc<str>` (a refcount bump), and drawing needs `&mut self`.
            let (text, shape, side, span, place, rgb) = {
                let line = &self.panes[idx].auto.lines[li];
                let Some(text) = line.text.clone() else {
                    continue;
                };
                if line.hover_only() && hovered != Some(line.key) {
                    continue;
                }
                (
                    text,
                    line.shape,
                    line.side,
                    line.span,
                    line.place(),
                    line.color,
                )
            };
            let Some(spot) = caption_spot(&map, &shape, side, span, place, label_x) else {
                continue;
            };
            let (dy, ay) = match place {
                CaptionPlace::BelowLine => (spot.line_y + LABEL_LINE_GAP, 0.0),
                _ => (spot.line_y - LABEL_LINE_GAP, 1.0),
            };
            let x = spot.x;
            match draw_label_text_run(
                &mut self.text_runs,
                &mut self.text_run_cursor,
                ctx,
                self.label_font_delta,
                &text,
                x,
                dy,
                spot.ax,
                ay,
                color(rgb),
            ) {
                Ok(m) => placed.push(PlacedLabel {
                    x,
                    y: dy,
                    ax: spot.ax,
                    ay,
                    w: m.width.as_f32(),
                    h: m.line_height.as_f32(),
                    solid: false,
                }),
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        result
    }
}
