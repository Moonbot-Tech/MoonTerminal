//! The strategy comparison table: the list card (header + filter bar + virtual list),
//! one row per strategy group, and the sortable column header. The controls it renders
//! under — filters, sort state, column masks — live in the parent (`list`).

use gpui::*;
use moon_ui::{
    h_flex, v_flex, MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette,
    MoonScrollbarVisibility, MoonVirtualList,
};
use rust_i18n::t;
use std::collections::HashSet;
use std::sync::Arc;

use super::super::super::AnalyticsView;
use super::super::{
    metric_bit, metric_cell, StratMode, COL_BIT_CORE, COL_BIT_KIND, COL_BIT_LASTEDIT, CORE_MIN_W,
    CORE_W, CORE_W_MAX, KIND_MIN_W, KIND_W, LASTEDIT_MIN_W, LASTEDIT_W, METRIC_COLS, SORT_CORE,
    SORT_KIND, SORT_LASTEDIT, SORT_NAME, STRAT_NAME_MIN_W,
};
use super::MAX_ROWS;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::GroupStat;

/// Height of one strategy row, in base px.
///
/// ONE definition because two consumers must agree exactly: the virtual list declares it as its
/// item pitch, and the row draws itself at it. Two copies of the same three numbers agree only
/// until someone edits one of them, and a pitch that disagrees with the drawn height overlaps or
/// gaps every row on screen.
fn strat_row_h(cx: &App) -> f32 {
    design::fit_h_value(cx, 25.0, 14.0, 5.5)
}

impl AnalyticsView {
    /// The list card: header (title + modes + counter), filter bar, its own scroll.
    pub(in crate::analytics::tuner) fn strat_list_card(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Resolved once for the whole list and captured by value into the row factory: a
        // per-cell lookup would clone the theme tokens twice for every drawn row × column.
        let scale = design::font_scale(cx);
        // The core column's content-measured width, computed once here and handed to both the
        // header and every row so they cannot disagree within a frame. `strat_core_w` documents
        // the cache and how it is invalidated; this match is only its read.
        let core_w = match self.strat_core_w {
            Some((s, w)) if s == scale => w,
            _ => {
                let w = self
                    .strategy_data
                    .data()
                    .map(|d| core_col_w(&d.strategies, scale, cx))
                    .unwrap_or(CORE_W);
                self.strat_core_w = Some((scale, w));
                w
            }
        };
        // The filter bar creates the search input (needs &mut) — build it before the immutable
        // data read below.
        let filter_bar = self.strat_filter_bar(p, window, cx);
        // Clone the Arc out of the load state first: the memo below takes `&mut self`, so it
        // cannot run while a borrow into `strategy_data` is alive.
        let data = self
            .strategy_data
            .view(|d| d.strategies.is_empty())
            .map(Arc::clone);
        let (list, total, shown): (AnyElement, usize, usize) = match data {
            Ok(d) => {
                // Filter + sort per the bar, memoized; count reflects the filtered set.
                let total = self.ensure_visible(&d.strategies).len();
                let shown = total.min(MAX_ROWS);
                if shown == 0 {
                    // Raw data exists but the filters/search matched nothing — say so instead
                    // of leaving a blank area.
                    let note = div()
                        .w_full()
                        .p(design::ui_px(cx, 18.0))
                        .text_center()
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.strat.no_match").to_string());
                    (note.into_any_element(), 0, 0)
                } else {
                    // The factory runs only for rows on screen, bounding the work triggered by
                    // `.hover()` notifications even though rows have no entity of their own.
                    let weak = cx.entity().downgrade();
                    (
                        MoonVirtualList::new(
                            "an-strat-rows",
                            shown,
                            strat_row_h(cx),
                            move |ix, _w, app| {
                                weak.upgrade()
                                    .and_then(|e| {
                                        // Rows AND their order are read from the view in one
                                        // go, so an index can never be applied to a group set
                                        // it was not computed against.
                                        let view = e.read(app);
                                        let all = &view.strategy_data.data()?.strategies;
                                        let g = view
                                            .visible_indices()
                                            .get(ix)
                                            .and_then(|i| all.get(*i))?;
                                        Some(strategy_row(view, &weak, g, p, scale, core_w, app))
                                    })
                                    .unwrap_or_else(|| div().into_any_element())
                            },
                        )
                        .surface(false)
                        .border(false)
                        .radius(0.0)
                        .scrollbar_visibility(MoonScrollbarVisibility::Hover)
                        .into_any_element(),
                        total,
                        shown,
                    )
                }
            }
            Err(note) => (
                super::super::super::note_el("an-strat-list-note", note, 18.0, p, cx),
                0,
                0,
            ),
        };

        let mode_btn = |id: &'static str, mode: StratMode, label: String| {
            let on = self.strat_mode == mode;
            MoonButton::new(id)
                .variant(if on {
                    MoonButtonVariant::Amber
                } else {
                    MoonButtonVariant::Soft
                })
                .size(MoonButtonSize::Micro)
                .selected(on)
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| this.set_strat_mode(mode, cx)))
                .render()
        };

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .rounded(design::ui_px(cx, 8.0))
            .bg(moon(p.panel))
            .border_1()
            .border_color(moon(p.border))
            .overflow_hidden()
            .child(
                h_flex()
                    .w_full()
                    .flex_none()
                    .px(design::ui_px(cx, 12.0))
                    .py(design::ui_px(cx, 8.0))
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        div()
                            .flex_none()
                            // Match the mode buttons' box so `items_center` aligns equal heights;
                            // the explicit line height also contains the title glyph without a
                            // low visual baseline.
                            .h(design::micro_control_h(cx))
                            .flex()
                            .items_center()
                            .text_size(design::t_title(cx))
                            .line_height(design::line_px(cx, 18.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("analytics.strat.title").to_string()),
                    )
                    // Order is deliberate and asserted by `theme_contract`: filter, then time,
                    // then coin. Each id stays bound to its own mode — the persisted per-axis
                    // column masks are keyed by mode, not by position.
                    .child(mode_btn(
                        "sm-filters",
                        StratMode::Filters,
                        t!("analytics.strat.mode_filter").to_string(),
                    ))
                    .child(mode_btn(
                        "sm-time",
                        StratMode::Time,
                        t!("analytics.strat.mode_time").to_string(),
                    ))
                    .child(mode_btn(
                        "sm-coins",
                        StratMode::Coins,
                        t!("analytics.strat.mode_coin").to_string(),
                    ))
                    .child(div().flex_1())
                    .child({
                        // In multi-select show the count (amber) so the user knows a bulk save
                        // is armed; otherwise the shown/total or the click hint.
                        let (txt, col) = if self.is_multi() {
                            (
                                t!("analytics.strat.selected_n", n = self.sel_extra.len() + 1)
                                    .to_string(),
                                p.amber,
                            )
                        } else if total > shown {
                            (
                                t!("analytics.strat.shown", shown = shown, total = total)
                                    .to_string(),
                                p.text_muted,
                            )
                        } else {
                            (t!("analytics.strat.hint").to_string(), p.text_muted)
                        };
                        div()
                            .text_size(design::t_caption(cx))
                            .text_color(moon(col))
                            .child(txt)
                    }),
            )
            .child(filter_bar)
            // The header row stays OUTSIDE the virtual list so it cannot scroll away.
            .child(self.header_row(p, core_w, cx))
            .child(div().w_full().flex_1().min_h_0().child(list))
            .into_any_element()
    }
}

/// A comparison-table row; a click selects/deselects the group.
///
/// A free function taking a WEAK handle, not a method: `MoonVirtualList`'s row factory is
/// `'static` and outlives the render, so a strong `cx.entity()` capture would close
/// `AnalyticsView -> element -> closure -> AnalyticsView` and leak the window — the same cycle
/// `theme_contract::moon_tree_closures_hold_weak_view_handles` guards for `MoonTree`.
///
/// `core_w` is the content-measured core-column width (see [`core_col_w`]) — passed in
/// because the header must lay the same value out, or it drifts off the column.
fn strategy_row(
    view: &AnalyticsView,
    weak: &WeakEntity<AnalyticsView>,
    g: &GroupStat,
    p: MoonPalette,
    scale: f32,
    core_w: f32,
    cx: &App,
) -> AnyElement {
    // Anchor = amber; Ctrl- or Shift-selected extras = a lighter amber. The anchor drives the
    // right-hand scope (suggest/KPI/detail); extras are bulk-write addressees.
    let is_anchor = view.sel_strategy.as_ref().is_some_and(|(k, _)| *k == g.key);
    let is_extra = view.sel_extra.iter().any(|(k, _)| *k == g.key);
    let key = g.key.clone();
    // strategyid=0 = manual orders — a label instead of a bare "0" (both in the
    // row and in the selection: the tuner/dialog titles take the same one).
    let name = super::super::super::summary::strat_display(&g.name);
    // "Alive right now" indicator: ● green — present in a core and enabled,
    // ● muted — present but disabled, ○ outline — deleted from the cores.
    let alive_dot = g.alive.map(|a| {
        let dot = div()
            .flex_none()
            .w(design::ui_px(cx, 6.0))
            .h(design::ui_px(cx, 6.0))
            .rounded_full();
        match a {
            2 => dot.bg(moon(p.green)),
            1 => dot.bg(moon_alpha(p.text_muted, 0.8)),
            _ => dot.border_1().border_color(moon_alpha(p.text_muted, 0.6)),
        }
    });
    let core_text = core_label(g);
    // Name on the left (flexible, truncated), all fixed columns in one rigid
    // cluster on the right (justify_between): when flex distribution glitches on
    // resize, the columns do not drift out of alignment between rows.
    let mut row = h_flex()
        .id(SharedString::from(format!("an-strat-{}", g.key)))
        .w_full()
        .h(px(strat_row_h(cx)))
        .px(design::ui_px(cx, 8.0))
        .gap(design::ui_px(cx, 8.0))
        .items_center()
        .justify_between()
        .cursor_pointer()
        .bg(moon(p.table_body))
        .border_t_1()
        .border_color(moon_alpha(p.border, 0.6))
        .child(
            h_flex()
                .flex_1()
                // The floor belongs on the flex item because a floor on the text cannot stop the
                // cluster from shrinking to zero and painting the name over the type column. The
                // header uses the same floor to stay aligned.
                .min_w(design::font_w_px(cx, STRAT_NAME_MIN_W))
                .gap(design::ui_px(cx, 6.0))
                .items_center()
                .children(alive_dot)
                // A flex basis prevents the truncated div from collapsing to an ellipsis, while
                // `min_w_0` lets it truncate inside the floor held by its parent.
                .child(div().flex_1().min_w_0().truncate().child(name.clone())),
        )
        .child(
            h_flex()
                // Shrinkable, not rigid: the columns inside carry their own floors, so the
                // row squeezes them evenly and only overflows once every one is at its
                // floor — instead of sliding off the edge at full width.
                .flex_shrink_1()
                .min_w_0()
                .gap(design::ui_px(cx, 8.0))
                .items_center()
                // Each identity/metric column renders only when its visibility bit is set
                // (column selector); the name column on the left is always shown.
                .children(view.col_shown(COL_BIT_KIND).then(|| {
                    div()
                        .w(design::font_w_px(cx, KIND_W))
                        .min_w(design::font_w_px(cx, KIND_MIN_W))
                        .flex_shrink_1()
                        .truncate()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(g.kind.clone())
                }))
                .children(view.col_shown(COL_BIT_CORE).then(|| {
                    div()
                        .w(design::font_w_px(cx, core_w))
                        .min_w(design::font_w_px(cx, CORE_MIN_W))
                        .flex_shrink_1()
                        .truncate()
                        .text_color(moon(p.text_soft))
                        .child(core_text)
                }))
                .children(
                    METRIC_COLS
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| view.col_shown(metric_bit(*i)))
                        .map(|(_, c)| metric_cell(c, g, p, scale)),
                )
                .children(view.col_shown(COL_BIT_LASTEDIT).then(|| {
                    div()
                        .w(design::font_w_px(cx, LASTEDIT_W))
                        .min_w(design::font_w_px(cx, LASTEDIT_MIN_W))
                        .flex_shrink_1()
                        .truncate()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(g.lastedit.clone())
                })),
        )
        .on_click({
            let weak = weak.clone();
            move |ev: &ClickEvent, _window, app| {
                // secondary() = Ctrl on Windows/Linux, ⌘ on macOS — the standard
                // multi-select modifier (mirrors the strategies tree in tree_moon.rs).
                // Shift takes precedence over it, as it does there. A keyboard-activated
                // click reports default modifiers, so it lands on the plain single-select —
                // which is the behaviour keyboard activation should have anyway.
                let m = ev.modifiers();
                let intent = super::row_click_intent(m.shift, m.secondary());
                let (key, name) = (key.clone(), name.clone());
                // The view may already be gone; a dropped window is not an error here.
                let _ = weak.update(app, |this, cx| match intent {
                    super::RowClick::Range => this.select_range(key, name, cx),
                    super::RowClick::Multi => this.toggle_multi(key, name, cx),
                    super::RowClick::Single => this.select_single(key, name, cx),
                });
            }
        });
    // BLUE marks "this strategy traded the coin you clicked below" — an answer to a question,
    // not a selection, so it never overrides the amber the user chose. BOTH amber branches
    // therefore come first: an extra is a bulk-write addressee, and painting it blue would hide
    // a row that Save is about to write to. A range selection makes the overlap ordinary rather
    // than rare, since it can hold a whole block at once.
    let traded_picked = view.coins.picked_strats.contains(&g.key);
    if is_anchor {
        row = row
            .bg(moon_alpha(p.amber, 0.12))
            .border_color(moon_alpha(p.amber, 0.5));
    } else if is_extra {
        row = row
            .bg(moon_alpha(p.amber, 0.06))
            .border_color(moon_alpha(p.amber, 0.3));
    } else if traded_picked {
        row = row
            .bg(moon_alpha(p.blue, 0.16))
            .border_color(moon_alpha(p.blue, 0.45));
    } else {
        // Hover communicates that the whole line is clickable. Its notification redraws the
        // shared Analytics view, so virtualization and the cached index order bound that redraw
        // to the visible rows.
        row = row.hover(move |s| s.bg(moon_alpha(p.panel_high, 0.9)));
    }
    row.into_any_element()
}

impl AnalyticsView {
    /// Comparison-table header: clicking a title sorts (descending; a repeat click —
    /// ascending); a ▼/▲ arrow marks the active column. Columns respect the visibility
    /// selector; the strategy name is always shown. `core_w` mirrors the rows' measured
    /// core-column width — the header shrinks and grows exactly like the cells under it.
    fn header_row(
        &self,
        p: MoonPalette,
        core_w: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let scale = design::font_scale(cx);
        // One clickable sort header. `w = None` → flexible (the name column).
        let sortable = |id: SharedString,
                        title: String,
                        key: &'static str,
                        w: Option<(f32, f32)>,
                        right: bool| {
            let arrow = self.sort_arrow(key);
            let mut d = div()
                .id(id)
                .flex_none()
                .truncate()
                .cursor_pointer()
                .text_color(if arrow.is_empty() {
                    moon(p.text_soft)
                } else {
                    moon(p.amber)
                })
                .child(format!("{title}{arrow}"))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_sort(key, cx)));
            if right {
                d = d.text_right();
            }
            match w {
                // Shrinks exactly like the body cell under it, floor included, or the
                // heading drifts off the column it labels the moment space runs short.
                Some((w, min)) => d.w(px(w * scale)).min_w(px(min * scale)).flex_shrink_1(),
                None => d.flex_1().min_w(px(STRAT_NAME_MIN_W * scale)),
            }
        };
        h_flex()
            .w_full()
            .flex_none()
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .px(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, 8.0))
            .items_center()
            .justify_between()
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            .child(sortable(
                "an-hdr-name".into(),
                t!("analytics.col.strategy").to_string(),
                SORT_NAME,
                None,
                false,
            ))
            // Cluster of fixed columns — mirrors the rows (justify_between).
            .child(
                h_flex()
                    .flex_shrink_1()
                    .min_w_0()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .children(self.col_shown(COL_BIT_KIND).then(|| {
                        sortable(
                            "an-hdr-kind".into(),
                            t!("analytics.col.kind").to_string(),
                            SORT_KIND,
                            Some((KIND_W, KIND_MIN_W)),
                            false,
                        )
                    }))
                    .children(self.col_shown(COL_BIT_CORE).then(|| {
                        sortable(
                            "an-hdr-core".into(),
                            t!("analytics.col.core").to_string(),
                            SORT_CORE,
                            Some((core_w, CORE_MIN_W)),
                            false,
                        )
                    }))
                    .children(
                        METRIC_COLS
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| self.col_shown(metric_bit(*i)))
                            .map(|(_, c)| {
                                sortable(
                                    c.key.into(),
                                    t!(c.key).to_string(),
                                    c.key,
                                    Some((c.w, c.min_w)),
                                    true,
                                )
                            }),
                    )
                    .children(self.col_shown(COL_BIT_LASTEDIT).then(|| {
                        sortable(
                            "an-hdr-le".into(),
                            t!("analytics.col.lastedit").to_string(),
                            SORT_LASTEDIT,
                            Some((LASTEDIT_W, LASTEDIT_MIN_W)),
                            false,
                        )
                    })),
            )
    }
}

/// The core cell's text: the localized "Cores: N" aggregate when the group spans several
/// cores, otherwise the raw core (server) name.
///
/// A named function because [`core_col_w`] sizes the column to what the rows will draw:
/// derived in two places, the two could disagree and the column would truncate the very
/// label it was sized for.
fn core_label(g: &GroupStat) -> String {
    if g.cores_n > 1 {
        t!("report.cores_n", n = g.cores_n).to_string()
    } else {
        g.core.clone()
    }
}

/// Preferred width of the core column, in font-scaled BASE px (the callers wrap it in
/// `font_w_px`): the widest single-core label the list can draw, measured in the font the
/// cell renders with (the window root's mono family at body size), clamped to
/// [`CORE_W`, `CORE_W_MAX`].
///
/// Content-measured because the single-core label is a free-form server name: any fixed
/// width either truncates it on a wide window or wastes the row on a narrow one. Measured
/// over ALL groups rather than the filtered view, so the column does not jump as the user
/// types in the search box. Only DISTINCT names are measured — hundreds of groups share a
/// handful of cores, and each measurement pays an uncached glyph layout per character (the
/// cost note on `strips::FittedCells`); the caller additionally caches the result per data
/// load. Multi-core rows draw the "Cores: N" aggregate instead of a name — at most ~12
/// characters in every locale it stays under the `CORE_W` floor, so it is not measured,
/// which also keeps the cached width independent of the active locale.
fn core_col_w(groups: &[GroupStat], scale: f32, cx: &App) -> f32 {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut w = 0.0f32;
    for g in groups {
        // Measure `core_label` (the row's actual text), not raw `g.core`, so the width tracks the
        // label formula; dedup on the raw name, which keys that label 1:1.
        if g.cores_n <= 1 && seen.insert(g.core.as_str()) {
            w = w.max(design::mono_body_text_width(
                cx,
                &core_label(g),
                FontWeight::NORMAL.0,
            ));
        }
    }
    // The measurement returns font-scaled px, but the cell width goes through `font_w_px`,
    // which scales again — divide back to base units (this also makes the cached value hold
    // across Font-slider moves). Ceil so a fractional shortfall cannot ellipsize the widest
    // name the column was sized for.
    (w / scale).ceil().clamp(CORE_W, CORE_W_MAX)
}
