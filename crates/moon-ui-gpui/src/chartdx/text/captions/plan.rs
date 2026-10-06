use super::model::*;
use super::*;

use super::fit::{caption_field, caption_style, group_lines, wrap_caption};

/// Pristine row plans, keyed only by inputs that affect grouping and item styling.
pub(in crate::chartdx) struct RowPlanCache {
    /// Rebuild revision of the pane's formatted texts.
    generation: u64,
    /// Retained identity prevents a freed configuration address from being reused as a hit.
    cfg: std::rc::Rc<ChartLabelsCfg>,
    /// Effective caption size in logical pixels.
    font_bits: u32,
    /// Drawing only mutates clones of these rows.
    rows: [[Vec<Row>; LabelAlign::ALL.len()]; LabelZone::ALL.len()],
}

impl RowPlanCache {
    /// Rebuild on a key miss, then clone the pristine rows for this frame's mutations.
    pub(super) fn working_rows(
        cache: &mut Option<Self>,
        generation: u64,
        cfg: &std::rc::Rc<ChartLabelsCfg>,
        font: f32,
        mut build: impl FnMut(LabelZone, LabelAlign) -> Vec<Row>,
    ) -> [[Vec<Row>; LabelAlign::ALL.len()]; LabelZone::ALL.len()] {
        let hit = cache.as_ref().is_some_and(|held| {
            held.generation == generation
                && std::rc::Rc::ptr_eq(&held.cfg, cfg)
                && held.font_bits == font.to_bits()
        });
        if !hit {
            *cache = Some(Self {
                generation,
                cfg: cfg.clone(),
                font_bits: font.to_bits(),
                rows: LabelZone::ALL.map(|zone| LabelAlign::ALL.map(|align| build(zone, align))),
            });
        }
        cache
            .as_ref()
            .expect("the miss installs a row plan")
            .rows
            .clone()
    }
}

impl RenderState {
    /// Gather this band's captions into the LINES they are printed on, styled and sized.
    ///
    /// The grouping itself is [`group_lines`] — the rule that decides what the chart looks like, and
    /// the only part of this pass that can be checked without a device. What is left here needs the
    /// pane: the font size the styles are measured against.
    pub(super) fn collect_rows(
        &self,
        cfg: &ChartLabelsCfg,
        texts: &[LabelText],
        zone: LabelZone,
        align: LabelAlign,
    ) -> Vec<Row> {
        let base = self.label_font_px();
        let item = |pos: usize| -> Option<Item> {
            let text = texts.get(pos)?;
            let row_cfg = cfg.rows.get(text.row)?;
            let style = caption_style(row_cfg, text.part)?;
            Some(Item {
                pos,
                row: text.row,
                part: text.part,
                style,
                plate: row_cfg.plate,
                size: (base * style.size_mult).clamp(6.0, CAPTION_SIZE_MAX),
                wraps: caption_field(row_cfg, text.part).is_some_and(ChartLabelField::wraps),
                lines: 1,
                wrap_ix: usize::MAX,
            })
        };
        // A module's gap in the chart's own logical pixels, as the configuration states it.
        let gap_of = |positions: &[usize]| -> f32 {
            positions
                .first()
                .and_then(|pos| texts.get(*pos))
                .and_then(|text| cfg.rows.get(text.row))
                .map_or(0.0, |row| f32::from(row.gap))
        };
        group_lines(cfg, texts, zone, align)
            .into_iter()
            .map(|line| Row {
                // The line is spaced by the module that OPENED it; the gaps of modules that join it
                // space their own columns instead.
                gap: line.first().map_or(0.0, |cell| gap_of(cell)),
                cells: line
                    .into_iter()
                    .map(|cell| Cell {
                        gap: gap_of(&cell),
                        items: cell.into_iter().filter_map(item).collect(),
                    })
                    .filter(|cell: &Cell| !cell.items.is_empty())
                    .collect(),
            })
            .collect()
    }

    /// How wide this band's prose would be on ONE line, or `0` when it holds none.
    ///
    /// The only thing the width split has to ASK for rather than read back from a drawn band: how
    /// wide a wrapped caption ends up is decided by the budget it is given, so it cannot also be
    /// what decides that budget. One shaping pass for one line — the figure bands are never
    /// measured here, they are drawn first and report what they took.
    pub(super) fn prose_width(
        &self,
        ctx: &gpui::GpuCanvasTextContext<'_>,
        texts: &[LabelText],
        rows: &[Row],
    ) -> f32 {
        rows.iter()
            .flat_map(|row| row.cells.iter())
            .flat_map(|cell| cell.items.iter())
            .filter(|item| item.wraps)
            .filter_map(|item| {
                let entry = texts.get(item.pos)?;
                // The prefix is empty on almost every caption, and on every prose one so far:
                // measuring the text as it stands saves the glue allocation on the frame path.
                Some(match entry.prefix.is_empty() {
                    true => super::super::measure_run_width(ctx, &entry.text, item.size),
                    false => super::super::measure_run_width(ctx, &entry.glued(), item.size),
                })
            })
            .fold(0.0_f32, f32::max)
    }

    /// Work out how many lines each wrapping caption takes at the width it will be drawn at.
    ///
    /// Before anything is stacked, because a band places its lines from their HEIGHTS — and a
    /// bottom-anchored one subtracts a line's height before drawing it. A wrapped caption that
    /// still reported one line would have its tail drawn over whatever the band placed next.
    ///
    /// The budget walk follows `draw_row`'s, on the MEASURED width of each column. The drawn width
    /// can exceed it — a caption split into a prefix run and a value run shapes as two strings — so
    /// a later column can be drawn at a slightly narrower budget than it was planned at. The line
    /// COUNT cannot drift with it: the lines planned here are the lines drawn, read back from
    /// `caption_wraps` rather than wrapped again.
    pub(super) fn plan_wraps(
        &mut self,
        ctx: &gpui::GpuCanvasTextContext<'_>,
        texts: &[LabelText],
        rows: &mut [Row],
        column: Column,
    ) {
        // Nothing on this pane wraps: the whole walk — a measurement per cell of every band —
        // is skipped, which is the case on every chart that prints no detect line and no skip list.
        if !rows.iter().any(|row| row.cells.iter().any(Cell::has_wrap)) {
            return;
        }
        // Modules that already have a wrapped PROSE caption: the continuation runs are per module,
        // so the second detect line in one module is cut instead. Nothing ships two. Column lines
        // each wrap on their own — they already have a run apiece in the ARB range, and cutting
        // the second skip reason would hide why that strategy did not fire.
        let mut wrapped_modules: Vec<usize> = Vec::new();
        for row in rows.iter_mut() {
            // The budget walk exists to reach the wrapping captions; the cells after the last one
            // on this line cost a measurement each and change nothing.
            let Some(last_wrap) = row.cells.iter().rposition(Cell::has_wrap) else {
                continue;
            };
            let mut budget = column.max_w;
            for (n, cell) in row.cells.iter_mut().enumerate().take(last_wrap + 1) {
                let gap = if n == 0 { 0.0 } else { CAPTION_GAP + cell.gap };
                budget -= gap;
                if budget < MIN_LEGIBLE_W {
                    break;
                }
                for item in cell.items.iter_mut() {
                    self.wrap_item(ctx, texts, item, budget, &mut wrapped_modules);
                }
                // AFTER the wrap, never before: `measure_cell` reads a prose caption's width off
                // the wrap, and asking it first would send the whole sentence through the
                // truncation walk — which measures one character at a time — every frame.
                budget -= self.measure_cell(ctx, texts, cell, budget);
            }
        }
    }

    /// Wrap one caption at `budget`, or skip it: a second prose caption in the same module is cut,
    /// and a caption that does not wrap is left as one line.
    pub(super) fn wrap_item(
        &mut self,
        ctx: &gpui::GpuCanvasTextContext<'_>,
        texts: &[LabelText],
        item: &mut Item,
        budget: f32,
        wrapped_modules: &mut Vec<usize>,
    ) {
        if !item.wraps {
            return;
        }
        let column_line = item.part >= ARB_PART_BASE;
        if !column_line {
            if wrapped_modules.contains(&item.row) {
                return;
            }
            wrapped_modules.push(item.row);
        }
        let Some(entry) = texts.get(item.pos) else {
            return;
        };
        let lines = wrap_caption(
            &mut self.caption_fit_memo,
            ctx,
            &entry.prefix,
            &entry.text,
            item,
            budget,
        );
        item.lines = lines.len().clamp(1, LABEL_WRAP_LINES) as u8;
        item.wrap_ix = self.caption_wraps.len();
        self.caption_wraps.push(lines);
    }

    /// Wrap each line of a hungry column at the width still free at that depth of the stack.
    ///
    /// A top band fills downward, so the first item sits at `travelled`; a bottom band fills
    /// upward, so the last item does — wrapping from the floor up, or a skip reason next to the
    /// core name would spend the whole plot and print through it.
    pub(super) fn wrap_hungry_row(
        &mut self,
        ctx: &gpui::GpuCanvasTextContext<'_>,
        texts: &[LabelText],
        row: &mut Row,
        budget: HungryBudget,
        travelled: f32,
        downward: bool,
    ) {
        let mut wrapped_modules = Vec::new();
        for cell in &mut row.cells {
            let mut depth = travelled;
            let n = cell.items.len();
            for i in 0..n {
                let ix = if downward { i } else { n - 1 - i };
                let max_w = widths::line_budget(
                    budget.total,
                    budget.align,
                    budget.taken,
                    depth,
                    budget.inset,
                );
                self.wrap_item(ctx, texts, &mut cell.items[ix], max_w, &mut wrapped_modules);
                depth += cell.items[ix].block_h();
            }
        }
    }
}
