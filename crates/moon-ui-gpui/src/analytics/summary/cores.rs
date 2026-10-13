//! The "Profit by core" card of the Summary tab: filter, concentration bar, and a ranking table
//! whose diverging bars share one zero, whose rows pick cores for the chart, and whose long
//! middle folds away.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::db::analytics::CoreSeries;
use moon_core::util::fmt;
use moon_ui::{
    MoonDisclosure, MoonDisclosureDirection, MoonPalette, MoonScrollbarVisibility, MoonSegmentItem,
    MoonSegmentedControl, MoonVirtualList, h_flex, v_flex,
};
use rust_i18n::t;

use super::charts::core_color;
use super::model::{Concentration, CoreRowItem, DivergingScale, folds, shown_cores};
use super::state::{CoreFilter, SummaryUi};
use super::{fmt_signed, sign_color};
use crate::analytics::tuner::plural_key;
use crate::design;
use crate::design::{moon, moon_alpha};

/// Rows past which a list virtualizes instead of laying out every row.
const VIRTUAL_FROM: usize = 14;
/// Rows a virtualized list shows before it scrolls.
const VIRTUAL_ROWS: usize = 16;

/// Callbacks the cores card raises; the view owns what they change.
#[derive(Clone)]
pub(in crate::analytics) struct CoresActs {
    /// A core row was clicked: pick or unpick that core for the chart.
    pub(in crate::analytics) on_pick: Rc<dyn Fn(u64, &mut Window, &mut App)>,
    /// The filter control was clicked.
    pub(in crate::analytics) on_filter: Rc<dyn Fn(CoreFilter, &mut Window, &mut App)>,
    /// The fold was toggled; `true` asks for every core.
    pub(in crate::analytics) on_fold: Rc<dyn Fn(bool, &mut Window, &mut App)>,
}

/// One core row, ready to render.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::analytics) struct CoreRow {
    /// Index into the snapshot's `core_days`, which also indexes the colour table.
    pub(in crate::analytics) ix: usize,
    /// Core uid the click picks.
    pub(in crate::analytics) uid: u64,
    /// Core name, never shortened.
    pub(in crate::analytics) name: String,
    /// Closed trades.
    pub(in crate::analytics) trades: String,
    /// Winrate with its percent sign.
    pub(in crate::analytics) winrate: String,
    /// Profit factor.
    pub(in crate::analytics) pf: String,
    /// Profit factor below 1: drawn in the warning colour.
    pub(in crate::analytics) pf_low: bool,
    /// Diverging bar as `(start, width)` fractions of the contribution track.
    pub(in crate::analytics) bar: (f32, f32),
    /// Signed profit text.
    pub(in crate::analytics) profit: String,
    /// Raw profit, for the sign colour.
    pub(in crate::analytics) total: f64,
    /// The chart draws this core's line: its square is filled.
    pub(in crate::analytics) filled: bool,
    /// The user picked this core: its row is highlighted.
    pub(in crate::analytics) picked: bool,
}

/// One rendered line of the ranking.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::analytics) enum RowModel {
    /// A core.
    Core(CoreRow),
    /// The folded middle (`unfolded == false`) or the collapse line under a full list.
    Fold {
        /// Muted text before the caret; empty on the collapse line.
        text: String,
        /// Whether the list is currently unfolded.
        unfolded: bool,
    },
}

/// Turn the ranking rows into render-ready lines.
///
/// Args:
///     cores: Per-core series of the period.
///     rows: Ranking order from [`super::model::core_rows`].
///     ui: Summary view state (line mode, picks, filter, fold).
///     scale: Shared diverging scale over every core.
///
/// Returns:
///     The lines; an unfolded unfiltered list long enough to fold ends with a collapse line.
pub(in crate::analytics) fn row_models(
    cores: &[CoreSeries],
    rows: &[CoreRowItem],
    ui: &SummaryUi,
    scale: &DivergingScale,
) -> Vec<RowModel> {
    let shown: std::collections::BTreeSet<usize> = shown_cores(cores, ui.lines, &ui.picked)
        .into_iter()
        .collect();
    let mut out: Vec<RowModel> = rows
        .iter()
        .map(|row| match *row {
            CoreRowItem::Core(ix) => {
                let c = &cores[ix];
                let pf = c.pf();
                RowModel::Core(CoreRow {
                    ix,
                    uid: c.uid,
                    name: c.name.clone(),
                    trades: super::kpi::trades_text(c.trades),
                    winrate: fmt::pct(c.winrate(), 0).map_or_else(|| "—".to_string(), |v| v.0),
                    pf: super::kpi::ratio_text(pf),
                    // Judged on the printed value: "1.00" must not be flagged.
                    pf_low: fmt::round_to(pf, 2).is_some_and(|v| v < 1.0),
                    bar: scale.bar(c.total),
                    profit: fmt_signed(c.total),
                    total: c.total,
                    filled: shown.contains(&ix),
                    picked: ui.picked.contains(&c.uid),
                })
            }
            CoreRowItem::Fold { hidden, between } => RowModel::Fold {
                text: t!(
                    plural_key("analytics.sum.cores.fold", hidden),
                    n = hidden,
                    a = fmt_signed(between.0),
                    b = fmt_signed(between.1)
                )
                .to_string(),
                unfolded: false,
            },
        })
        .collect();
    if ui.unfolded && ui.core_filter == CoreFilter::All && folds(cores.len()) {
        out.push(RowModel::Fold {
            text: String::new(),
            unfolded: true,
        });
    }
    out
}

/// Measured column widths of the table, shared by the header and every row.
#[derive(Clone, Copy)]
struct Cols {
    /// Square plus the widest name.
    name: f32,
    trades: f32,
    winrate: f32,
    pf: f32,
    /// Minimum width of the contribution track; it takes the rest of the row.
    contrib: f32,
    profit: f32,
    gap: f32,
    pad: f32,
    row_h: f32,
}

impl Cols {
    /// Measure every column against its header and every row's text, each in the font its
    /// cell sets: headers in the UI face, names and figures in mono.
    fn of(lines: &[RowModel], unit: &str, cx: &App) -> Self {
        let mono = |s: &str, w: f32| design::mono_body_text_width(cx, s, w);
        let head = |s: &str| design::ui_caption_text_width(cx, s, 400.0);
        let cores = lines.iter().filter_map(|l| match l {
            RowModel::Core(c) => Some(c),
            RowModel::Fold { .. } => None,
        });
        let mut c = Self {
            name: head(&t!("analytics.col.core")),
            trades: head(&t!("analytics.kpi.trades")),
            winrate: head(&t!("analytics.kpi.winrate")),
            pf: head(&t!("analytics.col.pf")),
            contrib: f32::max(
                head(&t!("analytics.sum.cores.col.contrib")),
                f32::from(design::font_w_px(cx, 120.0)),
            ),
            profit: head(&t!("analytics.sum.cores.col.profit_unit", unit = unit)),
            gap: f32::from(design::ui_px(cx, 10.0)),
            pad: f32::from(design::ui_px(cx, 8.0)),
            row_h: f32::from(design::fit_h_px(cx, 26.0, 14.0, 6.0)),
        };
        let square = f32::from(design::ui_px(cx, 10.0)) + f32::from(design::ui_px(cx, 6.0));
        for r in cores {
            c.name = c.name.max(square + mono(&r.name, 400.0));
            c.trades = c.trades.max(mono(&r.trades, 400.0));
            c.winrate = c.winrate.max(mono(&r.winrate, 400.0));
            c.pf = c.pf.max(mono(&r.pf, 400.0));
            c.profit = c.profit.max(mono(&r.profit, 600.0));
        }
        // A rounding pixel so measured text never sits exactly on its box edge.
        for w in [
            &mut c.name,
            &mut c.trades,
            &mut c.winrate,
            &mut c.pf,
            &mut c.profit,
        ] {
            *w = w.ceil() + 1.0;
        }
        c
    }

    /// Narrowest row before the table scrolls sideways.
    fn min_row(&self) -> f32 {
        self.name
            + self.trades
            + self.winrate
            + self.pf
            + self.contrib
            + self.profit
            + self.gap * 5.0
            + self.pad * 2.0
    }
}

/// Background of a picked row: its own chart colour at a low alpha, so the row and its line on
/// the chart read as one.
pub(in crate::analytics) fn row_tint(picked: bool, color: Hsla) -> Option<Hsla> {
    picked.then(|| color.alpha(0.14))
}

/// Right-aligned fixed cell.
fn num_cell(w: f32) -> Div {
    div().w(px(w)).flex_none().text_right()
}

/// The header row.
fn header_row(cols: Cols, unit: &str, p: MoonPalette, cx: &App) -> Div {
    h_flex()
        .debug_selector(|| "sum-cores-head".into())
        .w_full()
        .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
        .px(px(cols.pad))
        .gap(px(cols.gap))
        .items_center()
        .font_family(design::ui_font())
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_soft))
        .bg(moon(p.table_head))
        .child(
            div()
                .w(px(cols.name))
                .flex_none()
                .child(t!("analytics.col.core").to_string()),
        )
        .child(num_cell(cols.trades).child(t!("analytics.kpi.trades").to_string()))
        .child(num_cell(cols.winrate).child(t!("analytics.kpi.winrate").to_string()))
        .child(num_cell(cols.pf).child(t!("analytics.col.pf").to_string()))
        .child(
            div()
                .flex_1()
                .min_w(px(cols.contrib))
                .child(t!("analytics.sum.cores.col.contrib").to_string()),
        )
        .child(
            num_cell(cols.profit)
                .child(t!("analytics.sum.cores.col.profit_unit", unit = unit).to_string()),
        )
}

/// One core row: colour square and name, numbers, the diverging bar, and the profit.
fn core_row_el(
    k: usize,
    r: &CoreRow,
    cols: Cols,
    zero_frac: f32,
    color: Hsla,
    acts: &CoresActs,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let on_pick = acts.on_pick.clone();
    let uid = r.uid;
    let side = design::ui_px(cx, 10.0);
    let square = div()
        .debug_selector(move || format!("sum-cores-square-{k}"))
        .flex_none()
        .size(side)
        .rounded(design::ui_px(cx, 2.0))
        .border_1()
        .border_color(color);
    let square = if r.filled { square.bg(color) } else { square };
    let (start, width) = r.bar;
    let bar_color = sign_color(p, r.total);
    let track_h = design::ui_px(cx, 10.0);
    let contrib = div()
        .debug_selector(move || format!("sum-cores-contrib-{k}"))
        .relative()
        .flex_1()
        .min_w(px(cols.contrib))
        .h(track_h)
        // MoonUI has no diverging bar: the zero axis and the bar are two absolute divs on one
        // track, so every row shares the same zero.
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(zero_frac))
                .w(px(1.0))
                .bg(moon_alpha(p.border, 0.9)),
        )
        .child(
            div()
                .debug_selector(move || format!("sum-cores-bar-{k}"))
                .absolute()
                .top(design::ui_px(cx, 1.0))
                .bottom(design::ui_px(cx, 1.0))
                .left(relative(start))
                .w(relative(width))
                .rounded(design::ui_px(cx, 2.0))
                .bg(moon_alpha(bar_color, 0.75)),
        );
    let mut row = h_flex()
        .id(("an-sum-core", uid))
        .debug_selector(move || format!("sum-cores-row-{k}"))
        .w_full()
        .h(px(cols.row_h))
        .px(px(cols.pad))
        .gap(px(cols.gap))
        .items_center()
        .text_size(design::t_body(cx))
        .border_t_1()
        .border_color(moon_alpha(p.border, 0.6))
        .cursor_pointer()
        .hover(|s| s.bg(moon(p.table_hover)))
        .on_click(move |_, window, app| on_pick(uid, window, app));
    if let Some(bg) = row_tint(r.picked, color) {
        row = row.bg(bg);
    }
    row.child(
        h_flex()
            .debug_selector(move || format!("sum-cores-name-{k}"))
            // At least the measured column, and never narrower than its own name.
            .min_w(px(cols.name))
            .flex_none()
            .font_family(design::mono())
            .gap(design::ui_px(cx, 6.0))
            .items_center()
            .child(square)
            .child(
                div()
                    .debug_selector(move || format!("sum-cores-name-text-{k}"))
                    .flex_none()
                    .child(r.name.clone()),
            ),
    )
    .child(
        num_cell(cols.trades)
            .debug_selector(move || format!("sum-cores-trades-{k}"))
            .font_family(design::mono())
            .text_color(moon(p.text_soft))
            .child(r.trades.clone()),
    )
    .child(
        num_cell(cols.winrate)
            .debug_selector(move || format!("sum-cores-wr-{k}"))
            .font_family(design::mono())
            .text_color(moon(p.text_soft))
            .child(r.winrate.clone()),
    )
    .child(
        num_cell(cols.pf)
            .debug_selector(move || format!("sum-cores-pf-{k}"))
            .font_family(design::mono())
            .text_color(moon(if r.pf_low { p.orange } else { p.text_soft }))
            .child(r.pf.clone()),
    )
    .child(contrib)
    .child(
        num_cell(cols.profit)
            .debug_selector(move || format!("sum-cores-profit-{k}"))
            .font_family(design::mono())
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(moon(sign_color(p, r.total)))
            .child(r.profit.clone()),
    )
    .into_any_element()
}

/// The fold line: muted count text, the caret, and its label.
fn fold_row_el(
    text: &str,
    unfolded: bool,
    cols: Cols,
    acts: &CoresActs,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let on_fold = acts.on_fold.clone();
    let label = if unfolded {
        t!("analytics.sum.cores.fold_hide")
    } else {
        t!("analytics.sum.cores.fold_show")
    };
    h_flex()
        .debug_selector(|| "sum-cores-fold".into())
        .w_full()
        .min_h(px(cols.row_h))
        .px(px(cols.pad))
        .gap(design::ui_px(cx, 8.0))
        .items_center()
        .border_t_1()
        .border_color(moon_alpha(p.border, 0.6))
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_muted))
        .when(!text.is_empty(), |row| {
            row.child(
                div()
                    .debug_selector(|| "sum-cores-fold-text".into())
                    .flex_none()
                    .child(text.to_string()),
            )
        })
        .child(
            h_flex()
                .debug_selector(|| "sum-cores-fold-btn".into())
                .flex_none()
                .gap(design::ui_px(cx, 4.0))
                .items_center()
                .child(
                    MoonDisclosure::button("an-sum-fold", unfolded)
                        .direction(MoonDisclosureDirection::DownUp)
                        .size(design::DISCLOSURE_GLYPH)
                        .box_size(design::DISCLOSURE_BOX)
                        .hover_color(p.text)
                        .on_toggle(move |_, window, app| on_fold(!unfolded, window, app)),
                )
                .child(label.to_string()),
        )
        .into_any_element()
}

/// The 8px stacked bar of positive totals plus its four-span caption.
fn concentration_el(
    cores: &[CoreSeries],
    conc: &Concentration,
    colors: &[Hsla],
    p: MoonPalette,
    cx: &App,
) -> Stateful<Div> {
    let mut gains: Vec<(usize, f64)> = cores
        .iter()
        .enumerate()
        .filter(|(_, c)| c.total > 0.0)
        .map(|(i, c)| (i, c.total))
        .collect();
    gains.sort_by(|a, b| b.1.total_cmp(&a.1));
    let sum: f64 = gains.iter().map(|g| g.1).sum();
    let rest = gains.len().saturating_sub(5).max(1) as f32;
    let mut bar = h_flex()
        .debug_selector(|| "sum-cores-conc-bar".into())
        .w_full()
        .h(design::ui_px(cx, 8.0))
        .rounded(design::ui_px(cx, 4.0))
        .overflow_hidden()
        .bg(moon_alpha(p.border, 0.4));
    for (k, (ix, total)) in gains.iter().enumerate() {
        let share = (total / sum) as f32;
        // The five leaders carry their chart colours; the long tail fades out in green.
        let color = if k < 5 {
            core_color(colors, *ix, p)
        } else {
            moon_alpha(p.green, 0.7 - 0.5 * (k - 5) as f32 / rest)
        };
        let tip = format!(
            "{} · {}",
            cores[*ix].name,
            fmt::pct(f64::from(share) * 100.0, 1).map_or_else(String::new, |v| v.0)
        );
        bar = bar.child(
            div()
                .id(("an-sum-conc", k))
                .h_full()
                .w(relative(share))
                .flex_none()
                .bg(color)
                .tooltip(crate::panels::common::text_tooltip(tip)),
        );
    }
    let pct = |v: f64| fmt::pct(v, 0).map_or_else(|| "—".to_string(), |v| v.0);
    let spans = [
        t!(
            "analytics.sum.cores.conc.leader",
            pct = pct(conc.leader_pct)
        )
        .to_string(),
        t!("analytics.sum.cores.conc.top5", pct = pct(conc.top5_pct)).to_string(),
        t!(
            plural_key("analytics.sum.cores.conc.half", conc.n),
            k = conc.half_k,
            n = conc.n
        )
        .to_string(),
        t!("analytics.sum.cores.conc.eaten", pct = pct(conc.eaten_pct)).to_string(),
    ];
    let caption = h_flex()
        .w_full()
        .flex_wrap()
        .gap_x(design::ui_px(cx, 12.0))
        .gap_y(design::ui_px(cx, 2.0))
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_muted))
        .children(spans.into_iter().enumerate().map(|(i, s)| {
            div()
                .debug_selector(move || format!("sum-cores-conc-cap-{i}"))
                .flex_none()
                .child(s)
        }));
    v_flex()
        .id("an-sum-conc-block")
        .w_full()
        .gap(design::ui_px(cx, 4.0))
        .tooltip(crate::panels::common::text_tooltip(
            t!("analytics.sum.cores.conc.tip").to_string(),
        ))
        .child(bar)
        .child(caption)
}

/// Caption beside the card title: net profit of every core, the sum of the gaining cores, and
/// the signed sum of the losing ones, in the profit format the table uses.
///
/// Args:
///     cores: Per-core series of the period.
///
/// Returns:
///     The localized caption.
pub(in crate::analytics) fn sub_text(cores: &[CoreSeries]) -> String {
    let net: f64 = cores.iter().map(|c| c.total).sum();
    let gains: f64 = cores.iter().map(|c| c.total).filter(|v| *v > 0.0).sum();
    let losses: f64 = cores.iter().map(|c| c.total).filter(|v| *v < 0.0).sum();
    t!(
        "analytics.sum.cores.sub",
        total = fmt_signed(net),
        plus = fmt_signed(gains),
        minus = fmt_signed(losses)
    )
    .to_string()
}

/// Render the "Profit by core" card.
///
/// Args:
///     cores: Per-core series of the period.
///     rows: Ranking order for the active filter and fold.
///     scale: Shared diverging scale over every core.
///     conc: Concentration figures; `None` when no core gained.
///     ui: Summary view state.
///     colors: One chart colour per core, by snapshot index.
///     acts: Callbacks into the view.
///     p: Active palette.
///     cx: App context for scaled sizes and text measurement.
///
/// Returns:
///     The card; it schedules nothing.
#[allow(clippy::too_many_arguments)]
pub(in crate::analytics) fn cores_el(
    cores: &[CoreSeries],
    rows: &[CoreRowItem],
    scale: &DivergingScale,
    conc: Option<&Concentration>,
    ui: &SummaryUi,
    colors: &[Hsla],
    acts: &CoresActs,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let total = cores.len();
    let plus = cores.iter().filter(|c| c.total > 0.0).count();
    let minus = cores.iter().filter(|c| c.total < 0.0).count();
    let on_filter = acts.on_filter.clone();
    let filters = [
        (CoreFilter::All, "analytics.sum.cores.f_all", total),
        (CoreFilter::Pos, "analytics.sum.cores.f_pos", plus),
        (CoreFilter::Neg, "analytics.sum.cores.f_neg", minus),
    ];
    let seg = MoonSegmentedControl::new("an-sum-core-filter")
        .items(filters.map(|(f, key, n)| {
            MoonSegmentItem::new("", t!(key, n = n).to_string())
                .fit_width(cx, 40.0, 160.0)
                .selected(ui.core_filter == f)
        }))
        .on_click(move |ix, _, window, app| {
            let f = match ix {
                0 => CoreFilter::All,
                1 => CoreFilter::Pos,
                _ => CoreFilter::Neg,
            };
            on_filter(f, window, app);
        });
    let header = h_flex()
        .w_full()
        .flex_wrap()
        .items_center()
        .gap_x(design::ui_px(cx, 10.0))
        .gap_y(design::ui_px(cx, 4.0))
        .child(
            div()
                .debug_selector(|| "sum-cores-title".into())
                .flex_none()
                .font_family(design::ui_font())
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .child(t!("analytics.cores_title").to_string()),
        )
        .child(
            div()
                .debug_selector(|| "sum-cores-sub".into())
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(sub_text(cores)),
        )
        .child(
            div()
                .debug_selector(|| "sum-cores-filter".into())
                .ml_auto()
                .flex_none()
                .child(seg),
        );

    let unit = crate::analytics::pnl_unit_label().to_string();
    let lines = row_models(cores, rows, ui, scale);
    let cols = Cols::of(&lines, &unit, cx);
    let zero = scale.zero_frac;
    let color_of = |ix: usize| core_color(colors, ix, p);
    let core_count = lines
        .iter()
        .filter(|l| matches!(l, RowModel::Core(_)))
        .count();
    let body: AnyElement = if core_count > VIRTUAL_FROM {
        let shown = lines.len().min(VIRTUAL_ROWS);
        let list_colors: Vec<Hsla> = lines
            .iter()
            .map(|l| match l {
                RowModel::Core(r) => color_of(r.ix),
                RowModel::Fold { .. } => moon(p.text_muted),
            })
            .collect();
        let lines = Rc::new(lines);
        let acts = acts.clone();
        let scrollbar_gutter = design::ui_px(cx, 8.0);
        div()
            .debug_selector(|| "sum-cores-list".into())
            .w_full()
            .h(px(shown as f32 * cols.row_h))
            .child(
                MoonVirtualList::new(
                    "an-sum-cores-list",
                    lines.len(),
                    cols.row_h,
                    move |k, _, cx| {
                        let el = match &lines[k] {
                            RowModel::Core(r) => {
                                core_row_el(k, r, cols, zero, list_colors[k], &acts, p, cx)
                            }
                            RowModel::Fold { text, unfolded } => {
                                fold_row_el(text, *unfolded, cols, &acts, p, cx)
                            }
                        };
                        div().size_full().pr(scrollbar_gutter).child(el)
                    },
                )
                .surface(false)
                .border(false)
                .radius(0.0)
                .scrollbar_visibility(MoonScrollbarVisibility::Always),
            )
            .into_any_element()
    } else {
        v_flex()
            .w_full()
            .children(lines.iter().enumerate().map(|(k, l)| match l {
                RowModel::Core(r) => core_row_el(k, r, cols, zero, color_of(r.ix), acts, p, cx),
                RowModel::Fold { text, unfolded } => {
                    fold_row_el(text, *unfolded, cols, acts, p, cx)
                }
            }))
            .into_any_element()
    };
    let table = v_flex()
        .debug_selector(|| "sum-cores-table".into())
        .w_full()
        .min_w(px(cols.min_row()))
        .child(header_row(cols, &unit, p, cx))
        .child(body);
    v_flex()
        .debug_selector(|| "sum-cores-card".into())
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 8.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 10.0))
        .rounded(design::ui_px(cx, 8.0))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .child(header)
        .children(conc.map(|c| concentration_el(cores, c, colors, p, cx)))
        .child(
            div()
                .id("an-sum-cores-scroll")
                .debug_selector(|| "sum-cores-scroll".into())
                .w_full()
                .overflow_x_scroll()
                .child(table),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests;
