//! The Analytics window provides report analyzers over the `orders_rep` replica
//! (see the analytics-panel-plan: summary → comparisons → heatmap → calendar).
//!
//! It is a separate singleton OS window (following the Screener pattern), with geometry persisted
//! in `layout.analytics_window`. Its MoonButton tab strip (as in Settings) contains Summary,
//! Calendar, and Strategy Tuning; the tuning workspace provides By filter, By coin, and By time
//! modes.
//! `moon_core::db::analytics` computes the data on the background executor (the full-period
//! SQLite query never runs on the UI thread). Direct scope edits reload immediately; stale
//! tab/mode entry and committed report generations use the shared quiet-period and maximum-wait
//! gate. Hidden surfaces remain marked stale until entry, and automatic scans never overlap.
//!
//! A writer-driven catch-up preserves a settled snapshot only for a transient outcome that a
//! scheduled correction — a bounded retry or a newer generation — will resolve. Any durable
//! outcome still publishes, so stale data never looks current with no correction path left.

/// Analytics construct implementation.
mod construct;
/// Analytics controls implementation.
mod controls;
/// Analytics diag implementation.
mod diag;
/// Analytics load implementation.
mod load;
/// Analytics observe implementation.
mod observe;
/// Analytics query implementation.
mod query;
/// Analytics session implementation.
mod session;

use diag::{ANALYTICS_HEADER_H, BUSY_OVERLAY_DELAY, MASK_DEBOUNCE};
pub(in crate::analytics) use diag::{pnl_is_pct, pnl_suffix, pnl_unit_label, set_pnl_unit};
pub(crate) use diag::{probe_enabled, probe_select_spec, probe_selects_strategy};
pub(crate) use session::AnalyticsSessionState;
use session::{
    AnalyticsDisplayScope, AnalyticsWorkspaceScope, analytics_display_scope,
    analytics_workspace_scope,
};

/// The shared spawn+overlay envelope of every background DB read.
mod bg;
mod calendar;
/// Period presets, window tabs and date helpers — the time axis shared by every page.
pub(crate) mod period;
/// Typed load-state algebra for Analytics profit queries.
mod profit_load;
pub(crate) mod profit_monitor;
/// Deleting a strategy and its report trades in confirmed order, then requesting empty-folder cleanup.
mod purge;
/// Generation-aware load shedding for automatic report-driven refreshes.
mod refresh;
/// Composes the Analytics window frame and active-tab surface.
mod render;
mod summary;
/// The window's top chrome: tabs, filter combos, date fields, period bar.
mod toolbar;
/// The complete Strategy Tuning page (list, By filter/By coin/By time axes, and shared shell).
/// The former flat set of `strategies`/`tuner*`/`strat_time`/`time_tuner` modules at the
/// analytics root now lives under `tuner/`.
mod tuner;
/// The tape autoload of the Entry/Exit axis, driven from the coordination tick.
pub(crate) use tuner::ticks::fetch::autoload as tape_autoload;

// Pages reach these through the familiar `super::…`, unaware of the `period` module.
pub(in crate::analytics) use period::{
    Period, Tab, custom_bounds, day_of_secs, exact_secs_of_day, fmt_day, secs_of_day, seed_period,
};
pub(in crate::analytics) use profit_load::ProfitLoadState;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonDateTimePickerEvent, MoonDateTimePickerState, MoonInputEvent,
    MoonInputState, MoonVirtualListScrollHandle, Root,
};
use rust_i18n::t;

use crate::Backend;
use crate::controls::date_range::{self, Bound};
use moon_core::db::ReportAxis;
use moon_core::db::analytics::{DayCell, PreviousPeriodBasis, Query, StrategyBase, Summary};
use moon_core::db::valuation::{ValuationMode, ValuationStatus};
use moon_core::db::{ProfitMetric, ProfitUnit, ReadFail, SideFilter};
use moon_core::strategy_query::StrategyQuery;

use crate::load_state::{LoadState, note_el};
use refresh::{
    BusyRetryBudget, CatchUpOutcome, RefreshGate, RefreshPlan, RefreshUrgency, VisibleRefresh,
    visible_refresh,
};

/// A bulk tuner write still awaiting core evidence: a resolution or a reported `TimedOut` phase.
///
/// A timeout is not a rejection; a late core echo can still produce a resolution upstream. This
/// watch remains active until it observes either a resolution or the reported timeout phase, so a
/// queued send is not shown as an applied one without core evidence.
///
/// Each field is load-bearing:
/// - `pending` holds exact (core, strategy) PAIRS rather than two parallel lists — a strategy id
///   repeating across cores is normal here, and two lists would form a false Cartesian product.
/// - `since` is captured PER CORE rather than as one shared cursor, because
///   [`moon_core::feed::StrategyEditNote::seq`] is generated per `CoreData`; one scalar cursor
///   would suppress another core's lower-sequence notes.
/// - `since` is captured BEFORE the send loop: installed after dispatch, a fast resolution lands
///   before the cursor exists and the outcome is missed entirely.
/// - `pending` admits only successful queues: `send_bulk_changes` can fail per core, and waiting
///   on a pair whose send failed would hang that pair forever.
pub(super) struct StrategyEditWatch {
    /// Exact (core, strategy) pairs whose `edit_strategies` call SUCCEEDED.
    pub(super) pending: Vec<(moon_core::session::CoreId, u64)>,
    /// Per-core resolved-note cursor, captured before the first send and never advanced: each
    /// drain rescans from it, since [`moon_core::session::CoreData::strategy_edit_notes_since`]
    /// keeps its ring until eviction.
    pub(super) since: HashMap<moon_core::session::CoreId, u64>,
    pub(super) submitted: std::time::Instant,
}

/// State of the Analytics window.
pub struct AnalyticsView {
    backend: Entity<Backend>,
    /// User-selected zone applied to all civil Analytics periods and timestamps.
    display_zone: chrono_tz::Tz,
    /// The measured time axis this window reads replicated timestamps on, cached.
    ///
    /// This window is ONE gpui entity with none of the shell's repaint throttles, so any work in
    /// its render root runs on every notify. Rebuilding the axis there would walk the whole core
    /// list per frame for a value that moves only when a core adopts an offset or the zone
    /// changes — both of which write this field directly.
    axis: ReportAxis,
    /// Whether custom-period pickers need redisplay after a zone identity change.
    display_zone_fields_dirty: bool,
    /// Report-writer generation observed only while this window exists.
    report_generation: Option<Arc<AtomicU64>>,
    /// Historical-cache and current-rate data generation observed beside report commits.
    valuation_generation: Option<Arc<AtomicU64>>,
    /// Latest published worker health, refreshed only when its revision moves. Polled separately
    /// from the data generation because a stalled worker commits no rows at all.
    valuation_status: ValuationStatus,
    /// Health revision already folded into `valuation_status`.
    last_valuation_status_rev: u64,
    /// Debounce/max-wait state for automatic report-driven refreshes.
    report_refresh: RefreshGate,
    /// Bounded automatic retries for the active transient-outcome episode.
    report_busy_retries: BusyRetryBudget,
    tab: Tab,
    /// Period of the Summary tab (presets or the from/to range).
    period: Period,
    /// Period of the Strategy Tuning tab, INDEPENDENT of Summary: each tab has its own
    /// time window, and the period bar edits the active one (`active_period`).
    strat_period: Period,
    /// Period currently represented by `data` (summary/strategy list). Entering a tab
    /// with a different time window triggers a reload.
    data_period: Period,
    /// Resolved `[from, to)` bounds `data` was computed for. Presets survive civil rollovers, so
    /// the enum alone cannot tell when their re-resolved bounds make retained hovers stale.
    data_range: (i64, i64),
    /// Whether `data` predates the latest committed report generation.
    data_dirty: bool,
    /// Cores from the replica (for the combo box) plus multi-selection (empty = all), using
    /// the same controls as Orders and Report.
    cores: Vec<(u64, String)>,
    /// Last successful core-list query; frequent report refreshes reuse it for up to one minute.
    last_cores_at: Option<std::time::Instant>,
    /// Whether Calendar skipped a core-list query that still needs a trailing refresh.
    core_refresh_needed: bool,
    /// Whether a trailing core-list refresh timer is already scheduled.
    core_refresh_timer_armed: bool,
    /// Auto action scope mirrored from `WorkspaceRevision`; only a concrete rail core also pins
    /// the retained read filter.
    workspace_scope: Option<AnalyticsWorkspaceScope>,
    /// Classic display-membership narrowing mirrored from `WorkspaceRevision`; READ side only.
    /// Never consulted by `action_core_ids` — see `analytics_display_scope`.
    display_scope: Option<AnalyticsDisplayScope>,
    sel_cores: HashSet<u64>,
    /// Explicit saved-group provenance for the caption beside the numeric core trigger.
    core_caption: toolbar::CoreSelectionCaption,
    side: SideFilter,
    /// `None` means all, `Some(false)` real, and `Some(true)` emulated.
    emu: Option<bool>,
    /// Literal, case-insensitive part of the strategy NAME every tab narrows by. Empty = no
    /// filter. Persisted in `layout.analytics_strategy_mask`, so a family the user works in
    /// survives a restart the way the period does.
    strategy_mask: String,
    /// The mask the last started read actually used.
    ///
    /// Separate from [`Self::strategy_mask`], which follows the KEYSTROKES: without it, an Enter
    /// or a blur carrying the value a `Change` event has already mirrored looks like "nothing
    /// changed" and silently skips the immediate reload it exists to perform.
    strategy_mask_applied: String,
    /// Retained input widget for the mask, so its cursor and text survive every repaint.
    strategy_mask_input: Entity<MoonInputState>,
    /// The pending debounced reload, held so the NEXT keystroke drops it.
    ///
    /// Dropping a `Task` cancels it, which is the whole mechanism — the same one the tuner's coin
    /// axis uses. [`Self::reload`] has no in-flight guard of its own: it cancels every read, bumps
    /// the sequence and restarts every axis, so a reload per character would be a cancel storm
    /// over full-period scans rather than a queue that coalesces.
    strategy_mask_debounce: Option<Task<()>>,
    /// Profit metric: absolute quote money (`Quote`) or the report `Profit` column
    /// (`Percent` = profit ÷ spent). Persisted in `layout.analytics_profit_percent`.
    metric: ProfitMetric,
    /// Whether money is reported in USDT even when the scope has one native quote.
    prefer_usdt: bool,
    /// Which conversion turns quote money into USDT. Mirrors the application-wide backend setting;
    /// see [`Self::observe_valuation_mode`] for why it is a mirror rather than a live read.
    valuation_mode: ValuationMode,
    /// Current names of the configured cores, mirrored for the reason `valuation_mode` is; read
    /// paths show them in place of the name each trade stored at download time.
    core_names: moon_core::db::CoreNames,
    /// Background summary state with distinct loading, unavailable, ready, and
    /// failed outcomes so only a successful empty read appears empty.
    pub(in crate::analytics) data: ProfitLoadState<Summary>,
    /// Snapshot-owned Summary chart derivations reused on hover redraws.
    summary_derived: Option<std::rc::Rc<summary::derived::SummaryDerived>>,
    /// Compact strategy-list and coin-universe base, separate from the expensive Summary.
    pub(in crate::analytics) strategy_data: ProfitLoadState<StrategyBase>,
    /// Period currently represented by `strategy_data`.
    strategy_data_period: Period,
    /// Whether the compact Strategies base predates the latest report generation.
    strategy_dirty: bool,
    /// Closed trades the core never dated, under the CURRENT filters.
    ///
    /// `None` while unknown. Failures are tracked separately so the banner never claims that
    /// missing rows do not exist merely because the metadata query failed.
    pub(super) undated: Option<moon_core::db::analytics::UndatedCloses>,
    /// Classified failure of the latest undated-close read.
    pub(super) undated_error: Option<ReadFail>,
    /// Whether the undated-close notice is expanded right now; mirrors
    /// [`AnalyticsSessionState::undated_expanded`], which owns its lifetime.
    pub(super) undated_expanded: bool,
    /// The last write that did not reach a single core, in the user's words.
    ///
    /// A failed `edit_strategies` used to be a log line and nothing else, while the panel
    /// reloaded and put the strategy's OLD values back — which is exactly what a successful
    /// write looks like. The edit was gone and the user had been told it was saved.
    pub(super) write_error: Option<String>,
    /// A bulk write still awaiting core evidence about each edit it queued.
    ///
    /// `write_error` alone only ever reported a SEND failure; a write the core silently altered,
    /// overrode, or never answered rendered as success. This tracks the still-open pairs of the
    /// most recent bulk write until every pair resolves or reports `TimedOut`, so the panel can
    /// say what actually happened instead of merely that a command was queued.
    pub(super) strategy_edit_watch: Option<StrategyEditWatch>,
    /// The open "delete the strategy and its trades from the report" confirmation, if any.
    pub(in crate::analytics) strat_purge: Option<purge::PurgeOp>,
    /// Identity handed to each purge operation.
    ///
    /// Retained across close/reopen on purpose: a background count belonging to a dialog the user
    /// already dismissed must not publish into the next one, and only a counter that keeps moving
    /// can tell two operations on the same row apart.
    purge_seq: u64,
    /// Count of background operations: values above zero enable the blocking Loading overlay.
    /// Without it, long scans of a large database are invisible while filter and strategy clicks
    /// accumulate in the queue.
    busy_ops: usize,
    /// Every Analytics database task, including non-overlay selection cues and debounced rescans.
    ///
    /// Automatic report refresh waits for this to reach zero so a burst cannot start a second
    /// full-period scan over an existing `overlay = false` task.
    db_ops: usize,
    /// Cancellation ownership for replaceable background database destinations.
    latest_reads: bg::LatestReads,
    /// Start and identity of the current operation batch. The overlay appears only after
    /// `BUSY_OVERLAY_DELAY`, so quick recomputations do not flash the dimmer.
    busy_since: Option<std::time::Instant>,
    /// Request sequence number used to discard stale results.
    seq: u64,
    /// Hovered bucket of the "Daily profit" chart — popup of that DAY's per-core values.
    pub(super) hover_daily_bucket: Option<usize>,
    /// Hovered bucket of the cumulative chart — popup of the per-core RUNNING TOTALS. Its
    /// own state: one shared field would pop both charts open at once.
    pub(super) hover_cum_bucket: Option<usize>,
    /// Hovered bar of the "by strategy type" chart (single-day periods only) — popup of the
    /// cores behind that type.
    pub(super) hover_kind: Option<usize>,
    /// Delayed dismissal shared by chart buckets and their scrollable popup.
    summary_popup_hover: summary::PopupHover,
    /// Whether the Summary's per-core card shows every ranked core instead of the compact
    /// leaders/outsiders overview. A display lens, persisted as `layout.analytics_cores_show_all`
    /// so the choice survives a restart.
    pub(super) show_all_core_ranks: bool,
    /// Strategies tab: selected per-core row key (`strategyid@core_uid`), plus its name and
    /// details. Legacy bare strategy IDs remain parseable.
    pub(super) sel_strategy: Option<(String, String)>,
    /// Multi-select: the EXTRA selected rows beyond the anchor (`sel_strategy`), added one at a
    /// time with Ctrl or a whole display-order block at a time with Shift. The anchor drives
    /// scope/suggest/detail; these are bulk-write addressees only, stored as `(key, name)`.
    /// Empty = single selection. Order matters — removing the anchor promotes the first entry.
    pub(super) sel_extra: Vec<(String, String)>,
    /// Strategy-list filter bar (see tuner::list): name search text, kind filter (None = all),
    /// and "active only" (default on — hides strategies no longer present in any core).
    pub(super) strat_search: String,
    pub(super) strat_type: Option<String>,
    pub(super) strat_active_only: bool,
    /// Show only strategies that name coins in a list (blacklist / whitelist), or all.
    pub(in crate::analytics) strat_lists: tuner::StratListFilter,
    /// Lazily-created search input backing `strat_search`.
    pub(super) strat_search_input: Option<Entity<MoonInputState>>,
    /// List sort: `(column key, descending)`. None → the default profit-descending order.
    pub(super) strat_sort: Option<(String, bool)>,
    /// Visible-column bitmask of the strategy list, PER axis: the list sits beside a
    /// different tool in each mode and is asked a different question there.
    pub(super) strat_cols: moon_core::config::layout::StratColsByMode,
    /// Content-measured preferred width of the strategy list's core column as
    /// `(font_scale_it_was_measured_under, width_in_base_px)` (`tuner::list::table::core_col_w`).
    /// Filled lazily on render; measuring lays out a glyph per character for every distinct core
    /// name, too much to repay on an idle repaint. It is cleared only when a published base
    /// changes the single-core names (`tuner::core_names_changed`), and
    /// recomputed when the stored font scale no longer matches the current one, so a Font-slider
    /// move OR a theme whose mode carries a different base mono size re-measures instead of
    /// scaling a width that assumed the old base.
    strat_core_w: Option<(f32, f32)>,
    /// Content-measured pixel widths of the strategy list's numeric columns as
    /// `(font_scale_it_was_measured_under, widths)` (`tuner::list::table::metric_col_widths`).
    /// Filled lazily on render like `strat_core_w`; the measurement formats every loaded group
    /// once per column, which is thousands of strings at 53 cores and far too much to repay on
    /// an idle repaint of a window that has no repaint throttle. Cleared whenever a published
    /// base replaces the group set, and recomputed when the font scale moved.
    strat_metric_w: Option<(f32, std::sync::Arc<[f32]>)>,
    /// Memoized strategy-list row order, with the filter inputs it was built for.
    ///
    /// The filter-and-sort pass runs over every group the replica holds — thousands at 53 cores.
    /// Its key carries the group slice's address alongside the filter-bar state, while replacement
    /// explicitly clears the cache to protect against allocator address reuse.
    /// `tuner::list::ensure_visible` owns it.
    strat_visible: Option<tuner::VisibleRows>,
    /// Retained strategy-list scroll state reused by every virtual-list render.
    strat_scroll: MoonVirtualListScrollHandle,
    /// Calendar tab: cells (PnL, trades, and wins) for the loaded range; Day mode uses hourly cells.
    pub(in crate::analytics) cal_days: ProfitLoadState<Vec<DayCell>>,
    cal_seq: u64,
    /// Whether the series is stale for the current scope or report generation.
    cal_dirty: bool,
    cal_mode: calendar::CalMode,
    /// Displayed calendar month as `(year, month 1..12)`, controlled by the tab's OWN
    /// Previous/Next navigation; the window period bar does not affect Calendar.
    pub(super) cal_ym: (i32, u32),
    /// Selected day start for Day mode.
    pub(super) cal_day: i64,
    /// PREVIOUS month's aggregate `(profit, trades, wins)` for the KPI deltas against the
    /// previous period (calendar month versus calendar month, not 30 days).
    pub(super) cal_prev: LoadState<Option<moon_core::db::analytics::CellTotals>>,
    /// Strategies-tab mode (Filters / Coins / Time). Privacy is module-based: tab submodules
    /// can see their parent's fields without `pub(super)`.
    strat_mode: tuner::StratMode,
    /// Collapse the shared "Fact vs variants" KPI matrix to its two top rows (trades +
    /// profit). One flag for every axis (the matrix is the same widget in each), so the
    /// choice is consistent across Filters/Coins/Time. Persisted in
    /// `layout.analytics_kpi_collapsed`; a display lens, so toggling only repaints.
    kpi_collapsed: bool,
    /// Collapse the "By filter" distribution card to its title and subtitle, giving the fields
    /// grid and strategy list above it the vertical room back. Persisted in
    /// `layout.analytics_hist_collapsed`; like `kpi_collapsed` a display lens, so toggling only
    /// repaints — the histogram keeps loading underneath.
    hist_collapsed: bool,
    /// Collapse the tuner's whole RIGHT-HAND column — the shared "Fact vs variants" matrix and
    /// the axis' own tool below it — so the strategy list takes the freed width. One flag serves
    /// every axis, because it is the same column in each. Persisted in
    /// `layout.analytics_tuner_side_collapsed`; like `kpi_collapsed` a display lens, so toggling
    /// only repaints.
    ///
    /// Deliberately ORTHOGONAL to `kpi_collapsed`: folding the column away leaves the matrix's
    /// own two-row collapse untouched, so restoring the column restores exactly the matrix the
    /// user left inside it. Forcing either value here would destroy a choice they made on
    /// purpose and persisted.
    side_collapsed: bool,
    /// The shell colour the window's clear colour was last set to. The window paints no
    /// background of its own (`MoonBackgroundPolicy::NoFill`) so the chart of the tuner's trade
    /// pane — drawn UNDER the GPUI scene — shows through; the clear colour stands in for the
    /// root's fill and follows the palette from `render`, set only when it moved.
    clear_shell: Option<u32>,
    /// Threshold tuner (Filters mode), with its state defined in its own module.
    tuner: tuner::TunerState,
    /// The "By coin" mode: the table's view controls, the picked coins that define
    /// variant v1, and the two background results it renders from.
    coins: tuner::CoinsState,
    /// "Entry/Exit" axis: the deals, their tape and the model's verdicts.
    ticks: tuner::TicksState,
    /// The coin picker's read: the selected strategies' blacklist, with the core each coin
    /// belongs to and when it was added.
    coin_lists: tuner::CoinListsState,
    /// The By time mode: v1/v2 schedule bounds, the loaded profiles/KPI and their
    /// staleness — the whole axis state lives in one struct, like `coins`.
    time_tuner: tuner::TimeTunerState,
    /// Date+time fields of the custom from/to range (MoonUI `MoonDateTimePicker`); picking a day
    /// or spinning the clock switches the period to `Period::Custom`.
    ///
    /// The picker owns its own popup-open flag, so the view keeps none of its own.
    cal_from: Entity<MoonDateTimePickerState>,
    cal_to: Entity<MoonDateTimePickerState>,
    /// Whether a bound changed while its popup was open and still has to be applied.
    ///
    /// Every clock-drum step emits `Change`, and applying one reloads Summary, the tuner and every
    /// tuning axis. So an open popup only marks the range dirty; it is committed once, when the
    /// popup closes.
    range_dirty: bool,
    /// Whether the single delayed integrity-status poll is armed.
    integrity_poll_armed: bool,
    _cal_subs: Vec<Subscription>,
    focus: FocusHandle,
}

impl crate::controls::CoreComboHost for AnalyticsView {
    /// Auto on a concrete core pins the selector to its workspace scope and leaves the retained
    /// filter untouched; Auto on Overview leaves the selector free, exactly as in Classic.
    fn core_selection_pinned(&self, _cx: &App) -> bool {
        self.core_filter_pinned()
    }

    /// Return the retained Analytics core filter for shared picker edits.
    fn core_selection_mut(&mut self) -> &mut HashSet<u64> {
        &mut self.sel_cores
    }

    /// Retain exact saved-group provenance without requerying unchanged Analytics data.
    fn after_core_group_application(&mut self, group: Option<String>, cx: &mut Context<Self>) {
        if !self.core_caption.set_applied_group(group) {
            return;
        }
        let core_caption = self.core_caption.clone();
        self.backend.update(cx, |b, _| {
            b.ui_session.analytics.core_caption = core_caption;
        });
        cx.notify();
    }

    /// Requery every Analytics surface against the new core scope.
    fn after_core_selection_change(&mut self, cx: &mut Context<Self>) {
        self.core_selection_changed(cx);
    }
}

impl EventEmitter<()> for AnalyticsView {}
impl Focusable for AnalyticsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Open the singleton Analytics tool window, activating the live handle stored in `Backend`
/// when one exists and replacing a stale handle otherwise.
pub fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    if let Some(handle) = backend.read(cx).analytics_window
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let saved = backend.read(cx).layout.analytics_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(120.0), px(90.0)),
            size: size(px(1240.0), px(800.0)),
        },
        |g| Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
    );
    let display_id = crate::window::windowing::saved_or_owner_display_id(
        saved.and_then(|g| g.display_uuid),
        saved.map(|g| point(px(g.x as f32), px(g.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let mut opts = crate::window::windowing::tool_window_options(
        t!("analytics.window_title").to_string(),
        crate::window::windowing::restored_window_bounds(
            saved,
            crate::window::windowing::reachable_window_bounds(bounds, display_id, None, cx),
        ),
        Some(size(px(860.0), px(520.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| AnalyticsView::new(b, window, cx));
        // NoFill: the tuner's trade pane draws a chart UNDER the GPUI scene, and an opaque root
        // (or any fill above the pane) hides it whole. The clear colour is the shell's, the fill
        // the root used to paint; `AnalyticsView::render` keeps it on the palette.
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::NoFill))
    }) {
        backend.update(cx, |bk, _| bk.analytics_window = Some(handle));
        crate::window::windowing::activate_new_window(handle.into(), cx);
    }
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
