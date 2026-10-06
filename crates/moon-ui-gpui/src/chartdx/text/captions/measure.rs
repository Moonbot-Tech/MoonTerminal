use super::model::*;
use super::*;

use super::fit::fit_caption;

impl RenderState {
    /// Width of a line once every column is truncated to the budget it would actually get.
    pub(super) fn measure_row(
        &mut self,
        ctx: &gpui::GpuCanvasTextContext<'_>,
        texts: &[LabelText],
        cells: &[Cell],
        max_w: f32,
    ) -> f32 {
        let mut total = 0.0;
        let mut budget = max_w;
        for (n, cell) in cells.iter().enumerate() {
            let gap = if n == 0 { 0.0 } else { CAPTION_GAP + cell.gap };
            budget -= gap;
            if budget < MIN_LEGIBLE_W {
                break;
            }
            let w = self.measure_cell(ctx, texts, cell, budget);
            total += gap + w;
            budget -= w;
        }
        total
    }

    /// Width of one column: its widest caption, each truncated to the same budget.
    pub(super) fn measure_cell(
        &mut self,
        ctx: &gpui::GpuCanvasTextContext<'_>,
        texts: &[LabelText],
        cell: &Cell,
        budget: f32,
    ) -> f32 {
        let reserve = bar_zone(texts, cell);
        let text_w = cell
            .items
            .iter()
            .filter_map(|item| {
                let entry = texts.get(item.pos)?;
                // Same rule as the drawing pass: a split caption is a prefix plus a value, and its
                // width is the sum. Measuring only the glued form would misplace every centred line
                // holding one.
                // The whole caption, prefix and value as one string. Deliberately NOT measured
                // as two: the column's width is the same either way in a monospaced face, and a
                // second measurement here would shape the prefix again on every frame.
                // A wrapped caption is as wide as its WIDEST line, not as wide as its first: the
                // column it sits in is what centres the captions above and below it. Read from the
                // plan rather than wrapped again — measuring is what the plan exists to do once.
                // Measured WITHOUT the bar: the track is a column-wide reserve, added once below.
                let bar_reserve = reserve;
                // The button's own backing, charged per CAPTION rather than per column like the
                // bar track beside it: two buttons in one module each draw a plate of their own.
                let act_w = match entry.action.and_then(LabelAction::market) {
                    Some(_) => ACTION_PLATE_W,
                    None => 0.0,
                };
                match self.wrapped(item) {
                    Some(lines) => Some(lines.iter().map(|(_, w)| *w).fold(0.0_f32, f32::max)),
                    None => Some(
                        fit_caption(
                            &mut self.caption_fit_memo,
                            ctx,
                            &entry.prefix,
                            &entry.text,
                            item,
                            budget - bar_reserve - act_w,
                        )
                        .1 + act_w,
                    ),
                }
            })
            .fold(0.0_f32, f32::max);
        text_w + reserve
    }

    /// The lines `plan_wraps` broke this caption into, or `None` when it is not prose.
    pub(super) fn wrapped(&self, item: &Item) -> Option<&Vec<(String, f32)>> {
        self.caption_wraps.get(item.wrap_ix)
    }

    /// Resolve one caption's color.
    pub(super) fn caption_color(
        &self,
        mode: LabelColor,
        sign: Option<DeltaSign>,
        caption_fg: Hsla,
    ) -> Hsla {
        match mode {
            LabelColor::Theme => caption_fg,
            LabelColor::Fixed(rgb) => gpui::rgb(rgb).into(),
            LabelColor::BySign => match sign {
                Some(DeltaSign::Positive) => gpui::rgb(self.label_positive).into(),
                Some(DeltaSign::Negative) => gpui::rgb(self.label_negative).into(),
                // A figure that rounds to zero, or has no sign at all, keeps the caption color
                // rather than being coloured as a gain.
                _ => caption_fg,
            },
        }
    }

    /// Measure a caption's text through the retained run it will be DRAWN with.
    ///
    /// The run keeps its shaping, so measuring and then drawing the same string costs one shaping
    /// rather than two — which is the whole reason these runs are addressed by index instead of
    /// taken from a cursor.
    pub(super) fn measure_caption_run(
        &mut self,
        ctx: &gpui::GpuCanvasTextContext<'_>,
        pane_ix: usize,
        row_ix: usize,
        part_ix: usize,
        text: &str,
        size: f32,
    ) -> f32 {
        let run_ix = (pane_ix * CHART_LABEL_ROWS + row_ix) * ROW_RUN_STRIDE + part_ix;
        if self.caption_runs.len() <= run_ix {
            self.caption_runs
                .resize_with(run_ix + 1, gpui::GpuCanvasTextRun::default);
        }
        let metrics = self.caption_runs[run_ix].measure(
            ctx,
            text,
            gpui::font(crate::design::mono()),
            content_px(ctx, size),
            content_px(ctx, size + 4.0),
        );
        chart_metrics(ctx, metrics).width.as_f32()
    }

    /// Draw one caption through its OWN retained run.
    ///
    /// The run is addressed by `(pane * CHART_LABEL_ROWS + row) * ROW_RUN_STRIDE + part`, never by
    /// a running cursor: a caption that stops resolving must not hand its run to the next one,
    /// which would reshape both strings on every frame for as long as the pane stays that way. The
    /// stride reserves one index past the captions for the row's printed name, so switching that on
    /// renumbers nothing either.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_caption_run(
        &mut self,
        ctx: &mut gpui::GpuCanvasTextContext<'_>,
        pane_ix: usize,
        row_ix: usize,
        part_ix: usize,
        text: &str,
        size: f32,
        x: f32,
        y: f32,
        ax: f32,
        color: Hsla,
    ) -> anyhow::Result<gpui::GpuCanvasTextMetrics> {
        let run_ix = (pane_ix * CHART_LABEL_ROWS + row_ix) * ROW_RUN_STRIDE + part_ix;
        if self.caption_runs.len() <= run_ix {
            self.caption_runs
                .resize_with(run_ix + 1, gpui::GpuCanvasTextRun::default);
        }
        let metrics = self.caption_runs[run_ix].draw_aligned(
            ctx,
            point(content_px(ctx, x), content_px(ctx, y)),
            text,
            gpui::font(crate::design::mono()),
            content_px(ctx, size),
            content_px(ctx, size + 4.0),
            color,
            ax,
            0.0,
        )?;
        Ok(chart_metrics(ctx, metrics))
    }
}
