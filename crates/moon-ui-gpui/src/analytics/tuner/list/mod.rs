//! Strategy-list controls for the hand-built comparison table: the filter bar (name search,
//! strategy-type dropdown, "active only" toggle), the click-to-sort helpers, and the visible-
//! column selector (mirrors the Orders `columns_menu` bitmask pattern). All view-only — the
//! filters/sort/columns never change what is written, only what the list shows.
//! The table itself (card, rows, sortable header) renders in `table`.

/// The list card, its rows and the sortable header row.
mod table;

use gpui::*;
use moon_ui::{
    MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize,
    MoonDropdown, MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem, MoonMenuSize,
    MoonPalette, h_flex,
};
use rust_i18n::t;
use std::cmp::Ordering;

use super::super::AnalyticsView;
use super::{
    COL_BIT_CORE, COL_BIT_KIND, COL_BIT_LASTEDIT, METRIC_COLS, SORT_CORE, SORT_KIND, SORT_LASTEDIT,
    STRAT_COLS_ALL, metric_bit, sort_arrow_of, toggle_sort_key,
};
use crate::design;
use crate::design::moon;
use moon_core::db::analytics::GroupStat;

/// "Which strategies name coins in a list?" — the list filter of the strategy table.
///
/// Its own enum rather than a pair of booleans: the three states are exclusive, and a
/// `(bool, bool)` would admit a fourth that means nothing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::analytics) enum StratListFilter {
    All,
    /// Has at least one coin in its blacklist.
    Black,
    /// Has at least one coin in its whitelist.
    White,
}

impl StratListFilter {
    /// Does this strategy pass? Reads the counts the aggregate already carries, so the
    /// filter and the two columns can never disagree about what "has a list" means.
    fn keeps(self, g: &GroupStat) -> bool {
        match self {
            StratListFilter::All => true,
            StratListFilter::Black => g.bl > 0,
            StratListFilter::White => g.wl > 0,
        }
    }

    /// Stable, locale-independent element key.
    fn key(self) -> &'static str {
        match self {
            StratListFilter::All => "all",
            StratListFilter::Black => "bl",
            StratListFilter::White => "wl",
        }
    }

    fn label(self) -> String {
        match self {
            StratListFilter::All => t!("report.filter.all").to_string(),
            StratListFilter::Black => t!("analytics.strat.with_bl").to_string(),
            StratListFilter::White => t!("analytics.strat.with_wl").to_string(),
        }
    }
}

/// Show at most this many groups (the replica can hold thousands of names; the tail beyond
/// the top by |profit| carries little information, and the DOM is not infinitely stretchy).
pub(in crate::analytics::tuner) const MAX_ROWS: usize = 300;

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
        // The coin-list views need the strategies DB to say anything: without it every
        // row reads bl = wl = 0, and filtering on that would present "we cannot see the
        // lists" as "no strategy has one". Same shape as the active-only fallback below.
        let base: Vec<&GroupStat> = if base.iter().any(|g| g.bl > 0 || g.wl > 0) {
            base.into_iter()
                .filter(|g| self.strat_lists.keeps(g))
                .collect()
        } else {
            base
        };
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

    /// The visible-column mask of the axis currently on screen.
    pub(super) fn strat_cols(&self) -> u16 {
        let mut cols = self.strat_cols;
        *self.strat_mode.cols_slot(&mut cols)
    }

    /// Is the column with visibility `bit` currently shown?
    pub(super) fn col_shown(&self, bit: u16) -> bool {
        self.strat_cols() & bit != 0
    }

    /// Set the visible-column mask and PERSIST it (layout), so the choice survives restart —
    /// same mechanism as the tuning period (`layout.analytics_strat_cols2`).
    fn set_strat_cols(&mut self, cols: u16, cx: &mut Context<Self>) {
        let mode = self.strat_mode;
        if *mode.cols_slot(&mut self.strat_cols) == cols {
            return;
        }
        *mode.cols_slot(&mut self.strat_cols) = cols;
        let all = self.strat_cols;
        self.backend.update(cx, |b, _| {
            b.layout.analytics_strat_cols_modes = Some(all);
            b.layout_dirty = true;
        });
        cx.notify();
    }

    /// Click a column header — the shared rule (`toggle_sort_key`), so this table and the
    /// coin table below it cannot disagree about what a click means.
    pub(super) fn toggle_sort(&mut self, key: &str, cx: &mut Context<Self>) {
        toggle_sort_key(&mut self.strat_sort, key);
        cx.notify();
    }

    /// Sort arrow suffix for a header (`" ▼"`/`" ▲"`), or empty when this column isn't the key.
    pub(super) fn sort_arrow(&self, key: &str) -> &'static str {
        sort_arrow_of(&self.strat_sort, key)
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
            .child(self.strat_lists_menu(cx))
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

    /// Coin-list dropdown ("All" / has a blacklist / has a whitelist), between the type
    /// selector and the "active only" toggle.
    fn strat_lists_menu(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let cur = self.strat_lists;
        let view = cx.entity();
        let mut menu = MoonDropdown::new("an-strat-lists")
            .label(format!("{} ▾", cur.label()))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Micro)
            .trigger_width(design::font_w(cx, 96.0))
            .menu_width(design::font_w(cx, 130.0))
            .menu_size(MoonMenuSize::Compact);
        for f in [
            StratListFilter::All,
            StratListFilter::Black,
            StratListFilter::White,
        ] {
            let view = view.clone();
            menu = menu.item(
                // Keyed by the VARIANT, not the label: a localized key changes the
                // element's identity when the language does.
                MoonMenuItem::with_key(format!("lists-{}", f.key()), f.label())
                    .checked(cur == f)
                    .on_click(move |_, _, app| {
                        view.update(app, |this, cx| {
                            this.strat_lists = f;
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
        let cur = self.strat_cols();
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
                        let next = if this.strat_cols() == STRAT_COLS_ALL {
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
                            let next = this.strat_cols() ^ bit;
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
