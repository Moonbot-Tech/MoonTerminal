//! Strategy-list controls for the hand-built comparison table: the filter bar (name search,
//! strategy-type dropdown, "active only" toggle), the click-to-sort helpers, and the visible-
//! column selector (mirrors the Orders `columns_menu` bitmask pattern). All view-only — the
//! filters/sort/columns never change what is written, only what the list shows.
//! The table itself (card, rows, sortable header) renders in `table`.

/// Pure selection arithmetic: the drawn row order and the Shift-click range over it.
mod select;
/// The list card, its rows and the sortable header row.
mod table;

#[cfg(test)]
mod tests;

use gpui::*;
use moon_ui::{
    MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize,
    MoonDropdown, MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem, MoonMenuSize,
    MoonPalette, h_flex,
};
use rust_i18n::t;
use std::cmp::Ordering;

pub(in crate::analytics::tuner) use select::{
    RangeOutcome, RowClick, drawn_order, inclusive_report_bounds, range_extras, row_click_intent,
};

use super::super::AnalyticsView;
use super::{
    COL_BIT_CORE, COL_BIT_KIND, COL_BIT_LASTEDIT, METRIC_COLS, SORT_CORE, SORT_KIND, SORT_LASTEDIT,
    SORT_NAME, STRAT_COLS_ALL, STRAT_SORT_DEFAULT, metric_bit, sort_arrow_of, toggle_sort_key,
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

/// Restore a saved strategy-list sort only when its stable key still names a real column.
///
/// Args:
///     saved: Optional `(column key, descending)` value read from `layout.toml`.
///
/// Returns:
///     A valid saved choice, or the profit-descending default.
pub(in crate::analytics) fn restore_strat_sort(
    saved: Option<(String, bool)>,
) -> Option<(String, bool)> {
    saved
        .filter(|(key, _)| {
            matches!(
                key.as_str(),
                SORT_NAME | SORT_KIND | SORT_CORE | SORT_LASTEDIT
            ) || METRIC_COLS.iter().any(|column| column.key == key)
        })
        .or_else(|| Some((STRAT_SORT_DEFAULT.0.to_string(), STRAT_SORT_DEFAULT.1)))
}

/// Everything the strategy list's filter and sort depend on.
///
/// [`filter_sort_indices`] takes no state outside this type, so every filter control must extend
/// the key before it can affect the result. This keeps cache invalidation coupled to filtering.
#[derive(PartialEq)]
pub(in crate::analytics) struct VisibleKey {
    /// Address of the current group slice, paired with explicit invalidation whenever
    /// `strategy_data` is replaced so allocator address reuse cannot preserve a stale result.
    base: usize,
    /// The search box, folded once here rather than per row on every render.
    search_lower: String,
    kind: Option<String>,
    lists: StratListFilter,
    active_only: bool,
    sort: Option<(String, bool)>,
}

/// The memoized row-index order and the complete set of inputs that produced it.
pub(in crate::analytics) struct VisibleRows {
    key: VisibleKey,
    idx: Vec<usize>,
}

/// Sort only as deep as the list can draw.
///
/// The virtual list never asks for a row past [`MAX_ROWS`], so the tail's order is unobservable
/// and paying `O(n log n)` for it over thousands of groups is waste. Shared with the coin table,
/// which faces the same cap.
pub(in crate::analytics::tuner) fn partial_sort<T>(
    items: &mut [T],
    mut cmp: impl FnMut(&T, &T) -> Ordering,
) {
    if items.len() > MAX_ROWS {
        items.select_nth_unstable_by(MAX_ROWS, &mut cmp);
    }
    let head = items.len().min(MAX_ROWS);
    items[..head].sort_by(cmp);
}

/// Filter and sort the whole group set, returning INDICES into it.
///
/// Indices rather than references so the result can be cached on the view: a `Vec<&GroupStat>`
/// would borrow from `strategy_data` and make the cache self-referential.
///
/// The list-membership and aliveness fallbacks preserve visible rows when the strategies replica
/// does not provide enough information to evaluate those filters.
pub(in crate::analytics) fn filter_sort_indices(all: &[GroupStat], key: &VisibleKey) -> Vec<usize> {
    let q = key.search_lower.as_str();
    // Type and name are always known from the report groups, so apply them before filters that
    // depend on the optional strategies replica.
    let base: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, g)| {
            key.kind.as_ref().is_none_or(|t| &g.kind == t)
                && (q.is_empty() || g.name.to_lowercase().contains(q))
        })
        .map(|(i, _)| i)
        .collect();
    // The coin-list views need the strategies DB to say anything: without it every
    // row reads bl = wl = 0, and filtering on that would present "we cannot see the
    // lists" as "no strategy has one". Same shape as the active-only fallback below.
    let base: Vec<usize> = if base.iter().any(|i| all[*i].bl > 0 || all[*i].wl > 0) {
        base.into_iter()
            .filter(|i| key.lists.keeps(&all[*i]))
            .collect()
    } else {
        base
    };
    // Apply active-only ONLY when aliveness is actually known (at least one row has it). If
    // every row is alive = None (strategies replica absent), the filter has nothing to go on
    // and would blank the list — so skip it. When alive IS known, filter normally; a genuinely
    // all-deleted set then empties and the caller shows a "no matches" note.
    let mut out: Vec<usize> = if key.active_only && base.iter().any(|i| all[*i].alive.is_some()) {
        base.into_iter()
            .filter(|i| all[*i].alive.is_some_and(|a| a >= 1))
            .collect()
    } else {
        base
    };
    if let Some((sort_key, desc)) = &key.sort {
        let desc = *desc;
        // A numeric metric column sorts by its `sort` value; the name/kind/core columns
        // sort case-insensitively by text.
        // Every comparator below closes with the group key. Without it the order is only
        // partial, and `partial_sort` resolves a tie with `select_nth_unstable_by`, which is
        // free both to reorder equal elements AND to choose arbitrarily among them for the
        // drawn head — so tied rows could change places, or appear and disappear, between two
        // renders of identical data. `db::analytics::groups` makes its own order total for the
        // same reason; this is where that guarantee has to be repeated, because sorting here
        // discards it.
        if let Some(col) = METRIC_COLS.iter().find(|c| c.key == sort_key) {
            let f = col.sort;
            partial_sort(&mut out, |a, b| {
                let o = f(&all[*a])
                    .partial_cmp(&f(&all[*b]))
                    .unwrap_or(Ordering::Equal);
                let o = if desc { o.reverse() } else { o };
                o.then_with(|| all[*a].key.cmp(&all[*b].key))
            });
        } else {
            let sel: fn(&GroupStat) -> &str = match sort_key.as_str() {
                SORT_KIND => |g| g.kind.as_str(),
                SORT_CORE => |g| g.core.as_str(),
                SORT_LASTEDIT => |g| g.lastedit.as_str(),
                _ => |g| g.name.as_str(),
            };
            // Compared in ONE direction rather than sorted-then-reversed: reversing also flips
            // rows whose keys are equal, so descending would not mirror ascending.
            partial_sort(&mut out, |a, b| {
                let (x, y) = (sel(&all[*a]).to_lowercase(), sel(&all[*b]).to_lowercase());
                let o = if desc { y.cmp(&x) } else { x.cmp(&y) };
                o.then_with(|| all[*a].key.cmp(&all[*b].key))
            });
        }
    }
    out
}

/// Whether a cached result still describes the current inputs.
///
/// Split out so the memo's ONE decision can be exercised directly: "recomputed zero times while
/// nothing changed" is the whole objective, and it is invisible from the rendered output.
fn memo_is_fresh(cached: Option<&VisibleRows>, key: &VisibleKey) -> bool {
    cached.is_some_and(|c| &c.key == key)
}

impl AnalyticsView {
    /// Rebuild the row order unless the cache already holds the same inputs.
    ///
    /// Called once per render so hover and wheel notifications reuse the cached order instead of
    /// sorting the complete group set.
    ///
    /// A data change is caught two ways, and BOTH are needed: the group set's address sits in
    /// the key, and the memo is dropped where `strategy_data` is replaced. The address alone is
    /// only unique among live allocations — a failed load frees the buffer and a later one can
    /// be given the same address — so the explicit drop is what makes the key trustworthy.
    pub(super) fn ensure_visible(&mut self, all: &[GroupStat]) -> &[usize] {
        let key = VisibleKey {
            base: all.as_ptr() as usize,
            search_lower: self.strat_search.trim().to_lowercase(),
            kind: self.strat_type.clone(),
            lists: self.strat_lists,
            active_only: self.strat_active_only,
            sort: self.strat_sort.clone(),
        };
        if !memo_is_fresh(self.strat_visible.as_ref(), &key) {
            let idx = filter_sort_indices(all, &key);
            self.strat_visible = Some(VisibleRows { key, idx });
        }
        self.visible_indices()
    }

    /// Indices of the rows the list should draw, in order. Empty until [`Self::ensure_visible`].
    pub(super) fn visible_indices(&self) -> &[usize] {
        self.strat_visible
            .as_ref()
            .map_or(&[], |c| c.idx.as_slice())
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

    /// Click a column header, update the shared rule, and persist the exact key and direction.
    ///
    /// Every strategy header reaches this one method, so marking the layout dirty here makes
    /// persistence structural rather than a responsibility of each rendered column.
    pub(super) fn toggle_sort(&mut self, key: &str, cx: &mut Context<Self>) {
        toggle_sort_key(&mut self.strat_sort, key);
        let value = self.strat_sort.clone();
        self.backend.update(cx, |backend, _| {
            backend.layout.analytics_strat_sort = value;
            backend.layout_dirty = true;
        });
        cx.notify();
    }

    /// Sort arrow suffix for a header (`" ▼"`/`" ▲"`), or empty when this column isn't the key.
    pub(super) fn sort_arrow(&self, key: &str) -> &'static str {
        sort_arrow_of(&self.strat_sort, key)
    }

    /// Distinct strategy kinds present in the loaded list (sorted, for the type dropdown).
    fn strat_kinds(&self) -> Vec<String> {
        self.strategy_data
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
            .label(label)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Micro)
            .trigger_width_scaled(116.0)
            .menu_width_scaled(150.0)
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
            .label(cur.label())
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Micro)
            .trigger_width_scaled(96.0)
            .menu_width_scaled(130.0)
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
            .trigger_width_scaled(30.0)
            .menu_width_scaled(160.0)
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
