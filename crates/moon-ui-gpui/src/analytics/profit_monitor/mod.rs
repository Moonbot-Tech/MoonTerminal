//! Independent, automatically refreshed desktop Profit Monitor window.

mod broadcast;
mod format;
mod line;
mod model;
mod rows;
mod sections;
mod settings;
mod table;
mod window;

// The window's own lifecycle lives in `window.rs`; the toolbar and startup reach it through here,
// so the module path callers use does not change when the file it lives in does.
pub(crate) use window::{open, restore, ProfitMonitorOpenRequest};

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::db::analytics::{
    PreviousPeriodBasis, ProfitMonitorCore, ProfitMonitorSummary, Query,
};
use moon_core::db::valuation::ValuationMode;
use moon_core::db::{FailKind, ProfitMetric, ReadFail, SideFilter};
use moon_core::session::CoreId;
use moon_ui::{
    MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuSize, MoonPalette, MoonSegmentItem,
    MoonSegmentedControl, MoonVirtualListScrollHandle, MoonWindowFrame, h_flex, v_flex,
};
use rust_i18n::t;

use super::refresh::{
    report_result_is_stale, BusyRetryBudget, RefreshGate, RefreshPlan, RefreshUrgency,
};
use super::ProfitLoadState;
use crate::controls::core_run::{run_scope_rev, RunSlots};
use crate::core_order::CoreOrder;
use crate::design::{moon, moon_alpha};
use crate::{design, Backend};
#[cfg(test)]
use model::LAST_TRADE_WIDTH;
use model::{
    ContextChange, MonitorLayout, MonitorPeriod, MonitorSort, MonitorSortColumn, context_change,
    duration_until_period_refresh, duration_until_wall_clock_boundary, monitor_zone, next_sort,
    retain_last_known_venues, sort_rows,
};
use rows::{GroupMode, LiveContext, RowLabels, fold_total, grouped_rows};
use sections::SectionLabels;
use settings::MonitorPrefs;
use table::{centered_alert, centered_message, split_body};

const HEADER_HEIGHT: f32 = 32.0;
const CONTEXT_REFRESH_MS: u128 = 5_000;
const SECOND_MS: u128 = 1_000;
const MIN_NAME_COLUMN_WIDTH: f32 = 128.0;
/// Floor the name column keeps however much the run column claims from it.
///
/// The run controls are paid for by the NAME column, so the minimum window width never grows —
/// that is the whole constraint (`MIN_WINDOW_WIDTH` is exactly padding + name + gap + profit). This
/// floor is what stops a future third slot from squeezing the name down to nothing instead.
const NAME_COLUMN_FLOOR: f32 = 72.0;
const PROFIT_COLUMN_WIDTH: f32 = 154.0;
/// Extra profit-column width claimed by the `total(last)` form.
///
/// The suffix is a second signed amount plus its brackets, so the cell needs the room BEFORE it is
/// drawn — an ellipsis in a money column reads as a different number. Sized for the WIDEST quote,
/// not the common one: `QuoteCurrency::display_decimals` returns 8 for BTC-like quotes, so
/// `(-0.00000123)` is 13 monospace characters and a 60-unit allowance would truncate it.
const PROFIT_LAST_TRADE_EXTRA: f32 = 90.0;
/// Design-reference logo edge drawn before a row's name.
const EXCHANGE_LOGO_SIZE: f32 = 13.0;
/// Gap between that logo and the name it belongs to.
///
/// Named because THREE places have to agree on it: the row that draws the logo, the empty gutter a
/// logo-less row keeps, and the heading's own left padding.
const NAME_LOGO_GAP: f32 = 5.0;
const TRADES_COLUMN_WIDTH: f32 = 72.0;
const WIN_RATE_COLUMN_WIDTH: f32 = 76.0;
const AVERAGE_ORDER_COLUMN_WIDTH: f32 = 116.0;
const TABLE_HORIZONTAL_PADDING: f32 = 10.0;
const TABLE_COLUMN_GAP: f32 = 8.0;
const MIN_WINDOW_WIDTH: f32 = 310.0;

/// Return the run slots the current preferences reserve on every line of the table.
///
/// Reserved by the TABLE, not by the line: a caption offers less than a core row does, but both
/// must claim the same width or the name column stops lining up with its own heading.
///
/// Args:
///     prefs: Display preferences chosen in the popup.
///
/// Returns:
///     The reserved slots; `RunSlots::any` is false when the column is off entirely.
fn run_slots(prefs: MonitorPrefs) -> RunSlots {
    RunSlots {
        status: prefs.core_status,
        // A group-only trading control still needs the row-level slot reserved, or the captions
        // would carry a column the rows beneath them do not.
        trading: prefs.trading_buttons || prefs.group_trading,
    }
}

/// Return the name column's minimum width once the run column has taken its share.
///
/// Args:
///     slots: Slots the table reserves.
///
/// Returns:
///     Minimum design-reference width of the Name column.
fn name_min_width(slots: RunSlots) -> f32 {
    if !slots.any() {
        return MIN_NAME_COLUMN_WIDTH;
    }
    (MIN_NAME_COLUMN_WIDTH - slots.width() - TABLE_COLUMN_GAP).max(NAME_COLUMN_FLOOR)
}

/// Read the current UTC instant through the terminal's shared system-clock source.
///
/// Returns:
///     Current UTC time, or the Unix epoch when the platform clock is outside chrono's range.
fn now_utc() -> DateTime<Utc> {
    DateTime::from_timestamp_millis(moon_core::util::now_unix_ms_i64())
        .unwrap_or(DateTime::UNIX_EPOCH)
}

/// Return the arrow suffix for one sortable heading.
///
/// Args:
///     sort: Current explicit ordering.
///     column: Heading being rendered.
///
/// Returns:
///     Direction glyph for the active heading, otherwise an empty suffix.
fn sort_arrow(sort: Option<MonitorSort>, column: MonitorSortColumn) -> &'static str {
    match sort {
        Some(active) if active.column == column && active.descending => " ↓",
        Some(active) if active.column == column => " ↑",
        _ => "",
    }
}

/// Independently ticking terminal clock embedded in the Profit Monitor controls.
///
/// Keeping the second timer on a child entity prevents each clock tick from rebuilding, grouping,
/// and sorting the monitor's virtualized table.
struct MonitorClockView {
    backend: Entity<Backend>,
}

impl MonitorClockView {
    /// Construct the shared terminal clock and align its repaint loop to wall-clock seconds.
    ///
    /// Args:
    ///     backend: Shared terminal state containing the selected display zone and its revision.
    ///     cx: Child view context that owns the recurring timer.
    ///
    /// Returns:
    ///     Clock state whose repaint cadence is isolated from the parent monitor.
    fn new(backend: Entity<Backend>, cx: &mut Context<Self>) -> Self {
        let display_time_revision = backend.read(cx).display_time_revision.clone();
        cx.observe(&display_time_revision, |_this, _revision, cx| cx.notify())
            .detach();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            loop {
                executor
                    .timer(duration_until_wall_clock_boundary(
                        SystemTime::now(),
                        SECOND_MS,
                    ))
                    .await;
                let alive = cx.update(|cx| this.update(cx, |_this, cx| cx.notify()).is_ok());
                if !alive {
                    break;
                }
            }
        })
        .detach();
        Self { backend }
    }
}

impl Render for MonitorClockView {
    /// Render the same selected-city clock and picker used by the terminal header.
    ///
    /// Args:
    ///     window: Owning Profit Monitor window whose width selects clock precision.
    ///     cx: Clock render context used to read the active palette and backend state.
    ///
    /// Returns:
    ///     Shared terminal clock element.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = MoonPalette::active(cx);
        if MonitorLayout::for_width(window_width(window), design::ui_value(cx, 1.0)).clock_seconds {
            crate::chrome::clock::header_clock(&self.backend, palette, cx)
        } else {
            crate::chrome::clock::compact_header_clock(&self.backend, palette, cx)
        }
    }
}

/// Cached sibling surface containing every expensive Profit Monitor body state.
///
/// The child reads its owner weakly, so the parent can retain it without creating an entity cycle.
/// When the clock invalidates its own branch, GPUI marks the parent dirty but reuses this clean
/// cached sibling instead of regrouping and sorting the table.
struct ProfitMonitorBodyView {
    owner: WeakEntity<ProfitMonitorView>,
}

impl Render for ProfitMonitorBodyView {
    /// Render the current monitor body through its owning state entity.
    ///
    /// Args:
    ///     window: Owning window used for responsive column selection.
    ///     cx: Body view context used to read the parent entity and active palette.
    ///
    /// Returns:
    ///     Current loading, error, split-currency, or table surface.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let palette = MoonPalette::active(cx);
        let body = owner
            .read(cx)
            .body(window_width(window), palette, owner.clone(), cx);
        // Cached AnyView lays its rendered child out as an independent root. This full-size
        // vertical root transfers the allocated cache bounds to the body's elastic table slot.
        v_flex()
            .size_full()
            .min_h_0()
            .child(body)
            .into_any_element()
    }
}

/// State of the independent Profit Monitor window.
pub(crate) struct ProfitMonitorView {
    backend: Entity<Backend>,
    /// Exact native identity used so an old release cannot clear a replacement singleton.
    window_id: WindowId,
    /// Cancellation authority for this window's current background taskbar-hide burst.
    taskbar_hide: crate::window::windowing::TaskbarHideTask,
    clock: Entity<MonitorClockView>,
    content: Entity<ProfitMonitorBodyView>,
    report_generation: Option<Arc<AtomicU64>>,
    valuation_generation: Option<Arc<AtomicU64>>,
    refresh: RefreshGate,
    busy_retries: BusyRetryBudget,
    clock_timer_generation: u64,
    db_active: bool,
    seq: u64,
    period: MonitorPeriod,
    zone: Tz,
    group: GroupMode,
    sort: Option<MonitorSort>,
    prefs: MonitorPrefs,
    settings_open: bool,
    valuation: ValuationMode,
    live: LiveContext,
    data: ProfitLoadState<ProfitMonitorSummary>,
    refresh_error: Option<ReadFail>,
    /// Run-state token of every configured core, folded with the pending-intent register.
    ///
    /// The monitor observes `Backend` for this and nothing else, so the throttled backend
    /// notification — which fires while the fleet is merely trading — costs one store lookup per
    /// configured core and repaints only when a run cell would actually draw differently. Zero
    /// while the run column is switched off, which is also when the fold is skipped entirely.
    run_rev: u64,
    /// Newest close date and trade count already on screen, per report core.
    ///
    /// This is the arrival detector's whole memory. `None` means "no baseline": the next snapshot
    /// only records, because a query change replaces every value at once and that is not fourteen
    /// new trades. Once a baseline exists, a core APPEARING is an arrival too — that is a core's
    /// first trade of the hour, the one a user is most likely watching for.
    ///
    /// The count is carried beside the date because close dates have one-second resolution: a
    /// second trade inside the same second moves the count and nothing else.
    seen_trades: Option<HashMap<CoreId, (i64, i64)>>,
    /// When each core's latest arrival was observed.
    ///
    /// The shared [`crate::pulse::Arrivals`] owns the stamps, their expiry and the "a timer is
    /// running" flag — the same machine the News feed uses, so the two highlights cannot drift.
    flash: crate::pulse::Arrivals<CoreId>,
    scroll: MoonVirtualListScrollHandle,
    focus: FocusHandle,
}

impl ProfitMonitorView {
    /// Construct the monitor, arm taskbar suppression, and start its subscriptions and first read.
    ///
    /// Args:
    ///     backend: Shared terminal state.
    ///     window: Newly opened independent window.
    ///     cx: View context used for subscriptions and background work.
    ///
    /// Returns:
    ///     Fully initialized monitor state.
    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let window_id = window.window_handle().window_id();
        // Apply the shared independent-window taskbar policy now and after every activation;
        // `hide_window_from_taskbar_soon` owns the delayed retry rationale.
        let taskbar_hide = crate::window::windowing::hide_window_from_taskbar_soon(window);
        cx.observe_window_activation(window, |this, window, _cx| {
            this.taskbar_hide.cancel();
            this.taskbar_hide = crate::window::windowing::hide_window_from_taskbar_soon(window);
        })
        .detach();

        cx.observe_window_bounds(window, |this, window, cx| {
            let geom = crate::window::windowing::window_geom_rect(window, cx);
            this.backend.update(cx, |backend, _| {
                let geom = geom.keeping_display_of(backend.layout.profit_monitor_window);
                if backend.layout.profit_monitor_window != Some(geom) {
                    backend.layout.profit_monitor_window = Some(geom);
                    backend.layout_dirty = true;
                }
            });
        })
        .detach();

        // Closing the monitor by hand is a decision the next launch has to know about; quitting is
        // not. `quitting` separates them, exactly as the detached-panel windows do — during
        // shutdown the layout has already been flushed, so a release-time write here would replace
        // "the monitor was open" with "the monitor was closed" on every ordinary exit.
        // Deliberately NOT released here: the broadcast core filter outlives this window. Closing a
        // tool window must not silently change what five panels are showing, and the filter is not
        // invisible without the monitor — every panel that adopted it says "N cores" in its own
        // selector and can widen from there. The ⚙ checkbox is the one place that releases it,
        // because switching the feature off IS a request to stop filtering.
        cx.on_release(|this, app| {
            this.taskbar_hide.cancel();
            this.backend.update(app, |backend, cx| {
                // Everything here is guarded by the window id. A view released AFTER its
                // replacement registered — close and reopen inside one effect flush — would
                // otherwise clear the flag while a live monitor is on screen, and the next launch
                // would not reopen it.
                if backend
                    .profit_monitor_window
                    .is_none_or(|handle| handle.window_id() != this.window_id)
                {
                    return;
                }
                backend.profit_monitor_window = None;
                if !backend.quitting && backend.layout.profit_monitor_open {
                    backend.layout.profit_monitor_open = false;
                    backend.layout_dirty = true;
                }
                cx.notify();
            });
        })
        .detach();

        let clock = cx.new(|cx| MonitorClockView::new(backend.clone(), cx));
        let content_owner = cx.entity().downgrade();
        let content = cx.new(|_| ProfitMonitorBodyView {
            owner: content_owner,
        });
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
        let generation = combined_generation(&report_generation, &valuation_generation);
        let period = backend
            .read(cx)
            .layout
            .profit_monitor_period
            .as_deref()
            .and_then(MonitorPeriod::from_id)
            .unwrap_or_default();
        let zone = monitor_zone(backend.read(cx).header_clock_zone());
        let group = backend
            .read(cx)
            .layout
            .profit_monitor_group
            .as_deref()
            .and_then(GroupMode::from_id)
            .unwrap_or_default();
        let sort = backend
            .read(cx)
            .layout
            .profit_monitor_sort
            .as_ref()
            .and_then(|(id, descending)| MonitorSort::from_layout(id, *descending));
        let prefs = MonitorPrefs::restore(&backend.read(cx).layout);
        let valuation = backend.read(cx).valuation_mode();
        let live = capture_live_context(backend.read(cx));

        // The ONE Backend observation this window makes. Everything else it draws comes from the
        // report database or from the five-second context sample; the run controls are the only
        // part that has to follow live core state, and they are usually switched off.
        cx.observe(&backend, |this, _backend, cx| this.sync_run_state(cx))
            .detach();
        let report_revision = backend.read(cx).report_revision.clone();
        cx.observe(&report_revision, |this, _, cx| {
            this.observe_report_generation(cx);
        })
        .detach();
        let display_time_revision = backend.read(cx).display_time_revision.clone();
        cx.observe(&display_time_revision, |this, _, cx| this.sync_context(cx))
            .detach();
        broadcast::observe_core_filter(&backend, cx);
        let mut this = Self {
            backend,
            window_id,
            taskbar_hide,
            clock,
            content,
            report_generation,
            valuation_generation,
            refresh: RefreshGate::new(generation, std::time::Instant::now()),
            busy_retries: BusyRetryBudget::default(),
            clock_timer_generation: 0,
            db_active: false,
            seq: 0,
            period,
            zone,
            group,
            sort,
            prefs,
            settings_open: false,
            valuation,
            live,
            data: ProfitLoadState::default(),
            refresh_error: None,
            // Left at zero: the first backend notification fills it, and `sync_run_state` already
            // owns the "skip while the column is off" rule.
            run_rev: 0,
            seen_trades: None,
            flash: crate::pulse::Arrivals::default(),
            scroll: MoonVirtualListScrollHandle::new(),
            focus: cx.focus_handle(),
        };
        // Decode the logos before the first table frame needs them, off the render path — and only
        // when they will actually be drawn: someone who turned the icons off should not pay for
        // seven SVG rasters and the textures they retain for the rest of the session.
        if prefs.exchange_icons {
            cx.background_spawn(async { crate::media::exchange_logos::prewarm() })
                .detach();
        }
        this.reload(false, cx);
        this.start_clock_refresh(cx);
        this.start_context_refresh(cx);
        this
    }

    /// Sample live identity and valuation context on bounded wall-clock ticks.
    ///
    /// This avoids cloning every configured and live core name on unrelated high-rate Backend
    /// notifications while keeping reconnect and rename changes automatic.
    ///
    /// Args:
    ///     cx: View context used to own the recurring task.
    fn start_context_refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            loop {
                executor
                    .timer(duration_until_wall_clock_boundary(
                        SystemTime::now(),
                        CONTEXT_REFRESH_MS,
                    ))
                    .await;
                let alive =
                    cx.update(|cx| this.update(cx, |this, cx| this.sync_context(cx)).is_ok());
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    /// Mark the cached body dirty after one of its parent-owned inputs changes.
    ///
    /// GPUI cache invalidation follows view ancestry, not arbitrary entity reads. The body is a
    /// sibling of the clock, so body-state writers notify it explicitly while clock-only ticks do
    /// not.
    ///
    /// Args:
    ///     cx: Parent context used to notify the cached child entity.
    fn invalidate_content(&self, cx: &mut Context<Self>) {
        self.content.update(cx, |_content, cx| cx.notify());
    }

    /// Apply the latest non-database labels and valuation mode.
    ///
    /// Args:
    ///     cx: View context used to read Backend and repaint or reload as required.
    fn sync_context(&mut self, cx: &mut Context<Self>) {
        let backend = self.backend.read(cx);
        let next = retain_last_known_venues(&self.live, capture_live_context(backend));
        let valuation = backend.valuation_mode();
        let zone = monitor_zone(backend.header_clock_zone());
        let zone_changed = self.zone != zone;
        match context_change(&self.live, &next, self.valuation != valuation, zone_changed) {
            ContextChange::None => {}
            ContextChange::Regroup => {
                self.live = next;
                self.invalidate_content(cx);
                cx.notify();
            }
            ContextChange::Reload { restart_clock } => {
                self.live = next;
                self.valuation = valuation;
                self.zone = zone;
                if restart_clock {
                    self.start_clock_refresh(cx);
                }
                self.reload(false, cx);
            }
        }
    }

    /// Repaint the table when a core's run state, or a pending run intent, changed.
    ///
    /// Args:
    ///     cx: View context used to read the session and invalidate the cached body.
    fn sync_run_state(&mut self, cx: &mut Context<Self>) {
        if !run_slots(self.prefs).any() {
            return;
        }
        let rev = run_scope_rev(&self.backend, self.live.core_order.iter().copied(), cx);
        if rev == self.run_rev {
            return;
        }
        self.run_rev = rev;
        self.invalidate_content(cx);
        cx.notify();
    }

    /// Arm the exact next wall-clock boundary for the selected period.
    ///
    /// Args:
    ///     cx: View context used to own the recurring task.
    fn start_clock_refresh(&mut self, cx: &mut Context<Self>) {
        self.clock_timer_generation = self.clock_timer_generation.wrapping_add(1);
        let generation = self.clock_timer_generation;
        let Some(wait) = duration_until_period_refresh(self.period, self.zone, SystemTime::now())
        else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(wait).await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.clock_timer_generation != generation {
                        return;
                    }
                    this.busy_retries.reset();
                    // A period boundary MOVES the query window — Today becomes another day,
                    // Yesterday shifts back one. Every core's newest trade is replaced by a
                    // different one, so the memory has to be dropped here even though the reload
                    // keeps the visible rows: diffing across the boundary is comparing two
                    // different questions.
                    this.rebaseline_arrivals();
                    this.reload(true, cx);
                    this.start_clock_refresh(cx);
                });
            });
        })
        .detach();
    }

    /// Return the report plus valuation generation represented by a monitor query.
    ///
    /// Returns:
    ///     Wrapping sum of both monotonic generations.
    fn current_generation(&self) -> u64 {
        combined_generation(&self.report_generation, &self.valuation_generation)
    }

    /// Build the current unfiltered real-trade query.
    ///
    /// Returns:
    ///     Quote-profit query for the selected period and global valuation mode.
    fn query(&self) -> Query {
        // The window resolves in THIS monitor's own selected zone. Under Phase 1 it could not:
        // the bounds went straight onto a core-local column, so computing "today" in the user's
        // zone slid the period sideways relative to the rows it filtered, and UTC was the only
        // self-consistent choice. Now the read side converts each bound onto the core's own clock
        // per offset group, so a civil boundary means what it says — and leaving this on UTC would
        // put a different "today" here than in the Report window beside it.
        //
        // Only the ZONE is carried: the offsets on this axis are replaced by the read path, which
        // loads them inside its own pinned snapshot.
        let axis = moon_core::db::ReportAxis::from_measured(Default::default(), self.zone);
        let (from, to) = self.period.range_at(now_utc(), axis.zone());
        Query {
            axis,
            previous_period_basis: PreviousPeriodBasis::Civil,
            from,
            to,
            cores: Vec::new(),
            side: SideFilter::All,
            emulator: Some(false),
            strategies: Vec::new(),
            // No mask: this window has no Analytics toolbar, so there is no mask to inherit.
            strategy_name_mask: String::new(),
            metric: ProfitMetric::Quote,
            valuation: self.valuation,
            // The monitor already renders one line per core, each in its own quote, so pinning the
            // scale here would convert figures the panel deliberately keeps native.
            prefer_usdt: false,
        }
    }

    /// Start a compact database read, preserving visible rows for automatic catch-up refreshes.
    ///
    /// Args:
    ///     after_report: Whether an existing visible snapshot may remain until replacement.
    ///     cx: View context used to spawn and publish the read.
    fn reload(&mut self, after_report: bool, cx: &mut Context<Self>) {
        if !after_report {
            self.rebaseline_arrivals();
        }
        if self.db_active {
            if !after_report {
                self.seq = self.seq.wrapping_add(1);
                self.data = ProfitLoadState::default();
                self.refresh_error = None;
                self.invalidate_content(cx);
                cx.notify();
            }
            // WRITER throughout: this window's timing is not what the urgency split was added
            // for, and it keeps exactly the behaviour it had before the gate learned to tell a
            // person from a commit stream.
            self.refresh
                .request_refresh(std::time::Instant::now(), false, RefreshUrgency::Writer);
            self.schedule_refresh(cx);
            return;
        }
        if !after_report {
            self.data = ProfitLoadState::default();
            self.refresh_error = None;
            self.invalidate_content(cx);
        }
        self.seq = self.seq.wrapping_add(1);
        let request = self.seq;
        let started_generation = self.current_generation();
        self.refresh
            .refresh_started(started_generation, std::time::Instant::now());
        let query = self.query();
        self.db_active = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { moon_core::db::analytics::profit_monitor(&query) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.db_active = false;
                    if this.seq == request {
                        let error = result.as_ref().err().cloned();
                        if !after_report || error.is_none() {
                            this.data.apply(result);
                            this.observe_arrivals(cx);
                            this.invalidate_content(cx);
                        }
                        if error.is_none() {
                            this.refresh_error = None;
                        } else {
                            this.refresh_error.clone_from(&error);
                        }
                        let newer_generation = report_result_is_stale(
                            started_generation,
                            this.current_generation(),
                            false,
                        );
                        if newer_generation {
                            this.refresh.request_refresh(
                                std::time::Instant::now(),
                                false,
                                RefreshUrgency::Writer,
                            );
                        }
                        this.settle_busy_retry(error.as_ref(), cx);
                    }
                    this.schedule_refresh(cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Observe a committed report or valuation generation and debounce its replacement read.
    ///
    /// Args:
    ///     cx: View context used to schedule the refresh.
    fn observe_report_generation(&mut self, cx: &mut Context<Self>) {
        self.sync_context(cx);
        let generation = self.current_generation();
        if self
            .refresh
            .observe_generation(generation, std::time::Instant::now())
        {
            self.busy_retries.observe_generation();
            self.schedule_refresh(cx);
        }
    }

    /// Plan the single trailing refresh or timer.
    ///
    /// Args:
    ///     cx: View context used to arm work.
    fn schedule_refresh(&mut self, cx: &mut Context<Self>) {
        match self.refresh.plan(std::time::Instant::now(), self.db_active) {
            RefreshPlan::Idle => {}
            RefreshPlan::Now { .. } => self.reload(true, cx),
            RefreshPlan::After(wait) => {
                cx.spawn(async move |this, cx| {
                    let executor = cx.update(|cx| cx.background_executor().clone());
                    executor.timer(wait).await;
                    let _ = cx.update(|cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.refresh.timer_fired();
                            this.schedule_refresh(cx);
                        });
                    });
                })
                .detach();
            }
        }
    }

    /// Apply the bounded automatic retry policy for transient SQLite contention.
    ///
    /// Args:
    ///     error: Latest query failure, if any.
    ///     cx: View context used to schedule a retry.
    fn settle_busy_retry(&mut self, error: Option<&ReadFail>, cx: &mut Context<Self>) {
        if error.and_then(ReadFail::kind) != Some(FailKind::Busy) {
            self.busy_retries.resolve();
            return;
        }
        if self.busy_retries.claim() {
            // A bounded Busy retry keeps the quiet period: retrying at once would hammer a
            // database already under contention.
            self.refresh
                .request_refresh(std::time::Instant::now(), false, RefreshUrgency::Writer);
            self.schedule_refresh(cx);
        } else {
            log::warn!("profit monitor: automatic database retry budget exhausted");
        }
    }

    /// Record which cores closed a trade since the previous snapshot and start their highlight.
    ///
    /// The signal is the report's own newest close date per core, so a row lights up for the same
    /// reason its `(last)` value changes: one authoritative timestamp, not a trade count that a
    /// retention sweep or a period edge could also move.
    ///
    /// Args:
    ///     cx: View context used to arm the repaint chain.
    fn observe_arrivals(&mut self, cx: &mut Context<Self>) {
        let ProfitLoadState::Ready { data, .. } = &self.data else {
            // Split currencies, a failed read, or a report that is not ready yet all leave the
            // memory describing a snapshot nobody can see any more. Re-baselining here is what
            // stops an outage from ending in a table-wide flash.
            self.rebaseline_arrivals();
            return;
        };
        // An EMPTY previous snapshot is not a baseline to diff against. A table going from nothing
        // to forty rows is report replication catching up, not forty cores trading in one instant,
        // and treating it as arrivals lights the whole window at once. Nothing is lost: when the
        // table is empty, the row APPEARING is already the signal — the highlight exists to point
        // at a change inside a table that is already populated.
        let baseline = self.seen_trades.as_ref().filter(|seen| !seen.is_empty());
        let (seen, arrived) = arrivals(baseline, &data.cores);
        self.seen_trades = Some(seen);
        if !self.prefs.flash || arrived.is_empty() {
            return;
        }
        self.flash.mark(arrived);
        crate::pulse::arm_with(
            self,
            cx,
            |this| this.flash.armed(),
            |this| this.flash.live(),
            Self::on_flash_tick,
        );
    }

    /// Forget the arrival baseline, so the next snapshot only records.
    ///
    /// Every caller is a case where the QUESTION changed rather than the answer: a new period, a
    /// new valuation or zone, a local-midnight rollover, or a read with no comparable rows at all.
    fn rebaseline_arrivals(&mut self) {
        self.seen_trades = None;
        self.flash.clear();
    }

    /// Per-tick work of the shared pulse chain: drop finished stamps and dirty the cached table.
    ///
    /// The body is a cached SIBLING view. `cx.notify()` inside the pulse marks this view and its
    /// ancestors, which leaves that child clean and lets GPUI reuse the still-tinted subtree — so
    /// the tint has to be invalidated here or the fade never moves. Pruning first is deliberate:
    /// the tick that drops the last live stamp is the one that must erase the tint, and
    /// [`crate::pulse::Arrivals::live`] then ends the chain on the following tick.
    ///
    /// Args:
    ///     cx: View context used to invalidate the cached body.
    fn on_flash_tick(&mut self, cx: &mut Context<Self>) {
        self.flash.prune();
        self.invalidate_content(cx);
    }

    /// Select and persist one period, then replace the database snapshot immediately.
    ///
    /// Args:
    ///     period: New monitor period.
    ///     cx: View context used to persist and reload.
    fn set_period(&mut self, period: MonitorPeriod, cx: &mut Context<Self>) {
        if self.period == period {
            return;
        }
        self.period = period;
        self.busy_retries.reset();
        self.backend.update(cx, |backend, _| {
            backend.layout.profit_monitor_period = Some(period.id().to_string());
            backend.layout_dirty = true;
        });
        self.reload(false, cx);
        self.start_clock_refresh(cx);
    }

    /// Select and persist one grouping axis without touching the database.
    ///
    /// Args:
    ///     group: New grouping axis.
    ///     cx: View context used to persist and repaint.
    fn set_group(&mut self, group: GroupMode, cx: &mut Context<Self>) {
        if self.group == group {
            return;
        }
        self.group = group;
        self.invalidate_content(cx);
        self.backend.update(cx, |backend, _| {
            backend.layout.profit_monitor_group = Some(group.id().to_string());
            backend.layout_dirty = true;
        });
        cx.notify();
    }

    /// Toggle and persist the ordering selected through one table heading.
    ///
    /// Args:
    ///     column: Clicked visible column.
    ///     cx: View context used to persist and repaint.
    fn toggle_sort(&mut self, column: MonitorSortColumn, cx: &mut Context<Self>) {
        let sort = next_sort(self.sort, column);
        self.sort = Some(sort);
        self.invalidate_content(cx);
        self.backend.update(cx, |backend, _| {
            backend.layout.profit_monitor_sort =
                Some((sort.column.id().to_string(), sort.descending));
            backend.layout_dirty = true;
        });
        cx.notify();
    }

    /// Render the period and grouping selectors.
    ///
    /// Args:
    ///     width: Current window width used to select the complete responsive presentation.
    ///     palette: Active MoonUI palette.
    ///     cx: Monitor render context.
    ///
    /// Returns:
    ///     One inline selector row or a narrow two-row control surface.
    fn controls(&self, width: f32, palette: MoonPalette, cx: &mut Context<Self>) -> AnyElement {
        let layout = MonitorLayout::for_width(width, design::ui_value(cx, 1.0));
        let selected_group = self.group;
        let view = cx.entity();
        let groups = [GroupMode::Core, GroupMode::Exchange];
        let group_control = MoonSegmentedControl::new("profit-monitor-groups")
            .items(groups.map(|group| {
                // No tooltip: the segment already shows its own title in full.
                MoonSegmentItem::new("", group_title(group))
                    .fit_width(cx, 58.0, 110.0)
                    .selected(group == selected_group)
            }))
            .on_click(move |index, _, _, app| {
                let Some(group) = groups.get(index).copied() else {
                    return;
                };
                view.update(app, |this, cx| this.set_group(group, cx));
            })
            .render();
        let period = period_dropdown(self.period, cx.entity());
        let settings = self.settings_popover(settings_trigger(self.settings_open), palette, cx);
        let status_clock = h_flex()
            .flex_none()
            .gap(design::ui_px(cx, 10.0))
            .child(auto_status(
                self.db_active,
                self.refresh_error.is_some(),
                layout.status_label,
                palette,
                cx,
            ))
            .child(self.clock.clone())
            .child(settings);
        let content = if layout.inline_controls {
            h_flex()
                .justify_between()
                .gap(design::ui_px(cx, 10.0))
                .child(
                    h_flex()
                        .min_w_0()
                        .gap(design::ui_px(cx, 8.0))
                        .child(period)
                        .child(group_control),
                )
                .child(status_clock)
        } else {
            v_flex()
                .gap(design::ui_px(cx, 6.0))
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .gap(design::ui_px(cx, 10.0))
                        .child(period)
                        .child(status_clock),
                )
                .child(h_flex().w_full().justify_center().child(group_control))
        };

        content
            .w_full()
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 8.0))
            .bg(moon(palette.shell_high))
            .border_b(px(1.0))
            .border_color(moon(palette.border))
            .into_any_element()
    }

    /// Render the current typed load state.
    ///
    /// Args:
    ///     width: Current window width used for deterministic column degradation.
    ///     palette: Active MoonUI palette.
    ///     view: Owning monitor entity receiving sortable-header actions.
    ///     cx: Application context used for rendering.
    ///
    /// Returns:
    ///     Table, split-currency warning, loading placeholder, or error state.
    fn body(&self, width: f32, palette: MoonPalette, view: Entity<Self>, cx: &App) -> AnyElement {
        match &self.data {
            ProfitLoadState::Loading => {
                centered_message(t!("common.loading").to_string(), palette, cx)
            }
            ProfitLoadState::NotReady => {
                centered_message(t!("profit_monitor.not_ready").to_string(), palette, cx)
            }
            ProfitLoadState::Failed(error) => centered_alert(
                t!("profit_monitor.read_failed").to_string(),
                error.to_string(),
                cx,
            ),
            ProfitLoadState::Split(totals) => split_body(
                totals,
                MonitorLayout::for_width(width, design::ui_value(cx, 1.0)).trades,
                palette,
                cx,
            ),
            ProfitLoadState::Ready { unit, data } => {
                let core_label = t!("profit_monitor.core_fallback").to_string();
                let rows = grouped_rows(
                    data,
                    &self.live,
                    self.group,
                    self.prefs.idle_cores,
                    RowLabels { core: &core_label },
                );
                // Folded BEFORE the rows are split into groups, so the window's own total counts
                // every core exactly once. A core saved into two groups appears in both sections
                // on purpose; summing the sections instead would silently double its money here.
                let total = fold_total(&rows);
                let entries = if self.prefs.group_sections && self.group == GroupMode::Core {
                    let ungrouped = t!("profit_monitor.group.ungrouped").to_string();
                    sections::sectioned(
                        rows,
                        &self.live,
                        self.sort,
                        SectionLabels {
                            ungrouped: &ungrouped,
                            // Keep the user-authored name first: at the minimum width, ellipsis may
                            // hide the generic suffix but must not hide which group this closes.
                            subtotal: &|name| {
                                t!("profit_monitor.group.subtotal", name = name).to_string()
                            },
                        },
                    )
                } else {
                    sections::flat(rows, self.sort)
                };
                table::table(
                    entries,
                    total,
                    *unit,
                    width,
                    self.sort,
                    self.prefs,
                    &self.flash,
                    self.backend.read(cx).core_filter(),
                    &self.scroll,
                    palette,
                    view,
                    self.backend.clone(),
                    cx,
                )
            }
        }
    }
}

impl Focusable for ProfitMonitorView {
    /// Return the monitor root focus handle.
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ProfitMonitorView {
    /// Render the complete independent monitor surface.
    ///
    /// Args:
    ///     window: Owning window used for responsive column selection.
    ///     cx: Monitor render context.
    ///
    /// Returns:
    ///     Window chrome, controls, and current report state.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::hotkeys::restore_root_focus(&self.focus, window, cx);
        let palette = MoonPalette::active(cx);
        let width = window_width(window);
        v_flex()
            .size_full()
            .relative()
            .bg(moon(palette.shell))
            .text_color(moon(palette.text))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .track_focus(&self.focus)
            .child(window_header(palette, cx))
            .child(self.controls(width, palette, cx))
            .child(
                AnyView::from(self.content.clone())
                    .cached(StyleRefinement::default().flex_1().min_h(px(0.0)).w_full()),
            )
            .child(
                MoonWindowFrame::tool("profit-monitor-window-hit", width)
                    .header_height(HEADER_HEIGHT)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}

/// Diff one snapshot against the trades already on screen.
///
/// A core counts as having traded when its newest close date MOVED or its trade count GREW. The
/// count is not redundant: close dates carry whole seconds, so a second trade inside one second
/// moves nothing else. Growth only — retention trimming a period lowers the count, and that is not
/// a trade.
///
/// Args:
///     seen: Previous `(newest close date, trade count)` per core, or `None` for no baseline.
///     cores: Per-core aggregates from the snapshot just applied.
///
/// Returns:
///     The replacement memory, and the cores that traded since the previous snapshot. With no
///     baseline nothing arrives — that is the first snapshot after a query change, where every
///     value is new at once. With a baseline, a core APPEARING is an arrival: it is that core's
///     first trade of the period, which is exactly what the highlight is for. A core absent from
///     `cores` is dropped, keeping the memory bounded by the report's own core count.
fn arrivals(
    seen: Option<&HashMap<CoreId, (i64, i64)>>,
    cores: &[ProfitMonitorCore],
) -> (HashMap<CoreId, (i64, i64)>, Vec<CoreId>) {
    let mut next = HashMap::with_capacity(cores.len());
    let mut arrived = Vec::new();
    for core in cores {
        if let Some(seen) = seen {
            let traded = match seen.get(&core.core_uid) {
                Some((close, trades)) => core.last_close > *close || core.trades > *trades,
                None => true,
            };
            if traded {
                arrived.push(core.core_uid);
            }
        }
        next.insert(core.core_uid, (core.last_close, core.trades));
    }
    (next, arrived)
}

/// Return the current logical width for any window-bounds state.
///
/// Args:
///     window: Window whose responsive width is required.
///
/// Returns:
///     Logical width the window currently occupies, in every window state.
fn window_width(window: &Window) -> f32 {
    crate::window::windowing::responsive_width(window)
}

/// Read live labels and canonical core order without touching report SQLite.
///
/// Args:
///     backend: Current shared terminal state.
///
/// Returns:
///     Context sufficient to regroup cached per-core aggregates.
fn capture_live_context(backend: &Backend) -> LiveContext {
    let config = &backend.config;
    let core_names = config
        .servers
        .iter()
        .map(|server| (server.id, server.name.clone()))
        .collect();
    // The session's own admission rule, from `session::lifecycle`: a core inside a server group
    // that is switched off never connects, however active its own checkbox is. Reading the
    // checkbox alone would give such a core a zero row in a window that can never fill it.
    let active = config
        .servers
        .iter()
        // `group_ref`, not `group`: the latter clones a whole `GroupConfig` per server, and this
        // runs for every configured core on every five-second sample. An unlisted group defaults
        // to active (`GroupConfig::new`), which is what `is_none_or` states here.
        .filter(|server| {
            server.active
                && config
                    .group_ref(&server.group)
                    .is_none_or(|group| group.active)
        })
        .map(|server| server.id)
        .collect();
    let order = CoreOrder::new(config);
    let mut core_order = config
        .servers
        .iter()
        .map(|server| server.id)
        .collect::<Vec<_>>();
    order.sort_by(&mut core_order, |core| *core);
    LiveContext {
        venues: backend.session.core_venues().clone(),
        core_names,
        core_order,
        active,
        core_groups: config.core_groups.clone(),
    }
}

/// Combine the two monotonic generations that make projected report values stale.
///
/// Args:
///     report: Report-writer generation, when replication is enabled.
///     valuation: Historical/current valuation generation, when available.
///
/// Returns:
///     Wrapping sum used only as a change token.
fn combined_generation(report: &Option<Arc<AtomicU64>>, valuation: &Option<Arc<AtomicU64>>) -> u64 {
    report
        .as_ref()
        .map(|generation| generation.load(Ordering::Relaxed))
        .unwrap_or(0)
        .wrapping_add(
            valuation
                .as_ref()
                .map(|generation| generation.load(Ordering::Relaxed))
                .unwrap_or(0),
        )
}

/// Return the localized grouping title.
///
/// Args:
///     group: Grouping axis.
///
/// Returns:
///     User-facing selector label.
fn group_title(group: GroupMode) -> String {
    match group {
        GroupMode::Core => t!("profit_monitor.group.core"),
        GroupMode::Exchange => t!("profit_monitor.group.exchange"),
    }
    .to_string()
}

/// Render the period preset as one standard MoonUI dropdown.
///
/// Args:
///     selected: Currently active preset.
///     view: Monitor entity receiving selection changes.
///
/// Returns:
///     A compact dropdown carrying every period choice.
fn period_dropdown(selected: MonitorPeriod, view: Entity<ProfitMonitorView>) -> MoonDropdown {
    let options = MonitorPeriod::ALL.into_iter().map(|period| {
        (
            period,
            SharedString::from(period.id()),
            SharedString::from(period.title()),
        )
    });
    let items = crate::panels::radio_items(
        options,
        selected,
        crate::panels::RadioMark::Highlight,
        move |app, period| {
            view.update(app, |this, cx| this.set_period(period, cx));
        },
    );
    MoonDropdown::new("profit-monitor-period")
        .label(selected.title())
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::Action)
        .fit_trigger_width(96.0, 148.0)
        .fit_menu_width(132.0, 196.0)
        .menu_size(MoonMenuSize::Compact)
        .items(items)
}

/// Render the ⚙ button that opens the monitor's display settings.
///
/// Args:
///     open: Whether the settings popup is currently showing.
///
/// Returns:
///     The settings popover's trigger, from the shared gear helper.
fn settings_trigger(open: bool) -> impl IntoElement {
    crate::panels::popup_gear_trigger(
        "profit-monitor-settings",
        t!("profit_monitor.settings.title").to_string(),
        open,
    )
}

/// Render the custom title bar shared with other tool visuals.
///
/// Args:
///     palette: Active MoonUI palette.
///     cx: Render context.
///
/// Returns:
///     Title cluster and native-looking controls.
fn window_header(palette: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .h(design::fit_h_px(cx, HEADER_HEIGHT, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(moon(palette.shell_high))
        .border_b(px(1.0))
        .border_color(moon_alpha(palette.border, 1.0))
        .child(
            MoonWindowFrame::tool("profit-monitor-title", 0.0)
                .title_cluster(t!("profit_monitor.window_title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |element| {
            element.child(
                MoonWindowFrame::tool("profit-monitor-controls", 0.0)
                    .header_height(HEADER_HEIGHT)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Render automatic-refresh status without requiring a manual button.
///
/// Args:
///     active: Whether a database replacement is in flight.
///     failed: Whether the latest automatic replacement failed while old rows remain visible.
///     show_label: Whether the colored dot has room for its text label.
///     palette: Active MoonUI palette.
///     cx: Render context.
///
/// Returns:
///     Compact live-status cluster.
fn auto_status(
    active: bool,
    failed: bool,
    show_label: bool,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    let label = if active {
        t!("profit_monitor.refreshing").to_string()
    } else if failed {
        t!("profit_monitor.refresh_failed").to_string()
    } else {
        t!("profit_monitor.auto").to_string()
    };
    h_flex()
        .id("profit-monitor-auto-status")
        .flex_none()
        .gap(design::ui_px(cx, 6.0))
        .text_size(design::t_caption(cx))
        .text_color(moon(palette.text_muted))
        .tooltip(crate::panels::common::text_tooltip(label.clone()))
        .child(
            div()
                .w(design::ui_px(cx, 6.0))
                .h(design::ui_px(cx, 6.0))
                .rounded_full()
                .bg(moon(if active {
                    palette.orange
                } else if failed {
                    palette.red
                } else {
                    palette.green
                })),
        )
        .when(show_label, |status| status.child(label))
        .into_any_element()
}
