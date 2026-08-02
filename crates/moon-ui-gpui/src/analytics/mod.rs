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

/// The shared spawn+overlay envelope of every background DB read.
mod bg;
mod calendar;
/// Period presets, window tabs and date helpers — the time axis shared by every page.
mod period;
/// Generation-aware load shedding for automatic report-driven refreshes.
mod refresh;
mod summary;
/// The window's top chrome: tabs, filter combos, date fields, period bar.
mod toolbar;
/// The complete Strategy Tuning page (list, By filter/By coin/By time axes, and shared shell).
/// The former flat set of `strategies`/`tuner*`/`strat_time`/`time_tuner` modules at the
/// analytics root now lives under `tuner/`.
mod tuner;

// Pages reach these through the familiar `super::…`, unaware of the `period` module.
pub(in crate::analytics) use period::{Period, Tab, day_of_secs, fmt_day, secs_of_day};

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonAlert, MoonBackgroundPolicy, MoonCalendarEvent, MoonCalendarState, MoonDate,
    MoonInputState, MoonPalette, MoonVirtualListScrollHandle, MoonWindowFrame, Root, h_flex,
    v_flex,
};
use rust_i18n::t;

use crate::design::{moon, moon_alpha};
use crate::{Backend, design};
use moon_core::db::analytics::{DayCell, Query, StrategyBase, Summary};
use moon_core::db::{
    FailKind, ProfitMetric, ProfitScope, ProfitUnit, QuoteBreakdown, ReadFail, SideFilter,
};

use crate::load_state::{LoadState, Note, note_el};
use refresh::{BusyRetryBudget, RefreshGate, RefreshPlan, VisibleRefresh, visible_refresh};

const ANALYTICS_HEADER_H: f32 = 32.0;

/// Is the coin-table observation channel armed (`MOON_ANALYTICS_PROBE`, any value)?
///
/// A GUI panel has no observation channel by default, so this mirrors the convention
/// `diag.rs` already established for render counters: gated on an env var rather than
/// on `cfg(debug_assertions)` (this workspace builds dev with debug-assertions off),
/// read once, and inert in EVERY build unless it is set. Armed, it makes startup open
/// this window straight on the coin table and makes that table log what it renders —
/// which is the only way to observe the panel without clicking through the UI by hand.
pub(crate) fn probe_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MOON_ANALYTICS_PROBE").is_some())
}

/// `MOON_ANALYTICS_PROBE=select` — additionally adopt the first strategy that actually
/// carries a coin list, once the summary lands.
///
/// The interesting state of the "By coin" panels is the one WITH a strategy chosen, and
/// reaching it otherwise means clicking a row by hand — which is not an observation channel.
/// Inert unless the variable starts with `select`.
pub(crate) fn probe_selects_strategy() -> bool {
    probe_select_spec().is_some()
}

/// The selection spec, so any particular case — an empty list, one with no datable history —
/// can be put on screen without a human hunting for it in the list:
///
/// - `select` — the strategy with the biggest blacklist;
/// - `select:ID@CORE` — that exact strategy, addressed by the row key the page itself uses;
/// - `select:ID` — that strategy id on whichever core carries it first.
pub(crate) fn probe_select_spec() -> Option<&'static str> {
    use std::sync::OnceLock;
    static SPEC: OnceLock<Option<String>> = OnceLock::new();
    SPEC.get_or_init(|| {
        std::env::var("MOON_ANALYTICS_PROBE")
            .ok()
            .filter(|v| v == "select" || v.starts_with("select:"))
            .map(|v| v.strip_prefix("select:").unwrap_or("").to_string())
    })
    .as_deref()
}

/// Delay before showing the busy overlay, so quick recomputations do not flash the dimmer.
const BUSY_OVERLAY_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

// Comparable profit unit for this frame's shared formatters.
//
// Set once from the active metric at the top of `AnalyticsView::render` and read by the shared
// profit formatters (`summary::fmt_signed`, the calendar and tuner cells), so a "%" suffix
// appears in percent mode without threading the metric through every signature. The window
// renders on the UI thread, so a thread-local stays consistent within a frame.
thread_local! {
    static PNL_UNIT: std::cell::Cell<Option<ProfitUnit>> = const { std::cell::Cell::new(None) };
}
/// Record the active profit unit for this frame's formatters.
///
/// Args:
///     unit: Comparable quote or Percent unit, or `None` outside scalar data.
///
/// Returns:
///     Nothing.
pub(in crate::analytics) fn set_pnl_unit(unit: Option<ProfitUnit>) {
    PNL_UNIT.with(|cell| cell.set(unit));
}
/// Is the window rendering percent profit rather than raw quote money?
///
/// Returns:
///     `true` only when the active comparable unit is Percent.
pub(in crate::analytics) fn pnl_is_pct() -> bool {
    PNL_UNIT.with(|cell| matches!(cell.get(), Some(ProfitUnit::Percent)))
}
/// Unit suffix for a profit-metric figure: `%` in percent mode and empty for quote money.
///
/// Returns:
///     Percent suffix or an empty quote-money suffix.
pub(in crate::analytics) fn pnl_suffix() -> &'static str {
    if pnl_is_pct() { "%" } else { "" }
}
/// Standalone unit token for a label or axis caption that stands BESIDE a profit figure rather
/// than riding on it: the exact quote ticker in money mode, `%` in percent mode. A number already
/// carries its own unit via `pnl_suffix`, so this is only for the surrounding label. Tickers are
/// language-neutral (see locales/README.md), so — like `pnl_suffix` — it lives in code, not the
/// dictionary, and slots into a `%{unit}` placeholder.
///
/// Returns:
///     Exact quote ticker, `%`, or an empty label outside comparable scalar data.
pub(in crate::analytics) fn pnl_unit_label() -> &'static str {
    PNL_UNIT.with(|cell| match cell.get() {
        Some(ProfitUnit::Percent) => "%",
        Some(ProfitUnit::Quote(currency)) => currency.ticker(),
        None => "",
    })
}

/// UI load state that preserves the database's profit-scope invariant.
pub(in crate::analytics) enum ProfitLoadState<T> {
    /// A request is in flight and no scalar unit has been verified yet.
    Loading,
    /// Comparable or legitimately empty scalar data and its optional exact unit.
    Ready {
        /// `None` belongs only to an empty query that has no currency to infer.
        unit: Option<ProfitUnit>,
        /// Current scalar payload.
        data: Arc<T>,
    },
    /// Raw money is unsafe as one scalar; only per-quote totals are retained.
    Split(QuoteBreakdown),
    /// The report replica or required schema is not available yet.
    NotReady,
    /// A classified read failure with no stale scalar payload.
    Failed(ReadFail),
}

impl<T> Default for ProfitLoadState<T> {
    /// Begin with a fresh unitless loading state.
    ///
    /// Returns:
    ///     Loading state without stale scalar data.
    fn default() -> Self {
        Self::Loading
    }
}

impl<T> ProfitLoadState<T> {
    /// Publish one typed database result without creating contradictory unit/split fields.
    ///
    /// Args:
    ///     result: Comparable, empty, split-only, or failed database result.
    ///
    /// Returns:
    ///     Nothing.
    fn apply(&mut self, result: moon_core::db::ReadResult<ProfitScope<T>>) {
        *self = match result {
            Ok(ProfitScope::Comparable { unit, data }) => Self::Ready {
                unit: Some(unit),
                data: Arc::new(data),
            },
            Ok(ProfitScope::Empty(data)) => Self::Ready {
                unit: None,
                data: Arc::new(data),
            },
            Ok(ProfitScope::Split(totals)) => Self::Split(totals),
            Err(ReadFail::NotReady) => Self::NotReady,
            Err(error) => Self::Failed(error),
        };
    }

    /// Borrow scalar data when the scope is comparable or legitimately empty.
    ///
    /// Returns:
    ///     Current scalar payload, or `None` for loading, split, and failed states.
    fn data(&self) -> Option<&Arc<T>> {
        match self {
            Self::Ready { data, .. } => Some(data),
            Self::Loading | Self::Split(_) | Self::NotReady | Self::Failed(_) => None,
        }
    }

    /// Return scalar data or the exact placeholder for the current load outcome.
    ///
    /// Args:
    ///     empty: Predicate that classifies a successful scalar payload as empty.
    ///
    /// Returns:
    ///     Ready scalar data or a loading, empty, unavailable, split, or failure note.
    fn view(&self, empty: impl FnOnce(&T) -> bool) -> Result<&Arc<T>, Note> {
        match self {
            Self::Loading => Err(Note::Loading),
            Self::Ready { data, .. } if empty(data) => Err(Note::Empty),
            Self::Ready { data, .. } => Ok(data),
            Self::Split(_) => Err(Note::IncomparableQuote),
            Self::NotReady => Err(Note::NotReady),
            Self::Failed(ReadFail::IncomparableQuote) => Err(Note::IncomparableQuote),
            Self::Failed(error) => Err(Note::Failed {
                msg: error.to_string().into(),
                kind: error.kind().unwrap_or(FailKind::Other),
            }),
        }
    }

    /// Exact unit carried by the ready scalar payload.
    ///
    /// Returns:
    ///     Quote currency or Percent, or `None` outside a comparable scope.
    fn unit(&self) -> Option<ProfitUnit> {
        match self {
            Self::Ready { unit, .. } => *unit,
            Self::Loading | Self::Split(_) | Self::NotReady | Self::Failed(_) => None,
        }
    }

    /// Split totals retained for an incomparable raw-money scope.
    ///
    /// Returns:
    ///     Per-quote totals only for the split state.
    fn split(&self) -> Option<&QuoteBreakdown> {
        match self {
            Self::Split(totals) => Some(totals),
            Self::Loading | Self::Ready { .. } | Self::NotReady | Self::Failed(_) => None,
        }
    }
}

/// Process-lifetime Analytics choices restored when its OS window is recreated.
#[derive(Clone)]
pub(crate) struct AnalyticsSessionState {
    /// Last top-level page selected by the user.
    tab: Tab,
    /// Explicit core filter, preserving the selector's empty/full/stale set semantics.
    sel_cores: HashSet<u64>,
    /// Last Strategies axis selected while this process is running.
    strat_mode: tuner::StratMode,
    /// Whether the "closed trades the core never dated" notice is expanded.
    ///
    /// Deliberately here and NOT in `WindowLayout`: the notice starts collapsed in every
    /// process, for every user, so a restart cannot leave a warning about money missing from
    /// the figures silently switched off. Expanding it is a look-at-it-now action, not a
    /// preference — it survives closing the window and nothing more.
    undated_expanded: bool,
}

impl Default for AnalyticsSessionState {
    /// Create the state used for the first Analytics open in a fresh process.
    ///
    /// Returns:
    ///     Summary-tab state with the empty core set that represents all current cores.
    fn default() -> Self {
        Self {
            tab: Tab::Summary,
            sel_cores: HashSet::new(),
            strat_mode: tuner::StratMode::Filters,
            undated_expanded: false,
        }
    }
}

/// State of the Analytics window.
pub struct AnalyticsView {
    backend: Entity<Backend>,
    /// Report-writer generation observed only while this window exists.
    report_generation: Option<Arc<AtomicU64>>,
    /// Historical-valuation generation observed beside report commits.
    valuation_generation: Option<Arc<AtomicU64>>,
    /// Debounce/max-wait state for automatic report-driven refreshes.
    report_refresh: RefreshGate,
    /// Bounded automatic Busy retries for the active contention episode.
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
    sel_cores: HashSet<u64>,
    side: SideFilter,
    /// `None` means all, `Some(false)` real, and `Some(true)` emulated.
    emu: Option<bool>,
    /// Profit metric: absolute quote money (`Quote`) or the report `Profit` column
    /// (`Percent` = profit ÷ spent). Persisted in `layout.analytics_profit_percent`.
    metric: ProfitMetric,
    /// Background summary state with distinct loading, unavailable, ready, and
    /// failed outcomes so only a successful empty read appears empty.
    pub(in crate::analytics) data: ProfitLoadState<Summary>,
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
    /// Count of background operations: values above zero enable the blocking Loading overlay.
    /// Without it, long scans of a large database are invisible while filter and strategy clicks
    /// accumulate in the queue.
    busy_ops: usize,
    /// Every Analytics database task, including non-overlay selection cues and debounced rescans.
    ///
    /// Automatic report refresh waits for this to reach zero so a burst cannot start a second
    /// full-period scan over an existing `overlay = false` task.
    db_ops: usize,
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
    /// name, too much to repay on an idle repaint. Invalidated two ways: cleared to `None` where
    /// `strategy_data` is replaced (the names may have changed), and
    /// recomputed when the stored font scale no longer matches the current one, so a Font-slider
    /// move OR a theme whose mode carries a different base mono size re-measures instead of
    /// scaling a width that assumed the old base.
    strat_core_w: Option<(f32, f32)>,
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
    pub(super) cal_prev: LoadState<Option<(f64, i64, i64)>>,
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
    /// Threshold tuner (Filters mode), with its state defined in its own module.
    tuner: tuner::TunerState,
    /// The "By coin" mode: the table's view controls, the picked coins that define
    /// variant v1, and the two background results it renders from.
    coins: tuner::CoinsState,
    /// The coin picker's read: the selected strategies' blacklist, with the core each coin
    /// belongs to and when it was added.
    coin_lists: tuner::CoinListsState,
    /// The By time mode: v1/v2 schedule bounds, the loaded profiles/KPI and their
    /// staleness — the whole axis state lives in one struct, like `coins`.
    time_tuner: tuner::TimeTunerState,
    /// Calendars for the custom from/to range (MoonUI `MoonCalendar` popups); selecting a date
    /// switches the period to `Period::Custom`.
    cal_from: Entity<MoonCalendarState>,
    cal_to: Entity<MoonCalendarState>,
    cal_from_open: bool,
    cal_to_open: bool,
    /// Whether the single delayed integrity-status poll is armed.
    integrity_poll_armed: bool,
    _cal_subs: Vec<Subscription>,
    focus: FocusHandle,
}

impl AnalyticsView {
    /// Build an Analytics view from durable layout preferences and process-lifetime UI choices.
    ///
    /// Args:
    ///     backend: Shared application state containing layout and UI-session snapshots.
    ///     window: Newly opened Analytics window used to observe geometry and create controls.
    ///     cx: View context used to subscribe controls and start the initial reload.
    ///
    /// Returns:
    ///     A fully initialized Analytics view.
    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Window geometry lives in the layout, as it does for Screener and Strategies.
        cx.observe_window_bounds(window, |this, window, cx| {
            let Some((x, y, w, h)) = crate::window::windowing::window_geom(window) else {
                return;
            };
            this.backend.update(cx, |b, _| {
                if b.layout.analytics_window.map(|g| (g.x, g.y, g.w, g.h)) != Some((x, y, w, h)) {
                    b.layout.analytics_window =
                        Some(moon_core::config::layout::GeomRect { x, y, w, h });
                    b.layout_dirty = true;
                }
            });
        })
        .detach();

        // Observe the dedicated post-commit wake channel only while this view exists.
        let report_generation = backend
            .read(cx)
            .reports
            .as_ref()
            .map(|reports| reports.generation.clone());
        let valuation_generation = backend
            .read(cx)
            .valuation
            .as_ref()
            .map(|valuation| valuation.generation.clone());
        let initial_report_generation = report_generation
            .as_ref()
            .map(|generation| generation.load(Ordering::Relaxed))
            .unwrap_or(0)
            .wrapping_add(
                valuation_generation
                    .as_ref()
                    .map(|generation| generation.load(Ordering::Relaxed))
                    .unwrap_or(0),
            );
        let report_revision = backend.read(cx).report_revision.clone();
        cx.observe(&report_revision, |this, _revision, cx| {
            this.observe_report_generation(cx);
        })
        .detach();

        // Period: the previous layout selection, defaulting to the current calendar month.
        let saved_period = backend
            .read(cx)
            .layout
            .analytics_period
            .as_deref()
            .and_then(Period::from_id);
        // The Tuning period has its own persisted key, independent of Summary.
        let saved_strat_period = backend
            .read(cx)
            .layout
            .analytics_strat_period
            .as_deref()
            .and_then(Period::from_id);
        // Persisted "By filter" search knobs; `TunerState::load` owns their normalization.
        let saved_tuner_iters = backend.read(cx).layout.analytics_tuner_iters;
        let saved_tuner_edges = backend.read(cx).layout.analytics_tuner_edges;
        let saved_tuner_seed = backend.read(cx).layout.analytics_tuner_seed.clone();
        let saved_tuner_train = backend.read(cx).layout.analytics_tuner_train;
        let saved_tuner_fields = backend.read(cx).layout.analytics_tuner_fields.clone();
        let saved_tuner_compose = backend.read(cx).layout.analytics_tuner_compose;
        // Strategy-list sort is process-persistent. Unknown keys return to the same
        // profit-descending default used before this preference existed.
        let saved_strat_sort =
            tuner::restore_strat_sort(backend.read(cx).layout.analytics_strat_sort.clone());
        // Profit metric from the previous run (default raw quote money).
        let saved_metric = if backend.read(cx).layout.analytics_profit_percent {
            ProfitMetric::Percent
        } else {
            ProfitMetric::Quote
        };
        // KPI matrix collapse state from the previous run (default expanded).
        let saved_kpi_collapsed = backend.read(cx).layout.analytics_kpi_collapsed;
        // Distribution card collapse state from the previous run (default expanded).
        let saved_hist_collapsed = backend.read(cx).layout.analytics_hist_collapsed;
        // Visible strategy-list columns from the previous run, one mask per axis. An older
        // config holding the single-mask key seeds all three, so a choice already made is
        // carried over instead of reset; absent entirely, each axis takes its own default.
        let saved_strat_cols = {
            let layout = &backend.read(cx).layout;
            tuner::restore_strat_columns(
                layout.analytics_strat_cols_modes2,
                layout.analytics_strat_cols_modes,
                layout.analytics_strat_cols2,
            )
        };
        // Calendar mode from the previous run, defaulting to Month.
        let saved_mode = backend
            .read(cx)
            .layout
            .analytics_heat_mode
            .as_deref()
            .and_then(calendar::CalMode::from_id)
            .unwrap_or(calendar::CalMode::Month);
        let session = backend.read(cx).ui_session.analytics.clone();

        // From/to calendars: selecting a day closes the popup and switches the period.
        let cal_from = cx.new(|cx| MoonCalendarState::new(window, cx));
        let cal_to = cx.new(|cx| MoonCalendarState::new(window, cx));
        if let Some(Period::Custom(f, t)) = saved_period {
            if let Some(d) = (f >= 0).then(|| day_of_secs(f)).flatten() {
                cal_from.update(cx, |s, cx| s.set_date(d, window, cx));
            }
            if let Some(d) = day_of_secs(t - 86_400) {
                cal_to.update(cx, |s, cx| s.set_date(d, window, cx));
            }
        }
        let cal_subs = vec![
            cx.subscribe_in(&cal_from, window, |this, _, ev, window, cx| {
                let MoonCalendarEvent::Selected(_) = ev;
                this.cal_from_open = false;
                this.apply_custom_range(window, cx);
            }),
            cx.subscribe_in(&cal_to, window, |this, _, ev, window, cx| {
                let MoonCalendarEvent::Selected(_) = ev;
                this.cal_to_open = false;
                this.apply_custom_range(window, cx);
            }),
        ];
        // Armed probe → open straight on the surface under observation, so the channel
        // reports the coin table rather than the summary nobody asked about.
        let probe = probe_enabled();
        let mut this = Self {
            backend,
            report_generation,
            valuation_generation,
            report_refresh: RefreshGate::new(initial_report_generation, std::time::Instant::now()),
            report_busy_retries: BusyRetryBudget::default(),
            tab: if probe { Tab::Strategies } else { session.tab },
            period: saved_period.unwrap_or(Period::CurMonth),
            strat_period: saved_strat_period.unwrap_or(Period::CurMonth),
            data_period: saved_period.unwrap_or(Period::CurMonth),
            data_dirty: false,
            cores: Vec::new(),
            last_cores_at: None,
            core_refresh_needed: false,
            core_refresh_timer_armed: false,
            sel_cores: session.sel_cores,
            side: SideFilter::All,
            // Default to Real, as in Report, because emulated trades add noise to the statistics.
            emu: Some(false),
            metric: saved_metric,
            data: ProfitLoadState::default(),
            strategy_data: ProfitLoadState::default(),
            strategy_data_period: saved_strat_period.unwrap_or(Period::CurMonth),
            strategy_dirty: true,
            undated: None,
            undated_error: None,
            undated_expanded: session.undated_expanded,
            write_error: None,
            busy_ops: 0,
            db_ops: 0,
            busy_since: None,
            seq: 0,
            hover_daily_bucket: None,
            hover_cum_bucket: None,
            hover_kind: None,
            sel_strategy: None,
            sel_extra: Vec::new(),
            strat_search: String::new(),
            strat_type: None,
            strat_active_only: true,
            strat_lists: tuner::StratListFilter::All,
            strat_search_input: None,
            strat_sort: saved_strat_sort,
            strat_cols: saved_strat_cols,
            strat_core_w: None,
            strat_visible: None,
            strat_scroll: MoonVirtualListScrollHandle::new(),
            cal_days: ProfitLoadState::default(),
            cal_seq: 0,
            cal_dirty: true,
            cal_mode: saved_mode,
            cal_ym: {
                use chrono::Datelike;
                let d = day_of_secs(moon_core::util::now_unix_ms_i64() / 1000).unwrap_or_default();
                (d.year(), d.month())
            },
            cal_day: (moon_core::util::now_unix_ms_i64() / 1000).div_euclid(86_400) * 86_400,
            cal_prev: LoadState::default(),
            strat_mode: if probe {
                tuner::StratMode::Coins
            } else {
                session.strat_mode
            },
            kpi_collapsed: saved_kpi_collapsed,
            hist_collapsed: saved_hist_collapsed,
            tuner: tuner::TunerState::load(
                saved_tuner_iters,
                saved_tuner_edges,
                saved_tuner_seed,
                saved_tuner_train,
                saved_tuner_fields,
                saved_tuner_compose,
            ),
            coins: tuner::CoinsState::default(),
            coin_lists: tuner::CoinListsState::default(),
            time_tuner: tuner::TimeTunerState::load(),
            cal_from,
            cal_to,
            cal_from_open: false,
            cal_to_open: false,
            integrity_poll_armed: false,
            _cal_subs: cal_subs,
            focus: cx.focus_handle(),
        };
        this.reload(cx);
        this
    }

    /// Return the latest report-derived generation visible to this Analytics window.
    ///
    /// Returns:
    ///     Wrapping sum of report and valuation generations.
    fn current_report_generation(&self) -> u64 {
        let reports = self
            .report_generation
            .as_ref()
            .map(|generation| generation.load(Ordering::Relaxed))
            .unwrap_or(0);
        let valuation = self
            .valuation_generation
            .as_ref()
            .map(|generation| generation.load(Ordering::Relaxed))
            .unwrap_or(0);
        reports.wrapping_add(valuation)
    }

    /// Mark report-derived caches stale and schedule a load-shed automatic refresh.
    ///
    /// Args:
    ///     cx: GPUI context used to schedule or start the refresh.
    fn observe_report_generation(&mut self, cx: &mut Context<Self>) {
        let generation = self.current_report_generation();
        if !self
            .report_refresh
            .observe_generation(generation, std::time::Instant::now())
        {
            return;
        }
        self.report_busy_retries.observe_generation();
        self.mark_report_data_stale();
        self.schedule_report_refresh(cx);
    }

    /// Mark report-derived results stale without clearing drafts or retiring snapshot progress.
    ///
    /// The method has no return value; the refresh gate later reloads the visible surface.
    fn mark_report_data_stale(&mut self) {
        self.data_dirty = true;
        self.strategy_dirty = true;
        self.cal_dirty = true;
        self.tuner.mark_report_stale();
        self.time_tuner.mark_report_stale();
        self.coins.mark_report_stale();
    }

    /// Acknowledge every committed generation visible when a refresh begins.
    fn acknowledge_report_refresh(&mut self) {
        let generation = self.current_report_generation();
        self.report_refresh
            .refresh_started(generation, std::time::Instant::now());
    }

    /// Settle the Busy retry episode and optionally schedule its next bounded attempt.
    ///
    /// Permanent corruption and unclassified I/O failures remain visible instead of creating an
    /// endless full-history retry loop.
    ///
    /// Args:
    ///     error: Transient read failure, or `None` when the read escaped SQLite contention.
    ///     cx: GPUI context used to arm the quiet-period retry.
    fn settle_report_refresh_retry(&mut self, error: Option<&ReadFail>, cx: &mut Context<Self>) {
        if error.and_then(ReadFail::kind) != Some(FailKind::Busy) {
            self.report_busy_retries.resolve();
            return;
        }
        if !self.report_busy_retries.claim() {
            log::warn!("analytics: automatic database retry budget exhausted");
            return;
        }
        self.report_refresh
            .request_refresh(std::time::Instant::now(), false);
        self.schedule_report_refresh(cx);
    }

    /// Queue a visible catch-up behind any Analytics database work already in flight.
    ///
    /// Args:
    ///     show_overlay: Whether a user action requires blocking progress feedback.
    ///     cx: GPUI context used to arm the shared refresh gate.
    pub(super) fn request_report_refresh(&mut self, show_overlay: bool, cx: &mut Context<Self>) {
        self.report_refresh
            .request_refresh(std::time::Instant::now(), show_overlay);
        self.schedule_report_refresh(cx);
    }

    /// Plan or start the sole trailing report refresh for this open window.
    ///
    /// Args:
    ///     cx: GPUI context used to arm a timer or start database work.
    pub(super) fn schedule_report_refresh(&mut self, cx: &mut Context<Self>) {
        let db_active = self.db_ops > 0 || self.busy_ops > 0;
        match self
            .report_refresh
            .plan(std::time::Instant::now(), db_active)
        {
            RefreshPlan::Idle => {}
            RefreshPlan::Now { show_overlay } => {
                self.refresh_visible_report_data(show_overlay, cx);
            }
            RefreshPlan::After(wait) => {
                cx.spawn(async move |this, cx| {
                    let executor = cx.update(|cx| cx.background_executor().clone());
                    executor.timer(wait).await;
                    let _ = cx.update(|cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.report_refresh.timer_fired();
                            this.schedule_report_refresh(cx);
                        });
                    });
                })
                .detach();
            }
        }
    }

    /// Recompute only the surface visible when a report-driven refresh becomes due.
    ///
    /// Strategies uses a compact list/coin base instead of paying for Summary-only charts,
    /// rankings, comparisons, and per-core series on every report commit.
    ///
    /// Args:
    ///     show_overlay: Whether coalesced user work requires blocking progress feedback.
    ///     cx: GPUI context used to start the visible surface's background reads.
    fn refresh_visible_report_data(&mut self, show_overlay: bool, cx: &mut Context<Self>) {
        self.acknowledge_report_refresh();
        let base_dirty = match self.tab {
            Tab::Strategies => self.strategy_dirty || self.core_refresh_needed,
            _ => self.data_dirty,
        };
        match visible_refresh(self.tab, base_dirty) {
            VisibleRefresh::Summary => self.reload_summary(true, show_overlay, cx),
            VisibleRefresh::StrategyBaseAndAxis => {
                self.reload_strategy_base(true, true, show_overlay, cx);
            }
            VisibleRefresh::StrategyAxis => {
                self.reload_axis_after_report(self.strat_mode, show_overlay, cx);
            }
            VisibleRefresh::Calendar => self.reload_calendar_after_report(show_overlay, cx),
        }
    }

    /// Period of the active tab. Tuning keeps its OWN time window, separate from Summary.
    /// Calendar does not use the period bar because it has its own navigation, and
    /// `reload_calendar` builds its query directly without calling this method.
    fn active_period(&self) -> Period {
        match self.tab {
            Tab::Strategies => self.strat_period,
            _ => self.period,
        }
    }

    /// Decide whether the shared core selector is due and preserve its original deadline.
    ///
    /// Args:
    ///     cx: GPUI context used to arm the sole trailing metadata timer.
    ///
    /// Returns:
    ///     `true` when the caller should include cores in its compound snapshot.
    fn core_metadata_due(&mut self, cx: &mut Context<Self>) -> bool {
        let wait = refresh::core_metadata_wait(self.last_cores_at, std::time::Instant::now());
        self.core_refresh_needed = true;
        if wait.is_zero() {
            return true;
        }
        self.schedule_core_metadata_refresh(wait, cx);
        false
    }

    /// Arm one trailing core-list refresh shared by every Analytics tab.
    ///
    /// Args:
    ///     wait: Remaining time until the one-minute metadata cadence.
    ///     cx: GPUI context used to run and publish the timer.
    fn schedule_core_metadata_refresh(
        &mut self,
        wait: std::time::Duration,
        cx: &mut Context<Self>,
    ) {
        if self.core_refresh_timer_armed {
            return;
        }
        self.core_refresh_timer_armed = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(wait).await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.core_refresh_timer_armed = false;
                    if !this.core_refresh_needed {
                        return;
                    }
                    let remaining =
                        refresh::core_metadata_wait(this.last_cores_at, std::time::Instant::now());
                    if remaining.is_zero() {
                        this.request_report_refresh(false, cx);
                    } else {
                        this.schedule_core_metadata_refresh(remaining, cx);
                    }
                });
            });
        })
        .detach();
    }

    /// Current filters in the structure shared by Summary and Tuning, using the active tab's
    /// period (`active_period`).
    fn query(&self) -> Query {
        let (from, to) = self.active_period().range();
        Query {
            from,
            to,
            cores: self.cores_selected(),
            side: self.side,
            emulator: self.emu,
            strategies: Vec::new(),
            metric: self.metric,
        }
    }

    /// Return selected cores for a query, using an empty vector only for implicit or complete All.
    ///
    /// Returns:
    ///     Explicit selected ids for a partial or stale selection, or empty for no core filter.
    fn cores_selected(&self) -> Vec<u64> {
        crate::controls::normalized_core_filter_ids(
            self.cores.iter().map(|(core, _)| *core),
            &self.sel_cores,
        )
    }

    /// Start accounting for a blocking background operation.
    ///
    /// Every operation that calls `op_started` must decrement exactly once, including stale
    /// completions. Completion handlers publish or discard their result first, then balance this
    /// counter before deferred report work is scheduled.
    ///
    /// The idle-to-busy transition arms one delayed repaint for the whole batch. Keeping timer
    /// creation out of `render` prevents a repaint from scheduling its own successor.
    ///
    /// Args:
    ///     cx: GPUI context used to arm the delayed overlay repaint.
    pub(super) fn op_started(&mut self, cx: &mut Context<Self>) {
        self.busy_ops += 1;
        if self.busy_since.is_none() {
            let opened_at = std::time::Instant::now();
            self.busy_since = Some(opened_at);
            cx.spawn(async move |this, cx| {
                let executor = cx.update(|cx| cx.background_executor().clone());
                executor.timer(BUSY_OVERLAY_DELAY).await;
                let _ = cx.update(|cx| {
                    let _ = this.update(cx, |this, cx| {
                        // Detached timers outlive their batches, so match the captured start time
                        // before repainting; a later batch has its own delay and timer.
                        if this.busy_since == Some(opened_at) {
                            cx.notify();
                        }
                    });
                });
            })
            .detach();
        }
    }
    /// Raise a write failure where the user is looking, and log it.
    ///
    /// Shown until dismissed rather than for a few seconds: this is the difference between
    /// "your strategies were changed" and "they were not", and it must not be possible to
    /// miss by looking away.
    pub(super) fn set_write_error(&mut self, msg: String, cx: &mut Context<Self>) {
        log::warn!("analytics: {msg}");
        self.write_error = Some(msg);
        cx.notify();
    }

    /// Finish one blocking operation.
    ///
    /// Args:
    ///     cx: GPUI context used to repaint the overlay.
    pub(super) fn op_finished(&mut self, cx: &mut Context<Self>) {
        self.busy_ops = self.busy_ops.saturating_sub(1);
        if self.busy_ops == 0 {
            self.busy_since = None;
        }
        cx.notify();
    }

    /// Whether the current batch has been running long enough to show the busy overlay.
    ///
    /// This remains a pure render-time read; `op_started` owns the delayed repaint.
    ///
    /// Returns:
    ///     `true` once the open batch is older than `BUSY_OVERLAY_DELAY`.
    fn busy_overlay_due(&self) -> bool {
        self.busy_since
            .is_some_and(|since| since.elapsed() >= BUSY_OVERLAY_DELAY)
    }

    /// Reload Analytics after a user-controlled period or filter scope change.
    ///
    /// Args:
    ///     cx: GPUI context used to start all required background reads.
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.report_busy_retries.reset();
        self.acknowledge_report_refresh();
        // The tuner uses the same filters: invalidate it and recompute immediately in the active
        // mode, or defer recomputation until the next entry into Filters mode.
        self.tuner.invalidate();
        // The By time axis uses the shared filters and `strat_period`. This common reload path
        // conservatively retires its in-flight auto-suggestion even on a Summary-only period
        // change; otherwise a stale result could be written into v1 and become saveable.
        // The profile is marked stale the same way, recomputed immediately below when the
        // axis is the active one, or deferred until entry.
        self.time_tuner.invalidate();
        // The "By coin" axis lives on the same scope (its "Fact vs v1" matrix included):
        // mark it stale. It is NOT started here, unlike the other axes: it expands its coin
        // lists against the base coin set that this very request is about to replace, so
        // starting it now would plan against the PREVIOUS period's coins (or, on a first
        // show, against none at all). It is armed from the completion handler below.
        self.coins.invalidate();
        // The list panels ride the same reload, so they are retired with it — otherwise a
        // reply already in flight for the previous scope lands under the new heading.
        self.coin_lists.invalidate();
        // Calendar uses the same filters: mark it stale and recompute immediately on the active
        // tab, or defer until entry.
        self.cal_seq = self.cal_seq.wrapping_add(1);
        self.cal_dirty = true;
        if self.tab == Tab::Calendar {
            self.reload_calendar(cx);
        }
        self.data_dirty = true;
        self.strategy_dirty = true;
        self.undated = None;
        self.undated_error = None;
        match self.tab {
            Tab::Summary => self.reload_summary(false, true, cx),
            Tab::Strategies => self.reload_strategy_base(false, true, true, cx),
            Tab::Calendar => {}
        }
    }

    /// Reload the full Summary without resetting tuner drafts.
    ///
    /// Args:
    ///     after_report: Whether report-style catch-up may preserve the previous undated value
    ///         while exposing a classified read error.
    ///     show_overlay: Whether this refresh must block interaction with visible progress feedback.
    ///     cx: GPUI context used to run and publish the shared background query.
    fn reload_summary(&mut self, after_report: bool, show_overlay: bool, cx: &mut Context<Self>) {
        // Mark the request at its start so an error from another period cannot
        // remain under the current period label.
        // Quote identity belongs to the request scope. Retaining stale scalar data would render
        // it briefly under the new metric/core filters before the exact unit is known.
        self.data = ProfitLoadState::default();
        // Drop every chart hover: the bars/columns under the cursor are about to be
        // replaced (and on a single-day period the right card swaps its whole element
        // tree), so they never fire `hovered = false` and a stale index would re-open a
        // popup with no cursor on it — pointing at another period's bucket.
        self.hover_daily_bucket = None;
        self.hover_cum_bucket = None;
        self.hover_kind = None;
        self.seq = self.seq.wrapping_add(1);
        let req = self.seq;
        let report_req = self.current_report_generation();
        // Record the ACTIVE tab's time window that `data` is being computed for.
        self.data_period = self.active_period();
        let q = self.query();
        let read_cores = self.core_metadata_due(cx);
        self.spawn_db(
            show_overlay,
            cx,
            move || moon_core::db::analytics::summary_data(&q, read_cores),
            move |this, result, cx| {
                if this.seq != req {
                    return; // The period or filters have already changed.
                }
                let data = result.data;
                let undated = result.undated;
                let cores = result.cores;
                let data_error = data.as_ref().err().cloned();
                let undated_error = undated.as_ref().err().cloned();
                let cores_error = cores
                    .as_ref()
                    .and_then(|cores| cores.as_ref().err())
                    .cloned();
                let retry_error = data_error
                    .as_ref()
                    .filter(|error| error.kind() == Some(FailKind::Busy))
                    .or_else(|| {
                        undated
                            .as_ref()
                            .err()
                            .filter(|error| error.kind() == Some(FailKind::Busy))
                    })
                    .or_else(|| {
                        cores_error
                            .as_ref()
                            .filter(|error| error.kind() == Some(FailKind::Busy))
                    })
                    .cloned();
                if let Some(Ok(cores)) = cores {
                    this.cores = cores;
                    this.last_cores_at = Some(std::time::Instant::now());
                    this.core_refresh_needed = false;
                }
                this.data.apply(data);
                this.data_dirty = refresh::report_result_is_stale(
                    report_req,
                    this.current_report_generation(),
                    data_error.is_some() || undated_error.is_some() || cores_error.is_some(),
                );
                this.apply_undated_result(undated, after_report);
                this.settle_report_refresh_retry(retry_error.as_ref(), cx);
                cx.notify();
            },
        );
    }

    /// Reload the compact Strategies base and optionally continue with its visible axis.
    ///
    /// Args:
    ///     after_report: Whether to preserve the visible snapshot while loading and on read
    ///         failure, and to use report-style catch-up and retry semantics.
    ///     chain_visible_axis: Whether a successful base read should continue into the active axis.
    ///     show_overlay: Whether this refresh must block interaction with visible progress feedback.
    ///     cx: GPUI context used to run and publish the shared background query.
    fn reload_strategy_base(
        &mut self,
        after_report: bool,
        chain_visible_axis: bool,
        show_overlay: bool,
        cx: &mut Context<Self>,
    ) {
        // Manual scope changes must not show values from the old scope. An automatic report
        // refresh keeps the current snapshot until its replacement lands, so the strategy list,
        // quote selector, and trade count do not blink through Loading after every live trade.
        if !after_report {
            self.strategy_data = ProfitLoadState::default();
        }
        self.seq = self.seq.wrapping_add(1);
        let req = self.seq;
        let report_req = self.current_report_generation();
        self.strategy_data_period = self.strat_period;
        let q = self.query();
        let read_cores = self.core_metadata_due(cx);
        self.spawn_db(
            show_overlay,
            cx,
            move || moon_core::db::analytics::strategy_base_data(&q, read_cores),
            move |this, result, cx| {
                if this.seq != req {
                    return;
                }
                let data = result.data;
                let undated = result.undated;
                let cores = result.cores;
                let data_error = data.as_ref().err().cloned();
                let undated_error = undated.as_ref().err().cloned();
                let cores_error = cores
                    .as_ref()
                    .and_then(|cores| cores.as_ref().err())
                    .cloned();
                let retry_error = data_error
                    .as_ref()
                    .filter(|error| error.kind() == Some(FailKind::Busy))
                    .or_else(|| {
                        undated_error
                            .as_ref()
                            .filter(|error| error.kind() == Some(FailKind::Busy))
                    })
                    .or_else(|| {
                        cores_error
                            .as_ref()
                            .filter(|error| error.kind() == Some(FailKind::Busy))
                    })
                    .cloned();
                if let Some(Ok(cores)) = cores {
                    this.cores = cores;
                    this.last_cores_at = Some(std::time::Instant::now());
                    this.core_refresh_needed = false;
                }
                // A same-scope automatic failure must leave the last ready snapshot visible while
                // the retry gate settles. Manual scope changes have already retired the old data,
                // so they publish the classified failure instead of showing stale values.
                if !after_report || data_error.is_none() {
                    this.strategy_data.apply(data);
                    this.strat_core_w = None;
                    // Both caches describe the group set that was just replaced. The memo's key
                    // also carries that set's address, but an address is only unique among LIVE
                    // allocations: a failed load drops the old buffer and a later successful one
                    // can be handed the same address back, which unchanged filters would then
                    // accept as "same data". Dropping the memo here is what closes that.
                    this.strat_visible = None;
                }
                this.strategy_dirty = refresh::report_result_is_stale(
                    report_req,
                    this.current_report_generation(),
                    data_error.is_some() || undated_error.is_some() || cores_error.is_some(),
                );
                this.apply_undated_result(undated, after_report);
                let probe_took_over = probe_selects_strategy() && this.probe_select_first(cx);
                if refresh::strategy_base_allows_axis(
                    data_error.is_some(),
                    undated_error.is_some(),
                    cores_error.is_some(),
                ) && this.strategy_data.split().is_none()
                    && !probe_took_over
                    && chain_visible_axis
                    && this.tab == Tab::Strategies
                {
                    this.reload_axis_after_report(this.strat_mode, show_overlay, cx);
                }
                this.settle_report_refresh_retry(retry_error.as_ref(), cx);
                cx.notify();
            },
        );
    }

    /// Apply an undated-close result without erasing a same-scope value on automatic failure.
    fn apply_undated_result(
        &mut self,
        result: moon_core::db::ReadResult<moon_core::db::analytics::UndatedCloses>,
        preserve_previous: bool,
    ) {
        match result {
            Ok(undated) => {
                self.undated = Some(undated);
                self.undated_error = None;
            }
            Err(error) => {
                if !preserve_previous {
                    self.undated = None;
                }
                self.undated_error = Some(error);
            }
        }
    }

    // cal_query/cal_query_prev/reload_calendar live in calendar/mod.rs — a page's
    // recomputation belongs beside that page.

    /// Apply a period preset to the active tab and reload its scope immediately.
    ///
    /// Args:
    ///     p: Period selected by the user.
    ///     window: Window owning the shared date-picker states.
    ///     cx: GPUI context used to persist the choice and start the reload.
    fn set_period(&mut self, p: Period, window: &mut Window, cx: &mut Context<Self>) {
        // Clicking the active preset remains an immediate manual refresh, independent of the
        // load-shed automatic path. The period bar edits the ACTIVE tab's time window: Summary and
        // Tuning are independent.
        let strat = self.tab == Tab::Strategies;
        if strat {
            self.strat_period = p;
        } else {
            self.period = p;
        }
        // A preset supersedes the custom range, so clear the from/to fields.
        if !matches!(p, Period::Custom(..)) {
            for cal in [&self.cal_from, &self.cal_to] {
                cal.update(cx, |s, cx| s.set_date(MoonDate::Single(None), window, cx));
            }
        }
        // Persist the selection under its OWN key so the window reopens with it next time.
        let id = Some(p.persist_id());
        self.backend.update(cx, |b, _| {
            let slot = if strat {
                &mut b.layout.analytics_strat_period
            } else {
                &mut b.layout.analytics_period
            };
            if *slot != id {
                *slot = id;
                b.layout_dirty = true;
            }
        });
        self.reload(cx);
        cx.notify();
    }

    /// Synchronize the shared `MoonCalendarState` from/to fields with the active tab's period,
    /// so after a tab switch the period bar shows that tab's OWN range rather than the previous
    /// tab's range.
    fn sync_period_pickers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (from_date, to_date) = match self.active_period() {
            Period::Custom(f, t) => (
                if f >= 0 { day_of_secs(f) } else { None },
                day_of_secs(t - 86_400),
            ),
            _ => (None, None),
        };
        self.cal_from.update(cx, |s, cx| {
            s.set_date(MoonDate::Single(from_date), window, cx)
        });
        self.cal_to.update(cx, |s, cx| {
            s.set_date(MoonDate::Single(to_date), window, cx)
        });
    }

    /// Recompute the period from the from/to calendars. If both are empty, keep the existing
    /// period. Otherwise, an empty from means all history, an empty to means until tomorrow, and
    /// bounds are swapped if to precedes from.
    fn apply_custom_range(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut f = self.cal_from.read(cx).date().start();
        let mut t = self.cal_to.read(cx).date().start();
        if let (Some(a), Some(b)) = (f, t) {
            if b < a {
                (f, t) = (Some(b), Some(a));
                self.cal_from.update(cx, |s, cx| s.set_date(b, window, cx));
                self.cal_to.update(cx, |s, cx| s.set_date(a, window, cx));
            }
        }
        if f.is_none() && t.is_none() {
            return;
        }
        let now = moon_core::util::now_unix_ms_i64() / 1000;
        let tomorrow = now.div_euclid(86_400) * 86_400 + 86_400;
        let from = f.map(secs_of_day).unwrap_or(-1);
        // The to date is inclusive, so the range ends at the following midnight.
        let to = t.map(|d| secs_of_day(d) + 86_400).unwrap_or(tomorrow);
        self.set_period(Period::Custom(from, to), window, cx);
    }

    /// Toggle one core or the All row in the multi-selection.
    ///
    /// The All row clears a selection containing every current core, or replaces any partial or
    /// stale selection with the current full set. Both empty and full representations query all.
    ///
    /// Args:
    ///     core: Core to toggle, or `None` for the All row.
    ///     cx: Analytics context used to reload data and request a repaint.
    ///
    /// Returns:
    ///     Nothing; the in-memory filter and loaded analytics are updated in place.
    fn toggle_core(&mut self, core: Option<u64>, cx: &mut Context<Self>) {
        match core {
            None => {
                let all = self.cores.iter().map(|(core, _)| *core).collect();
                crate::controls::toggle_all_core_selection(&mut self.sel_cores, all);
            }
            Some(c) => {
                if !self.sel_cores.remove(&c) {
                    self.sel_cores.insert(c);
                }
            }
        }
        self.core_selection_changed(cx);
    }

    /// Toggle every still-available core from one clicked exchange section.
    ///
    /// Empty means All before the click, so the first exchange selection becomes explicit. A second
    /// click removes the exchange when all of its available cores are selected. Partial selections
    /// retain cores from other exchanges, while a stale-only batch is a no-op.
    ///
    /// Args:
    ///     exchange_cores: Core ids captured from the rendered Analytics exchange section.
    ///     cx: Analytics context used to persist and reload a changed selection.
    ///
    /// Returns:
    ///     Nothing; a changed selection is persisted and loaded atomically.
    fn toggle_exchange_cores(&mut self, exchange_cores: Vec<u64>, cx: &mut Context<Self>) {
        let available = self.cores.iter().map(|(core, _)| *core).collect();
        if crate::controls::toggle_exchange_cores(&mut self.sel_cores, &available, exchange_cores) {
            self.core_selection_changed(cx);
        }
    }

    /// Persist the Analytics core filter and reload every dependent surface.
    ///
    /// Args:
    ///     cx: Analytics context used to update session state, reload data, and request a repaint.
    ///
    /// Returns:
    ///     Nothing; the current core selection is published and reloaded in place.
    fn core_selection_changed(&mut self, cx: &mut Context<Self>) {
        let selected = self.sel_cores.clone();
        self.backend.update(cx, |b, _| {
            b.ui_session.analytics.sel_cores = selected;
        });
        self.reload(cx);
        cx.notify();
    }

    fn set_side(&mut self, side: SideFilter, cx: &mut Context<Self>) {
        if self.side != side {
            self.side = side;
            self.reload(cx);
            cx.notify();
        }
    }

    fn set_emu(&mut self, emu: Option<bool>, cx: &mut Context<Self>) {
        if self.emu != emu {
            self.emu = emu;
            self.reload(cx);
            cx.notify();
        }
    }

    /// Switch between raw quote money and percent. Every figure and tuner sweep is computed under
    /// the selected lens, so the switch persists and reloads rather than only changing labels.
    ///
    /// Args:
    ///     metric: New raw-quote or Percent lens.
    ///     cx: Analytics view context used to persist and reload.
    ///
    /// Returns:
    ///     Nothing.
    fn set_metric(&mut self, metric: ProfitMetric, cx: &mut Context<Self>) {
        if self.metric == metric {
            return;
        }
        self.metric = metric;
        self.backend.update(cx, |b, _| {
            b.layout.analytics_profit_percent = metric == ProfitMetric::Percent;
            b.layout_dirty = true;
        });
        self.reload(cx);
        cx.notify();
    }

    /// Collapse/expand the shared "Fact vs variants" KPI matrix. Collapsed keeps only its two
    /// top rows (trades + profit), so the fields grid below it fits on short screens. A pure
    /// display lens — unlike `set_metric` it changes no query, so it only persists and repaints.
    fn toggle_kpi_collapsed(&mut self, cx: &mut Context<Self>) {
        self.kpi_collapsed = !self.kpi_collapsed;
        self.backend.update(cx, |b, _| {
            b.layout.analytics_kpi_collapsed = self.kpi_collapsed;
            b.layout_dirty = true;
        });
        cx.notify();
    }

    /// Collapse/expand the "By filter" distribution card. Collapsed keeps its title and subtitle
    /// and folds the chart away, so the fields grid above it fits on short screens.
    ///
    /// A pure display lens, like `toggle_kpi_collapsed`: it persists and repaints, and it must NOT
    /// gate the histogram read. `TunerState::needs_reload` counts `hist_dirty`, so a read suppressed
    /// while collapsed would leave that flag permanently set — the reload gate would re-fire every
    /// frame, and expanding would show a spinner where the user left a chart.
    fn toggle_hist_collapsed(&mut self, cx: &mut Context<Self>) {
        self.hist_collapsed = !self.hist_collapsed;
        self.backend.update(cx, |b, _| {
            b.layout.analytics_hist_collapsed = self.hist_collapsed;
            b.layout_dirty = true;
        });
        cx.notify();
    }
}

impl EventEmitter<()> for AnalyticsView {}
impl Focusable for AnalyticsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AnalyticsView {
    /// Render the active Analytics tab with quote-safe scope state and shared chrome.
    ///
    /// Args:
    ///     window: Owning window used for responsive chrome width.
    ///     cx: Analytics view context.
    ///
    /// Returns:
    ///     Complete Analytics surface.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let (unit, split) = match self.tab {
            Tab::Summary => (self.data.unit(), self.data.split().cloned()),
            Tab::Strategies => (
                self.strategy_data.unit(),
                self.strategy_data.split().cloned(),
            ),
            Tab::Calendar => (self.cal_days.unit(), self.cal_days.split().cloned()),
        };
        // Arm shared formatters with the exact comparable unit published by the active tab.
        set_pnl_unit(unit);
        let chrome_width = match window.window_bounds() {
            WindowBounds::Windowed(b)
            | WindowBounds::Maximized(b)
            | WindowBounds::Fullscreen(b) => f32::from(b.size.width),
        };
        let body = match split {
            Some(totals) => quote_split_note(&totals, p, cx),
            None => match self.tab {
                Tab::Summary => self.summary_tab(p, cx),
                Tab::Strategies => self.strategies_tab(p, window, cx),
                Tab::Calendar => self.calendar_tab(p, cx),
            },
        };
        // Tabs divide their own height, pinning bottom bars to the window and scrolling content
        // internally, so there is no outer scroll.
        let body_scrolls = false;
        let integrity = self.integrity_note(cx);
        let write_error = self.write_error.clone();
        let busy_overlay = self.busy_overlay_due();
        v_flex()
            .size_full()
            .relative()
            .bg(moon(p.shell))
            .text_color(moon(p.text))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .track_focus(&self.focus)
            .child(analytics_header(p, cx))
            .child(self.tabs_bar(p, cx))
            // Calendar has its OWN month navigation, so hide the from/to period bar there; its
            // body has a separate Previous/month/Next row.
            .when(self.tab != Tab::Calendar, |el| {
                el.child(self.period_bar(p, cx))
            })
            // Show the integrity banner on EVERY tab: a damaged replica matters on Calendar too,
            // because it reads the same database.
            .when_some(integrity, |el, (title, detail)| {
                el.child(
                    // Do not use `.banner()`: MoonAlert renders the title only in the
                    // non-banner form (alert.rs `when(!self.banner, ..title..)`),
                    // so the banner variant would drop the localized heading and
                    // show the bare SQLite diagnostic line.
                    div()
                        .px(design::ui_px(cx, 10.0))
                        .pb(design::ui_px(cx, 6.0))
                        .child(MoonAlert::warning("an-integrity-banner", detail).title(title)),
                )
            })
            // A write that reached nobody. Above the undated-close notice deliberately: that one is
            // about numbers being incomplete, this one is about the user's strategies not
            // having changed when they were told they had.
            .when_some(write_error, |el, msg| {
                el.child(
                    div()
                        .px(design::ui_px(cx, 10.0))
                        .pb(design::ui_px(cx, 6.0))
                        .child(
                            h_flex()
                                .w_full()
                                .gap(design::ui_px(cx, 6.0))
                                .items_start()
                                .child(
                                    div().flex_1().min_w_0().child(
                                        MoonAlert::error("an-write-error", msg)
                                            .title(t!("analytics.write_failed_title").to_string()),
                                    ),
                                )
                                .child(
                                    moon_ui::MoonButton::new("an-write-error-x")
                                        .variant(moon_ui::MoonButtonVariant::Ghost)
                                        .size(moon_ui::MoonButtonSize::Micro)
                                        .label(t!("analytics.write_failed_ok").to_string())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.write_error = None;
                                            cx.notify();
                                        }))
                                        .render(),
                                ),
                        ),
                )
            })
            // Keep omitted money adjacent to the period bar; `notice_strip` returns no element
            // when the scoped query has nothing to report.
            .children(self.notice_strip(p, cx))
            .child(
                div()
                    .id("analytics-body")
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .when(body_scrolls, |el| el.overflow_y_scroll())
                    .child(body),
            )
            // If a background operation outlasts the delay, dim the window and occlude clicks;
            // otherwise long scans are invisible while clicks accumulate.
            .when(busy_overlay, |el| {
                el.child(
                    div()
                        .id("an-busy-overlay")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(moon_alpha(p.shell, 0.45))
                        .child(
                            h_flex()
                                .px(design::ui_px(cx, 14.0))
                                .py(design::ui_px(cx, 7.0))
                                .rounded(design::ui_px(cx, 6.0))
                                .bg(moon(p.panel_high))
                                .border_1()
                                .border_color(moon(p.border))
                                .text_size(design::t_body(cx))
                                .text_color(moon(p.text_soft))
                                .child(t!("common.loading").to_string()),
                        ),
                )
            })
            .child(
                MoonWindowFrame::tool("analytics-window-frame-hit", chrome_width)
                    .header_height(ANALYTICS_HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}

/// Render the safe replacement for raw analytics over mixed or unknown quote currencies.
///
/// Args:
///     totals: Known quote buckets and unknown row count for the active scope.
///     p: Active MoonUI palette.
///     cx: Analytics render context.
///
/// Returns:
///     A centered, wrapping explanation with split totals and recovery guidance.
fn quote_split_note(
    totals: &QuoteBreakdown,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    let mut chips = h_flex().flex_wrap().justify_center().gap_2();
    for total in &totals.totals {
        let color = if total.profit > 0.0 {
            p.green
        } else if total.profit < 0.0 {
            p.red
        } else {
            p.text_soft
        };
        chips = chips.child(
            div()
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(moon(p.table_head))
                .text_color(moon(color))
                .child(quote_total_text(*total)),
        );
    }
    if totals.unknown_orders > 0 {
        chips = chips.child(
            div()
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(moon(p.table_head))
                .text_color(moon(p.orange))
                .child(t!("analytics.quote_unknown_orders", n = totals.unknown_orders).to_string()),
        );
    }
    let coverage_note = totals.valuation.and_then(|coverage| {
        (coverage.eligible_orders > 0).then(|| {
            let mut text = t!(
                "analytics.quote_valuation_progress",
                ready = coverage.valued_orders,
                total = coverage.eligible_orders
            )
            .to_string();
            if coverage.unavailable_orders > 0 {
                text.push_str(" · ");
                text.push_str(
                    &t!(
                        "analytics.quote_valuation_unavailable",
                        n = coverage.unavailable_orders
                    )
                    .to_string(),
                );
            }
            text
        })
    });
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .p_6()
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(moon(p.orange))
                .child(t!("analytics.quote_split_title").to_string()),
        )
        .child(
            div()
                .max_w(design::font_w_px(cx, 700.0))
                .text_center()
                .text_color(moon(p.text_soft))
                .child(
                    if totals.unknown_orders > 0 {
                        t!("analytics.quote_unknown_detail")
                    } else {
                        t!("analytics.quote_split_detail")
                    }
                    .to_string(),
                ),
        )
        .child(chips)
        .children(coverage_note.map(|note| {
            div()
                .text_color(moon(p.orange))
                .child(note)
                .into_any_element()
        }))
        .child(
            div()
                .text_color(moon(p.text_muted))
                .child(t!("analytics.quote_split_orders", n = totals.orders).to_string()),
        )
        .into_any_element()
}

/// Format one exact quote total with enough precision for crypto-denominated reports.
///
/// Args:
///     total: Known quote aggregate.
///
/// Returns:
///     Signed compact amount followed by its ticker.
fn quote_total_text(total: moon_core::db::QuoteTotal) -> String {
    let amount =
        moon_core::util::fmt::compact(total.profit.abs(), total.currency.display_decimals());
    let sign = if total.profit >= 0.0 { "+" } else { "-" };
    format!("{sign}{amount} {}", total.currency.ticker())
}

fn analytics_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("analytics-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, ANALYTICS_HEADER_H, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(moon(p.shell_high))
        .border_b(px(1.0))
        .border_color(moon_alpha(p.border, 1.0))
        .child(
            MoonWindowFrame::tool("analytics-titlebar-title", 0.0)
                .title_cluster(t!("analytics.window_title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("analytics-window-frame-visual", 0.0)
                    .header_height(ANALYTICS_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Open the singleton Analytics tool window, activating the live handle stored in `Backend`
/// when one exists and replacing a stale handle otherwise.
pub fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    if let Some(handle) = backend.read(cx).analytics_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
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
        saved.map(|g| point(px(g.x as f32), px(g.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let mut opts = crate::window::windowing::tool_window_options(
        t!("analytics.window_title").to_string(),
        WindowBounds::Windowed(bounds),
        Some(size(px(860.0), px(520.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| AnalyticsView::new(b, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        backend.update(cx, |bk, _| bk.analytics_window = Some(handle));
        crate::window::windowing::activate_new_window(handle.into(), cx);
    }
}
