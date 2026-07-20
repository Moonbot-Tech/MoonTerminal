//! Strategy-list controls for the hand-built comparison table: the filter bar (name search,
//! strategy-type dropdown, "active only" toggle), the click-to-sort helpers, and the visible-
//! column selector (mirrors the Orders `columns_menu` bitmask pattern). All view-only — the
//! filters/sort/columns never change what is written, only what the list shows.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonCheckboxSize, MoonDropdown, MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem,
    MoonMenuSize, MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;
use std::cmp::Ordering;

use super::super::AnalyticsView;
use super::{
    COL_BIT_CORE, COL_BIT_KIND, COL_BIT_LASTEDIT, LASTEDIT_W, METRIC_COLS, SORT_CORE, SORT_KIND,
    SORT_LASTEDIT, SORT_NAME, STRAT_COLS_ALL, StratMode, metric_bit, metric_cell,
};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::GroupStat;

impl AnalyticsView {
    /// The strategy list after the filter bar (active-only, kind, name search) and the current
    /// sort. Returns references into the loaded data — the caller still caps to `MAX_ROWS`, so
    /// this may hand back more than that when the replica holds thousands of groups.
    pub(super) fn visible_strategies<'a>(&self, all: &'a [GroupStat]) -> Vec<&'a GroupStat> {
        let q = self.strat_search.trim().to_lowercase();
        // Type + name are explicit user filters. Active-only is applied on top, but falls back to
        // the pre-active set when it would empty a non-empty base — otherwise a period whose
        // strategies replica is absent (alive = NULL for every row) or all-deleted would show an
        // empty list even though rows exist. "Active" = still present in a core (alive >= 1).
        let base: Vec<&GroupStat> = all
            .iter()
            .filter(|g| {
                self.strat_type.as_ref().is_none_or(|t| &g.kind == t)
                    && (q.is_empty() || g.name.to_lowercase().contains(&q))
            })
            .collect();
        // Apply active-only ONLY when aliveness is actually known (at least one row has it). If
        // every row is alive = None (strategies replica absent), the filter has nothing to go on
        // and would blank the list — so skip it. When alive IS known, filter normally; a genuinely
        // all-deleted set then empties and the caller shows a "no matches" note.
        let mut out: Vec<&GroupStat> =
            if self.strat_active_only && base.iter().any(|g| g.alive.is_some()) {
                base.into_iter()
                    .filter(|g| g.alive.is_some_and(|a| a >= 1))
                    .collect()
            } else {
                base
            };
        if let Some((key, desc)) = self.strat_sort.clone() {
            // A numeric metric column sorts by its `sort` value; the name/kind/core columns
            // sort case-insensitively by text (key cached once per row, not per comparison).
            if let Some(col) = METRIC_COLS.iter().find(|c| c.key == key) {
                let f = col.sort;
                out.sort_by(|a, b| {
                    let o = f(a).partial_cmp(&f(b)).unwrap_or(Ordering::Equal);
                    if desc { o.reverse() } else { o }
                });
            } else {
                let sel: fn(&GroupStat) -> &str = match key.as_str() {
                    SORT_KIND => |g| g.kind.as_str(),
                    SORT_CORE => |g| g.core.as_str(),
                    SORT_LASTEDIT => |g| g.lastedit.as_str(),
                    _ => |g| g.name.as_str(),
                };
                out.sort_by_cached_key(|g| sel(g).to_lowercase());
                if desc {
                    out.reverse();
                }
            }
        }
        out
    }

    /// Is the column with visibility `bit` currently shown?
    pub(super) fn col_shown(&self, bit: u16) -> bool {
        self.strat_cols & bit != 0
    }

    /// Set the visible-column mask and PERSIST it (layout), so the choice survives restart —
    /// same mechanism as the tuning period (`layout.analytics_strat_cols`).
    fn set_strat_cols(&mut self, cols: u16, cx: &mut Context<Self>) {
        if self.strat_cols == cols {
            return;
        }
        self.strat_cols = cols;
        self.backend.update(cx, |b, _| {
            b.layout.analytics_strat_cols = Some(cols);
            b.layout_dirty = true;
        });
        cx.notify();
    }

    /// Click a column header: first click sorts descending, clicking the active column again
    /// flips to ascending.
    pub(super) fn toggle_sort(&mut self, key: &str, cx: &mut Context<Self>) {
        let desc = match &self.strat_sort {
            Some((k, d)) if k == key => !d,
            _ => true,
        };
        self.strat_sort = Some((key.to_string(), desc));
        cx.notify();
    }

    /// Sort arrow suffix for a header (`" ▼"`/`" ▲"`), or empty when this column isn't the key.
    pub(super) fn sort_arrow(&self, key: &str) -> &'static str {
        match &self.strat_sort {
            Some((k, d)) if k == key => {
                if *d {
                    " ▼"
                } else {
                    " ▲"
                }
            }
            _ => "",
        }
    }

    /// Distinct strategy kinds present in the loaded list (sorted, for the type dropdown).
    fn strat_kinds(&self) -> Vec<String> {
        self.data
            .view(|_| false)
            .ok()
            .map(|d| {
                let mut ks: Vec<String> = d
                    .strategies
                    .iter()
                    .map(|g| g.kind.clone())
                    .filter(|k| !k.is_empty())
                    .collect();
                ks.sort();
                ks.dedup();
                ks
            })
            .unwrap_or_default()
    }

    /// Lazily-created search input backing `strat_search`; commits (and filters) on every change.
    fn strat_search_state(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        if let Some(s) = &self.strat_search_input {
            return s.clone();
        }
        let val = self.strat_search.clone();
        let state = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(val)
                .placeholder(t!("analytics.strat.search_ph").to_string())
        });
        cx.subscribe(&state, |this, st, ev: &MoonInputEvent, cx| {
            if matches!(
                ev,
                MoonInputEvent::Change | MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }
            ) {
                // Commit on Change too so the list filters live as you type (the state entity
                // is cached, so the re-render keeps input focus).
                this.strat_search = st.read(cx).value().to_string();
                cx.notify();
            }
        })
        .detach();
        self.strat_search_input = Some(state.clone());
        state
    }

    /// The list filter bar row: name search · type · "active only" · column selector.
    pub(super) fn strat_filter_bar(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let search = self.strat_search_state(window, cx);
        h_flex()
            .w_full()
            .flex_none()
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 4.0))
            .gap(design::ui_px(cx, 6.0))
            .items_center()
            .bg(moon(p.table_head))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(MoonInput::new("an-strat-search").state(&search).small()),
            )
            .child(self.strat_type_menu(cx))
            .child(
                MoonCheckbox::new("an-strat-active")
                    .checked(self.strat_active_only)
                    .size(MoonCheckboxSize::Compact)
                    .label(t!("analytics.strat.active_only").to_string())
                    .on_change({
                        let view = cx.entity();
                        move |ch: &bool, _w, app| {
                            let on = *ch;
                            view.update(app, |this, cx| {
                                this.strat_active_only = on;
                                cx.notify();
                            });
                        }
                    }),
            )
            .child(self.strat_column_menu(cx))
            .into_any_element()
    }

    /// Strategy-type dropdown ("All" + each distinct kind); selecting sets `strat_type`.
    fn strat_type_menu(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let cur = self.strat_type.clone();
        let label = cur
            .clone()
            .unwrap_or_else(|| t!("report.filter.all").to_string());
        let view = cx.entity();
        let mut menu = MoonDropdown::new("an-strat-type")
            .label(format!("{label} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Micro)
            .trigger_width(design::font_w(cx, 116.0))
            .menu_width(design::font_w(cx, 150.0))
            .menu_size(MoonMenuSize::Compact);
        let all_view = view.clone();
        menu = menu.item(
            MoonMenuItem::with_key("type-all", t!("report.filter.all").to_string())
                .checked(cur.is_none())
                .on_click(move |_, _, app| {
                    all_view.update(app, |this, cx| {
                        this.strat_type = None;
                        cx.notify();
                    });
                }),
        );
        for k in self.strat_kinds() {
            let sel = cur.as_deref() == Some(k.as_str());
            let view = view.clone();
            let val = k.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("type-{k}"), k.clone())
                    .checked(sel)
                    .on_click(move |_, _, app| {
                        let val = val.clone();
                        view.update(app, |this, cx| {
                            this.strat_type = Some(val);
                            cx.notify();
                        });
                    }),
            );
        }
        menu
    }

    /// Visible-column selector (glyph "▦"); each item toggles a column bit, "All" toggles all.
    /// The menu stays open across clicks; the name column is always shown, so hiding every
    /// toggleable column is allowed (unlike Orders, which locks the last one).
    fn strat_column_menu(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let view = cx.entity();
        let cur = self.strat_cols;
        let mut menu = MoonDropdown::new("an-strat-cols")
            .segment(MoonButtonSegment::new("▦"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Micro)
            .trigger_width(design::font_w(cx, 30.0))
            .menu_width(design::font_w(cx, 160.0))
            .menu_size(MoonMenuSize::Compact)
            .close_on_select(false);
        let all_view = view.clone();
        menu = menu.item(
            MoonMenuItem::with_key("col-all", t!("report.filter.all").to_string())
                .checked(cur == STRAT_COLS_ALL)
                .on_click(move |_, _, app| {
                    all_view.update(app, |this, cx| {
                        let next = if this.strat_cols == STRAT_COLS_ALL {
                            0
                        } else {
                            STRAT_COLS_ALL
                        };
                        this.set_strat_cols(next, cx);
                    });
                }),
        );
        let add = |menu: MoonDropdown, bit: u16, title: String| {
            let view = view.clone();
            menu.item(
                MoonMenuItem::with_key(format!("col-{bit}"), title)
                    .checked(cur & bit != 0)
                    .on_click(move |_, _, app| {
                        view.update(app, |this, cx| {
                            let next = this.strat_cols ^ bit;
                            this.set_strat_cols(next, cx);
                        });
                    }),
            )
        };
        menu = add(menu, COL_BIT_KIND, t!("analytics.col.kind").to_string());
        menu = add(menu, COL_BIT_CORE, t!("analytics.col.core").to_string());
        for (i, c) in METRIC_COLS.iter().enumerate() {
            menu = add(menu, metric_bit(i), t!(c.key).to_string());
        }
        menu = add(
            menu,
            COL_BIT_LASTEDIT,
            t!("analytics.col.lastedit").to_string(),
        );
        menu
    }
}

/// Show at most this many groups (the replica can hold thousands of names; the tail beyond
/// the top by |profit| carries little information, and the DOM is not infinitely stretchy).
pub(in crate::analytics::tuner) const MAX_ROWS: usize = 300;

impl AnalyticsView {
    /// The list card: header (title + modes + counter), filter bar, its own scroll.
    pub(super) fn strat_list_card(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Resolved once for the whole list: this table is not virtualized, so a per-cell lookup
        // would clone the theme tokens twice for each of up to 300 rows × 7 columns.
        let scale = design::font_scale(cx);
        // The filter bar creates the search input (needs &mut) — build it before the immutable
        // data read below.
        let filter_bar = self.strat_filter_bar(p, window, cx);
        let (list, total, shown): (AnyElement, usize, usize) =
            match self.data.view(|d| d.strategies.is_empty()) {
                Ok(d) => {
                    // Filter + sort per the bar; count reflects the filtered set.
                    let rows = self.visible_strategies(&d.strategies);
                    let total = rows.len();
                    let shown = total.min(MAX_ROWS);
                    if rows.is_empty() {
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
                        let mut list = v_flex().w_full().gap_0();
                        for g in rows.into_iter().take(MAX_ROWS) {
                            list = list.child(self.strategy_row(g, p, scale, cx));
                        }
                        (list.into_any_element(), total, shown)
                    }
                }
                Err(note) => (
                    super::super::note_el("an-strat-list-note", note, 18.0, p, cx),
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
                            .text_size(design::t_title(cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("analytics.strat.title").to_string()),
                    )
                    .child(mode_btn(
                        "sm-filters",
                        StratMode::Filters,
                        t!("analytics.strat.mode_filter").to_string(),
                    ))
                    .child(mode_btn(
                        "sm-coins",
                        StratMode::Coins,
                        t!("analytics.strat.mode_coin").to_string(),
                    ))
                    .child(mode_btn(
                        "sm-time",
                        StratMode::Time,
                        t!("analytics.strat.mode_time").to_string(),
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
            .child(self.header_row(p, cx))
            .child(
                div()
                    .id("an-strat-list")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list),
            )
            .into_any_element()
    }

    /// A comparison-table row; a click selects/deselects the group.
    fn strategy_row(
        &self,
        g: &GroupStat,
        p: MoonPalette,
        scale: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        // Anchor = amber; Ctrl-selected extras = a lighter amber. The anchor drives the
        // right-hand scope (suggest/KPI/detail); extras are bulk-write addressees.
        let is_anchor = self.sel_strategy.as_ref().is_some_and(|(k, _)| *k == g.key);
        let is_extra = self.sel_extra.iter().any(|(k, _)| *k == g.key);
        let key = g.key.clone();
        // strategyid=0 = manual orders — a label instead of a bare "0" (both in the
        // row and in the selection: the tuner/dialog titles take the same one).
        let name = super::super::summary::strat_display(&g.name);
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
        let core_label = if g.cores_n > 1 {
            t!("report.cores_n", n = g.cores_n).to_string()
        } else {
            g.core.clone()
        };
        // Name on the left (flexible, truncated), all fixed columns in one rigid
        // cluster on the right (justify_between): when flex distribution glitches on
        // resize, the columns do not drift out of alignment between rows.
        let mut row = h_flex()
            .id(SharedString::from(format!("an-strat-{}", g.key)))
            .w_full()
            .h(design::fit_h_px(cx, 25.0, 14.0, 5.5))
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
                    .min_w_0()
                    .gap(design::ui_px(cx, 6.0))
                    .items_center()
                    .children(alive_dot)
                    // flex_1 is mandatory: a div with truncate() and no flex basis
                    // collapses to "…" (that is how strategy names used to vanish).
                    .child(div().flex_1().min_w_0().truncate().child(name.clone())),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    // Each identity/metric column renders only when its visibility bit is set
                    // (column selector); the name column on the left is always shown.
                    .children(self.col_shown(COL_BIT_KIND).then(|| {
                        div()
                            .w(design::font_w_px(cx, 72.0))
                            .flex_none()
                            .truncate()
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .child(g.kind.clone())
                    }))
                    .children(self.col_shown(COL_BIT_CORE).then(|| {
                        div()
                            .w(design::font_w_px(cx, 88.0))
                            .flex_none()
                            .truncate()
                            .text_color(moon(p.text_soft))
                            .child(core_label)
                    }))
                    .children(
                        METRIC_COLS
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| self.col_shown(metric_bit(*i)))
                            .map(|(_, c)| metric_cell(c, g, p, scale)),
                    )
                    .children(self.col_shown(COL_BIT_LASTEDIT).then(|| {
                        div()
                            .w(design::font_w_px(cx, LASTEDIT_W))
                            .flex_none()
                            .truncate()
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .child(g.lastedit.clone())
                    })),
            )
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                // secondary() = Ctrl on Windows/Linux, ⌘ on macOS — the standard multi-select
                // modifier (mirrors the strategies tree in tree_moon.rs).
                if ev.modifiers().secondary() {
                    this.toggle_multi(key.clone(), name.clone(), cx);
                } else {
                    this.select_single(key.clone(), name.clone(), cx);
                }
            }));
        if is_anchor {
            row = row
                .bg(moon_alpha(p.amber, 0.12))
                .border_color(moon_alpha(p.amber, 0.5));
        } else if is_extra {
            row = row
                .bg(moon_alpha(p.amber, 0.06))
                .border_color(moon_alpha(p.amber, 0.3));
        } else {
            row = row.hover(move |s| s.bg(moon_alpha(p.panel_high, 0.9)));
        }
        row
    }
}

impl AnalyticsView {
    /// Comparison-table header: clicking a title sorts (descending; a repeat click —
    /// ascending); a ▼/▲ arrow marks the active column. Columns respect the visibility
    /// selector; the strategy name is always shown.
    fn header_row(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement + use<> {
        let scale = design::font_scale(cx);
        // One clickable sort header. `w = None` → flexible (the name column).
        let sortable =
            |id: SharedString, title: String, key: &'static str, w: Option<f32>, right: bool| {
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
                    Some(w) => d.w(px(w * scale)),
                    None => d.flex_1().min_w_0(),
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
                    .flex_none()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .children(self.col_shown(COL_BIT_KIND).then(|| {
                        sortable(
                            "an-hdr-kind".into(),
                            t!("analytics.col.kind").to_string(),
                            SORT_KIND,
                            Some(72.0),
                            false,
                        )
                    }))
                    .children(self.col_shown(COL_BIT_CORE).then(|| {
                        sortable(
                            "an-hdr-core".into(),
                            t!("analytics.col.core").to_string(),
                            SORT_CORE,
                            Some(88.0),
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
                                    Some(c.w),
                                    true,
                                )
                            }),
                    )
                    .children(self.col_shown(COL_BIT_LASTEDIT).then(|| {
                        sortable(
                            "an-hdr-le".into(),
                            t!("analytics.col.lastedit").to_string(),
                            SORT_LASTEDIT,
                            Some(LASTEDIT_W),
                            false,
                        )
                    })),
            )
    }
}
