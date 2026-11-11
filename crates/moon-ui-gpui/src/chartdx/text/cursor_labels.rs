//! The crosshair's three values — armed order size, book depth, percent — drawn at the places the
//! tab stored for them (#1118).
//!
//! Which figure goes to which home and how far out it stacks is decided once, backend-free, in
//! `moon_chart::cursor_labels`; this module only turns a home and a rank into pixels and draws.
//! The real cursor (`prepare.rs`) and the compare-mode ghost (`runs.rs`) both call it, so a figure
//! the reader moved moves for both.

use super::*;
use moon_chart::cursor_labels::{CursorLabelHomes, resolve};
use moon_core::config::{CursorLabel, CursorLabelAnchor};

/// The values one frame has for the crosshair, already formatted; `None` for a figure with no
/// value this frame.
pub(super) struct CursorLabelTexts {
    /// The armed manual order size.
    pub order_size: Option<String>,
    /// Cumulative book notional to the cursor.
    pub depth: Option<String>,
    /// The cursor's percent from the best price of its side.
    pub percent: Option<String>,
}

impl CursorLabelTexts {
    /// The text for one figure.
    fn get(&self, label: CursorLabel) -> Option<&str> {
        match label {
            CursorLabel::OrderSize => self.order_size.as_deref(),
            CursorLabel::Depth => self.depth.as_deref(),
            CursorLabel::Percent => self.percent.as_deref(),
        }
    }
}

/// Where the frame's crosshair is and what sits around it, in logical pixels.
pub(super) struct CursorLabelFrame {
    /// The order book's left edge — or the control zone's when the book is hidden.
    pub zone_left: f32,
    /// The crosshair's horizontal line.
    pub cy: f32,
    /// The crosshair's vertical line; `None` for the ghost, which has none on this pane.
    pub cross_x: Option<f32>,
    /// Distance from the line to the nearest figure's text, so no plate covers the line.
    pub gap: f32,
    /// Bottom of the mode badge riding below-right of the crosshair, so figures placed there
    /// start under it rather than on it.
    pub badge_bottom: Option<f32>,
    /// The pane's left and right edges: a figure riding the crosshair is pulled back inside them
    /// when the cursor nears an edge, as the mode badge is.
    pub pane_x: [f32; 2],
    /// The figures' shared colour.
    pub color: Hsla,
}

impl RenderState {
    /// Draw the crosshair's figures at the tab's stored places.
    ///
    /// Each figure gets the same opaque plate it always had (`placed`), and figures sharing a
    /// column and a side stack outward one plate-height apart, so their plates touch but never
    /// overlap. The two homes beside the crosshair are one column (`moon_chart::cursor_labels`),
    /// which keeps that true when the pane-edge clamp below carries a figure across the crosshair.
    ///
    /// Args:
    ///     ctx: Text context of the current pass.
    ///     idx: Pane index; supplies the stored places and the ladder's volume column.
    ///     frame: The crosshair's position and surroundings.
    ///     texts: The formatted values.
    ///     placed: The pane's label layout for this frame, extended with every figure drawn.
    ///
    /// Errors:
    ///     Propagates a failed text draw.
    pub(super) fn draw_cursor_labels(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        idx: usize,
        frame: &CursorLabelFrame,
        texts: &CursorLabelTexts,
        placed: &mut Vec<PlacedLabel>,
    ) -> anyhow::Result<()> {
        let (cfg, volumes_right) = {
            let pr = &self.panes[idx];
            (pr.cursor_labels, pr.ladder_volumes_right)
        };
        let homes = CursorLabelHomes {
            row_volumes: volumes_right.is_some(),
            cross: frame.cross_x.is_some(),
        };
        let slots = resolve(&cfg, |label| texts.get(label).is_some(), homes);
        // One plate's height: the line plus its padding above and below.
        let step = label_line_px(self.label_font_delta) + READOUT_PAD_Y * 2.0;
        for slot in slots.into_iter().flatten() {
            let Some(text) = texts.get(slot.label) else {
                continue;
            };
            let (x, ax) = match slot.anchor {
                CursorLabelAnchor::Chart => (frame.zone_left - READOUT_PAD_X, 1.0),
                CursorLabelAnchor::Book => (frame.zone_left + READOUT_PAD_X, 0.0),
                // Past the widest volume's own text by one plate pad plus a hair, so the plate
                // starts clear of the digits instead of on them.
                CursorLabelAnchor::AfterVolumes => (
                    volumes_right.unwrap_or(frame.zone_left) + READOUT_PAD_X + READOUT_INSET,
                    0.0,
                ),
                CursorLabelAnchor::CrossLeft => (
                    frame.cross_x.unwrap_or(frame.zone_left) - CURSOR_BADGE_DX,
                    1.0,
                ),
                CursorLabelAnchor::CrossRight => (
                    frame.cross_x.unwrap_or(frame.zone_left) + CURSOR_BADGE_DX,
                    0.0,
                ),
            };
            // Only the crosshair's homes follow the pointer to an edge; the others are fixed to
            // the book and already inside the pane. `clamp_anchor` survives a pane narrower than
            // the text.
            let x = if matches!(
                slot.anchor,
                CursorLabelAnchor::CrossLeft | CursorLabelAnchor::CrossRight
            ) {
                let w = self.measure_label_text(ctx, text).width.as_f32();
                let [left, right] = frame.pane_x;
                let lo = left + READOUT_PAD_X + w * ax;
                let hi = right - READOUT_PAD_X - w * (1.0 - ax);
                clamp_anchor(x, lo, hi)
            } else {
                x
            };
            let offset = slot.rank as f32 * step;
            let (y, ay) = if slot.below {
                let mut start = frame.cy + frame.gap;
                if slot.anchor == CursorLabelAnchor::CrossRight
                    && let Some(bottom) = frame.badge_bottom
                {
                    start = start.max(bottom + READOUT_PAD_Y * 2.0);
                }
                (start + offset, 0.0)
            } else {
                (frame.cy - frame.gap - offset, 1.0)
            };
            let m = self.draw_label_text(ctx, text, x, y, ax, ay, frame.color)?;
            placed.push(PlacedLabel {
                x,
                y,
                ax,
                ay,
                w: m.width.as_f32(),
                h: m.line_height.as_f32(),
                solid: true,
            });
        }
        Ok(())
    }
}
