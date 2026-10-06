use super::model::*;
use super::*;

/// A scrollable column's wheel target grows with every line it drew, wrapped or not.
///
/// Both draw branches register through here: a column line drawn wrapped that skipped it would
/// leave its list without a band, so the wheel over it would pan the chart instead.
pub(super) fn grow_column_band(
    bands: &mut Vec<crate::chartdx::ColumnBand>,
    item: &Item,
    rect: [f32; 4],
) {
    if item.part >= ARB_PART_BASE {
        crate::chartdx::ColumnBand::grow(bands, item.row, rect);
    }
}

/// The field a caption's part index names, including a column line past [`ARB_PART_BASE`].
///
/// Column lines are not configured parts — there are more of them than a module holds — so the
/// part index does not look them up. They inherit the field of the visible column caption that
/// produced them. Names and filter headers have no field, preserving their single-line titles.
pub(super) fn caption_field(row: &ChartLabelRow, part: usize) -> Option<ChartLabelField> {
    if part == ROW_NAME_PART || part == FILTER_HEADER_PART {
        return None;
    }
    if part >= ARB_PART_BASE {
        return row.parts[..row.used_parts()]
            .iter()
            .find(|p| p.field.is_column() && p.visible)
            .map(|p| p.field);
    }
    row.parts.get(part).map(|p| p.field)
}

/// Style one caption of a module draws with, or `None` when the module holds no such caption.
///
/// The row's own name and the filter control keep the ordinary name style. Neither is a
/// configured caption; column entries inherit the style of the field that produced them.
pub(super) fn caption_style(row: &ChartLabelRow, part: usize) -> Option<ResolvedLabelStyle> {
    if part == ROW_NAME_PART || part == FILTER_HEADER_PART {
        return Some(ChartLabelRow::name_style());
    }
    // An arbitrage line is drawn in its OWN run range, past every part index, and takes the style
    // of the caption that produced it — the whole column is one configured caption, so its size and
    // its plate are set once. Only the colour differs per line, and that rides on the line itself.
    if part >= ARB_PART_BASE {
        let column = row.parts[..row.used_parts()]
            .iter()
            .find(|p| p.field.is_column() && p.visible)?;
        return Some(column.resolved_style());
    }
    Some(row.parts.get(part)?.resolved_style())
}

/// Which captions of a band land on which LINE, and in which column of it.
///
/// The grouping rule alone, with no styling and no geometry: the drawing pass needs the same answer
/// but cannot be run without a device, and this is the part that decides what the chart looks like.
/// Two questions, asked in this order:
///
/// 1. Does this module open a LINE? Only its placement decides — [`LabelFlow::Column`] starts one
///    under the previous line, [`LabelFlow::Row`] continues it. A module that runs down a column is
///    NOT excluded: it joins as a block, which is the whole point of the two axes being separate.
/// 2. Does this caption open a COLUMN inside that line? A module whose captions run across the line
///    gives each of them its own column; a module that runs down a column keeps them in one.
///
/// Args:
///     cfg: The pane's caption configuration.
///     texts: Every resolved caption of the pane, in draw order.
///     zone: Band being collected.
///     align: Edge of that band.
///
/// Returns:
///     One vector per printed line, holding one vector of `texts` indices per column.
pub(super) fn group_lines(
    cfg: &ChartLabelsCfg,
    texts: &[LabelText],
    zone: LabelZone,
    align: LabelAlign,
) -> Vec<Vec<Vec<usize>>> {
    let mut lines: Vec<Vec<Vec<usize>>> = Vec::new();
    let mut current: Option<usize> = None;
    // Whether the caption placed last was an arbitrage line.
    let mut was_column_line = false;
    for (pos, text) in texts.iter().enumerate() {
        let Some(row_cfg) = cfg.rows.get(text.row) else {
            continue;
        };
        if row_cfg.zone != zone || row_cfg.align != align {
            continue;
        }
        if caption_style(row_cfg, text.part).is_none() {
            continue;
        }
        let same_module = current == Some(text.row);
        if !same_module && (lines.is_empty() || !row_cfg.placement.is_row()) {
            lines.push(Vec::new());
        }
        let line = lines.last_mut().expect("a line was opened above");
        // An arbitrage line is part of a COLUMN by its own nature — one venue under another — and
        // the module's flow does not apply to it: that switch decides how the module's ordinary
        // captions run, and a module can hold both. So arbitrage lines join each other and nothing
        // else joins them.
        // Filters keep their header above their lines even if the module's ordinary flow is Row.
        let is_column_line = text.part >= FILTER_HEADER_PART;
        let joins = match (is_column_line, was_column_line) {
            (true, true) => same_module,
            (true, false) | (false, true) => false,
            (false, false) => same_module && !row_cfg.flow.is_row(),
        };
        match line.last_mut() {
            Some(cell) if joins => cell.push(pos),
            _ => line.push(vec![pos]),
        }
        current = Some(text.row);
        was_column_line = is_column_line;
    }
    lines
}

/// Split a caption into the lines it is actually drawn on.
///
/// A caption that is not prose answers with the one line it always had, cut to its budget. A prose
/// one is broken on WORD boundaries into at most [`LABEL_WRAP_LINES`] lines, and whatever is still
/// left over is cut into the last one — so the ellipsis lands at the end of the block rather than
/// in the middle of the first line, which is the whole point of wrapping it.
///
/// The rule itself is [`crate::design::wrap_text`], which is pure and tested there; what is here is
/// only the measurement this pass draws with.
pub(super) fn wrap_caption(
    memo: &mut FitMemo,
    ctx: &gpui::GpuCanvasTextContext<'_>,
    prefix: &str,
    text: &str,
    item: &Item,
    budget: f32,
) -> Vec<(String, f32)> {
    memo.lookup(
        super::fit_memo::FitInput {
            prefix,
            text,
            budget,
            size: item.size,
            wraps: item.wraps,
        },
        |text, size| super::super::measure_run_width(ctx, text, size),
    )
}

/// Truncate one caption to the width it is allowed, returning the text and its measured width.
///
/// THE one place the truncation rule lives. The drawing pass and the measuring pass both go
/// through it because their answers have to agree: a centred row placed from one rule and drawn by
/// another walks off its own centre. Truncating at all is what the fixed caption always did to the
/// coin and the core name — without it a long core name runs past the plot's left edge.
pub(super) fn fit_caption(
    memo: &mut FitMemo,
    ctx: &gpui::GpuCanvasTextContext<'_>,
    prefix: &str,
    text: &str,
    item: &Item,
    budget: f32,
) -> (String, f32) {
    memo.lookup(
        super::fit_memo::FitInput {
            prefix,
            text,
            budget,
            size: item.size,
            wraps: false,
        },
        |text, size| super::super::measure_run_width(ctx, text, size),
    )
    .pop()
    .expect("single-line fit always returns one line")
}
