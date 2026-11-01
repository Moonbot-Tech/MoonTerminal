//! Strategy-list controls for the hand-built comparison table: the filter bar (name search,
//! strategy-type dropdown, "enabled only" toggle), the click-to-sort helpers, and the visible-
//! column selector (mirrors the Orders `columns_menu` bitmask pattern). All view-only — the
//! filters/sort/columns never change what is written, only what the list shows.
//! The table itself (card, rows, sortable header) renders in `table`.

/// Numeric conditions and their window-lifetime editor.
mod conditions;
/// The strategy row's right-click menu and the gate deciding whether its action can run.
mod menu;
pub(in crate::analytics) use conditions::Conditions;
/// Pure selection arithmetic: the drawn row order and the Shift-click range over it.
mod select;
/// The list card, its rows and the sortable header row.
mod table;

#[cfg(test)]
mod tests;

use gpui::*;
use moon_ui::{
    MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonDropdown, MoonInput, MoonInputEvent,
    MoonInputState, MoonMenuItem, MoonPalette, h_flex,
};
use rust_i18n::t;
use std::cmp::Ordering;

pub(in crate::analytics::tuner) use select::{
    RangeOutcome, RowClick, drawn_order, inclusive_report_bounds, range_extras, row_click_intent,
};

use super::super::AnalyticsView;
use super::auto_picks::{AutoFilter, AutoView};
use super::ready::{ReadyFilter, ReadyView};
use super::{
    COL_BIT_AUTO, COL_BIT_CORE, COL_BIT_KIND, COL_BIT_LASTEDIT, COL_BIT_READY, METRIC_COLS,
    SORT_AUTO, SORT_CORE, SORT_KIND, SORT_LASTEDIT, SORT_NAME, SORT_READY, STRAT_COLS_ALL,
    STRAT_SORT_DEFAULT, StratMode, metric_bit, sort_arrow_of, toggle_sort_key,
};
use crate::design;
use crate::design::moon;
use moon_core::db::ProfitScope;
use moon_core::db::analytics::{GroupStat, StrategyBase};

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
pub(in crate::analytics::tuner) const MAX_ROWS: usize = 500;

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
                SORT_NAME | SORT_KIND | SORT_CORE | SORT_LASTEDIT | SORT_AUTO | SORT_READY
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
    /// Editable numeric conditions shared across all modes, including invalid drafts.
    conditions: Vec<conditions::Condition>,
    /// Cell formatters use this frame's profit lens and precision.
    percent: bool,
    sort: Option<(String, bool)>,
    /// The batch auto-pick's classes and the "Auto-pick" filter.
    auto: AutoView,
    /// The readiness pass's classes and the "Tuning" filter.
    ready: ReadyView,
}

/// The memoized row order, enabled-filter availability and inputs that produced them.
pub(in crate::analytics) struct VisibleRows {
    key: VisibleKey,
    idx: Vec<usize>,
    enabled_filter_available: bool,
    /// Rows passing every filter but the auto-pick and tuning ones: the counter's "of M", so
    /// turning either on cannot move the denominator.
    pub(in crate::analytics) pool: usize,
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

/// Filter the whole group set and order its drawable head, returning indices into it.
///
/// Indices rather than references so the result can be cached on the view: a `Vec<&GroupStat>`
/// would borrow from `strategy_data` and make the cache self-referential.
///
/// The list-membership and aliveness fallbacks preserve visible rows when the strategies replica
/// does not provide enough information to evaluate those filters.
///
/// Args:
///     all: Complete strategy group set.
///     key: Search, filters, sort, and visible-head limit.
///
/// Returns:
///     Stable display-order indices through [`MAX_ROWS`], an unordered undisplayed tail,
///     whether replica status is available, and how many rows passed before the auto-pick
///     and tuning filters.
pub(in crate::analytics) fn filter_sort_indices(
    all: &[GroupStat],
    key: &VisibleKey,
) -> (Vec<usize>, bool, usize) {
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
    // Apply enabled-only ONLY when aliveness is actually known (at least one row has it). If
    // every row is alive = None (strategies replica absent), the filter has nothing to go on
    // and would blank the list — so skip it. When alive IS known, filter normally; a genuinely
    // all-disabled or all-deleted set then empties and the caller shows a "no matches" note.
    let enabled_filter_available = base.iter().any(|i| all[*i].alive.is_some());
    let mut out: Vec<usize> = if key.active_only && enabled_filter_available {
        base.into_iter()
            .filter(|i| all[*i].alive == Some(2))
            .collect()
    } else {
        base
    };
    let pool = out.len();
    out.retain(|i| conditions::keeps(&all[*i], &key.conditions));
    if key.auto.filter != AutoFilter::All {
        out.retain(|i| key.auto.filter.keeps(key.auto.class(&all[*i].key)));
    }
    if key.ready.filter != ReadyFilter::All {
        out.retain(|i| key.ready.filter.keeps(key.ready.class(&all[*i].key)));
    }
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
        if sort_key == SORT_AUTO {
            // Descending (the first click) puts confirmed findings first, then noise with
            // candidates, then the rest; a bigger finding leads its own group.
            partial_sort(&mut out, |a, b| {
                let (x, y) = (key.auto.class(&all[*a].key), key.auto.class(&all[*b].key));
                let o = x.rank().cmp(&y.rank()).then(y.size().cmp(&x.size()));
                let o = if desc { o } else { o.reverse() };
                o.then_with(|| all[*a].key.cmp(&all[*b].key))
            });
        } else if sort_key == SORT_READY {
            // Descending (the first click) puts the tunable strategies first, the bigger
            // sample leading, then the ones closest to it.
            partial_sort(&mut out, |a, b| {
                let (x, y) = (key.ready.class(&all[*a].key), key.ready.class(&all[*b].key));
                let o = x.rank().cmp(&y.rank()).then(y.size().cmp(&x.size()));
                let o = if desc { o } else { o.reverse() };
                o.then_with(|| all[*a].key.cmp(&all[*b].key))
            });
        } else if let Some(col) = METRIC_COLS.iter().find(|c| c.key == sort_key) {
            let f = col.sort;
            partial_sort(&mut out, |a, b| {
                let (left, right) = (f(&all[*a]), f(&all[*b]));
                // Mixed/unknown quote groups publish NaN for raw-money columns. Keep those
                // unavailable rows at the bottom in BOTH directions instead of letting the
                // key tie-break scatter them through valid comparable values.
                let o = match (left.is_finite(), right.is_finite()) {
                    (false, false) => Ordering::Equal,
                    (false, true) => Ordering::Greater,
                    (true, false) => Ordering::Less,
                    (true, true) => {
                        let numeric = left.partial_cmp(&right).unwrap_or(Ordering::Equal);
                        if desc { numeric.reverse() } else { numeric }
                    }
                };
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
    (out, enabled_filter_available, pool)
}

/// Offer the conditions reset only when it would restore rows under every other active filter.
/// The caller already knows the current list is empty. A running classifier keeps its computing
/// explanation even after partial results arrive: an unseen strategy may still match later.
pub(super) fn conditions_clear_restores_rows(
    all: &[GroupStat],
    mut key: VisibleKey,
    computing: bool,
) -> bool {
    if computing {
        return false;
    }
    key.conditions.clear();
    !filter_sort_indices(all, &key).0.is_empty()
}

/// The rows a readiness pass counts first, in order: the list's own filters and sort with every
/// tuning class passing — under the default "can tune" filter a row not counted yet fails, and
/// the drawn list is empty until the pass answers. A sort by the Tuning column itself, which the
/// pass has yet to fill, goes by trade count, the most first.
///
/// Args:
///     all: Complete strategy group set.
///     key: The inputs the list is drawn with.
///
/// Returns:
///     Indices into `all`, the drawable head in the list's order.
pub(in crate::analytics) fn ready_priority_indices(
    all: &[GroupStat],
    mut key: VisibleKey,
) -> Vec<usize> {
    key.ready = ReadyView::default();
    let by_ready = key.sort.as_ref().is_some_and(|(k, _)| k == SORT_READY);
    if by_ready {
        key.sort = None;
    }
    let (mut idx, _, _) = filter_sort_indices(all, &key);
    if by_ready {
        idx.sort_by(|a, b| {
            all[*b]
                .n
                .cmp(&all[*a].n)
                .then_with(|| all[*a].key.cmp(&all[*b].key))
        });
    }
    idx
}

/// The saved strategy sort as one axis applies it: a sort by the Auto column only where that
/// column is drawn ("By coin"), by the Tuning column only on "Entry/Exit". The saved choice
/// itself is kept for the return there.
///
/// Args:
///     sort: The saved sort key and direction.
///     mode: The axis on screen.
///
/// Returns:
///     The sort to apply, `None` for a sort by a column this axis does not draw.
pub(in crate::analytics) fn axis_sort(
    sort: &Option<(String, bool)>,
    mode: StratMode,
) -> Option<(String, bool)> {
    sort.clone()
        .filter(|(k, _)| mode.shows_auto() || k != SORT_AUTO)
        .filter(|(k, _)| mode.shows_ready() || k != SORT_READY)
}

/// Whether a cached result still describes the current inputs.
///
/// Split out so the memo's ONE decision can be exercised directly: "recomputed zero times while
/// nothing changed" is the whole objective, and it is invisible from the rendered output.
fn memo_is_fresh(cached: Option<&VisibleRows>, key: &VisibleKey) -> bool {
    cached.is_some_and(|c| &c.key == key)
}

/// Whether a published group set changes the names measured for the core-column width.
///
/// This matches `core_col_w`'s `cores_n <= 1` filter so the cache invalidates only when its
/// measured text changes; multi-core rows render an aggregate instead of a name. No new group
/// set means no replacement was published, while a missing old set must measure the first one.
pub(in crate::analytics) fn core_names_changed(
    old: Option<&[GroupStat]>,
    new: Option<&[GroupStat]>,
) -> bool {
    let Some(new) = new else {
        return false;
    };
    let Some(old) = old else {
        return true;
    };
    fn names(rows: &[GroupStat]) -> std::collections::HashSet<&str> {
        rows.iter()
            .filter(|g| g.cores_n <= 1)
            .map(|g| g.core.as_str())
            .collect()
    }
    names(old) != names(new)
}

/// Borrow the strategy rows a just-completed base read published, when it published any.
///
/// `Split` carries no comparable group set (only raw per-quote totals), and a failed read
/// carries none at all — both read as "nothing new to measure", not as "the set became empty".
pub(in crate::analytics) fn published_groups(
    result: &moon_core::db::ReadResult<ProfitScope<StrategyBase>>,
) -> Option<&[GroupStat]> {
    match result {
        Ok(ProfitScope::Comparable { data, .. }) | Ok(ProfitScope::Empty(data)) => {
            Some(&data.strategies)
        }
        Ok(ProfitScope::Split(_)) | Err(_) => None,
    }
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
        let key = self.strat_visible_key(all);
        if !memo_is_fresh(self.strat_visible.as_ref(), &key) {
            let (idx, enabled_filter_available, pool) = filter_sort_indices(all, &key);
            self.strat_visible = Some(VisibleRows {
                key,
                idx,
                enabled_filter_available,
                pool,
            });
        }
        self.visible_indices()
    }

    /// The filter-and-sort inputs the list is drawn with now.
    pub(super) fn strat_visible_key(&self, all: &[GroupStat]) -> VisibleKey {
        VisibleKey {
            base: all.as_ptr() as usize,
            search_lower: self.strat_search.trim().to_lowercase(),
            kind: self.strat_type.clone(),
            lists: self.strat_lists,
            active_only: self.strat_active_only,
            conditions: self.strat_conditions.rows.clone(),
            percent: crate::analytics::pnl_is_pct(),
            // A saved sort by the Auto column applies only where that column is drawn; the
            // saved choice itself is kept for the return to "By coin".
            sort: axis_sort(&self.strat_sort, self.strat_mode),
            auto: self.auto_picks.view(self.strat_mode.shows_auto()),
            ready: self.ready.view(self.strat_mode.shows_ready()),
        }
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

    /// Is the column with visibility `bit` currently shown? The Auto column only on "By coin",
    /// the Tuning column only on "Entry/Exit".
    pub(super) fn col_shown(&self, bit: u16) -> bool {
        self.strat_mode.drawn_cols(self.strat_cols()) & bit != 0
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
            b.layout.analytics_strat_cols_modes4 = Some(all);
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
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(val));
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

    /// The list filter bar row: name search, type, enabled-only and column selector.
    /// Missing replica data disables enabled-only without changing the toggle state.
    pub(super) fn strat_filter_bar(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let search = self.strat_search_state(window, cx);
        let data = self.strategy_data.data().cloned();
        let enabled_available = data.is_some_and(|data| {
            self.ensure_visible(&data.strategies);
            self.strat_visible
                .as_ref()
                .is_some_and(|rows| rows.enabled_filter_available)
        });
        h_flex()
            .debug_selector(|| "an-strat-filter".into())
            .w_full()
            .flex_none()
            .flex_wrap()
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 4.0))
            .gap(design::ui_px(cx, 6.0))
            .items_center()
            .bg(moon(p.table_head))
            .child(
                div().flex_1().min_w(design::ui_px(cx, 120.0)).child(
                    MoonInput::new("an-strat-search")
                        .state(&search)
                        .placeholder(t!("analytics.strat.search_ph").to_string())
                        .size(design::INPUT_SIZE),
                ),
            )
            .child(self.strat_type_menu(cx))
            .child(self.strat_lists_menu(cx))
            .children(self.strat_auto_menu(cx))
            .children(self.strat_ready_menu(cx))
            .child(self.conditions_control(p, window, cx))
            .child(
                div()
                    .id("an-strat-active-tip")
                    .font_family(design::ui_font())
                    .tooltip({
                        let text = if enabled_available {
                            t!("analytics.strat.active_only_tip")
                        } else {
                            t!("analytics.strat.active_only_unavailable")
                        };
                        crate::panels::common::text_tooltip(text.to_string())
                    })
                    .child(
                        MoonCheckbox::new("an-strat-active")
                            .checked(self.strat_active_only)
                            .disabled(!enabled_available)
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
                    ),
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
            .trigger_size(MoonButtonSize::density(cx))
            .trigger_width_scaled(116.0)
            .fit_menu_width(150.0, 560.0);
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
    /// selector and the "enabled only" toggle.
    fn strat_lists_menu(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let cur = self.strat_lists;
        let view = cx.entity();
        let mut menu = MoonDropdown::new("an-strat-lists")
            .label(cur.label())
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::density(cx))
            .trigger_width_scaled(96.0)
            .fit_menu_width(130.0, 560.0);
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

    /// The "Auto-pick" filter: all / with a finding / confirmed. Amber while it narrows the
    /// list, like the active axis button, so a short list never looks like missing data.
    fn strat_auto_menu(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let view = cx.entity();
        auto_filter_on_axis(
            self.strat_mode,
            self.auto_filter(),
            std::rc::Rc::new(move |f, app| {
                view.update(app, |this, cx| {
                    this.auto_picks.chosen = Some(f);
                    cx.notify();
                });
            }),
            cx,
        )
    }

    /// The auto-pick filter in force on the axis currently on screen.
    pub(super) fn auto_filter(&self) -> AutoFilter {
        self.auto_picks.filter(self.strat_mode.shows_auto())
    }

    /// The "Tuning" filter: all / can tune. Amber while it narrows the list, like the
    /// auto-pick filter on "By coin".
    fn strat_ready_menu(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let view = cx.entity();
        ready_filter_on_axis(
            self.strat_mode,
            self.ready_filter(),
            std::rc::Rc::new(move |f, app| {
                view.update(app, |this, cx| {
                    this.ready.chosen = Some(f);
                    cx.notify();
                });
            }),
            cx,
        )
    }

    /// The tuning filter in force on the axis currently on screen.
    pub(super) fn ready_filter(&self) -> ReadyFilter {
        self.ready.filter(self.strat_mode.shows_ready())
    }

    /// Visible-column selector ([`design::COLUMN_SELECTOR_ICON`]); each item toggles a column bit,
    /// "All" toggles all.
    /// The menu stays open across clicks; the name column is always shown, so hiding every
    /// toggleable column is allowed (unlike Orders, which locks the last one).
    ///
    /// The trigger carries no label, so it takes a tooltip like the four other pickers do — it
    /// reuses `report.columns_menu`, the same borrowing this menu already does for
    /// `report.filter.all`, rather than adding a fifth dictionary key that says "Columns".
    fn strat_column_menu(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let view = cx.entity();
        let cur = self.strat_cols();
        let mut menu = MoonDropdown::new("an-strat-cols")
            // The shared column-selector asset; the choice and the childless trigger are
            // `design::COLUMN_SELECTOR_ICON`'s contract. This one alone stays MICRO and keeps its
            // own pill width: it sits in the tuner's compact strip, not in a chrome toolbar, so
            // `glyph_btn_w` (the Action preset's 26px height) would not fit the row.
            .trigger_icon(design::COLUMN_SELECTOR_ICON)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::density(cx))
            .trigger_width_scaled(30.0)
            .fit_menu_width(160.0, 560.0)
            .close_on_select(false);
        let all_view = view.clone();
        menu = menu.item(
            MoonMenuItem::with_key("col-all", t!("report.filter.all").to_string())
                .checked(
                    self.strat_mode.drawn_cols(cur) == self.strat_mode.drawn_cols(STRAT_COLS_ALL),
                )
                .on_click(move |_, _, app| {
                    all_view.update(app, |this, cx| {
                        // Judged and toggled over the columns this axis draws; a bit it does
                        // not draw (Auto off "By coin", Tuning off "Entry/Exit") keeps its
                        // saved value.
                        let (mode, cur) = (this.strat_mode, this.strat_cols());
                        let all = mode.drawn_cols(STRAT_COLS_ALL);
                        let drawn = mode.drawn_cols(cur);
                        let next = if drawn == all { 0 } else { all };
                        this.set_strat_cols(next | (cur ^ drawn), cx);
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
        if self.strat_mode.shows_auto() {
            menu = add(menu, COL_BIT_AUTO, t!("analytics.auto.col").to_string());
        }
        if self.strat_mode.shows_ready() {
            menu = add(menu, COL_BIT_READY, t!("analytics.ready.col").to_string());
        }
        div()
            .id("an-strat-cols-tip")
            .tooltip(crate::panels::common::text_tooltip(
                t!("report.columns_menu").to_string(),
            ))
            .child(menu)
    }
}

/// The toolbar's "Auto-pick" filter on one axis: drawn on "By coin" only, absent elsewhere.
///
/// Args:
///     mode: The axis on screen.
///     cur: The active filter.
///     pick: Applies a chosen filter.
///     cx: App, for sizing.
///
/// Returns:
///     The filter element, or `None` off "By coin".
pub(in crate::analytics::tuner) fn auto_filter_on_axis(
    mode: StratMode,
    cur: AutoFilter,
    pick: std::rc::Rc<dyn Fn(AutoFilter, &mut App)>,
    cx: &App,
) -> Option<impl IntoElement + use<>> {
    mode.shows_auto().then(|| auto_filter_el(cur, pick, cx))
}

/// The toolbar's "Tuning" filter on one axis: drawn on "Entry/Exit" only, absent elsewhere.
///
/// Args:
///     mode: The axis on screen.
///     cur: The active filter.
///     pick: Applies a chosen filter.
///     cx: App, for sizing.
///
/// Returns:
///     The filter element, or `None` off "Entry/Exit".
pub(in crate::analytics::tuner) fn ready_filter_on_axis(
    mode: StratMode,
    cur: ReadyFilter,
    pick: std::rc::Rc<dyn Fn(ReadyFilter, &mut App)>,
    cx: &App,
) -> Option<impl IntoElement + use<>> {
    mode.shows_ready().then(|| ready_filter_el(cur, pick, cx))
}

/// The toolbar's "Tuning" filter, as a free function so a layout probe can draw it: amber
/// while it narrows the list, its trigger sized to the current label.
///
/// Args:
///     cur: The active filter.
///     pick: Applies a chosen filter.
///     cx: App, for sizing.
///
/// Returns:
///     The dropdown wrapped in its hover tip (`an-strat-ready-tip`).
pub(in crate::analytics::tuner) fn ready_filter_el(
    cur: ReadyFilter,
    pick: std::rc::Rc<dyn Fn(ReadyFilter, &mut App)>,
    cx: &App,
) -> impl IntoElement + use<> {
    let mut menu = MoonDropdown::new("an-strat-ready")
        .label(t!("analytics.ready.filter_lbl", value = cur.label()).to_string())
        .trigger_caret(true)
        .trigger_variant(if cur == ReadyFilter::All {
            MoonButtonVariant::Soft
        } else {
            MoonButtonVariant::Amber
        })
        .trigger_size(MoonButtonSize::density(cx))
        .fit_trigger_width(96.0, 240.0)
        .fit_menu_width(150.0, 560.0);
    for f in ReadyFilter::ALL {
        let pick = pick.clone();
        menu = menu.item(
            MoonMenuItem::with_key(format!("ready-{}", f.key()), f.label())
                .checked(cur == f)
                .on_click(move |_, _, app| pick(f, app)),
        );
    }
    div()
        .id("an-strat-ready-tip")
        .debug_selector(|| "an-strat-ready-tip".to_string())
        .flex_none()
        .tooltip(crate::panels::common::text_tooltip(
            t!("analytics.ready.filter_tip").to_string(),
        ))
        .child(menu)
}

/// The toolbar's "Auto-pick" filter, as a free function so a layout probe can draw it: amber
/// while it narrows the list, its trigger sized to the current label.
///
/// Args:
///     cur: The active filter.
///     pick: Applies a chosen filter.
///     cx: App, for sizing.
///
/// Returns:
///     The dropdown wrapped in its hover tip (`an-strat-auto-tip`).
pub(in crate::analytics::tuner) fn auto_filter_el(
    cur: AutoFilter,
    pick: std::rc::Rc<dyn Fn(AutoFilter, &mut App)>,
    cx: &App,
) -> impl IntoElement + use<> {
    let mut menu = MoonDropdown::new("an-strat-auto")
        .label(t!("analytics.auto.filter_lbl", value = cur.label()).to_string())
        .trigger_caret(true)
        .trigger_variant(if cur == AutoFilter::All {
            MoonButtonVariant::Soft
        } else {
            MoonButtonVariant::Amber
        })
        .trigger_size(MoonButtonSize::density(cx))
        .fit_trigger_width(96.0, 220.0)
        .fit_menu_width(150.0, 560.0);
    for f in AutoFilter::ALL {
        let pick = pick.clone();
        menu = menu.item(
            MoonMenuItem::with_key(format!("auto-{}", f.key()), f.label())
                .checked(cur == f)
                .on_click(move |_, _, app| pick(f, app)),
        );
    }
    div()
        .id("an-strat-auto-tip")
        .debug_selector(|| "an-strat-auto-tip".to_string())
        .flex_none()
        .tooltip(crate::panels::common::text_tooltip(
            t!("analytics.auto.filter_tip").to_string(),
        ))
        .child(menu)
}
