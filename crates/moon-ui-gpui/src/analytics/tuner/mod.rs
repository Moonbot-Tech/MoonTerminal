//! The "Strategies" tab of the "Analytics" window — the strategy analysis workspace.
//! The list is always on the left (comparison by ID: trades/WR/profit/avg/PF/best/worst,
//! its own scroll), the modes are buttons in the list header (default — "Filters"):
//! - "Filters" — the threshold tuner on the right (KPI Fact vs v1/v2 + a from/to grid)
//!   SCOPED to the selected strategy, with the field histogram pinned at the bottom;
//! - "Coins" — the coin table UNDER the list, with the "Fact vs picked coins" KPI and the
//!   coin picker on the right.
//!
//! The whole "Strategy tuning" page. This root module keeps the selection state (anchor +
//! Ctrl multi-select) and the mode dispatcher; everything else lives in submodules. Shared by
//! every axis: `list` (the strategy list — filter bar, sort, column selector, table),
//! `columns` + `strat_columns` (the comparison-table descriptors and cells), `kpi` (the
//! "Fact vs variants" matrix), `save` (the write-confirmation dialog), `shared` (`TunerKind`,
//! the write targets, the two common widgets), `shell` (the toolbar and suggestion row). One
//! folder per axis: `filter/` — "By filter", `time/` — "By time", `coins/` — "By coin".

// The page splits three ways: what EVERY axis uses sits here at the root, and each axis owns a
// folder. A helper that only one axis calls belongs in that folder — kept at the root it stops
// being reviewable as shared, and the next axis inherits a dependency nobody chose.

// ————— shared by every axis —————
/// Its column descriptors: the metric pool both comparison tables draw from …
mod columns;
/// The "Fact vs variants" KPI matrix, rendered by every axis out of plain `VarStats`.
mod kpi;
/// The strategy list — it stands beside all three axes, so it is not any one of them.
mod list;
/// The write-confirmation dialog (assemble → render → execute).
mod save;
/// The axis tag, the write targets, and the two widgets all three axes draw with.
mod shared;
/// The common toolbar and suggestion row, dispatched by `TunerKind`.
mod shell;
/// … and the subset plus visibility bits that belong to the strategy list itself.
mod strat_columns;

// ————— one folder per axis —————
/// "By coin": the coin table, its row cache, its loads and its working coin lists.
mod coins;
/// "By filter": the threshold grid, its histogram and its auto-suggestion.
mod filter;
/// "By time": the weekly schedule grid, the hour profile and the sliders.
mod time;

// State types held by `AnalyticsView` (the parent).
pub(super) use coins::picker::CoinListsState;
pub(super) use coins::state::CoinsState;
pub(super) use filter::state::TunerState;
pub(super) use list::StratListFilter;
pub(super) use time::state::TimeTunerState;

// Column descriptors of the comparison tables — re-exported so the submodules (`list`) take
// them via the usual `super::…` instead of reaching into `columns` directly. Visibility is
// exactly `tuner`.
pub(in crate::analytics::tuner) use coins::columns::{
    COIN_COLS, COIN_DEFAULT_SORT, COIN_NAME_MIN_W, COIN_ROW_GAP, COIN_ROW_PAD_X, COIN_TICK_W,
};
pub(in crate::analytics::tuner) use columns::{metric_cell, sort_arrow_of, toggle_sort_key};
use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;
pub(in crate::analytics::tuner) use strat_columns::{
    COL_BIT_CORE, COL_BIT_KIND, COL_BIT_LASTEDIT, CORE_MIN_W, CORE_W, CORE_W_MAX, KIND_MIN_W,
    KIND_W, LASTEDIT_MIN_W, LASTEDIT_W, METRIC_COLS, SORT_CORE, SORT_KIND, SORT_LASTEDIT,
    SORT_NAME, STRAT_NAME_MIN_W, metric_bit,
};
/// The default visible-column mask is read by the parent too (`analytics::mod`, when it
/// creates the view).
pub(super) use strat_columns::{STRAT_COLS_ALL, STRAT_COLS_DEFAULT, STRAT_COLS_DEFAULT_COINS};

use super::AnalyticsView;
use crate::design;
use moon_core::config::layout::StratColsByMode;

/// Parse a strategy-list row key `"strategyid@core_uid"` (the list is split PER CORE)
/// → `(strategyid, Some(core_uid))`. Legacy without a core (`"5"`) → `(5, None)`;
/// a non-numeric key (not a strategy) → `None`.
pub(super) fn parse_strat_key(key: &str) -> Option<(i64, Option<u64>)> {
    match key.split_once('@') {
        Some((sid, core)) => Some((sid.parse().ok()?, core.parse::<u64>().ok())),
        None => Some((key.parse().ok()?, None)),
    }
}

/// Right-panel mode of the "Strategy tuning" tab (default — "By filter"):
/// by filter (thresholds), by coin (table), by time (the "hour of day" profile).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StratMode {
    Filters,
    Coins,
    Time,
}

/// Every axis, for the code that has to touch all of them (seeding the per-axis column masks).
///
/// A fourth axis added to `StratMode` without a slot in `cols_slot` fails to compile, and
/// without an entry here fails this array's length — so neither can be forgotten silently.
pub(super) const STRAT_MODES: [StratMode; 3] =
    [StratMode::Filters, StratMode::Coins, StratMode::Time];

impl StratMode {
    /// This axis' slot in the persisted per-axis column masks.
    ///
    /// Reading and writing go through the SAME accessor, so an axis cannot end up saving into
    /// one slot and restoring from another.
    pub(super) fn cols_slot(self, m: &mut StratColsByMode) -> &mut u16 {
        match self {
            StratMode::Filters => &mut m.filter,
            StratMode::Coins => &mut m.coins,
            StratMode::Time => &mut m.time,
        }
    }

    /// What this axis shows before the user chooses: only "By coin" spends width on the
    /// strategy's coin-list counts, because only there are they the subject.
    pub(super) fn default_cols(self) -> u16 {
        match self {
            StratMode::Coins => STRAT_COLS_DEFAULT_COINS,
            _ => STRAT_COLS_DEFAULT,
        }
    }
}

impl AnalyticsView {
    /// Change the selected strategy: detail + tuner scope.
    fn set_sel_strategy(&mut self, sel: Option<(String, String)>, cx: &mut Context<Self>) {
        self.sel_strategy = sel;
        // The tuner/profile scope changed — the old computations (suggest included) are wrong.
        self.tuner.invalidate();
        // The "By time" schedule grid is PER strategy: reset it so the previous strategy's
        // v1 does not leak onto the new one (otherwise Save would write foreign values).
        self.time_tuner.reset_grid();
        self.time_tuner.dirty = true;
        // The coin lists were edited against the PREVIOUS strategy; carried over they would
        // read as "this strategy's coins". `invalidate` retires them along with the numbers.
        self.coins.invalidate();
        self.coin_lists.invalidate();
        self.reload_axis(self.strat_mode, cx);
        cx.notify();
    }

    /// All currently selected strategies as write targets: the anchor first, then the
    /// Ctrl-selected extras. Each key `strategyid@core_uid` → `(sid, core, name)`.
    fn selected_targets(&self) -> Vec<shared::SaveTarget> {
        let mut out = Vec::new();
        let mut push = |key: &str, name: &str| {
            if let Some((sid, core)) = parse_strat_key(key) {
                out.push(shared::SaveTarget {
                    sid,
                    core,
                    name: name.to_string(),
                });
            }
        };
        if let Some((k, n)) = &self.sel_strategy {
            push(k, n);
        }
        for (k, n) in &self.sel_extra {
            push(k, n);
        }
        out
    }

    /// The shared filters WITHOUT the strategy scope — the query for questions asked ABOUT
    /// strategies rather than within a chosen set ("who traded this coin?").
    pub(in crate::analytics::tuner) fn tuner_query_all(&self) -> moon_core::db::analytics::Query {
        self.query()
    }

    /// Observation channel only (`MOON_ANALYTICS_PROBE=select`): adopt the first strategy
    /// that actually carries a coin list, so the "By coin" panels can be read in their
    /// loaded state without a human clicking a row. Never called in a normal run.
    ///
    /// Goes through `set_sel_strategy` rather than assigning the field, so the probe drives
    /// the SAME path a click does — a shortcut here would observe a state the app cannot
    /// otherwise reach.
    /// Returns whether it adopted one, so the caller can skip its own reload: adopting a
    /// strategy starts that pass already, and running both leaves two full scans racing.
    pub(super) fn probe_select_first(&mut self, cx: &mut Context<Self>) -> bool {
        if self.sel_strategy.is_some() {
            return false;
        }
        // Addressed by the row KEY — `strategyid@core_uid`, the identity the whole page uses
        // — not by name: a name is a label, and the same one routinely sits on several cores
        // with entirely different lists, so naming one picks whichever copy comes first.
        // A bare `strategyid` is accepted too and takes the first core carrying it.
        let want = super::probe_select_spec().unwrap_or("");
        let pick = self.data.data().and_then(|d| {
            if want.is_empty() {
                // Nothing addressed: take the BIGGEST blacklist, since the unnamed form
                // exists to exercise the field's ordering and shading at once, and a
                // strategy holding one coin observes neither.
                d.strategies
                    .iter()
                    .filter(|g| g.bl > 0)
                    .max_by_key(|g| g.bl)
            } else {
                // The exact key first; failing that, the strategy id on any core. Taken as
                // asked, empty list included — putting the "this list is empty" state on
                // screen is one of the reasons to address a specific strategy.
                d.strategies.iter().find(|g| g.key == want).or_else(|| {
                    d.strategies
                        .iter()
                        .find(|g| g.key.split('@').next() == Some(want))
                })
            }
            .map(|g| (g.key.clone(), g.name.clone()))
        });
        match pick {
            Some(sel) => {
                self.set_sel_strategy(Some(sel), cx);
                true
            }
            None => false,
        }
    }

    /// Multi-select active — Ctrl added extra strategies beyond the anchor.
    pub(super) fn is_multi(&self) -> bool {
        !self.sel_extra.is_empty()
    }

    /// Label for "what these numbers cover", used by every card title on the page.
    ///
    /// The numbers are computed over the WHOLE selection (`tuner_query`), so naming the
    /// anchor while several rows are highlighted would assert one strategy over N
    /// strategies' figures — a count is the only honest label there.
    pub(super) fn scope_label(&self) -> String {
        let n = self.selected_targets().len();
        match self.sel_strategy.as_ref() {
            None => t!("analytics.strat.scope_all").to_string(),
            Some((_, name)) if n <= 1 => name.clone(),
            _ => t!("analytics.strat.scope_many", n = n).to_string(),
        }
    }

    /// THE rule for every "in strategy" / "now" current-value display: a per-strategy current
    /// value is meaningful only when exactly ONE strategy is selected. With several selected
    /// each has its own value, so this returns `None` and callers render a neutral "varies" /
    /// blank instead of the anchor's (which would misrepresent the rest). Both tuning axes and
    /// the save dialog go through this so the behavior can't drift between them.
    pub(super) fn current_if_single<T>(&self, value: T) -> Option<T> {
        (!self.is_multi()).then_some(value)
    }

    /// The SET of selected strategies changed while the anchor stayed put (Ctrl added or
    /// removed a row). Every number on the page — the KPI matrix, the histogram, the time
    /// profile — is computed over that set by `tuner_query`, so all of it is now stale.
    ///
    /// Deliberately NOT `set_sel_strategy`: that resets the schedule grid, and multi-select
    /// exists precisely to tune values once and write them to many strategies. The anchor's
    /// own coin detail does not change either.
    fn selection_scope_changed(&mut self, cx: &mut Context<Self>) {
        self.tuner.invalidate();
        // A bare `dirty` write, NOT `time_tuner.invalidate()`: this path has never retired
        // an in-flight time suggestion (unlike the filter axis' `invalidate` above), and the
        // refactor keeps that behavior rather than silently changing it.
        self.time_tuner.dirty = true;
        // The coin table's numbers AND its lists are scoped to the whole selection, so
        // adding or removing a strategy retires both — including any unsaved tick, whose
        // baseline (the union of the selected strategies' saved lists) just changed.
        self.coins.invalidate();
        self.coin_lists.invalidate();
        // The write banner speaks about an edit that was kept for a retry. `invalidate` has
        // just thrown that edit away with the scope it belonged to, so the banner would go on
        // promising something recoverable that no longer exists.
        self.write_error = None;
        self.reload_axis(self.strat_mode, cx);
        cx.notify();
    }

    /// Plain click: single-select this row (clear the multi-set). Clicking the current anchor
    /// collapses a multi-select to just that anchor WITHOUT re-scoping (keeps its tuned v1);
    /// clicking the sole current selection clears it (toggle off).
    fn select_single(&mut self, key: String, name: String, cx: &mut Context<Self>) {
        let is_anchor = self.sel_strategy.as_ref().is_some_and(|(k, _)| *k == key);
        if is_anchor {
            if self.sel_extra.is_empty() {
                self.set_sel_strategy(None, cx); // sole selection → toggle off
            } else {
                // Anchor unchanged: drop the extras only, so set_sel_strategy's scope reset
                // (which wipes the tuned schedule) does not fire. The scope still NARROWED
                // from N strategies to one, so the numbers have to be recomputed.
                self.sel_extra.clear();
                self.selection_scope_changed(cx);
            }
        } else {
            self.sel_extra.clear();
            self.set_sel_strategy(Some((key, name)), cx);
        }
    }

    /// Ctrl click: toggle this row in/out of the multi-set. The anchor (`sel_strategy`) drives
    /// scope/suggest/detail; the extras are bulk-write addressees only. Removing the anchor
    /// promotes the first extra (re-scoping to it), or clears the whole selection.
    fn toggle_multi(&mut self, key: String, name: String, cx: &mut Context<Self>) {
        match &self.sel_strategy {
            None => {
                self.set_sel_strategy(Some((key, name)), cx);
                return;
            }
            Some((ak, _)) if *ak == key => {
                if self.sel_extra.is_empty() {
                    self.set_sel_strategy(None, cx);
                } else {
                    let (nk, nn) = self.sel_extra.remove(0);
                    self.set_sel_strategy(Some((nk, nn)), cx);
                }
                return;
            }
            _ => {}
        }
        if let Some(pos) = self.sel_extra.iter().position(|(k, _)| *k == key) {
            self.sel_extra.remove(pos);
        } else {
            self.sel_extra.push((key, name));
        }
        // The selected SET just changed — recompute over it, or the page would keep showing
        // the previous set's numbers under the new highlight.
        self.selection_scope_changed(cx);
    }

    /// Change strategy mode and refresh the entered axis' data when it is stale.
    fn set_strat_mode(&mut self, mode: StratMode, cx: &mut Context<Self>) {
        if self.strat_mode == mode {
            return;
        }
        self.strat_mode = mode;
        self.reload_axis_if_stale(mode, cx);
        cx.notify();
    }

    /// The tab body: it divides the height itself (the window's outer scroll is off) —
    /// the bottom bar is always on screen.
    pub(super) fn strategies_tab(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mode = self.strat_mode;
        // "By time" has a layout of its own (list + map + KPI + schedule grid).
        if mode == StratMode::Time {
            return self.strat_time(p, window, cx);
        }
        // Left half: the list, with the axis' own detail pinned below it — the field
        // histogram in "Filters", the coin table in "Coins". The right column spans the
        // FULL height of the tab.
        let list_card = self.strat_list_card(p, window, cx);
        // "Coins" builds its table before the immutable reads of the right column
        // (the lazy search input needs &mut self).
        let coins_card = (mode == StratMode::Coins).then(|| self.coins_card(p, window, cx));
        let mut left = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .gap(design::ui_px(cx, 8.0))
            .child(list_card);
        match mode {
            StratMode::Filters => left = left.child(self.hist_card(p, cx)),
            // The coin table sits UNDER the list, where the histogram sits in "Filters":
            // both are "the detail behind the selected strategy".
            StratMode::Coins => left = left.children(coins_card),
            StratMode::Time => unreachable!("Time mode returns early above"),
        }

        let mut main = h_flex()
            .size_full()
            .p(design::ui_px(cx, 10.0))
            .gap(design::ui_px(cx, 8.0))
            .child(left);
        match mode {
            StratMode::Filters => {
                // KPI is pinned at the top; ONLY the thresholds container scrolls.
                main = main.child(
                    v_flex()
                        .w(design::font_w_px(cx, 470.0))
                        .flex_none()
                        .h_full()
                        .min_h_0()
                        .gap(design::ui_px(cx, 8.0))
                        .child(self.kpi_matrix(p, cx))
                        // fields_grid fills the column and scrolls its rows internally.
                        .child(self.fields_grid(p, window, cx)),
                );
            }
            StratMode::Coins => {
                // Same right column as every other axis: the shared "Fact vs variants"
                // matrix on top, the axis' own tool below it — here the list field, which
                // now owns the whole remaining height.
                let pick = self.coins_field_card(p, cx);
                main = main.child(
                    v_flex()
                        .w(design::font_w_px(cx, 470.0))
                        .flex_none()
                        .h_full()
                        .min_h_0()
                        .gap(design::ui_px(cx, 8.0))
                        .child(self.coins_kpi(p, cx))
                        .child(pick),
                );
            }
            StratMode::Time => unreachable!("Time mode returns early above"),
        }
        // The save confirmation window — an overlay on top of the tab.
        let dialog = self.save_dialog_overlay(p, cx);
        div()
            .relative()
            .size_full()
            .child(main)
            .children(dialog)
            .into_any_element()
    }
}

impl AnalyticsView {
    /// Fully reload one axis' data — the dispatch every scope change (selection, filters,
    /// period) goes through, so the "which reloads belong to which axis" list exists once.
    ///
    /// Exhaustive on purpose — no wildcard. A fourth axis must fail to compile here
    /// instead of silently reloading the filter tuner's data under its own name.
    pub(super) fn reload_axis(&mut self, mode: StratMode, cx: &mut Context<Self>) {
        match mode {
            StratMode::Filters => {
                self.reload_tuner(cx);
                self.reload_hist(cx);
            }
            StratMode::Time => self.reload_time(cx),
            StratMode::Coins => self.reload_coins(cx),
        }
    }

    /// Reload an axis only when its data is stale — the dispatch for ENTERING an axis
    /// (a mode button, a tab switch): fresh data stays, a stale axis recomputes.
    pub(super) fn reload_axis_if_stale(&mut self, mode: StratMode, cx: &mut Context<Self>) {
        let stale = match mode {
            StratMode::Filters => self.tuner.needs_reload(),
            StratMode::Time => self.time_tuner.needs_reload(),
            StratMode::Coins => self.coins.needs_reload(),
        };
        if stale {
            self.reload_axis(mode, cx);
        }
    }

    /// Reload the data of the ACTIVE tuning axis after WRITING to a strategy — refresh
    /// its chips/KPI. Deliberately NOT `reload_axis`: a write changes no past trades, so
    /// the filter histogram (a distribution over them) is left alone.
    pub(super) fn reload_active_tuner(&mut self, cx: &mut Context<Self>) {
        match self.strat_mode {
            StratMode::Filters => self.reload_tuner(cx),
            StratMode::Time => self.reload_time(cx),
            StratMode::Coins => self.reload_coins(cx),
        }
    }
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests {
    use super::{STRAT_MODES, StratMode};
    use moon_core::config::layout::StratColsByMode;

    /// Each axis must address its OWN slot. Two axes sharing one is the copy-paste that makes
    /// the whole per-axis layout pointless — and it would look like "my columns keep changing
    /// when I switch tabs", which is exactly what this feature exists to stop.
    #[test]
    fn each_axis_owns_its_column_slot() {
        let mut cols = StratColsByMode::default();
        for (i, mode) in STRAT_MODES.into_iter().enumerate() {
            *mode.cols_slot(&mut cols) = i as u16 + 1;
        }
        assert_eq!((cols.filter, cols.coins, cols.time), (1, 2, 3));
        // And reading back returns what that axis wrote, not a neighbour's.
        for (i, mode) in STRAT_MODES.into_iter().enumerate() {
            assert_eq!(*mode.cols_slot(&mut cols), i as u16 + 1);
        }
    }

    /// Only the coin axis spends width on the coin-list columns — that difference is the
    /// reason the mask is per axis at all.
    #[test]
    fn coin_axis_defaults_to_showing_the_lists() {
        assert_ne!(
            StratMode::Coins.default_cols(),
            StratMode::Filters.default_cols()
        );
        assert_eq!(
            StratMode::Filters.default_cols(),
            StratMode::Time.default_cols()
        );
    }
}
