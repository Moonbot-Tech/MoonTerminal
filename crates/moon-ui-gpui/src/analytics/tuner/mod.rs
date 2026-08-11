//! The "Strategies" tab of the "Analytics" window — the strategy analysis workspace.
//! The list is always on the left (comparison by ID: trades/WR/profit/avg/PF/best/worst,
//! its own scroll), the modes are buttons in the list header (default — "Filters"):
//! - "Filters" — the threshold tuner on the right (KPI Fact vs v1/v2 + a from/to grid)
//!   SCOPED to the selected strategy, with the field histogram pinned at the bottom;
//! - "Coins" — the coin table UNDER the list, with the "Fact vs picked coins" KPI and the
//!   coin picker on the right.
//!
//! The whole "Strategy tuning" page. This root module keeps the selection state (anchor plus
//! Ctrl/Shift multi-selection) and the mode dispatcher; everything else lives in submodules. Shared by
//! every axis: `list` (the strategy list — filter bar, sort, column selector, table),
//! `columns` + `strat_columns` (the comparison-table descriptors and cells), `kpi` (the
//! "Fact vs variants" matrix), `save` (the write-confirmation dialog), `shared` (`TunerKind`,
//! the write targets and common UI helpers), `shell` (the toolbar and suggestion row). One
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
/// The axis tag, write targets, and UI helpers shared by all three axes.
mod shared;
/// The common toolbar and suggestion row, dispatched by `TunerKind`.
mod shell;
/// … and the subset plus visibility bits that belong to the strategy list itself.
mod strat_columns;
pub(in crate::analytics) use strat_columns::restore_strat_columns;

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
pub(super) use list::{StratListFilter, VisibleRows, restore_strat_sort};
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
    SORT_NAME, STRAT_NAME_MIN_W, STRAT_SORT_DEFAULT, metric_bit,
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

/// Return whether a retained strategy row belongs to a caller-provided core scope.
///
/// Args:
///     key: Retained `strategy@core` row identity.
///     workspace: Admitted core ids, or `None` for an unrestricted scope.
///
/// Returns:
///     `true` when the row may participate in the caller's read or action operation.
fn strategy_selection_visible(key: &str, workspace: Option<&[u64]>) -> bool {
    workspace.is_none_or(|cores| {
        parse_strat_key(key)
            .and_then(|(_, core)| core)
            .is_some_and(|core| cores.contains(&core))
    })
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
    /// Open the singleton Report window with this exact strategy and current Analytics filters.
    ///
    /// Args:
    ///     key: Strategy row key in `strategyid@core_uid` form.
    ///     name: Display name retained by the Report selector.
    ///     window: Analytics owner window.
    ///     cx: Analytics context used to open or retarget the Report window.
    ///
    /// Returns:
    ///     Nothing; malformed legacy keys are ignored.
    fn open_strategy_report(
        &self,
        key: &str,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((strategy_id, Some(core_uid))) = parse_strat_key(key) else {
            return;
        };
        let query = self.query();
        let workspace_group = self
            .backend
            .read(cx)
            .singleton_workspace()
            .map(|workspace| workspace.group);
        if !self
            .backend
            .read(cx)
            .workspace_action_allows_core(workspace_group.as_deref(), core_uid)
        {
            return;
        }
        let (date_from, date_to) = list::inclusive_report_bounds(query.from, query.to);
        let owner_display = window.display(cx).map(|display| display.id());
        crate::panels::open_scoped_report(
            self.backend.clone(),
            crate::panels::ReportScope {
                strategy: moon_core::db::ReportStrategyKey {
                    core_uid,
                    strategy_id,
                },
                strategy_name: name,
                date_from,
                date_to,
                side: query.side,
                emulator: query.emulator,
            },
            Some(window.window_handle()),
            owner_display,
            cx,
        );
    }

    /// Resolve an exact live strategy target from a row key.
    ///
    /// Args:
    ///     key: Strategy row key in `strategyid@core_uid` form.
    ///     cx: Application context used to read the live store.
    ///
    /// Returns:
    ///     Exact `(core_uid, unsigned strategy id)` when the non-manual row is live.
    fn live_strategy_target(&self, key: &str, cx: &App) -> Option<(u64, u64)> {
        let Some((strategy_id, Some(core_uid))) = parse_strat_key(key) else {
            return None;
        };
        if strategy_id == 0 {
            return None;
        }
        let live_id = strategy_id as u64;
        self.backend
            .read(cx)
            .session
            .store()
            .core(core_uid)
            .is_some_and(|core| {
                core.strategies
                    .iter()
                    .any(|strategy| strategy.id == live_id)
            })
            .then_some((core_uid, live_id))
    }

    /// Open Strategies on an exact live pair, rechecking liveness at click time.
    ///
    /// Args:
    ///     key: Strategy row key in `strategyid@core_uid` form.
    ///     window: Analytics owner window.
    ///     cx: Analytics context used to navigate.
    ///
    /// Returns:
    ///     Nothing; stale, manual, and malformed targets are ignored.
    fn open_live_strategy(&self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((core_uid, strategy_id)) = self.live_strategy_target(key, cx) else {
            return;
        };
        let workspace_group = self
            .backend
            .read(cx)
            .singleton_workspace()
            .map(|workspace| workspace.group);
        let owner_display = window.display(cx).map(|display| display.id());
        crate::strategies::open_goto(
            self.backend.clone(),
            core_uid,
            strategy_id,
            workspace_group,
            Some(window.window_handle()),
            owner_display,
            cx,
        );
    }

    /// Change the selected strategy: detail + tuner scope.
    ///
    /// Args:
    ///     sel: New anchor strategy key and display name, or `None` for all strategies.
    ///     cx: GPUI context used to start replacement axis reads and repaint.
    fn set_sel_strategy(&mut self, sel: Option<(String, String)>, cx: &mut Context<Self>) {
        self.cancel_axis_reads();
        self.sel_strategy = sel;
        // The tuner/profile scope changed — the old computations (suggest included) are wrong.
        self.tuner.invalidate();
        // The "By time" schedule grid is PER strategy: reset it so the previous strategy's
        // v1 does not leak onto the new one (otherwise Save would write foreign values).
        self.time_tuner.reset_grid();
        self.time_tuner.invalidate();
        // The coin lists were edited against the PREVIOUS strategy; carried over they would
        // read as "this strategy's coins". `invalidate` retires them along with the numbers.
        self.coins.invalidate();
        self.coin_lists.invalidate();
        self.reload_axis(self.strat_mode, cx);
        cx.notify();
    }

    /// All currently selected strategies as write targets: the anchor first, then the
    /// Additional Ctrl- or Shift-selected targets. Each key `strategyid@core_uid` maps to
    /// `(sid, core, name)`.
    ///
    /// Gated by the ACTION authority, so it never offers a core the focused Auto group may not
    /// write to. Read and display callers use [`Self::visible_targets`] with
    /// [`Self::read_core_ids`] instead — the two answers differ on Auto+Overview, where the
    /// unpinned core filter can put an out-of-group row on screen.
    ///
    /// Returns:
    ///     Action-authorized targets only; refused retained rows are omitted without mutation.
    fn selected_targets(&self) -> Vec<shared::SaveTarget> {
        self.targets_visible_in(self.action_core_ids())
    }

    /// Walk the retained selection ONCE, anchor first, keeping the rows one core scope admits.
    ///
    /// The borrowing form is the one every caller is built on, because most of them want a count
    /// or a `(sid, core)` pair rather than an owned target: the page's card titles and both
    /// multi-selection predicates run on the render path, where materializing a `SaveTarget` per
    /// selected row — each cloning its name — would allocate once per frame and throw it away.
    ///
    /// Args:
    ///     workspace: Core ids the caller admits, or `None` to admit every selected row.
    ///
    /// Returns:
    ///     Anchor-first `(sid, core, name)` for the passing rows, borrowing the retained selection.
    fn visible_targets<'a>(
        &'a self,
        workspace: Option<&'a [u64]>,
    ) -> impl Iterator<Item = (i64, Option<u64>, &'a str)> + 'a {
        self.sel_strategy
            .iter()
            .chain(self.sel_extra.iter())
            .filter(move |(key, _)| strategy_selection_visible(key, workspace))
            .filter_map(|(key, name)| {
                parse_strat_key(key).map(|(sid, core)| (sid, core, name.as_str()))
            })
    }

    /// How many selected rows one core scope admits, without building a target for any of them.
    fn visible_target_count(&self, workspace: Option<&[u64]>) -> usize {
        self.visible_targets(workspace).count()
    }

    /// The passing rows as bare query identities — the shape both scoped reads need.
    fn visible_target_keys(&self, workspace: Option<&[u64]>) -> Vec<(i64, Option<u64>)> {
        self.visible_targets(workspace)
            .map(|(sid, core, _)| (sid, core))
            .collect()
    }

    /// Resolve the retained selection against one concrete core scope, with owned names.
    ///
    /// Args:
    ///     workspace: Core ids the caller admits, or `None` to admit every selected row.
    ///
    /// Returns:
    ///     Anchor-first targets whose cores pass, leaving the retained selection untouched.
    fn targets_visible_in(&self, workspace: Option<&[u64]>) -> Vec<shared::SaveTarget> {
        self.visible_targets(workspace)
            .map(|(sid, core, name)| shared::SaveTarget {
                sid,
                core,
                name: name.to_string(),
            })
            .collect()
    }

    /// Return action-authorized selected row keys without changing retained selection state.
    ///
    /// Returns:
    ///     Retained anchor and extra keys whose cores the action authority admits.
    pub(in crate::analytics) fn effective_strategy_selection(&self) -> Vec<String> {
        // Deliberately NOT derived from `visible_targets`: this keeps a key `parse_strat_key`
        // rejects, so an unparseable retained row still counts as present and cannot make the
        // observer read the selection as having silently changed.
        let workspace = self.action_core_ids();
        self.sel_strategy
            .iter()
            .chain(self.sel_extra.iter())
            .filter(|(key, _)| strategy_selection_visible(key, workspace))
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Retire tuner state whose retained selection became hidden by an Auto workspace change.
    ///
    /// Returns:
    ///     Nothing; retained selection fields remain untouched for Classic restoration.
    pub(in crate::analytics) fn reconcile_workspace_strategy_scope(&mut self) {
        self.cancel_axis_reads();
        self.tuner.invalidate();
        self.time_tuner.reset_grid();
        self.time_tuner.invalidate();
        self.coins.invalidate();
        self.coin_lists.invalidate();
        self.write_error = None;
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
        let pick = self.strategy_data.data().and_then(|d| {
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

    /// Whether any Ctrl- or Shift-selected strategies extend the anchor selection.
    pub(super) fn is_multi(&self) -> bool {
        self.visible_target_count(self.read_core_ids()) > 1
    }

    /// Label for "what these numbers cover", used by every card title on the page.
    ///
    /// The numbers are computed over the whole read-visible selection (`tuner_query`), so naming
    /// the anchor while several rows are highlighted would assert one strategy over N strategies'
    /// figures — a count is the only honest label there.
    pub(super) fn scope_label(&self) -> String {
        // One pass, and the only allocation is the label itself: this runs on every card title of
        // the page, and every name past the first is never read.
        let mut visible = self.visible_targets(self.read_core_ids());
        let Some((_, _, first)) = visible.next() else {
            return t!("analytics.strat.scope_all").to_string();
        };
        match visible.count() {
            0 => first.to_string(),
            rest => t!("analytics.strat.scope_many", n = rest + 1).to_string(),
        }
    }

    /// THE rule for every "in strategy" / "now" current-value display: a per-strategy current
    /// value is meaningful only when exactly ONE read-visible strategy is selected. With several
    /// selected each has its own value, so this returns `None` and callers render a neutral
    /// "varies" / blank instead of the anchor's (which would misrepresent the rest). Both tuning
    /// axes and the save dialog go through this so the behavior can't drift between them.
    pub(super) fn current_if_single<T>(&self, value: T) -> Option<T> {
        (self.visible_target_count(self.read_core_ids()) == 1).then_some(value)
    }

    /// The SET of selected strategies changed while the anchor stayed put. Every number on the
    /// page — the KPI matrix, histogram, and time profile — is computed over that set by
    /// `tuner_query`, so Ctrl toggles and Shift range replacements make all of it stale.
    ///
    /// Deliberately NOT `set_sel_strategy`: that resets the schedule grid, and multi-select
    /// exists precisely to tune values once and write them to many strategies. The anchor's
    /// own coin detail does not change either.
    ///
    /// Args:
    ///     cx: GPUI context used to start replacement axis reads and repaint.
    fn selection_scope_changed(&mut self, cx: &mut Context<Self>) {
        self.cancel_axis_reads();
        self.tuner.invalidate();
        self.time_tuner.invalidate();
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

    /// Ensure a strategy opened by double-click remains visibly selected behind its Report.
    ///
    /// Native double-click delivery includes an initial single click, which can toggle the sole
    /// anchor off. Existing anchor or extra membership is left untouched; only an absent clicked
    /// row becomes the sole anchor.
    ///
    /// Args:
    ///     key: Exact strategy/core key opened by the double-click.
    ///     name: Display label retained with a restored anchor.
    ///     cx: Analytics context used to refresh a changed scope.
    ///
    /// Returns:
    ///     Nothing; selection changes only when the clicked row is currently absent.
    fn select_for_report(&mut self, key: &str, name: &str, cx: &mut Context<Self>) {
        let selected = self
            .sel_strategy
            .as_ref()
            .is_some_and(|(selected_key, _)| selected_key == key)
            || self
                .sel_extra
                .iter()
                .any(|(selected_key, _)| selected_key == key);
        if !selected {
            self.sel_extra.clear();
            self.set_sel_strategy(Some((key.to_string(), name.to_string())), cx);
        }
    }

    /// Ctrl click: toggle this row in/out of the multi-set. The anchor (`sel_strategy`) drives
    /// detail, while the anchor and extras form the read scope and the action-authorized bulk
    /// targets. Removing the anchor promotes the first extra, or clears the whole selection.
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

    /// Shift click: hold every drawn row between the anchor and this one.
    ///
    /// The anchor is `sel_strategy` itself, and Shift deliberately does NOT move it — the same
    /// rule Ctrl-click follows. The tuner has exactly one visible anchor (the amber row) and it
    /// drives scope/suggest/detail/KPI; a second, invisible range anchor would let the range
    /// origin and the highlighted row disagree with nothing on screen to explain it.
    ///
    /// The range REPLACES the extras rather than adding to them, and a click that resolves to the
    /// extras already held returns without touching anything: `selection_scope_changed` retires
    /// every axis and starts fresh reads, which is far too expensive to run for no change.
    ///
    /// With no anchor, or a row the drawn order does not hold, this falls back to a plain
    /// single-select — the row the user actually clicked is never lost to a modifier that had
    /// nothing to span. A drawn order that is momentarily EMPTY is the one case that does
    /// nothing at all, because there the fallback would be destructive; see `range_extras`.
    ///
    /// Args:
    ///     key: Row key that was shift-clicked.
    ///     name: Its display name, already through `strat_display`.
    ///     cx: GPUI context used to start replacement axis reads and repaint.
    fn select_range(&mut self, key: String, name: String, cx: &mut Context<Self>) {
        // The drawn order is read out in full before the mutation below, so no borrow into
        // `strategy_data` is alive while `self` is mutated.
        let order = self
            .strategy_data
            .data()
            .map(|d| list::drawn_order(&d.strategies, self.visible_indices()))
            .unwrap_or_default();
        let anchor = self.sel_strategy.as_ref().map(|(k, _)| k.as_str());
        let outcome = list::range_extras(anchor, &key, &order);
        match outcome {
            list::RangeOutcome::Extras(extras) if extras != self.sel_extra => {
                self.sel_extra = extras;
                self.selection_scope_changed(cx);
            }
            // Already holding exactly this span, or the order is momentarily unknown — see
            // `range_extras`. Doing nothing beats collapsing the selection the user was
            // extending, and beats re-running every axis read for an unchanged set.
            list::RangeOutcome::Extras(_) | list::RangeOutcome::Ignore => {}
            list::RangeOutcome::SingleSelect => self.select_single(key, name, cx),
        }
    }

    /// Change strategy mode and refresh the entered axis' data when it is stale.
    fn set_strat_mode(&mut self, mode: StratMode, cx: &mut Context<Self>) {
        if self.strat_mode == mode {
            return;
        }
        self.strat_mode = mode;
        self.backend.update(cx, |backend, _| {
            backend.ui_session.analytics.strat_mode = mode;
        });
        self.request_axis_if_stale(mode, cx);
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
                self.reload_filter_axis(cx);
            }
            StratMode::Time => self.reload_time(cx),
            StratMode::Coins => self.reload_coins(cx),
        }
    }

    /// Reload a report-stale axis without overlapping its full-history scans.
    ///
    /// Args:
    ///     mode: Visible tuning axis to refresh.
    ///     show_overlay: Whether queued user work requires blocking progress feedback.
    ///     cx: GPUI context used to execute the serialized reads.
    pub(super) fn reload_axis_after_report(
        &mut self,
        mode: StratMode,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        match mode {
            StratMode::Filters => self.reload_tuner_after_report(show_overlay, cx),
            StratMode::Time => self.reload_time_after_report(show_overlay, cx),
            StratMode::Coins => self.reload_coins_after_report(show_overlay, cx),
        }
    }

    /// Return whether one tuning axis is stale for the current report and user scope.
    ///
    /// Args:
    ///     mode: Tuning axis whose cached state should be inspected.
    ///
    /// Returns:
    ///     `true` when entering the axis must trigger a recomputation.
    pub(super) fn axis_needs_reload(&self, mode: StratMode) -> bool {
        match mode {
            StratMode::Filters => self.tuner.needs_reload(),
            StratMode::Time => self.time_tuner.needs_reload(),
            StratMode::Coins => self.coins.needs_reload(),
        }
    }

    /// Queue a stale tab-entry axis behind any Analytics database work already in flight.
    ///
    /// Args:
    ///     mode: Newly visible tuning axis.
    ///     cx: GPUI context used to arm the shared report refresh gate.
    pub(super) fn request_axis_if_stale(&mut self, mode: StratMode, cx: &mut Context<Self>) {
        if self.axis_needs_reload(mode) {
            self.report_busy_retries.reset();
            self.request_report_refresh(true, cx);
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
mod tests;
