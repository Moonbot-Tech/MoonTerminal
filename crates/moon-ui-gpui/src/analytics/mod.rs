//! The Analytics window provides report analyzers over the `orders_rep` replica
//! (see the analytics-panel-plan: summary → comparisons → heatmap → calendar).
//!
//! It is a separate singleton OS window (following the Screener pattern), with geometry persisted
//! in `layout.analytics_window`. Its MoonButton tab strip (as in Settings) contains Summary,
//! Calendar, and Strategy Tuning; the tuning workspace provides By filter, By coin, and By time
//! modes.
//! `moon_core::db::analytics` computes the data on the background executor (the full-period
//! SQLite query never runs on the UI thread). Data is re-queried ONLY in response to user actions:
//! opening the window, changing the period or filters, or clicking the active period preset again
//! to refresh manually. A tab switch reloads only when its data is stale, missing, or for a
//! different period.

/// The shared spawn+overlay envelope of every background DB read.
mod bg;
mod calendar;
/// Period presets, window tabs and date helpers — the time axis shared by every page.
mod period;
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

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonAlert, MoonBackgroundPolicy, MoonCalendarEvent, MoonCalendarState, MoonDate,
    MoonInputState, MoonPalette, MoonWindowFrame, Root, h_flex, v_flex,
};
use rust_i18n::t;

use crate::design::{moon, moon_alpha};
use crate::{Backend, design};
use moon_core::db::analytics::{DayCell, Query, Summary};
use moon_core::db::{ProfitMetric, SideFilter};

use crate::load_state::{LoadState, note_el};

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

// Percent-vs-USDT profit unit for this frame's profit formatters.
//
// Set once from the active metric at the top of `AnalyticsView::render` and read by the shared
// profit formatters (`summary::fmt_signed`, the calendar and tuner cells), so a "%" suffix
// appears in percent mode without threading the metric through every signature. The window
// renders on the UI thread, so a thread-local stays consistent within a frame.
thread_local! {
    static PNL_PCT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
/// Record the active profit unit for this frame's formatters.
pub(in crate::analytics) fn set_pnl_pct(on: bool) {
    PNL_PCT.with(|c| c.set(on));
}
/// Is the window rendering percent profit (report `Profit` column) rather than USDT?
pub(in crate::analytics) fn pnl_is_pct() -> bool {
    PNL_PCT.with(|c| c.get())
}
/// Unit suffix for a profit-metric figure: "%" in percent mode, empty in USDT mode.
pub(in crate::analytics) fn pnl_suffix() -> &'static str {
    if pnl_is_pct() { "%" } else { "" }
}

/// State of the Analytics window.
pub struct AnalyticsView {
    backend: Entity<Backend>,
    tab: Tab,
    /// Period of the Summary tab (presets or the from/to range).
    period: Period,
    /// Period of the Strategy Tuning tab, INDEPENDENT of Summary: each tab has its own
    /// time window, and the period bar edits the active one (`active_period`).
    strat_period: Period,
    /// Period currently represented by `data` (summary/strategy list). Entering a tab
    /// with a different time window triggers a reload.
    data_period: Period,
    /// Cores from the replica (for the combo box) plus multi-selection (empty = all), using
    /// the same controls as Orders and Report.
    cores: Vec<(u64, String)>,
    sel_cores: HashSet<u64>,
    side: SideFilter,
    /// `None` means all, `Some(false)` real, and `Some(true)` emulated.
    emu: Option<bool>,
    /// Profit metric: absolute money (`Usdt`) or the report `Profit` column
    /// (`Percent` = profit ÷ spent). Persisted in `layout.analytics_profit_percent`.
    metric: ProfitMetric,
    /// Background summary state with distinct loading, unavailable, ready, and
    /// failed outcomes so only a successful empty read appears empty.
    pub(super) data: LoadState<Summary>,
    /// Closed trades the core never dated, under the CURRENT filters.
    ///
    /// `None` while unknown (not read yet, or the read failed — logged at the origin): the
    /// banner is a claim about missing money, and it must not appear on a guess. Read on
    /// every filter change alongside the summary, since it shares the same filters and is
    /// deliberately outside the period.
    pub(super) undated: Option<moon_core::db::analytics::UndatedCloses>,
    /// Attribute LIQUIDATION trades to the strategy named in the row (see
    /// `db::analytics::Query::attribute_liq`).
    ///
    /// Cached on the view rather than read from the layout per query, because `query()` has no
    /// `cx` — and because it must not change under a reload that is already in flight.
    pub(super) attr_liq: bool,
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
    /// Start of the current operation batch; the overlay appears only after
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
    /// Multi-select (Ctrl): the EXTRA selected rows beyond the anchor (`sel_strategy`).
    /// The anchor drives scope/suggest/detail; these are bulk-write addressees only,
    /// stored as `(key, name)`. Empty = single selection.
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
    /// `data` is replaced (`data.apply`, the single site — the names may have changed), and
    /// recomputed when the stored font scale no longer matches the current one, so a Font-slider
    /// move OR a theme whose mode carries a different base mono size re-measures instead of
    /// scaling a width that assumed the old base.
    strat_core_w: Option<(f32, f32)>,
    /// Calendar tab: cells (PnL, trades, and wins) for the loaded range; Day mode uses hourly cells.
    pub(super) cal_days: Option<Arc<Vec<DayCell>>>,
    cal_seq: u64,
    /// Whether the series is stale for the current filters and must be reloaded on entry.
    cal_dirty: bool,
    cal_mode: calendar::CalMode,
    /// Displayed calendar month as `(year, month 1..12)`, controlled by the tab's OWN
    /// Previous/Next navigation; the window period bar does not affect Calendar.
    pub(super) cal_ym: (i32, u32),
    /// Selected day start for Day mode.
    pub(super) cal_day: i64,
    /// PREVIOUS month's aggregate `(profit, trades, wins)` for the KPI deltas against the
    /// previous period (calendar month versus calendar month, not 30 days).
    pub(super) cal_prev: Option<(f64, i64, i64)>,
    /// Calendar day under the cursor, stored as the day start for cell highlighting.
    pub(super) cal_hover: Option<i64>,
    /// Strategies-tab mode (Filters / Coins / Time). Privacy is module-based: tab submodules
    /// can see their parent's fields without `pub(super)`.
    strat_mode: tuner::StratMode,
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
    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Window geometry lives in the layout, as it does for Screener and Strategies.
        cx.observe_window_bounds(window, |this, window, cx| {
            let Some((x, y, w, h)) = crate::windowing::window_geom(window) else {
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

        // New reports intentionally trigger NO automatic reload: recomputation (full period scans
        // plus grouping) starts only after a relevant user action such as opening the window,
        // changing the period or filters, or clicking a preset again.

        // Period: the previous layout selection, defaulting to the current calendar month.
        let saved_period = backend
            .read(cx)
            .layout
            .analytics_period
            .as_deref()
            .and_then(Period::from_id);
        // Read before `backend` is moved into the struct below.
        let attr_liq = backend.read(cx).layout.analytics_attribute_liq;
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
        // Profit metric from the previous run (default USDT).
        let saved_metric = if backend.read(cx).layout.analytics_profit_percent {
            ProfitMetric::Percent
        } else {
            ProfitMetric::Usdt
        };
        // Visible strategy-list columns from the previous run, one mask per axis. An older
        // config holding the single-mask key seeds all three, so a choice already made is
        // carried over instead of reset; absent entirely, each axis takes its own default.
        let saved_strat_cols = {
            let layout = &backend.read(cx).layout;
            layout.analytics_strat_cols_modes.unwrap_or_else(|| {
                let seed = layout.analytics_strat_cols2;
                let mut by_mode = moon_core::config::layout::StratColsByMode::default();
                // Each axis through its OWN accessor and its OWN default, so this seeding
                // cannot disagree with what the selector reads back.
                for mode in tuner::STRAT_MODES {
                    *mode.cols_slot(&mut by_mode) = seed.unwrap_or(mode.default_cols());
                }
                by_mode
            })
        };
        // Calendar mode from the previous run, defaulting to Month.
        let saved_mode = backend
            .read(cx)
            .layout
            .analytics_heat_mode
            .as_deref()
            .and_then(calendar::CalMode::from_id)
            .unwrap_or(calendar::CalMode::Month);

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
            tab: if probe { Tab::Strategies } else { Tab::Summary },
            period: saved_period.unwrap_or(Period::CurMonth),
            strat_period: saved_strat_period.unwrap_or(Period::CurMonth),
            data_period: saved_period.unwrap_or(Period::CurMonth),
            cores: Vec::new(),
            sel_cores: HashSet::new(),
            side: SideFilter::All,
            // Default to Real, as in Report, because emulated trades add noise to the statistics.
            emu: Some(false),
            metric: saved_metric,
            data: LoadState::default(),
            undated: None,
            attr_liq,
            write_error: None,
            busy_ops: 0,
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
            strat_sort: Some(("analytics.col.profit".to_string(), true)),
            strat_cols: saved_strat_cols,
            strat_core_w: None,
            cal_days: None,
            cal_seq: 0,
            cal_dirty: true,
            cal_mode: saved_mode,
            cal_ym: {
                use chrono::Datelike;
                let d = day_of_secs(moon_core::util::now_unix_ms_i64() / 1000).unwrap_or_default();
                (d.year(), d.month())
            },
            cal_day: (moon_core::util::now_unix_ms_i64() / 1000).div_euclid(86_400) * 86_400,
            cal_prev: None,
            cal_hover: None,
            strat_mode: if probe {
                tuner::StratMode::Coins
            } else {
                tuner::StratMode::Filters
            },
            tuner: tuner::TunerState::load(saved_tuner_iters, saved_tuner_edges),
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

    /// Period of the active tab. Tuning keeps its OWN time window, separate from Summary.
    /// Calendar does not use the period bar because it has its own navigation, and
    /// `reload_calendar` builds its query directly without calling this method.
    fn active_period(&self) -> Period {
        match self.tab {
            Tab::Strategies => self.strat_period,
            _ => self.period,
        }
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
            attribute_liq: self.attr_liq,
            metric: self.metric,
        }
    }

    /// Selected cores for a query; empty or all means no core filter.
    fn cores_selected(&self) -> Vec<u64> {
        if self.sel_cores.is_empty() || self.sel_cores.len() == self.cores.len() {
            Vec::new()
        } else {
            self.sel_cores.iter().copied().collect()
        }
    }

    /// Start/finish accounting for background operations. Every operation that calls
    /// `op_started` must decrement exactly once, BEFORE its sequence check, because stale
    /// completions still count.
    pub(super) fn op_started(&mut self) {
        self.busy_ops += 1;
        if self.busy_since.is_none() {
            self.busy_since = Some(std::time::Instant::now());
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

    pub(super) fn op_finished(&mut self, cx: &mut Context<Self>) {
        self.busy_ops = self.busy_ops.saturating_sub(1);
        if self.busy_ops == 0 {
            self.busy_since = None;
        }
        cx.notify();
    }

    /// Whether to show the busy overlay; if the batch is younger than the delay, arms a timer
    /// to repaint when the delay expires.
    fn busy_overlay_due(&self, cx: &mut Context<Self>) -> bool {
        let Some(since) = self.busy_since else {
            return false;
        };
        let waited = since.elapsed();
        if waited >= BUSY_OVERLAY_DELAY {
            return true;
        }
        let left = BUSY_OVERLAY_DELAY - waited;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(left).await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |_, cx| cx.notify());
            });
        })
        .detach();
        false
    }

    /// Reload the Analytics data and dependent views for the current period/filter scope.
    fn reload(&mut self, cx: &mut Context<Self>) {
        // Mark the request at its start so an error from another period cannot
        // remain under the current period label.
        self.data.begin();
        // Drop every chart hover: the bars/columns under the cursor are about to be
        // replaced (and on a single-day period the right card swaps its whole element
        // tree), so they never fire `hovered = false` and a stale index would re-open a
        // popup with no cursor on it — pointing at another period's bucket.
        self.hover_daily_bucket = None;
        self.hover_cum_bucket = None;
        self.hover_kind = None;
        self.seq = self.seq.wrapping_add(1);
        let req = self.seq;
        // Record the ACTIVE tab's time window that `data` is being computed for.
        self.data_period = self.active_period();
        // The tuner uses the same filters: invalidate it and recompute immediately in the active
        // mode, or defer recomputation until the next entry into Filters mode.
        self.tuner.invalidate();
        // The By time axis uses the shared filters and `strat_period`. This common reload path
        // conservatively retires its in-flight auto-suggestion even on a Summary-only period
        // change; otherwise a stale result could be written into v1 and become saveable.
        // The profile is marked stale the same way, recomputed immediately below when the
        // axis is the active one, or deferred until entry.
        self.time_tuner.invalidate();
        // The active Filters/Time axis recomputes now. "By coin" is the deliberate
        // exception — see the `coins.invalidate()` comment below.
        if self.tab == Tab::Strategies
            && matches!(
                self.strat_mode,
                tuner::StratMode::Filters | tuner::StratMode::Time
            )
        {
            self.reload_axis(self.strat_mode, cx);
        }
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
        self.cal_dirty = true;
        if self.tab == Tab::Calendar {
            self.reload_calendar(cx);
        }
        let q = self.query();
        let undated_q = q.clone();
        self.spawn_db(
            true,
            cx,
            move || {
                let d = moon_core::db::analytics::summary(&q);
                // Rides the same pass: it answers a question ABOUT this filter set, and
                // a second round trip would let the two disagree about which cores.
                let u = moon_core::db::analytics::undated_closes(&undated_q);
                (d, u)
            },
            move |this, (data, undated), cx| {
                if this.seq != req {
                    return; // The period or filters have already changed.
                }
                // Keep the last known core list when a read produces no
                // summary: an empty `cores` makes `cores_selected()` read as
                // "no filter", which renders as if every core were selected.
                if let Ok(d) = &data {
                    this.cores = d.cores.clone();
                }
                this.data.apply(data);
                // The set of core names may have changed with the data — remeasure the
                // list's core column on the next render.
                this.strat_core_w = None;
                // A failed read leaves it unknown rather than "nothing missing": the
                // origin already logged why, and a silent zero here would be the very
                // thing this banner exists to stop.
                this.undated = undated.ok();
                // Observation channel only — see `probe_selects_strategy`. It adopts a
                // strategy, which starts the coin reload itself; the arm below is then
                // skipped so the two do not race two full passes over the same period.
                let probe_took_over = probe_selects_strategy() && this.probe_select_first(cx);
                // Now that the base coin set belongs to this scope, the coin axis can
                // plan against it.
                if !probe_took_over
                    && this.tab == Tab::Strategies
                    && this.strat_mode == tuner::StratMode::Coins
                {
                    this.reload_axis_if_stale(tuner::StratMode::Coins, cx);
                }
                cx.notify();
            },
        );
    }

    // cal_query/cal_query_prev/reload_calendar live in calendar/mod.rs — a page's
    // recomputation belongs beside that page.

    fn set_period(&mut self, p: Period, window: &mut Window, cx: &mut Context<Self>) {
        // Clicking the active preset again manually refreshes the data because new reports do not
        // trigger automatic reloads. The period bar edits the ACTIVE tab's time window: Summary
        // and Tuning are independent.
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

    /// Toggle a core in the multi-selection. `None` is All: it fills an empty set with every core,
    /// but clears any nonempty set. Both the empty and full representations query all cores.
    fn toggle_core(&mut self, core: Option<u64>, cx: &mut Context<Self>) {
        match core {
            None => {
                if self.sel_cores.is_empty() {
                    self.sel_cores = self.cores.iter().map(|(c, _)| *c).collect();
                } else {
                    self.sel_cores.clear();
                }
            }
            Some(c) => {
                if !self.sel_cores.remove(&c) {
                    self.sel_cores.insert(c);
                }
            }
        }
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

    /// Switch the profit metric (USDT ⇔ percent). Every figure and the tuner sweep are
    /// computed under it, so the switch persists and reloads — it is not a display-only toggle.
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
}

impl EventEmitter<()> for AnalyticsView {}
impl Focusable for AnalyticsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AnalyticsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        // Arm the profit formatters for this frame: "%" suffix in percent mode, none in USDT.
        set_pnl_pct(self.metric == ProfitMetric::Percent);
        let chrome_width = match window.window_bounds() {
            WindowBounds::Windowed(b)
            | WindowBounds::Maximized(b)
            | WindowBounds::Fullscreen(b) => f32::from(b.size.width),
        };
        let body = match self.tab {
            Tab::Summary => self.summary_tab(p, cx),
            Tab::Strategies => self.strategies_tab(p, window, cx),
            Tab::Calendar => self.calendar_tab(p, cx),
        };
        // Tabs divide their own height, pinning bottom bars to the window and scrolling content
        // internally, so there is no outer scroll.
        let body_scrolls = false;
        let integrity = self.integrity_note(cx);
        let undated = self.undated_note(cx);
        let write_error = self.write_error.clone();
        let busy_overlay = self.busy_overlay_due(cx);
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
            // A write that reached nobody. Above the undated note deliberately: that one is
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
            // Money that is in NO figure on this window, plus the liquidation-attribution
            // switch — one strip, always present: see `notice_strip`.
            .child(self.notice_strip(undated, p, cx))
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
    let display_id = crate::windowing::saved_or_owner_display_id(
        saved.map(|g| point(px(g.x as f32), px(g.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let mut opts = crate::windowing::tool_window_options(
        t!("analytics.window_title").to_string(),
        WindowBounds::Windowed(bounds),
        Some(size(px(860.0), px(520.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        crate::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| AnalyticsView::new(b, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        backend.update(cx, |bk, _| bk.analytics_window = Some(handle));
        crate::windowing::activate_new_window(handle.into(), cx);
    }
}
