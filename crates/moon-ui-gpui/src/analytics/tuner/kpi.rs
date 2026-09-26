//! The "Fact vs variants" KPI matrix — drawn by EVERY tuning axis.
//!
//! It knows nothing about fields, hours or coins: it renders purely out of `VarStats`, which is
//! exactly why it lives at the tuner root rather than inside whichever axis happened to need it
//! first. An axis that wants a differently-shaped matrix should widen this one, not fork it.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::summary::{fmt_signed, sign_color};
use super::super::{AnalyticsView, LoadState};
use super::shared::{card, collapse_caret};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::VarStats;
use moon_core::util::fmt::compact_si;

/// Numeric presentation contract for one KPI matrix row.
#[derive(Clone, Copy)]
enum CellFormat {
    /// Whole-number trade count without a forced sign.
    Integer,
    /// Signed profit carrying the active percent suffix when applicable.
    Profit,
}

/// Which Fact-vs-variants row is being painted. Loss rows store a positive magnitude.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MetricKind {
    Trades,
    Profit,
    Average,
    Winrate,
    ProfitFactor,
    AvgWin,
    AvgLoss,
    MaxDrawdown,
}

/// How a painted figure takes its colour. The same rule in every column: a loss is never green
/// because a variant matched the fact, and a profit is never black in one column and orange
/// in the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FigureTone {
    /// A count, a defined win rate, or a defined profit factor.
    Neutral,
    /// An em dash: the ratio or average has nothing to divide by.
    Muted,
    /// Green, orange, or muted from the signed figure, via [`sign_color`].
    Signed,
}

/// One KPI cell: the text, and the tone that colours every column the same way.
#[derive(Debug, PartialEq)]
struct Figure {
    text: KpiCellText,
    tone: FigureTone,
    /// Value [`sign_color`] reads when `tone` is [`FigureTone::Signed`].
    signed: f64,
}

/// Paint one KPI from a column's stats.
///
/// Avg loss and max drawdown are stored as positive magnitudes. They are shown as a negative
/// loss, the way the Summary tile draws drawdown, so a loss is never a green plus. Win rate,
/// profit factor and average win are undefined without the trades they divide by, and then
/// read as an em dash; a defined win rate keeps its percent sign. Profit factor is also
/// undefined when the win sum and the loss sum are both zero (only break-even trades): the
/// shared formula returns a finite 0 there. All-winners stay at 99, which is what that
/// formula reports when the loss sum is zero and the win sum is not.
///
/// Args:
///     kind: Which matrix row.
///     stats: The column's aggregate.
///
/// Returns:
///     Display text, optional exact tooltip, and the colour tone.
fn figure_of(kind: MetricKind, stats: &VarStats) -> Figure {
    let dash = || Figure {
        text: KpiCellText {
            display: "—".to_string(),
            tooltip: None,
        },
        tone: FigureTone::Muted,
        signed: 0.0,
    };
    let signed = |value: f64| Figure {
        text: format_kpi_cell(CellFormat::Profit, value),
        tone: FigureTone::Signed,
        signed: value,
    };
    match kind {
        MetricKind::Trades => Figure {
            text: format_kpi_cell(CellFormat::Integer, stats.n as f64),
            tone: FigureTone::Neutral,
            signed: 0.0,
        },
        MetricKind::Profit => signed(stats.profit),
        MetricKind::Average => {
            if stats.n <= 0 {
                dash()
            } else {
                signed(stats.avg)
            }
        }
        MetricKind::Winrate => {
            if stats.n <= 0 {
                dash()
            } else {
                Figure {
                    text: KpiCellText {
                        display: format!("{:.1}%", stats.winrate()),
                        tooltip: None,
                    },
                    tone: FigureTone::Neutral,
                    signed: 0.0,
                }
            }
        }
        MetricKind::ProfitFactor => {
            if profit_factor_undefined(stats) {
                dash()
            } else {
                Figure {
                    text: plain_factor(stats.pf),
                    tone: FigureTone::Neutral,
                    signed: 0.0,
                }
            }
        }
        MetricKind::AvgWin => {
            if stats.wins <= 0 {
                dash()
            } else {
                signed(stats.avg_win)
            }
        }
        // No losing trades: an average loss does not exist. Zero drawdown on an empty
        // column is the same absence.
        MetricKind::AvgLoss => {
            let losses = stats.n - stats.wins;
            if losses <= 0 {
                dash()
            } else {
                signed(-stats.avg_loss.abs())
            }
        }
        MetricKind::MaxDrawdown => {
            if stats.n <= 0 {
                dash()
            } else {
                signed(-stats.max_dd.abs())
            }
        }
    }
}

/// Whether the profit-factor cell has nothing to show.
///
/// Empty samples and non-finite values are undefined. So is a non-empty sample whose win
/// sum and loss sum are both zero: `profit_factor` returns the finite fallback 0, and a
/// cell of `0.00` would read as a defined ratio. All-losers stay defined — their loss
/// average is positive, so the fallback 0 is a real zero. All-winners stay defined too:
/// their win count is positive and the formula reports 99.
///
/// Args:
///     stats: The column's aggregate. `avg_loss` is the positive loss average; it is zero
///         when the loss sum is zero.
///
/// Returns:
///     Whether the cell is an em dash.
fn profit_factor_undefined(stats: &VarStats) -> bool {
    stats.n <= 0 || !stats.pf.is_finite() || (stats.wins == 0 && stats.avg_loss == 0.0)
}

/// Profit factor without a forced plus, two decimals under the SI threshold.
///
/// Args:
///     value: Finite profit factor.
///
/// Returns:
///     Display text and, above the SI threshold, the exact value for the tooltip.
fn plain_factor(value: f64) -> KpiCellText {
    let exact = format!("{value:.2}");
    if value.abs() >= 1_000.0 {
        KpiCellText {
            display: compact_si(value),
            tooltip: Some(exact),
        }
    } else {
        KpiCellText {
            display: exact,
            tooltip: None,
        }
    }
}

/// Fixed-width KPI text plus the optional unabridged value shown on hover.
#[derive(Debug, PartialEq, Eq)]
struct KpiCellText {
    /// Text painted inside the matrix cell.
    display: String,
    /// Original formatter output when `display` uses SI notation.
    tooltip: Option<String>,
}

/// Format one KPI value for a fixed-width cell without losing access to its full value.
///
/// Values below the SI threshold retain the existing formatter exactly. Larger finite values use
/// the shared K/M/B/T formatter, while the former full text becomes the hover tooltip. The active
/// percent suffix remains exclusive to profit rows.
///
/// Args:
///     format: Count, profit, or dimensionless-ratio presentation contract.
///     value: Raw KPI value; comparisons and colors continue to use this unrounded value.
///
/// Returns:
///     Contained display text and an optional exact-value tooltip.
fn format_kpi_cell(format: CellFormat, value: f64) -> KpiCellText {
    let exact = match format {
        CellFormat::Integer => format!("{}", value as i64),
        CellFormat::Profit => fmt_signed(value),
    };
    let (display, tooltip) = if value.is_finite() && value.abs() >= 1_000.0 {
        let compact = compact_si(value);
        let display = match format {
            CellFormat::Integer => compact,
            CellFormat::Profit => {
                let signed = if value > 0.0 {
                    format!("+{compact}")
                } else {
                    compact
                };
                format!("{signed}{}", crate::analytics::pnl_suffix())
            }
        };
        (display, Some(exact))
    } else {
        (exact, None)
    };
    KpiCellText { display, tooltip }
}

/// A KPI column heading: its name, plus an optional second line saying what the variant is
/// made of. Two fields rather than one long string — see [`kpi_matrix_card`].
#[derive(Clone)]
pub(super) struct VarLabel {
    pub(super) title: String,
    pub(super) sub: Option<String>,
    /// Full heading, shown on hover when the visible label is shortened to one line.
    pub(super) tip: Option<String>,
}

impl VarLabel {
    pub(super) fn new(title: String) -> Self {
        Self {
            title,
            sub: None,
            tip: None,
        }
    }

    pub(super) fn with_sub(title: String, sub: String) -> Self {
        Self {
            title,
            sub: Some(sub),
            tip: None,
        }
    }

    /// Attach the unabridged heading. The visible title stays short.
    ///
    /// Args:
    ///     tip: Hover text.
    ///
    /// Returns:
    ///     The same label with a tooltip.
    pub(super) fn with_tip(mut self, tip: String) -> Self {
        self.tip = Some(tip);
        self
    }
}

/// The UNIVERSAL "Fact vs variants" KPI matrix: drawn PURELY out of `VarStats`,
/// knowing nothing about fields/hours/coins — hence one for every tuning mode.
/// `scope` is the subtitle (usually the strategy name); `var_labels` are the
/// v1..vN column headings (column 0 is always "Fact"); shorter than the variant
/// set falls back to "v{i}".
///
/// A heading carries an optional SECOND line for what the variant is made of ("BL 2 / WL 129").
/// It is a line of its own rather than a longer title on purpose: that text grows with the
/// data, and inside a fixed-width column a one-liner wraps wherever it happens to run out —
/// which put "129)" alone on the next row and shoved the whole header down.
///
/// `collapsed` folds the matrix to its two top rows (trades + profit), keeping the column
/// headings — the caret in the title bar toggles it, for short screens where the fields grid
/// below would not otherwise fit.
///
/// Args:
///     stats: Classified Fact-versus-variant load state.
///     scope: Active strategy or selection caption.
///     var_labels: Ordered headings after the Fact column.
///     collapsed: Whether only trades and profit are visible.
///     p: Active MoonUI palette.
///     cx: Analytics view context.
///
/// Returns:
///     Complete KPI matrix card or its classified placeholder.
pub(super) fn kpi_matrix_card(
    stats: &LoadState<Vec<VarStats>>,
    scope: String,
    var_labels: &[VarLabel],
    collapsed: bool,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    let fact = VarLabel::new(t!("analytics.tuner.fact").to_string());
    kpi_matrix_card_over(stats, scope, &fact, var_labels, collapsed, p, cx)
}

/// [`kpi_matrix_card`] with column 0 headed `base` rather than "Fact" — for an axis whose
/// baseline is not the whole fact (the Entry/Exit axis compares its variants with the trades the
/// model reproduces). Every column uses the same sign colour; a variant is not tinted
/// against column 0.
pub(super) fn kpi_matrix_card_over(
    stats: &LoadState<Vec<VarStats>>,
    scope: String,
    base: &VarLabel,
    var_labels: &[VarLabel],
    collapsed: bool,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    // The collapse caret is part of the title bar in EVERY state — built up front so it does
    // not blink out while the matrix is loading or after a read error.
    let caret = collapse_caret(
        "an-tuner-kpi-collapse",
        collapsed,
        t!("analytics.tuner.kpi_collapse").to_string(),
        t!("analytics.tuner.kpi_expand").to_string(),
        p,
        cx.listener(|this, _, _, cx| this.toggle_kpi_collapsed(cx)),
    );
    // No empty state by design: `stats.len() == variants.len()` always, so
    // the only non-data cases are loading / not-ready / read failure.
    let stats = match stats.view(|_| false) {
        Ok(s) => s.clone(),
        Err(note) => {
            return card(
                t!("analytics.tuner.kpi_title").to_string(),
                scope,
                super::super::note_el("an-tuner-kpi-note", note, 8.0, p, cx),
                Some(caret),
                p,
                cx,
            );
        }
    };
    // The first two entries are the headline pair (trades + profit); collapsed mode shows
    // exactly these via `COLLAPSED_ROWS`. Keep them first if this vec is ever reordered.
    // Colour is the sign of the painted figure in every column — see [`figure_of`].
    const COLLAPSED_ROWS: usize = 2;
    let rows: Vec<(String, MetricKind)> = vec![
        (t!("analytics.kpi.trades").to_string(), MetricKind::Trades),
        (
            t!(
                "analytics.kpi.profit",
                unit = crate::analytics::pnl_unit_label()
            )
            .to_string(),
            MetricKind::Profit,
        ),
        // Order mirrors the strategy table on the left of this screen.
        (
            t!("analytics.kpi.avg_short").to_string(),
            MetricKind::Average,
        ),
        (t!("analytics.kpi.winrate").to_string(), MetricKind::Winrate),
        (t!("analytics.col.pf").to_string(), MetricKind::ProfitFactor),
        (
            t!("analytics.tuner.avg_win").to_string(),
            MetricKind::AvgWin,
        ),
        (
            t!("analytics.tuner.avg_loss").to_string(),
            MetricKind::AvgLoss,
        ),
        (
            t!("analytics.kpi.maxdd").to_string(),
            MetricKind::MaxDrawdown,
        ),
    ];
    let col_w = 92.0;
    let headings: Vec<VarLabel> = (0..stats.len())
        .map(|i| {
            if i == 0 {
                base.clone()
            } else {
                var_labels
                    .get(i - 1)
                    .cloned()
                    .unwrap_or_else(|| VarLabel::new(format!("v{i}")))
            }
        })
        .collect();
    // A second line only when some heading still carries one. A short title plus a tooltip
    // stays on one line, so a narrow pane does not wrap "2 of 2 with tape…" onto an ellipsis.
    let head_h = if headings.iter().any(|label| label.sub.is_some()) {
        34.0
    } else {
        22.0
    };
    let mut head = h_flex()
        .w_full()
        .px(design::ui_px(cx, 8.0))
        .h(design::fit_h_px(cx, head_h, 12.0, 5.0))
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .text_size(design::t_caption(cx))
        .font_family(design::ui_font())
        .text_color(moon(p.text_soft))
        .bg(moon(p.table_head))
        .child(
            div()
                .flex_1()
                .child(t!("analytics.tuner.metric").to_string()),
        );
    for (i, label) in headings.into_iter().enumerate() {
        let tip = label.tip.clone();
        let column = v_flex()
            .w(design::font_w_px(cx, col_w))
            .flex_none()
            .items_end()
            .child(div().truncate().child(label.title))
            .children(label.sub.map(|s| {
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .truncate()
                    .child(s)
            }));
        let column = match tip {
            Some(tip) => column
                .id(SharedString::from(format!("an-tuner-kpi-head-{i}")))
                .tooltip(crate::panels::common::text_tooltip(tip))
                .into_any_element(),
            None => column.into_any_element(),
        };
        head = head.child(column);
    }

    // Collapsed keeps only the headline rows; the column headings stay, so the Fact-vs-variant
    // comparison is still readable, just short enough for the grid below.
    let shown = if collapsed {
        COLLAPSED_ROWS
    } else {
        rows.len()
    };
    let mut body = v_flex().w_full().child(head);
    for (row_index, (label, kind)) in rows.into_iter().take(shown).enumerate() {
        let mut row = h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
            .items_center()
            .gap(design::ui_px(cx, 8.0))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.5))
            .child(
                div()
                    .flex_1()
                    .font_family(design::ui_font())
                    .text_color(moon(p.text_soft))
                    .child(label),
            );
        for (i, s) in stats.iter().enumerate() {
            let figure = figure_of(kind, s);
            let color = match figure.tone {
                FigureTone::Signed => sign_color(p, figure.signed),
                FigureTone::Muted => p.text_muted,
                FigureTone::Neutral => p.text,
            };
            // Screen readers receive the exact value while static cells stay out of the tab order.
            let accessibility_label = figure
                .text
                .tooltip
                .clone()
                .unwrap_or_else(|| figure.text.display.clone());
            let mut value = div()
                .id(("an-tuner-kpi-value", row_index * stats.len() + i))
                .role(Role::Label)
                .aria_label(accessibility_label)
                .w(design::font_w_px(cx, col_w))
                .min_w_0()
                .flex_none()
                .truncate()
                .text_right()
                .font_family(design::mono())
                .text_color(moon(color))
                .child(figure.text.display);
            if let Some(tooltip) = figure.text.tooltip {
                value = value.tooltip(crate::panels::common::text_tooltip(tooltip));
            }
            row = row.child(value);
        }
        body = body.child(row);
    }
    card(
        t!("analytics.tuner.kpi_title").to_string(),
        scope,
        body.into_any_element(),
        Some(caret),
        p,
        cx,
    )
}

#[cfg(test)]
mod tests;
