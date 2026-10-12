//! Faces of the Summary tab other than its data, and the page frame every face sits in: the
//! loading skeleton, the empty period, the read failure, the catch-up banner, and the data page
//! whose main row stacks on narrow windows.

use std::rc::Rc;

use gpui::*;
use moon_core::db::{FailCode, FailKind};
use moon_ui::{
    MoonAlert, MoonButton, MoonButtonVariant, MoonPalette, MoonSkeleton, h_flex, v_flex,
};
use rust_i18n::t;

use super::kpi::KpiCols;
use crate::design;
use crate::design::{moon, moon_alpha};
use crate::load_state::Note;

/// Which face the Summary renders for its current load outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics) enum Face {
    /// Figures are ready.
    Data,
    /// The first read has not completed: the static skeleton.
    Skeleton,
    /// The period has no closed trades.
    Empty,
    /// The reports database could not be read.
    Error,
    /// Any other placeholder (replica not ready, access denied, mixed quotes, bad period): the
    /// shared note.
    Note,
}

/// Pick the face for a derived Summary outcome.
///
/// Args:
///     derived: The readiness fold of the Summary.
///
/// Returns:
///     Data for figures; the skeleton while loading; the empty face for an empty period; the error
///     face for a read failure other than an access denial, whose shared recovery alert stays.
pub(in crate::analytics) fn face_of<T>(derived: &Result<T, Note>) -> Face {
    match derived {
        Ok(_) => Face::Data,
        Err(Note::Loading) => Face::Skeleton,
        Err(Note::Empty) => Face::Empty,
        Err(Note::Failed { kind, .. }) if *kind != FailKind::ReplicaAccessDenied => Face::Error,
        Err(_) => Face::Note,
    }
}

/// Callbacks of the empty and error faces; the view owns what they change.
#[derive(Clone)]
pub(in crate::analytics) struct StateActs {
    /// "Show 365 days".
    pub(in crate::analytics) on_show_year: Rc<dyn Fn(&mut Window, &mut App)>,
    /// "Reset filters".
    pub(in crate::analytics) on_reset: Rc<dyn Fn(&mut Window, &mut App)>,
    /// "Retry".
    pub(in crate::analytics) on_retry: Rc<dyn Fn(&mut Window, &mut App)>,
}

/// One static skeleton block filling its slot.
fn block(id: &'static str, k: usize, h: f32) -> Div {
    div().flex_1().min_w_0().child(
        MoonSkeleton::new(format!("{id}-{k}"))
            .height(h)
            .animated(false),
    )
}

/// The loading face: KPI, main row and ranking blocks laid out like the data page, plus a
/// muted caption. Static — the render root never schedules a repaint for it.
///
/// Args:
///     cols: Column layout of the page.
///     p: Active palette.
///     cx: App context for scaled sizes.
///
/// Returns:
///     The skeleton face.
pub(in crate::analytics) fn skeleton_el(cols: KpiCols, p: MoonPalette, cx: &App) -> AnyElement {
    let gap = design::ui_px(cx, 12.0);
    let kpis = (0..4).map(|k| block("an-sum-sk-kpi", k, 96.0));
    let kpi = match cols {
        KpiCols::Four => h_flex().w_full().gap(gap).children(kpis).into_any_element(),
        KpiCols::Two => {
            let mut it = kpis;
            v_flex()
                .w_full()
                .gap(gap)
                .child(h_flex().w_full().gap(gap).children(it.by_ref().take(2)))
                .child(h_flex().w_full().gap(gap).children(it))
                .into_any_element()
        }
        KpiCols::One => v_flex().w_full().gap(gap).children(kpis).into_any_element(),
    };
    let main = if cols == KpiCols::Four {
        h_flex()
            .w_full()
            .gap(gap)
            .child(block("an-sum-sk-main", 0, 330.0).flex_grow(2.0))
            .child(block("an-sum-sk-main", 1, 330.0))
            .into_any_element()
    } else {
        v_flex()
            .w_full()
            .gap(gap)
            .child(block("an-sum-sk-main", 0, 330.0))
            .child(block("an-sum-sk-main", 1, 330.0))
            .into_any_element()
    };
    v_flex()
        .debug_selector(|| "sum-skeleton".into())
        .w_full()
        .gap(gap)
        .p(gap)
        .rounded(design::ui_px(cx, 6.0))
        .border_1()
        .border_color(moon(p.border))
        .child(kpi)
        .child(main)
        .child(block("an-sum-sk-cores", 0, 220.0))
        .child(
            div()
                .debug_selector(|| "sum-skeleton-caption".into())
                .self_center()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(t!("analytics.sum.loading").to_string()),
        )
        .into_any_element()
}

/// A labelled action button wrapped in a measurable slot.
fn action(
    id: &'static str,
    k: usize,
    label: String,
    primary: bool,
    on: Rc<dyn Fn(&mut Window, &mut App)>,
) -> Div {
    let button = MoonButton::new(id)
        .variant(if primary {
            MoonButtonVariant::Blue
        } else {
            MoonButtonVariant::Neutral
        })
        .label(label)
        .on_click(move |_, window, app| on(window, app));
    div()
        .debug_selector(move || format!("sum-state-btn-{k}"))
        .flex_none()
        .child(button.render())
}

/// The centred frame shared by the empty and error faces.
fn state_frame(selector: &'static str, p: MoonPalette, cx: &App) -> Div {
    v_flex()
        .debug_selector(move || selector.into())
        .w_full()
        .min_h(design::ui_px(cx, 340.0))
        .items_center()
        .justify_center()
        .gap(design::ui_px(cx, 10.0))
        .px(design::ui_px(cx, 16.0))
        .py(design::ui_px(cx, 64.0))
        .rounded(design::ui_px(cx, 6.0))
        .border_1()
        .border_dashed()
        .border_color(moon(p.border))
}

/// State title.
fn state_title(text: String, cx: &App) -> Div {
    div()
        .debug_selector(|| "sum-state-title".into())
        .max_w_full()
        .text_center()
        .font_family(design::ui_font())
        .text_size(design::t_title(cx))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

/// State body paragraph.
fn state_body(text: String, p: MoonPalette, cx: &App) -> Div {
    div()
        .debug_selector(|| "sum-state-body".into())
        .max_w(design::ui_px(cx, 520.0))
        .text_center()
        .text_size(design::t_body(cx))
        .text_color(moon(p.text_soft))
        .child(text)
}

/// Civil days a bounded period spans: a partial day rounds up, and one DST hour either way never
/// adds or drops a day, so a 25-hour fall-back day still counts as one.
///
/// Args:
///     range: Resolved `[from, to)` Unix seconds; `from < 0` is unbounded.
///
/// Returns:
///     The day count, or `None` for an unbounded or empty period.
pub(in crate::analytics) fn period_days(range: (i64, i64)) -> Option<i64> {
    let (from, to) = range;
    (from >= 0 && to > from).then(|| ((to - from - 3_600).max(1) + 86_399) / 86_400)
}

/// The empty face: what period had nothing, which filters matched nothing, and the two ways out.
///
/// Args:
///     days: Days the period spans, when bounded.
///     filters: Quoted filters that narrowed the read; empty when none did.
///     marker: Scope marker line and its tooltip, when a preset or the Auto rail narrowed cores.
///     acts: Callbacks into the view.
///     p: Active palette.
///     cx: App context for scaled sizes.
///
/// Returns:
///     The empty face.
pub(in crate::analytics) fn empty_el(
    days: Option<i64>,
    filters: String,
    marker: Option<(String, String)>,
    acts: &StateActs,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let title = match days {
        Some(n) => {
            let key =
                crate::analytics::tuner::plural_key("analytics.sum.empty.title_days", n as usize);
            t!(key, n = n).to_string()
        }
        None => t!("analytics.sum.empty.title_period").to_string(),
    };
    let body = if filters.is_empty() {
        t!("analytics.sum.empty.body_none").to_string()
    } else {
        t!("analytics.sum.empty.body", filters = filters).to_string()
    };
    state_frame("sum-state-empty", p, cx)
        .child(state_title(title, cx))
        .child(state_body(body, p, cx))
        .children(marker.map(|(line, tip)| {
            div()
                .id("an-summary-scope-marker")
                .max_w_full()
                .text_center()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .tooltip(crate::panels::common::text_tooltip(tip))
                .child(line)
        }))
        .child(
            h_flex()
                .flex_wrap()
                .justify_center()
                .gap(design::ui_px(cx, 8.0))
                .child(action(
                    "an-sum-show-year",
                    0,
                    t!("analytics.sum.empty.show_year").to_string(),
                    true,
                    acts.on_show_year.clone(),
                ))
                .child(action(
                    "an-sum-reset",
                    1,
                    t!("analytics.sum.empty.reset").to_string(),
                    false,
                    acts.on_reset.clone(),
                )),
        )
        .into_any_element()
}

/// The read-failure face: title, the plain-language cause, the copyable path · operation · code
/// line, and Retry.
///
/// Args:
///     kind: Classified failure kind; picks the cause sentence.
///     path: Database file the read targeted.
///     operation: Failing operation label.
///     code: SQLite or OS code.
///     acts: Callbacks into the view.
///     p: Active palette.
///     cx: App context for scaled sizes.
///
/// Returns:
///     The error face.
pub(in crate::analytics) fn error_el(
    kind: FailKind,
    path: SharedString,
    operation: SharedString,
    code: FailCode,
    acts: &StateActs,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let side = design::ui_px(cx, 34.0);
    state_frame("sum-state-error", p, cx)
        .child(
            div()
                .size(side)
                .flex_none()
                .rounded_full()
                .bg(moon_alpha(p.red, 0.15))
                .text_color(moon(p.red))
                .font_family(design::mono())
                .font_weight(FontWeight::BOLD)
                .flex()
                .items_center()
                .justify_center()
                .child("!"),
        )
        .child(state_title(t!("common.db_read_failed").to_string(), cx))
        .child(state_body(
            crate::load_state::db_read_failed_hint(kind),
            p,
            cx,
        ))
        .child(
            div()
                .debug_selector(|| "sum-state-detail".into())
                .max_w_full()
                .child(crate::load_state::fail_detail_el(
                    path, operation, code, p, cx,
                )),
        )
        .child(
            h_flex()
                .flex_wrap()
                .justify_center()
                .gap(design::ui_px(cx, 8.0))
                .child(action(
                    "an-sum-retry",
                    0,
                    t!("analytics.sum.error.retry").to_string(),
                    true,
                    acts.on_retry.clone(),
                )),
        )
        .into_any_element()
}

/// Clock time of a snapshot in the display zone.
pub(in crate::analytics) fn as_of_text(secs: i64, zone: chrono_tz::Tz) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|d| d.with_timezone(&zone).format("%H:%M").to_string())
        .unwrap_or_else(|| "—".to_string())
}

/// The catch-up banner: the figures on screen are a kept snapshot while newer trades load.
///
/// Args:
///     as_of: Clock text of the snapshot on screen.
///
/// Returns:
///     The warning banner.
pub(in crate::analytics) fn catchup_banner_el(as_of: String) -> AnyElement {
    div()
        .debug_selector(|| "sum-banner".into())
        .w_full()
        .child(
            MoonAlert::warning("an-sum-catchup", t!("analytics.sum.catchup", time = as_of))
                .banner()
                .render(),
        )
        .into_any_element()
}

/// The blocks of the data page, rendered by their own `*_el` functions.
pub(in crate::analytics) struct DataBlocks {
    /// The catch-up banner, when a snapshot is kept.
    pub(in crate::analytics) banner: Option<AnyElement>,
    /// The KPI band.
    pub(in crate::analytics) kpi: AnyElement,
    /// The profit chart card.
    pub(in crate::analytics) chart: AnyElement,
    /// The insights card.
    pub(in crate::analytics) insights: AnyElement,
    /// The trades card.
    pub(in crate::analytics) trades: AnyElement,
    /// The cores card.
    pub(in crate::analytics) cores: AnyElement,
}

/// Assemble the data page: banner, KPI band, the main row, and the cores card. Wide windows put
/// the chart beside the side column (2:1); below the four-column width the chart takes the full
/// row with the side cards in two columns under it; below the two-column width everything
/// stacks.
///
/// Args:
///     cols: Column layout from `kpi_cols`.
///     b: The rendered blocks.
///     cx: App context for scaled sizes.
///
/// Returns:
///     The page column, not yet scrolled.
pub(in crate::analytics) fn data_page_el(cols: KpiCols, b: DataBlocks, cx: &App) -> Div {
    let gap = design::ui_px(cx, 8.0);
    let slot = |sel: &'static str, el: AnyElement| {
        div()
            .debug_selector(move || sel.into())
            .flex_1()
            .min_w_0()
            .child(el)
    };
    let main = match cols {
        KpiCols::Four => h_flex()
            .debug_selector(|| "sum-main".into())
            .w_full()
            .items_start()
            .gap(gap)
            .child(slot("sum-main-chart", b.chart).flex_grow(2.0))
            .child(
                v_flex()
                    .debug_selector(|| "sum-side".into())
                    .flex_1()
                    .min_w_0()
                    .gap(gap)
                    .child(b.insights)
                    .child(b.trades),
            ),
        KpiCols::Two => v_flex()
            .debug_selector(|| "sum-main".into())
            .w_full()
            .gap(gap)
            .child(
                div()
                    .debug_selector(|| "sum-main-chart".into())
                    .w_full()
                    .child(b.chart),
            )
            .child(
                h_flex()
                    .debug_selector(|| "sum-side".into())
                    .w_full()
                    .items_start()
                    .gap(gap)
                    .child(slot("sum-side-0", b.insights))
                    .child(slot("sum-side-1", b.trades)),
            ),
        KpiCols::One => v_flex()
            .debug_selector(|| "sum-main".into())
            .w_full()
            .gap(gap)
            .child(
                div()
                    .debug_selector(|| "sum-main-chart".into())
                    .w_full()
                    .child(b.chart),
            )
            .child(
                v_flex()
                    .debug_selector(|| "sum-side".into())
                    .w_full()
                    .gap(gap)
                    .child(b.insights)
                    .child(b.trades),
            ),
    };
    v_flex()
        .debug_selector(|| "sum-page".into())
        .w_full()
        .p(design::ui_px(cx, 10.0))
        .gap(gap)
        .children(b.banner)
        .child(
            div()
                .debug_selector(|| "sum-page-kpi".into())
                .w_full()
                .child(b.kpi),
        )
        .child(main)
        .child(
            div()
                .debug_selector(|| "sum-page-cores".into())
                .w_full()
                .child(b.cores),
        )
}

#[cfg(test)]
mod tests;
