//! Row text for the order-book ladder: each row's volume and price, printed inside the book zone.
//!
//! The bars are the book layer's level lines (`moon_core::data::ladder_instances`); this pass only
//! prints over them. It runs every present, like the rest of the text layer, over the rows and the
//! strings the last book sync built — no book read and no formatting happens here.

use super::*;

/// Horizontal inset of the row text from the zone's edges, logical pixels.
const LADDER_TEXT_PAD_X: f32 = 4.0;

/// Slots in a pane's ladder run pool. A row's slot is its grid index modulo this, so two rows
/// share one only when they are this many rows apart — farther than any zone shows at once.
const LADDER_RUN_SLOTS: i64 = 512;

/// Format each row's volume and price for the text pass, once per rebuild.
///
/// Args:
///     rows: The rows just built.
///     tick: The grid's tick; the price prints with its decimals, so adjacent rows line up.
///     out: Cleared, then filled index-aligned with `rows`.
pub(in crate::chartdx) fn format_ladder_labels(
    rows: &[moon_core::data::LadderRow],
    tick: f64,
    out: &mut Vec<(String, String)>,
) {
    let decimals = moon_chart::ladder::tick_decimals(tick);
    out.clear();
    out.extend(
        rows.iter()
            .map(|r| (fmt_amount(r.notional), format!("{:.*}", decimals, r.price))),
    );
}

impl RenderState {
    /// Print each visible ladder row's volume at the zone's left edge and its price at the right.
    ///
    /// Rows too thin for a line print nothing — the bars still show the grid. A row whose notional
    /// reaches the tab's highlight threshold of the biggest visible row prints its volume in the
    /// last-price line's colour.
    ///
    /// The text goes through the pane's own run pool, a row's two runs fixed by its grid index, so
    /// it never moves the shared cursor the chart's other labels are cached on.
    ///
    /// Args:
    ///     ctx: Text context of the current pass.
    ///     idx: Pane index.
    ///     sf: Physical pixels per logical pixel of the text layer.
    ///
    /// Errors:
    ///     Propagates a failed text draw.
    pub(super) fn draw_ladder_text(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        idx: usize,
        sf: f32,
    ) -> anyhow::Result<()> {
        let (grid, view, max_notional, big_pct) = {
            let pr = &self.panes[idx];
            let Some(grid) = pr.ladder_grid else {
                return Ok(());
            };
            if pr.ladder_rows.is_empty() {
                return Ok(());
            }
            (
                grid,
                pr.orderbook_view,
                pr.ladder_max_notional,
                pr.ladder_big_pct,
            )
        };
        let row_h = grid.row_height();
        let row_px = (row_h * f64::from(view.price_to_px)) as f32 / sf;
        // A tenth of slack: the step sized the row from the sync's pixel ratio, which can differ from
        // this pass's by the window's content zoom.
        if row_px < label_line_px(self.label_font_delta) * 0.9 {
            return Ok(());
        }
        let left = view.bounds[0] / sf + LADDER_TEXT_PAD_X;
        let right = (view.bounds[0] + view.bounds[2]) / sf - LADDER_TEXT_PAD_X;
        let top = view.bounds[1] / sf;
        let bottom = (view.bounds[1] + view.bounds[3]) / sf;
        let big_at =
            (big_pct > 0 && max_notional > 0.0).then(|| max_notional * f32::from(big_pct) / 100.0);
        let ink = color(self.axis_label);
        let big_ink = color(self.ladder_big_label);
        let delta = self.label_font_delta;
        let pr = &mut self.panes[idx];
        let mut result = Ok(());
        for (row, (volume, price)) in pr.ladder_rows.iter().zip(&pr.ladder_labels) {
            let y = (view.bounds[1] + view.bounds[3]
                - (row.price as f32 - view.view_price0) * view.price_to_px)
                / sf;
            // A row prints while its centre is inside the zone; the emitted margin's rows do not.
            if y < top || y > bottom {
                continue;
            }
            let row_ink = if big_at.is_some_and(|at| row.notional >= at) {
                big_ink
            } else {
                ink
            };
            let grid_index = (row.price / row_h).round() as i64;
            let mut slot = grid_index.rem_euclid(LADDER_RUN_SLOTS) as usize * 2;
            if pr.ladder_runs.len() < slot + 2 {
                pr.ladder_runs
                    .resize_with(slot + 2, GpuCanvasTextRun::default);
            }
            let drawn = draw_label_text_run(
                &mut pr.ladder_runs,
                &mut slot,
                ctx,
                delta,
                volume,
                left,
                y,
                0.0,
                0.5,
                row_ink,
            )
            .and_then(|_| {
                draw_label_text_run(
                    &mut pr.ladder_runs,
                    &mut slot,
                    ctx,
                    delta,
                    price,
                    right,
                    y,
                    1.0,
                    0.5,
                    ink,
                )
            });
            if let Err(err) = drawn {
                result = Err(err);
                break;
            }
        }
        result
    }
}
