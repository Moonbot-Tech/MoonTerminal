//! Independent, automatically refreshed desktop Profit Monitor window.

mod rows;

#[cfg(test)]
mod tests;

use std::cmp::Ordering as SortOrdering;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Days, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::db::analytics::{ProfitMonitorSummary, Query};
use moon_core::db::valuation::ValuationMode;
use moon_core::db::{FailKind, ProfitMetric, ProfitUnit, ReadFail, SideFilter};
use moon_core::util::fmt::DeltaSign;
use moon_ui::{
    MoonAlert, MoonBackgroundPolicy, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuSize,
    MoonPalette, MoonScrollbarVisibility, MoonSegmentItem, MoonSegmentedControl, MoonVirtualList,
    MoonVirtualListScrollHandle, MoonWindowFrame, Root, h_flex, v_flex,
};
use rust_i18n::t;

use super::ProfitLoadState;
use super::refresh::{BusyRetryBudget, RefreshGate, RefreshPlan, report_result_is_stale};
use crate::core_order::CoreOrder;
use crate::design::{moon, moon_alpha};
use crate::{Backend, design};
use rows::{GroupMode, LiveContext, MonitorRow, RowLabels, grouped_rows};

const HEADER_HEIGHT: f32 = 32.0;
const CONTEXT_REFRESH_MS: u128 = 5_000;
const SECOND_MS: u128 = 1_000;
const MINUTE_MS: u128 = 60_000;
const PROFIT_COLUMN_WIDTH: f32 = 154.0;
const TRADES_COLUMN_WIDTH: f32 = 72.0;
const WIN_RATE_COLUMN_WIDTH: f32 = 76.0;
const AVERAGE_ORDER_COLUMN_WIDTH: f32 = 116.0;

/// Compact monitor-specific period choices.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum MonitorPeriod {
    /// Rolling hour ending now.
    Hour,
    /// Current calendar day in the header clock's selected city.
    #[default]
    Today,
    /// Previous calendar day in the header clock's selected city.
    Yesterday,
    /// Rolling seven calendar days through tomorrow in the selected city.
    Week,
    /// Current calendar month in the header clock's selected city.
    CurrentMonth,
    /// Rolling thirty days.
    Month,
    /// Rolling year.
    Year,
    /// Complete retained report history.
    All,
}

impl MonitorPeriod {
    /// Presets in their stable display order.
    const ALL: [Self; 8] = [
        Self::Hour,
        Self::Today,
        Self::Yesterday,
        Self::Week,
        Self::CurrentMonth,
        Self::Month,
        Self::Year,
        Self::All,
    ];

    /// Restore a persisted monitor period.
    ///
    /// Args:
    ///     id: Stable layout id.
    ///
    /// Returns:
    ///     Matching period, or `None` for an unknown future value.
    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|period| period.id() == id)
    }

    /// Return the stable layout id.
    ///
    /// Returns:
    ///     A short period key.
    fn id(self) -> &'static str {
        match self {
            Self::Hour => "m-hour",
            Self::Today => "m-today",
            Self::Yesterday => "m-yesterday",
            Self::Week => "m-week",
            Self::CurrentMonth => "m-current-month",
            Self::Month => "m-month",
            Self::Year => "m-year",
            Self::All => "m-all",
        }
    }

    /// Return the localized button title.
    ///
    /// Returns:
    ///     User-facing period label.
    fn title(self) -> String {
        match self {
            Self::Hour => t!("profit_monitor.period.hour"),
            Self::Today => t!("analytics.period.today"),
            Self::Yesterday => t!("analytics.period.yesterday"),
            Self::Week => t!("analytics.period.week"),
            Self::CurrentMonth => t!("analytics.period.cur_month"),
            Self::Month => t!("analytics.period.month"),
            Self::Year => t!("analytics.period.year"),
            Self::All => t!("analytics.period.all"),
        }
        .to_string()
    }

    /// Resolve the query's UTC Unix-second bounds in the selected header-clock zone.
    ///
    /// Args:
    ///     now: Current UTC instant pinned for this calculation.
    ///     zone: IANA zone selected by the window's header clock.
    ///
    /// Returns:
    ///     Inclusive lower and exclusive upper bound.
    fn range_at(self, now: DateTime<Utc>, zone: Tz) -> (i64, i64) {
        if self == Self::Hour {
            let now = now.timestamp();
            return (now.saturating_sub(3_600), now.saturating_add(1));
        }
        let today = now.with_timezone(&zone).date_naive();
        let tomorrow_date = shift_date(today, 1);
        let today_start = zoned_day_start(zone, today);
        let tomorrow = zoned_day_start(zone, tomorrow_date);
        match self {
            Self::Today => (today_start, tomorrow),
            Self::Yesterday => (zoned_day_start(zone, shift_date(today, -1)), today_start),
            Self::Week => (zoned_day_start(zone, shift_date(today, -6)), tomorrow),
            Self::CurrentMonth => {
                let month_start =
                    NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap_or(today);
                (zoned_day_start(zone, month_start), tomorrow)
            }
            Self::Month => (zoned_day_start(zone, shift_date(today, -29)), tomorrow),
            Self::Year => (zoned_day_start(zone, shift_date(today, -364)), tomorrow),
            Self::All => (-1, tomorrow),
            Self::Hour => unreachable!("rolling hour returned above"),
        }
    }
}

/// Shift one civil date without assuming that its UTC day is always 86,400 seconds.
///
/// Args:
///     date: Local calendar date to move.
///     days: Signed number of civil dates.
///
/// Returns:
///     Shifted date, or the input at chrono's representable boundary.
fn shift_date(date: NaiveDate, days: i64) -> NaiveDate {
    if days >= 0 {
        date.checked_add_days(Days::new(days as u64))
            .unwrap_or(date)
    } else {
        date.checked_sub_days(Days::new(days.unsigned_abs()))
            .unwrap_or(date)
    }
}

/// Resolve the first valid local instant at or after one civil date in an IANA zone.
///
/// A few zones have historically advanced at midnight, making `00:00` nonexistent; walking to
/// the first valid minute preserves the correct civil-day boundary, including a date skipped
/// completely by a dateline change. An ambiguous midnight uses its earlier occurrence so no real
/// instant at the start of the date is omitted.
///
/// Args:
///     zone: IANA time zone defining the civil date.
///     date: Local calendar date whose start is required.
///
/// Returns:
///     UTC Unix seconds at the first valid local minute at or after `date`.
///
/// Panics:
///     When the zone has no valid local instant within the following 48 hours.
fn zoned_day_start(zone: Tz, date: NaiveDate) -> i64 {
    let midnight = date
        .and_hms_opt(0, 0, 0)
        .expect("a valid date always has midnight");
    for minute in 0..=48 * 60 {
        let Some(candidate) = midnight.checked_add_signed(chrono::Duration::minutes(minute)) else {
            break;
        };
        match zone.from_local_datetime(&candidate) {
            LocalResult::Single(value) => return value.timestamp(),
            LocalResult::Ambiguous(first, second) => return first.min(second).timestamp(),
            LocalResult::None => {}
        }
    }
    panic!("no valid local instant within 48 hours of {date} in {zone}")
}

/// Read the current UTC instant through the terminal's shared system-clock source.
///
/// Returns:
///     Current UTC time, or the Unix epoch when the platform clock is outside chrono's range.
fn now_utc() -> DateTime<Utc> {
    DateTime::from_timestamp_millis(moon_core::util::now_unix_ms_i64())
        .unwrap_or(DateTime::UNIX_EPOCH)
}

/// Resolve a saved header-clock zone through the same curated-city policy as the visible clock.
///
/// Args:
///     zone_id: Persisted IANA zone id from the shared header clock.
///
/// Returns:
///     Displayed city zone, or UTC when no valid curated selection exists.
fn monitor_zone(zone_id: Option<&str>) -> Tz {
    crate::chrome::clock::resolved_header_clock_zone(zone_id)
}

/// Effect of a non-report Backend update on the open monitor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContextChange {
    /// No visible or query input changed.
    None,
    /// Cached database rows need only a new display grouping pass.
    Regroup,
    /// Query inputs changed, so database values must be re-read and the clock may need re-arming.
    Reload {
        /// Whether a selected-zone change invalidated the current midnight timer.
        restart_clock: bool,
    },
}

/// Columns retained at one monitor width.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VisibleColumns {
    /// Whether win rate remains visible.
    win_rate: bool,
    /// Whether average order remains visible.
    average_order: bool,
}

impl VisibleColumns {
    /// Select columns in deterministic least-important-first degradation order.
    ///
    /// Args:
    ///     width: Current logical window width.
    ///
    /// Returns:
    ///     Name, Profit, and Trades are always present; Win rate appears at 620 and Average order
    ///     at 760 logical pixels.
    fn for_width(width: f32) -> Self {
        Self {
            win_rate: width >= 620.0,
            average_order: width >= 760.0,
        }
    }
}

/// Sortable columns exposed by the Profit Monitor header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MonitorSortColumn {
    /// Visible group label.
    Name,
    /// Projected profit.
    Profit,
    /// Closed-trade count.
    Trades,
    /// Profitable-trade percentage.
    WinRate,
    /// Average positive order spend.
    AverageOrder,
}

impl MonitorSortColumn {
    /// Restore one stable persisted column id.
    ///
    /// Args:
    ///     id: Stable layout value.
    ///
    /// Returns:
    ///     Matching column, or `None` for an unknown future value.
    fn from_id(id: &str) -> Option<Self> {
        match id {
            "name" => Some(Self::Name),
            "profit" => Some(Self::Profit),
            "trades" => Some(Self::Trades),
            "win-rate" => Some(Self::WinRate),
            "average-order" => Some(Self::AverageOrder),
            _ => None,
        }
    }

    /// Return the stable layout id.
    ///
    /// Returns:
    ///     Column key suitable for `layout.toml`.
    fn id(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Profit => "profit",
            Self::Trades => "trades",
            Self::WinRate => "win-rate",
            Self::AverageOrder => "average-order",
        }
    }

    /// Return the useful direction for a first click on this column.
    ///
    /// Returns:
    ///     Name sorts ascending; numeric columns sort descending.
    fn first_click_descending(self) -> bool {
        self != Self::Name
    }

    /// Compare two rows in this column's ascending order.
    ///
    /// Args:
    ///     left: First row.
    ///     right: Second row.
    ///
    /// Returns:
    ///     Ascending column ordering.
    fn compare(self, left: &MonitorRow, right: &MonitorRow) -> SortOrdering {
        match self {
            Self::Name => left
                .sort_name
                .to_lowercase()
                .cmp(&right.sort_name.to_lowercase()),
            Self::Profit => left.profit.total_cmp(&right.profit),
            Self::Trades => left.trades.cmp(&right.trades),
            Self::WinRate => left.win_rate().total_cmp(&right.win_rate()),
            Self::AverageOrder => left.average_order().total_cmp(&right.average_order()),
        }
    }
}

/// One explicit user-selected table ordering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MonitorSort {
    /// Primary column.
    column: MonitorSortColumn,
    /// Whether the primary comparison is reversed.
    descending: bool,
}

impl MonitorSort {
    /// Restore one leniently parsed layout tuple.
    ///
    /// Args:
    ///     id: Persisted stable column key.
    ///     descending: Persisted direction.
    ///
    /// Returns:
    ///     Valid sort, or `None` when a future column is unknown.
    fn from_layout(id: &str, descending: bool) -> Option<Self> {
        Some(Self {
            column: MonitorSortColumn::from_id(id)?,
            descending,
        })
    }
}

/// Derive the explicit ordering produced by one header click.
///
/// Args:
///     current: Current explicit ordering, if the user has selected one.
///     column: Clicked header column.
///
/// Returns:
///     Same-column direction toggle or the column's useful first-click direction.
fn next_sort(current: Option<MonitorSort>, column: MonitorSortColumn) -> MonitorSort {
    MonitorSort {
        column,
        descending: match current {
            Some(sort) if sort.column == column => !sort.descending,
            _ => column.first_click_descending(),
        },
    }
}

/// Apply an explicit ordering while preserving deterministic ascending name ties.
///
/// Args:
///     rows: Grouped rows to reorder in place.
///     sort: User-selected ordering, or `None` for the grouping's natural order.
fn sort_rows(rows: &mut [MonitorRow], sort: Option<MonitorSort>) {
    let Some(sort) = sort else {
        return;
    };
    rows.sort_by(|left, right| {
        let primary = sort.column.compare(left, right);
        let primary = if sort.descending {
            primary.reverse()
        } else {
            primary
        };
        primary
            .then_with(|| {
                left.sort_name
                    .to_lowercase()
                    .cmp(&right.sort_name.to_lowercase())
            })
            .then_with(|| left.primary_core.cmp(&right.primary_core))
    });
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

/// Classify a live-context update without coupling identity changes to SQLite work.
///
/// Args:
///     before: Context represented by the current table.
///     after: Newly sampled context.
///     valuation_changed: Whether the application-wide projection mode changed.
///     zone_changed: Whether the header clock selected a different curated zone.
///
/// Returns:
///     Nothing, a cheap regroup, or a database reload plan with its timer effect.
fn context_change(
    before: &LiveContext,
    after: &LiveContext,
    valuation_changed: bool,
    zone_changed: bool,
) -> ContextChange {
    if valuation_changed || zone_changed {
        ContextChange::Reload {
            restart_clock: zone_changed,
        }
    } else if before != after {
        ContextChange::Regroup
    } else {
        ContextChange::None
    }
}

/// Preserve the last known exchange name while a core is temporarily disconnected.
///
/// Args:
///     previous: Exchange context already represented by the open monitor.
///     sampled: Newly sampled live and configured context.
///
/// Returns:
///     New context with missing exchange names filled from their last observed values.
fn retain_last_known_exchange_names(
    previous: &LiveContext,
    mut sampled: LiveContext,
) -> LiveContext {
    for (core, exchange) in &previous.exchange_names {
        sampled
            .exchange_names
            .entry(*core)
            .or_insert_with(|| exchange.clone());
    }
    sampled
}

/// Return the delay to the next wall-clock boundary of one interval.
///
/// Recomputing from Unix time after every tick prevents refreshes from drifting with process start
/// time or executor delays.
///
/// Args:
///     now: Current system wall clock.
///     interval_ms: UTC-aligned boundary interval in milliseconds.
///
/// Returns:
///     Positive delay to the next interval boundary.
fn duration_until_wall_clock_boundary(now: SystemTime, interval_ms: u128) -> Duration {
    let Ok(since_epoch) = now.duration_since(UNIX_EPOCH) else {
        return Duration::from_millis(interval_ms as u64);
    };
    let elapsed_ms = since_epoch.as_millis();
    let remaining_ms = interval_ms - elapsed_ms % interval_ms;
    Duration::from_millis(remaining_ms as u64)
}

/// Return the next wall-clock refresh for one period.
///
/// Args:
///     period: Active monitor preset.
///     zone: IANA zone selected by the window's header clock.
///     now: Current system wall clock.
///
/// Returns:
///     Minute-boundary wait for Hour, next local midnight for calendar presets, or `None` for All.
fn duration_until_period_refresh(
    period: MonitorPeriod,
    zone: Tz,
    now: SystemTime,
) -> Option<Duration> {
    match period {
        MonitorPeriod::Hour => Some(duration_until_wall_clock_boundary(now, MINUTE_MS)),
        MonitorPeriod::All => None,
        MonitorPeriod::Today
        | MonitorPeriod::Yesterday
        | MonitorPeriod::Week
        | MonitorPeriod::CurrentMonth
        | MonitorPeriod::Month
        | MonitorPeriod::Year => {
            let since_epoch = now.duration_since(UNIX_EPOCH).ok()?;
            let now_utc =
                DateTime::from_timestamp_millis(i64::try_from(since_epoch.as_millis()).ok()?)?;
            let (_, next_midnight) = MonitorPeriod::Today.range_at(now_utc, zone);
            let target_ms = u128::try_from(next_midnight).ok()?.saturating_mul(1_000);
            let remaining_ms = target_ms.saturating_sub(since_epoch.as_millis()).max(1);
            Some(Duration::from_millis(
                u64::try_from(remaining_ms).unwrap_or(u64::MAX),
            ))
        }
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
    ///     backend: Shared terminal state containing the selected clock city.
    ///     cx: Child view context that owns the recurring timer.
    ///
    /// Returns:
    ///     Clock state whose repaint cadence is isolated from the parent monitor.
    fn new(backend: Entity<Backend>, cx: &mut Context<Self>) -> Self {
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
    ///     _window: Owning Profit Monitor window.
    ///     cx: Clock render context used to read the active palette and backend state.
    ///
    /// Returns:
    ///     Shared terminal clock element.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = MoonPalette::active(cx);
        crate::chrome::clock::header_clock(&self.backend, palette, cx)
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
    valuation: ValuationMode,
    live: LiveContext,
    data: ProfitLoadState<ProfitMonitorSummary>,
    refresh_error: Option<ReadFail>,
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
        // Apply the shared independent-window taskbar policy now and after every activation;
        // `hide_window_from_taskbar_soon` owns the delayed retry rationale.
        crate::window::windowing::hide_window_from_taskbar_soon(window.window_handle(), cx);
        cx.observe_window_activation(window, |_this, window, cx| {
            crate::window::windowing::hide_window_from_taskbar_soon(window.window_handle(), cx);
        })
        .detach();

        cx.observe_window_bounds(window, |this, window, cx| {
            let Some((x, y, w, h)) = crate::window::windowing::window_geom(window) else {
                return;
            };
            this.backend.update(cx, |backend, _| {
                if backend
                    .layout
                    .profit_monitor_window
                    .map(|geometry| (geometry.x, geometry.y, geometry.w, geometry.h))
                    != Some((x, y, w, h))
                {
                    backend.layout.profit_monitor_window =
                        Some(moon_core::config::layout::GeomRect { x, y, w, h });
                    backend.layout_dirty = true;
                }
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
        let valuation = backend.read(cx).valuation_mode();
        let live = capture_live_context(backend.read(cx));

        let report_revision = backend.read(cx).report_revision.clone();
        cx.observe(&report_revision, |this, _, cx| {
            this.observe_report_generation(cx);
        })
        .detach();
        let mut this = Self {
            backend,
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
            valuation,
            live,
            data: ProfitLoadState::default(),
            refresh_error: None,
            scroll: MoonVirtualListScrollHandle::new(),
            focus: cx.focus_handle(),
        };
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
        let next = retain_last_known_exchange_names(&self.live, capture_live_context(backend));
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
        let (from, to) = self.period.range_at(now_utc(), self.zone);
        Query {
            from,
            to,
            cores: Vec::new(),
            side: SideFilter::All,
            emulator: Some(false),
            strategies: Vec::new(),
            metric: ProfitMetric::Quote,
            valuation: self.valuation,
        }
    }

    /// Start a compact database read, preserving visible rows for automatic catch-up refreshes.
    ///
    /// Args:
    ///     after_report: Whether an existing visible snapshot may remain until replacement.
    ///     cx: View context used to spawn and publish the read.
    fn reload(&mut self, after_report: bool, cx: &mut Context<Self>) {
        if self.db_active {
            if !after_report {
                self.seq = self.seq.wrapping_add(1);
                self.data = ProfitLoadState::default();
                self.refresh_error = None;
                self.invalidate_content(cx);
                cx.notify();
            }
            self.refresh
                .request_refresh(std::time::Instant::now(), false);
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
                            this.refresh
                                .request_refresh(std::time::Instant::now(), false);
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
            self.refresh
                .request_refresh(std::time::Instant::now(), false);
            self.schedule_refresh(cx);
        } else {
            log::warn!("profit monitor: automatic database retry budget exhausted");
        }
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
    ///     width: Current window width used to compact the status.
    ///     palette: Active MoonUI palette.
    ///     cx: Monitor render context.
    ///
    /// Returns:
    ///     One compact selector row with period, grouping, and refresh status.
    fn controls(&self, width: f32, palette: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let selected_group = self.group;
        let view = cx.entity();
        let groups = [GroupMode::Core, GroupMode::Exchange];
        let group_control = MoonSegmentedControl::new("profit-monitor-groups")
            .items(groups.map(|group| {
                let title = group_title(group);
                MoonSegmentItem::new("", title.clone())
                    .fit_width(cx, 58.0, 110.0)
                    .tooltip(title)
                    .selected(group == selected_group)
            }))
            .on_click(move |index, _, _, app| {
                let Some(group) = groups.get(index).copied() else {
                    return;
                };
                view.update(app, |this, cx| this.set_group(group, cx));
            })
            .render();

        h_flex()
            .w_full()
            .justify_between()
            .gap(design::ui_px(cx, 10.0))
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 8.0))
            .bg(moon(palette.shell_high))
            .border_b(px(1.0))
            .border_color(moon(palette.border))
            .child(
                h_flex()
                    .min_w_0()
                    .gap(design::ui_px(cx, 8.0))
                    .child(period_dropdown(self.period, cx.entity()))
                    .child(group_control),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap(design::ui_px(cx, 10.0))
                    .child(auto_status(
                        self.db_active,
                        self.refresh_error.is_some(),
                        width < 700.0,
                        palette,
                        cx,
                    ))
                    .child(self.clock.clone()),
            )
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
            ProfitLoadState::Split(totals) => split_body(totals, palette, cx),
            ProfitLoadState::Ready { unit, data } => {
                let core_label = t!("profit_monitor.core_fallback").to_string();
                let unknown_exchange = t!("profit_monitor.unknown_exchange").to_string();
                let spot = t!("common.exchange_spot").to_string();
                let mut rows = grouped_rows(
                    data,
                    &self.live,
                    self.group,
                    RowLabels {
                        core: &core_label,
                        unknown_exchange: &unknown_exchange,
                        spot: &spot,
                    },
                );
                sort_rows(&mut rows, self.sort);
                table(
                    rows,
                    *unit,
                    width,
                    self.sort,
                    &self.scroll,
                    palette,
                    view,
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

/// Return the current logical width for any window-bounds state.
///
/// Args:
///     window: Window whose responsive width is required.
///
/// Returns:
///     Logical width for windowed, maximized, or fullscreen bounds.
fn window_width(window: &Window) -> f32 {
    match window.window_bounds() {
        WindowBounds::Windowed(bounds)
        | WindowBounds::Maximized(bounds)
        | WindowBounds::Fullscreen(bounds) => f32::from(bounds.size.width),
    }
}

/// Read live labels and canonical core order without touching report SQLite.
///
/// Args:
///     backend: Current shared terminal state.
///
/// Returns:
///     Context sufficient to regroup cached per-core aggregates.
fn capture_live_context(backend: &Backend) -> LiveContext {
    let exchange_names = backend.session.market_source().core_exchange_names();
    let (core_names, core_order) = capture_config_context(&backend.config);
    LiveContext {
        exchange_names,
        core_names,
        core_order,
    }
}

/// Read configured core names in the canonical order used by every terminal core list.
///
/// Args:
///     config: Current application configuration.
///
/// Returns:
///     Name lookup and canonical configured-core order for one render or identity sample.
fn capture_config_context(
    config: &moon_core::config::AppConfig,
) -> (std::collections::HashMap<u64, String>, Vec<u64>) {
    let core_names = config
        .servers
        .iter()
        .map(|server| (server.id, server.name.clone()))
        .collect();
    let order = CoreOrder::new(config);
    let mut core_order = config
        .servers
        .iter()
        .map(|server| server.id)
        .collect::<Vec<_>>();
    order.sort_by(&mut core_order, |core| *core);
    (core_names, core_order)
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
///     compact: Whether only the colored dot fits beside the grouping control.
///     palette: Active MoonUI palette.
///     cx: Render context.
///
/// Returns:
///     Compact live-status cluster.
fn auto_status(
    active: bool,
    failed: bool,
    compact: bool,
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
        .when(!compact, |status| status.child(label))
        .into_any_element()
}

/// Render a centered neutral message.
///
/// Args:
///     message: User-facing text.
///     palette: Active MoonUI palette.
///     cx: Render context.
///
/// Returns:
///     Full-height centered placeholder.
fn centered_message(message: String, palette: MoonPalette, cx: &App) -> AnyElement {
    div()
        .flex_1()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(moon(palette.text_muted))
        .text_size(design::t_body(cx))
        .child(message)
        .into_any_element()
}

/// Render a centered classified read failure.
///
/// Args:
///     title: Localized heading.
///     detail: Classified database detail.
///     cx: Render context.
///
/// Returns:
///     Centered MoonAlert.
fn centered_alert(title: String, detail: String, cx: &App) -> AnyElement {
    div()
        .flex_1()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .px(design::ui_px(cx, 20.0))
        .child(MoonAlert::error("profit-monitor-error", detail).title(title))
        .into_any_element()
}

/// Render exact split quote totals without a false combined scalar.
///
/// Args:
///     totals: Per-quote safe totals.
///     palette: Active MoonUI palette.
///     cx: Render context.
///
/// Returns:
///     Explanation, quote chips, and complete trade count.
fn split_body(
    totals: &moon_core::db::QuoteBreakdown,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    let mut chips = h_flex()
        .flex_wrap()
        .justify_center()
        .gap(design::ui_px(cx, 6.0));
    for total in &totals.totals {
        let (amount, sign) = total.signed_display();
        chips = chips.child(
            div()
                .px(design::ui_px(cx, 9.0))
                .py(design::ui_px(cx, 5.0))
                .rounded(design::ui_px(cx, 5.0))
                .bg(moon(palette.table_head))
                .text_color(moon(sign.pick(
                    design::positive_color(palette),
                    design::danger_color(palette),
                    palette.text,
                )))
                .child(amount),
        );
    }
    v_flex()
        .flex_1()
        .w_full()
        .items_center()
        .justify_center()
        .gap(design::ui_px(cx, 12.0))
        .px(design::ui_px(cx, 20.0))
        .text_align(TextAlign::Center)
        .child(
            div()
                .text_color(moon(palette.text))
                .child(t!("profit_monitor.split_title").to_string()),
        )
        .child(
            div()
                .max_w(design::ui_px(cx, 560.0))
                .text_color(moon(palette.text_muted))
                .child(t!("profit_monitor.split_detail").to_string()),
        )
        .child(chips)
        .child(
            div()
                .text_color(moon(palette.text_soft))
                .child(t!("profit_monitor.trades_total", n = totals.orders).to_string()),
        )
        .into_any_element()
}

/// Render the responsive monitor table and exact total footer.
///
/// Args:
///     rows: Already grouped and sorted display rows.
///     unit: Comparable exact unit, or `None` for an empty result.
///     width: Current window width.
///     sort: Explicit user-selected ordering, if any.
///     scroll: Retained vertical-list position.
///     palette: Active MoonUI palette.
///     view: Owning monitor entity receiving sortable-header actions.
///     cx: Application context used for rendering.
///
/// Returns:
///     Fixed header/footer with a vertically scrolling row body.
fn table(
    rows: Vec<MonitorRow>,
    unit: Option<ProfitUnit>,
    width: f32,
    sort: Option<MonitorSort>,
    scroll: &MoonVirtualListScrollHandle,
    palette: MoonPalette,
    view: Entity<ProfitMonitorView>,
    cx: &App,
) -> AnyElement {
    let columns = VisibleColumns::for_width(width);
    let show_win = columns.win_rate;
    let show_average = columns.average_order;
    let total = rows.iter().fold(MonitorRow::default(), |mut total, row| {
        total.profit += row.profit;
        total.trades += row.trades;
        total.wins += row.wins;
        total.positive_spent += row.positive_spent;
        total.positive_orders += row.positive_orders;
        total
    });
    let header = table_header(columns, sort, view, palette, cx);
    let rows = Arc::new(rows);
    let row_count = rows.len();
    let list_rows = rows.clone();
    let row_height = design::fit_h_value(cx, 34.0, 13.0, 8.0);
    let body = MoonVirtualList::new(
        "profit-monitor-rows",
        row_count,
        row_height,
        move |index, _window, app| {
            let Some(row) = list_rows.get(index) else {
                return div().into_any_element();
            };
            let (profit, profit_sign) = format_profit(row.profit, unit);
            table_row(
                row.name.clone(),
                profit,
                profit_sign,
                format_trade_count(row.trades),
                Some(format_win_rate(row.win_rate())),
                Some(format_amount(row.average_order(), unit)),
                show_win,
                show_average,
                palette,
                app,
            )
            .when(index % 2 == 1, |element| {
                element.bg(moon_alpha(palette.table_head, 0.45))
            })
            .into_any_element()
        },
    )
    .track_scroll(scroll)
    .scrollbar_visibility(MoonScrollbarVisibility::Always)
    .surface(false)
    .border(false)
    .radius(0.0);
    let (total_profit, total_profit_sign) = format_profit(total.profit, unit);
    let footer = table_row(
        t!("profit_monitor.total").to_string(),
        total_profit,
        total_profit_sign,
        format_trade_count(total.trades),
        Some(format_win_rate(total.win_rate())),
        Some(format_amount(total.average_order(), unit)),
        show_win,
        show_average,
        palette,
        cx,
    )
    .h(design::fit_h_px(cx, 42.0, 14.0, 10.0))
    .bg(moon(palette.table_head))
    .text_size(design::t_title(cx))
    .font_weight(FontWeight::SEMIBOLD)
    .border_t(px(2.0))
    .border_color(moon_alpha(palette.amber, 0.7));

    v_flex()
        .flex_1()
        .min_h_0()
        .w_full()
        .child(header)
        .child(div().flex_1().min_h_0().w_full().child(body))
        .child(footer)
        .into_any_element()
}

/// Render the fixed clickable header whose geometry mirrors the data rows.
///
/// Args:
///     columns: Responsive visible-column selection.
///     sort: Current explicit ordering.
///     view: Monitor entity receiving heading clicks.
///     palette: Active MoonUI palette.
///     cx: Render context used for scaled geometry.
///
/// Returns:
///     Fixed-height sortable table header.
fn table_header(
    columns: VisibleColumns,
    sort: Option<MonitorSort>,
    view: Entity<ProfitMonitorView>,
    palette: MoonPalette,
    cx: &App,
) -> Div {
    let sortable = |id: &'static str,
                    title: String,
                    column: MonitorSortColumn,
                    width: Option<f32>,
                    right: bool| {
        let active = sort.is_some_and(|active| active.column == column);
        let tooltip = title.clone();
        let target = view.clone();
        let mut cell = div()
            .id(id)
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .cursor_pointer()
            .hover(|style| style.text_color(moon(palette.amber)))
            .text_color(moon(if active {
                palette.amber
            } else {
                palette.text_soft
            }))
            .tooltip(crate::panels::common::text_tooltip(tooltip))
            .child(format!("{title}{}", sort_arrow(sort, column)))
            .on_click(move |_, _, app| {
                target.update(app, |this, cx| this.toggle_sort(column, cx));
            });
        if right {
            cell = cell.text_align(TextAlign::Right);
        }
        match width {
            Some(width) => cell.w(design::ui_px(cx, width)).flex_none(),
            None => cell.flex_1(),
        }
    };

    h_flex()
        .w_full()
        .h(design::fit_h_px(cx, 34.0, 13.0, 8.0))
        .px(design::ui_px(cx, 10.0))
        .gap(design::ui_px(cx, 8.0))
        .bg(moon(palette.table_head))
        .border_b(px(1.0))
        .border_color(moon_alpha(palette.border, 0.7))
        .child(sortable(
            "profit-monitor-heading-name",
            t!("profit_monitor.column.name").to_string(),
            MonitorSortColumn::Name,
            None,
            false,
        ))
        .child(sortable(
            "profit-monitor-heading-profit",
            t!("profit_monitor.column.profit").to_string(),
            MonitorSortColumn::Profit,
            Some(PROFIT_COLUMN_WIDTH),
            true,
        ))
        .child(sortable(
            "profit-monitor-heading-trades",
            t!("profit_monitor.column.trades").to_string(),
            MonitorSortColumn::Trades,
            Some(TRADES_COLUMN_WIDTH),
            true,
        ))
        .when(columns.win_rate, |header| {
            header.child(sortable(
                "profit-monitor-heading-win-rate",
                t!("profit_monitor.column.win_rate").to_string(),
                MonitorSortColumn::WinRate,
                Some(WIN_RATE_COLUMN_WIDTH),
                true,
            ))
        })
        .when(columns.average_order, |header| {
            header.child(sortable(
                "profit-monitor-heading-average-order",
                t!("profit_monitor.column.average_order").to_string(),
                MonitorSortColumn::AverageOrder,
                Some(AVERAGE_ORDER_COLUMN_WIDTH),
                true,
            ))
        })
}

/// Render one responsive table line.
///
/// Args:
///     name: Leading label.
///     profit: Profit text.
///     profit_sign: Sign represented by the already-rounded profit text.
///     trades: Trade-count text.
///     win_rate: Optional win-rate text.
///     average_order: Optional average-order text.
///     show_win: Whether the current width retains win rate.
///     show_average: Whether the current width retains average order.
///     palette: Active MoonUI palette.
///     cx: Render context.
///
/// Returns:
///     One fixed-height table row.
#[allow(clippy::too_many_arguments)]
fn table_row(
    name: String,
    profit: String,
    profit_sign: DeltaSign,
    trades: String,
    win_rate: Option<String>,
    average_order: Option<String>,
    show_win: bool,
    show_average: bool,
    palette: MoonPalette,
    cx: &App,
) -> Div {
    let name_tooltip = name.clone();
    let profit_color = profit_sign.pick(
        design::positive_color(palette),
        design::danger_color(palette),
        palette.text,
    );
    h_flex()
        .w_full()
        .h(design::fit_h_px(cx, 34.0, 13.0, 8.0))
        .px(design::ui_px(cx, 10.0))
        .gap(design::ui_px(cx, 8.0))
        .border_b(px(1.0))
        .border_color(moon_alpha(palette.border, 0.7))
        .child(
            div()
                .id(SharedString::from(format!(
                    "profit-monitor-name:{name_tooltip}"
                )))
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .tooltip(crate::panels::common::text_tooltip(name_tooltip))
                .child(name),
        )
        .child(numeric_cell(profit, PROFIT_COLUMN_WIDTH, cx).text_color(moon(profit_color)))
        .child(numeric_cell(trades, TRADES_COLUMN_WIDTH, cx))
        .when(show_win, |element| {
            element.child(numeric_cell(
                win_rate.unwrap_or_default(),
                WIN_RATE_COLUMN_WIDTH,
                cx,
            ))
        })
        .when(show_average, |element| {
            element.child(numeric_cell(
                average_order.unwrap_or_default(),
                AVERAGE_ORDER_COLUMN_WIDTH,
                cx,
            ))
        })
}

/// Render one fixed-width numeric cell without allowing a value to create a second row.
///
/// Args:
///     text: Complete formatted value.
///     width: Design-reference column width.
///     cx: Application context used for UI scaling.
///
/// Returns:
///     Right-aligned, single-line, safely truncated cell.
fn numeric_cell(text: String, width: f32, cx: &App) -> Div {
    div()
        .w(design::ui_px(cx, width))
        .flex_none()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_align(TextAlign::Right)
        .child(text)
}

/// Format profit with its exact comparable unit.
///
/// Args:
///     value: Projected profit.
///     unit: Exact quote or percent unit.
///
/// Returns:
///     Signed compact text carrying its unit and the sign represented after display rounding.
fn format_profit(value: f64, unit: Option<ProfitUnit>) -> (String, DeltaSign) {
    let decimals = match unit {
        Some(ProfitUnit::Quote(currency)) => currency.display_decimals(),
        Some(ProfitUnit::Percent) | None => 2,
    };
    let (amount, sign) = moon_core::util::fmt::signed_amount(value, decimals);
    let text = match unit {
        Some(ProfitUnit::Quote(currency)) => format!("{amount} {}", currency.ticker()),
        Some(ProfitUnit::Percent) => format!("{amount}%"),
        None => amount,
    };
    (text, sign)
}

/// Format a monitor trade count with the terminal's shared thousands grouping.
///
/// Args:
///     value: Closed-trade count.
///
/// Returns:
///     ASCII digits separated into space-grouped thousands.
fn format_trade_count(value: i64) -> String {
    moon_core::util::fmt::group_thousands(&value.to_string())
}

/// Format win rate with the terminal's shared half-away-from-zero percentage rounding.
///
/// Args:
///     value: Win percentage in `0..=100`.
///
/// Returns:
///     Percentage with one decimal place.
fn format_win_rate(value: f64) -> String {
    moon_core::util::fmt::pct(value, 1)
        .map(|(text, _)| text)
        .unwrap_or_else(|| "0.0%".to_string())
}

/// Format average order spend in the query's comparable quote unit.
///
/// Args:
///     value: Average positive spend.
///     unit: Exact query unit.
///
/// Returns:
///     Compact unsigned order size with a quote ticker when known.
fn format_amount(value: f64, unit: Option<ProfitUnit>) -> String {
    match unit {
        Some(ProfitUnit::Quote(currency)) => format!(
            "{} {}",
            moon_core::util::fmt::compact(value, currency.display_decimals()),
            currency.ticker()
        ),
        Some(ProfitUnit::Percent) | None => moon_core::util::fmt::compact(value, 2),
    }
}

/// Open or focus the independent singleton Profit Monitor window.
///
/// This toolbar action is one route back to the taskbar-hidden monitor; Alt+Tab is the other.
/// `activate_window` restores an iconic window before foregrounding it, so it reopens a monitor
/// that the user minimized.
///
/// Args:
///     backend: Shared terminal state retaining the singleton handle.
///     owner: Launching Main window, used only to choose the initial display.
///     owner_display: Display captured by the toolbar click.
///     cx: Application context used to create or activate the window.
pub(crate) fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    if let Some(handle) = backend.read(cx).profit_monitor_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let saved = backend.read(cx).layout.profit_monitor_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(160.0), px(120.0)),
            size: size(px(720.0), px(520.0)),
        },
        |geometry| Bounds {
            origin: point(px(geometry.x as f32), px(geometry.y as f32)),
            size: size(px(geometry.w as f32), px(geometry.h as f32)),
        },
    );
    let display_id = crate::window::windowing::saved_or_owner_display_id(
        saved.map(|geometry| point(px(geometry.x as f32), px(geometry.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let options = crate::window::windowing::profit_monitor_window_options(
        t!("profit_monitor.window_title").to_string(),
        WindowBounds::Windowed(bounds),
        display_id,
        Some(size(px(460.0), px(320.0))),
    );
    let view_backend = backend.clone();
    if let Ok(handle) = cx.open_window(options, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        crate::window::windowing::set_group_window_icon(window, 0);
        let view = cx.new(|cx| ProfitMonitorView::new(view_backend, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        backend.update(cx, |backend, _| {
            backend.profit_monitor_window = Some(handle)
        });
        crate::window::windowing::activate_new_window(handle.into(), cx);
    }
}
