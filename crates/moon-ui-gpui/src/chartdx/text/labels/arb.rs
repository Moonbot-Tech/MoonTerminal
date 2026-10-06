use super::*;

use super::format::colored_sign;

/// Build the arbitrage column's lines for one module.
///
/// One line per venue the roster shows, in the roster's order, addressed from [`ARB_PART_BASE`] so
/// a venue that stops reporting cannot hand its retained run to the venue below it — which would
/// reshape every line under the gap on every frame. Only the window from `first` is emitted, with
/// indicator lines for the venues scrolled out of it.
pub(super) fn push_arb_rows(
    out: &mut Vec<LabelText>,
    row_ix: usize,
    inputs: &LabelInputs,
    view: Option<&ArbViewCfg>,
    style: moon_core::config::ResolvedLabelStyle,
    first: u32,
) {
    let Some(view) = view else {
        return;
    };
    // Every line of the column shares the caption's style, so the threshold is read once.
    let min_pct = style.color_min_pct;
    // Formatted for the WHOLE column before anything is padded: a column is aligned against its
    // own widest cell, which cannot be known one line at a time. Measured over ALL rows, not just
    // the scrolled window, on purpose: the columns keep their widths while the window scrolls.
    let cells: Vec<ArbCell> = view
        .arrange(&inputs.arb)
        .into_iter()
        .map(|row| {
            // Formatted ONCE and read twice: the text carries it, and the sign it rounded to picks
            // the colour, so the two cannot disagree about a spread that rounds away.
            let spread = fmt::signed_pct(row.quote.spread_pct, 2);
            ArbCell {
                code: row.quote.venue.code(),
                dex: row.quote.dex_name.clone(),
                sign: spread
                    .as_ref()
                    .and_then(|(_, sign)| colored_sign(min_pct, row.quote.spread_pct, *sign)),
                price: match view.show.shows_price() {
                    true => fmt::adaptive(row.quote.price),
                    false => String::new(),
                },
                pct: match view.show.shows_spread() {
                    true => spread.map(|(pct, _)| pct).unwrap_or_default(),
                    false => String::new(),
                },
                // A venue that cannot be deposited to or withdrawn from is marked, not hidden: the
                // spread is real, the settlement is not, and a reader must not take one for the
                // other.
                blocked: view.mark_blocked
                    && (row.quote.deposit_blocked || row.quote.withdraw_blocked),
                label: row.label,
                color: row.color,
            }
        })
        .collect();
    // Column widths, in CHARACTERS. The chart draws its captions in a monospaced face — see
    // `design::mono` — so padding with spaces aligns them exactly, and it does so inside the two
    // runs the line already has instead of adding a run per column. That is what makes the prices
    // line up under each other the way the reference terminal's column does.
    let name_w = cells
        .iter()
        .map(|c| c.label.chars().count())
        .max()
        .unwrap_or(0);
    let price_w = cells
        .iter()
        .map(|c| c.price.chars().count())
        .max()
        .unwrap_or(0);
    let pct_w = cells
        .iter()
        .map(|c| c.pct.chars().count())
        .max()
        .unwrap_or(0);
    let window = column_scroll::visible_window(cells.len(), first, moon_core::config::ARB_MAX_ROWS);
    let mut n = 0;
    if window.above > 0 {
        out.push(plain_column_line(row_ix, n, more_above(window.above)));
        n += 1;
    }
    for cell in cells
        .into_iter()
        .skip(window.range.start)
        .take(window.range.len())
    {
        // The venue's NAME is this line's prefix: it is the word, the rest is the figure, and a
        // value-only colour then paints the price and the spread while the venue stays readable.
        let prefix = format!("{:<name_w$} ", cell.label);
        let mut text = String::new();
        if !cell.price.is_empty() {
            // Prices right-align, so their decimal points stand in one line; a name left-aligns,
            // because a word read left to right does.
            text.push_str(&format!("{:>price_w$}", cell.price));
        }
        if !cell.pct.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&format!("{:>pct_w$}", cell.pct));
        }
        if cell.blocked {
            text.push_str(" ⛔");
        }
        out.push(LabelText {
            row: row_ix,
            part: ARB_PART_BASE + n,
            text,
            prefix,
            // The venue directory's own rule, the same one the click uses to find a core and the
            // column uses to keep a chart's own exchange out of it.
            reachable: inputs.arb_reachable.iter().any(|(code, dex)| {
                moon_core::venue::arb_row_matches((*code, dex.as_str()), (cell.code, &cell.dex))
            }),
            venue: Some((cell.code, cell.dex)),
            // The SPREAD is what carries a direction here; the venue's own colour, when it has one,
            // overrides whatever the sign would have picked.
            sign: cell.sign,
            color: cell.color,
            bar: None,
            volume_menu: false,
            action: None,
        });
        n += 1;
    }
    if window.below > 0 {
        out.push(plain_column_line(row_ix, n, more_below(window.below)));
    }
}

/// The "N above" line of a scrolled column.
fn more_above(n: usize) -> String {
    t!("chart_labels.column.more_above", n = n).to_string()
}

/// The "N more" line of a column with rows below its window.
fn more_below(n: usize) -> String {
    t!("chart_labels.column.more_below", n = n).to_string()
}

/// A plain line at column slot `n` (a filter line or an indicator): no venue to click, no action.
fn plain_column_line(row_ix: usize, n: usize, text: String) -> LabelText {
    LabelText {
        row: row_ix,
        part: ARB_PART_BASE + n,
        text,
        prefix: String::new(),
        sign: None,
        reachable: false,
        venue: None,
        color: None,
        bar: None,
        volume_menu: false,
        action: None,
    }
}

/// The collapse control's title and, when folded, the number of lines the column would emit.
/// Geist Mono lacks the triangular carets, so the control uses supported ASCII markers.
fn filter_header(row: &moon_core::config::ChartLabelRow, lines: &[String]) -> String {
    let title = crate::controls::row_title(row)
        .unwrap_or_else(|| t!("chart_labels.field.strategy_filters").to_string());
    if row.collapsed {
        let count = lines.iter().filter(|line| !line.is_empty()).count();
        format!("> {title} \u{b7} {count}")
    } else {
        format!("v {title}")
    }
}

/// Build the strategy-filter column with its control immediately before the skip reasons.
///
/// The header has its own run; entries share [`moon_core::config::ARB_MAX_ROWS`] lines of the
/// column range with the indicators of a scrolled window starting at `first`. Folding omits only
/// these entries, preserving ordinary row captions.
pub(super) fn push_filter_rows(
    out: &mut Vec<LabelText>,
    row_ix: usize,
    row: &moon_core::config::ChartLabelRow,
    lines: &[String],
    first: u32,
) {
    out.push(LabelText {
        row: row_ix,
        part: FILTER_HEADER_PART,
        text: filter_header(row, lines),
        prefix: String::new(),
        sign: None,
        reachable: false,
        venue: None,
        color: None,
        bar: None,
        volume_menu: false,
        // This header IS the control; the row's ordinary name remains noninteractive.
        action: Some(LabelAction::ToggleStrategyFilters),
    });
    if row.collapsed {
        return;
    }
    let lines: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .filter(|line| !line.is_empty())
        .collect();
    let window = column_scroll::visible_window(lines.len(), first, moon_core::config::ARB_MAX_ROWS);
    let mut n = 0;
    if window.above > 0 {
        out.push(plain_column_line(row_ix, n, more_above(window.above)));
        n += 1;
    }
    for line in &lines[window.range.clone()] {
        out.push(plain_column_line(row_ix, n, (*line).to_owned()));
        n += 1;
    }
    if window.below > 0 {
        out.push(plain_column_line(row_ix, n, more_below(window.below)));
    }
}

/// One arbitrage line before it is padded into a column.
pub(in crate::chartdx) struct ArbCell {
    /// Protocol platform code, and the DEX name when the venue is a deployer.
    pub(in crate::chartdx) code: u8,
    pub(in crate::chartdx) dex: String,
    pub(in crate::chartdx) label: String,
    pub(in crate::chartdx) price: String,
    pub(in crate::chartdx) pct: String,
    pub(in crate::chartdx) blocked: bool,
    pub(in crate::chartdx) sign: Option<DeltaSign>,
    pub(in crate::chartdx) color: Option<u32>,
}
