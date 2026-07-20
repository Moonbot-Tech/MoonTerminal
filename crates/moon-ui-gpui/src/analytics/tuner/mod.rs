//! The "Strategies" tab of the "Analytics" window — the strategy analysis workspace.
//! The list is always on the left (comparison by ID: trades/WR/profit/avg/PF/best/worst,
//! its own scroll), the modes are buttons in the list header (default — "Filters"):
//! - "Filters" — the threshold tuner on the right (KPI Fact vs v1/v2 + a from/to grid)
//!   SCOPED to the selected strategy, with the field histogram pinned at the bottom;
//! - "Coins" — a per-coin table on the right for the selected strategy (or all trades).
//!
//! The whole "Strategy tuning" page. This root module keeps the selection state (anchor +
//! Ctrl multi-select), the mode dispatcher and the per-coin table; everything else lives in
//! submodules: `list` (the strategy list — filter bar, sort, column selector, table),
//! `columns` (the comparison-table descriptors and cells), `save` (the write-confirmation
//! dialog), `state` (state + `TunerKind`), `shell` (the shared toolbar/suggest row),
//! `fields`/`actions`/`hist` — the "By filter" axis, `time`/`grid`/`sliders` — "By time".

// Submodules of the tuning page.
mod actions;
mod columns;
mod fields;
mod grid;
mod hist;
mod list;
/// The write-confirmation dialog (assemble → render → execute), shared by both axes.
mod save;
mod shell;
mod sliders;
mod state;
mod time;

// State types held by `AnalyticsView` (the parent).
pub(super) use grid::TimeTunerState;
pub(super) use state::TunerState;

// Column descriptors of the comparison tables — re-exported so the submodules (`list`) take
// them via the usual `super::…` instead of reaching into `columns` directly. Visibility is
// exactly `tuner`.
/// The default visible-column mask is read by the parent too (`analytics::mod`, when it
/// creates the view).
pub(super) use columns::STRAT_COLS_ALL;
pub(in crate::analytics::tuner) use columns::{
    COIN_COLS, COIN_PANEL_W, COIN_ROW_GAP, COIN_ROW_PAD_X, COL_BIT_CORE, COL_BIT_KIND,
    COL_BIT_LASTEDIT, LASTEDIT_W, METRIC_COLS, SORT_CORE, SORT_KIND, SORT_LASTEDIT, SORT_NAME,
    head_cell, metric_bit, metric_cell,
};
// The row cap for both comparison tables lives with the list — the coin table takes the same.
use list::MAX_ROWS;

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::AnalyticsView;
use super::LoadState;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::GroupStat;

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

impl AnalyticsView {
    /// Change the selected strategy: detail + tuner scope.
    fn set_sel_strategy(&mut self, sel: Option<(String, String)>, cx: &mut Context<Self>) {
        self.sel_strategy = sel;
        // Retain ready detail while another strategy loads to avoid a loading
        // flash; deselection or a completed non-data result clears it.
        self.reload_detail(cx);
        // The tuner/profile scope changed — the old computations (suggest included) are wrong.
        self.tuner.invalidate();
        // The "By time" schedule grid is PER strategy: reset it so the previous strategy's
        // v1 does not leak onto the new one (otherwise Save would write foreign values).
        self.time_tuner.reset_grid();
        self.time_dirty = true;
        if self.strat_mode == StratMode::Filters {
            self.reload_tuner(cx);
            self.reload_hist(cx);
        }
        if self.strat_mode == StratMode::Time {
            self.reload_time(cx);
        }
        cx.notify();
    }

    /// All currently selected strategies as write targets: the anchor first, then the
    /// Ctrl-selected extras. Each key `strategyid@core_uid` → `(sid, core, name)`.
    fn selected_targets(&self) -> Vec<state::SaveTarget> {
        let mut out = Vec::new();
        let mut push = |key: &str, name: &str| {
            if let Some((sid, core)) = parse_strat_key(key) {
                out.push(state::SaveTarget {
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

    /// Multi-select active — Ctrl added extra strategies beyond the anchor.
    pub(super) fn is_multi(&self) -> bool {
        !self.sel_extra.is_empty()
    }

    /// THE rule for every "in strategy" / "now" current-value display: a per-strategy current
    /// value is meaningful only when exactly ONE strategy is selected. With several selected
    /// each has its own value, so this returns `None` and callers render a neutral "varies" /
    /// blank instead of the anchor's (which would misrepresent the rest). Both tuning axes and
    /// the save dialog go through this so the behavior can't drift between them.
    pub(super) fn current_if_single<T>(&self, value: T) -> Option<T> {
        (!self.is_multi()).then_some(value)
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
                // (which wipes the tuned schedule) does not fire.
                self.sel_extra.clear();
                cx.notify();
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
        cx.notify();
    }

    /// Change strategy mode and refresh dirty tuner data when entering Filters.
    fn set_strat_mode(&mut self, mode: StratMode, cx: &mut Context<Self>) {
        if self.strat_mode == mode {
            return;
        }
        self.strat_mode = mode;
        if mode == StratMode::Filters && self.tuner.needs_reload() {
            self.reload_tuner(cx);
            self.reload_hist(cx);
        }
        if mode == StratMode::Time && (self.time_profiles.is_none() || self.time_dirty) {
            self.reload_time(cx);
        }
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
        // Left half: the list; in "Filters" a pinned histogram sits below it,
        // in "Overview" — the per-coin contribution. The right column (Filters/Coins)
        // spans the FULL height of the tab.
        let list_card = self.strat_list_card(p, window, cx);
        let mut left = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .gap(design::ui_px(cx, 8.0))
            .child(list_card);
        match mode {
            StratMode::Filters => left = left.child(self.hist_card(p, cx)),
            StratMode::Coins => {}
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
                main = main.child(self.strat_coins_table(p, cx));
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

    /// The "Coins" right panel: a per-coin table for the selected strategy (or all trades).
    fn strat_coins_table(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        // The selected-strategy detail and the all-strategies summary each keep
        // their own load note so either read failure remains visible.
        let scale = design::font_scale(cx);
        let (coins, scope): (Result<Vec<GroupStat>, super::Note>, String) = match &self.sel_strategy
        {
            Some((_, name)) => (
                self.detail
                    .view(|d| d.coins.is_empty())
                    .map(|d| d.coins.clone()),
                name.clone(),
            ),
            None => (
                self.data
                    .view(|d| d.coins.is_empty())
                    .map(|d| d.coins.clone()),
                t!("analytics.strat.scope_all").to_string(),
            ),
        };
        let head = h_flex()
            .w_full()
            .flex_none()
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .px(design::ui_px(cx, COIN_ROW_PAD_X))
            .gap(design::ui_px(cx, COIN_ROW_GAP))
            .items_center()
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            .child(div().flex_1().child(t!("analytics.col.coin").to_string()))
            .children(COIN_COLS.iter().map(|c| head_cell(c, scale)));

        let body: AnyElement = match coins {
            Err(note) => super::note_el("an-strat-coins-note", note, 10.0, p, cx),
            Ok(coins) => {
                let mut list = v_flex().w_full();
                for c in coins.iter().take(MAX_ROWS) {
                    list = list.child(
                        h_flex()
                            .w_full()
                            .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
                            .px(design::ui_px(cx, COIN_ROW_PAD_X))
                            .gap(design::ui_px(cx, COIN_ROW_GAP))
                            .items_center()
                            .border_t_1()
                            .border_color(moon_alpha(p.border, 0.5))
                            .child(div().flex_1().min_w_0().truncate().child(c.name.clone()))
                            .children(COIN_COLS.iter().map(|col| metric_cell(col, c, p, scale))),
                    );
                }
                list.into_any_element()
            }
        };

        v_flex()
            .w(design::font_w_px(cx, COIN_PANEL_W))
            .flex_none()
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
                            .child(t!("analytics.tab.coins").to_string()),
                    )
                    .child(
                        div()
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .min_w_0()
                            .truncate()
                            .child(scope),
                    ),
            )
            .child(head)
            .child(
                div()
                    .id("an-strat-coins")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .into_any_element()
    }
}

impl AnalyticsView {
    /// Background detail load for the selected strategy (moved out of `analytics::mod` —
    /// page logic belongs with its own page). Scope — the filters + the selected row.
    pub(super) fn reload_detail(&mut self, cx: &mut Context<Self>) {
        let Some((key, _)) = self.sel_strategy.clone() else {
            self.detail = LoadState::default();
            return;
        };
        // The key `strategyid@core_uid` — detail for the strategy ON THAT CORE.
        let Some((id, core)) = parse_strat_key(&key) else {
            self.detail = LoadState::default();
            return;
        };
        self.detail_seq = self.detail_seq.wrapping_add(1);
        let req = self.detail_seq;
        let mut q = self.query();
        q.strat_core = core;
        self.detail.begin();
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let detail = executor
                .spawn(async move { moon_core::db::analytics::strategy_detail(&q, id) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.detail_seq != req {
                        return;
                    }
                    this.detail.apply(detail);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Reload the data of the ACTIVE tuning axis (after writing to a strategy — refresh
    /// its chips/KPI: for "By time" — `reload_time`, otherwise — `reload_tuner`).
    pub(super) fn reload_active_tuner(&mut self, cx: &mut Context<Self>) {
        match self.strat_mode {
            StratMode::Time => self.reload_time(cx),
            _ => self.reload_tuner(cx),
        }
    }
}
