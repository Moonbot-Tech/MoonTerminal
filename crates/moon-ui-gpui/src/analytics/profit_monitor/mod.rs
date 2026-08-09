//! Independent, automatically refreshed desktop Profit Monitor window.

mod rows;
mod settings;

#[cfg(test)]
mod tests;

use std::cmp::Ordering as SortOrdering;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Utc};
use chrono_tz::Tz;
use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::db::analytics::{
    PreviousPeriodBasis, ProfitMonitorCore, ProfitMonitorSummary, Query,
};
use moon_core::db::valuation::ValuationMode;
use moon_core::db::{FailKind, ProfitMetric, ProfitUnit, ReadFail, SideFilter};
use moon_core::session::CoreId;
use moon_core::util::fmt::DeltaSign;
use moon_ui::{
    MoonAlert, MoonBackgroundPolicy, MoonButton, MoonButtonIconSlot, MoonButtonSize,
    MoonButtonVariant, MoonDropdown, MoonMenuSize, MoonPalette, MoonScrollbarVisibility,
    MoonSegmentItem, MoonSegmentedControl, MoonVirtualList, MoonVirtualListScrollHandle,
    MoonWindowFrame, Root, h_flex, v_flex,
};
use rust_i18n::t;

use super::ProfitLoadState;
use super::refresh::{BusyRetryBudget, RefreshGate, RefreshPlan, report_result_is_stale};
use crate::core_order::CoreOrder;
use crate::design::{moon, moon_alpha};
use crate::media::exchange_logos::exchange_logo;
use crate::pulse::FLASH;
use crate::{Backend, design};
use rows::{GroupMode, LiveContext, MonitorRow, RowLabels, grouped_rows};
use settings::MonitorPrefs;

const HEADER_HEIGHT: f32 = 32.0;
const CONTEXT_REFRESH_MS: u128 = 5_000;
const SECOND_MS: u128 = 1_000;
const MINUTE_MS: u128 = 60_000;
const MIN_NAME_COLUMN_WIDTH: f32 = 128.0;
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
const TRADES_WIDTH: f32 = 390.0;
const LAST_TRADE_WIDTH: f32 = 500.0;
const STACKED_CONTROLS_WIDTH: f32 = 460.0;
const WIN_RATE_WIDTH: f32 = 620.0;
const STATUS_LABEL_WIDTH: f32 = 700.0;
const AVERAGE_ORDER_WIDTH: f32 = 760.0;
const MIN_WINDOW_WIDTH: f32 = 310.0;

/// Compact monitor-specific period choices.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum MonitorPeriod {
    /// Rolling hour ending now.
    Hour,
    /// Current calendar day in the application-wide selected zone.
    #[default]
    Today,
    /// Previous calendar day in the application-wide selected zone.
    Yesterday,
    /// Rolling seven calendar days through tomorrow in the selected zone.
    Week,
    /// Current calendar month in the application-wide selected zone.
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
        let shift = |days| moon_core::util::display_time::shift_date(today, days);
        let start = |date| {
            moon_core::util::display_time::day_start(date, zone)
                .expect("current display-zone date has a valid nearby instant")
        };
        let today_start = start(today);
        let tomorrow = start(shift(1));
        let shifted = |days| {
            moon_core::util::display_time::shift_day_start(today_start, days, zone)
                .unwrap_or(today_start)
        };
        match self {
            Self::Today => (today_start, tomorrow),
            Self::Yesterday => (shifted(-1), today_start),
            Self::Week => (shifted(-6), tomorrow),
            Self::CurrentMonth => {
                let month_start = today.with_day(1).unwrap_or(today);
                (start(month_start), tomorrow)
            }
            Self::Month => (shifted(-29), tomorrow),
            Self::Year => (shifted(-364), tomorrow),
            Self::All => (-1, tomorrow),
            Self::Hour => unreachable!("rolling hour returned above"),
        }
    }
}

/// Read the current UTC instant through the terminal's shared system-clock source.
///
/// Returns:
///     Current UTC time, or the Unix epoch when the platform clock is outside chrono's range.
fn now_utc() -> DateTime<Utc> {
    DateTime::from_timestamp_millis(moon_core::util::now_unix_ms_i64())
        .unwrap_or(DateTime::UNIX_EPOCH)
}

/// Resolve a saved header-clock zone through the same exact-IANA policy as the visible clock.
///
/// Args:
///     zone_id: Persisted IANA zone id from the shared header clock.
///
/// Returns:
///     Display zone, or UTC when no valid saved selection exists.
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

/// Complete responsive presentation selected from one monitor width.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MonitorLayout {
    /// Whether controls remain on one horizontal line.
    inline_controls: bool,
    /// Whether the clock includes seconds.
    clock_seconds: bool,
    /// Whether automatic-refresh status includes its text label.
    status_label: bool,
    /// Whether trade counts remain visible.
    trades: bool,
    /// Whether the profit cell has room for its `total(last)` suffix.
    ///
    /// This is a question about SPACE only; whether the user wants the suffix at all is
    /// [`MonitorPrefs::last_trade`], and the cell needs both.
    last_trade: bool,
    /// Whether win rate remains visible.
    win_rate: bool,
    /// Whether average order remains visible.
    average_order: bool,
}

impl MonitorLayout {
    /// Select every responsive decision in deterministic priority order.
    ///
    /// Args:
    ///     width: Current logical window width.
    ///     ui_scale: Active MoonUI geometry scale applied to rendered controls and table cells.
    ///
    /// Returns:
    ///     Name and Profit are always present; Trades appears at the scaled 390-design-pixel
    ///     boundary. Controls stack and the clock drops seconds below scaled 460. The last-trade
    ///     suffix appears at scaled 500 — the first width where the wider profit column still
    ///     leaves the name its 128-unit minimum. Win rate appears at scaled 620, the status label
    ///     at scaled 700, and Average order at scaled 760.
    fn for_width(width: f32, ui_scale: f32) -> Self {
        Self {
            inline_controls: width >= STACKED_CONTROLS_WIDTH * ui_scale,
            clock_seconds: width >= STACKED_CONTROLS_WIDTH * ui_scale,
            status_label: width >= STATUS_LABEL_WIDTH * ui_scale,
            trades: width >= TRADES_WIDTH * ui_scale,
            last_trade: width >= LAST_TRADE_WIDTH * ui_scale,
            win_rate: width >= WIN_RATE_WIDTH * ui_scale,
            average_order: width >= AVERAGE_ORDER_WIDTH * ui_scale,
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
///     zone_changed: Whether the header clock selected a different IANA zone.
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
    /// When each core's latest arrival was observed, pruned to stamps still inside [`FLASH`].
    flash: HashMap<CoreId, Instant>,
    /// Whether the [`crate::pulse::PULSE_TICK`] repaint chain is already running.
    flash_timer_armed: bool,
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

        // Closing the monitor by hand is a decision the next launch has to know about; quitting is
        // not. `quitting` separates them, exactly as the detached-panel windows do — during
        // shutdown the layout has already been flushed, so a release-time write here would replace
        // "the monitor was open" with "the monitor was closed" on every ordinary exit.
        let window_id = window.window_handle().window_id();
        cx.on_release(move |this, app| {
            this.backend.update(app, |backend, _| {
                // Everything here is guarded by the window id. A view released AFTER its
                // replacement registered — close and reopen inside one effect flush — would
                // otherwise clear the flag while a live monitor is on screen, and the next launch
                // would not reopen it.
                if backend
                    .profit_monitor_window
                    .is_none_or(|handle| handle.window_id() != window_id)
                {
                    return;
                }
                backend.profit_monitor_window = None;
                if backend.quitting || !backend.layout.profit_monitor_open {
                    return;
                }
                backend.layout.profit_monitor_open = false;
                backend.layout_dirty = true;
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

        let report_revision = backend.read(cx).report_revision.clone();
        cx.observe(&report_revision, |this, _, cx| {
            this.observe_report_generation(cx);
        })
        .detach();
        let display_time_revision = backend.read(cx).display_time_revision.clone();
        cx.observe(&display_time_revision, |this, _, cx| this.sync_context(cx))
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
            prefs,
            settings_open: false,
            valuation,
            live,
            data: ProfitLoadState::default(),
            refresh_error: None,
            seen_trades: None,
            flash: HashMap::new(),
            flash_timer_armed: false,
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
        let (from, to) = self.period.range_at(now_utc(), self.zone);
        Query {
            time_zone: self.zone,
            previous_period_basis: PreviousPeriodBasis::Civil,
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
        let at = Instant::now();
        self.flash
            .extend(arrived.into_iter().map(|core| (core, at)));
        crate::pulse::arm_with(
            self,
            cx,
            |this| &mut this.flash_timer_armed,
            Self::flash_live,
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

    /// Return whether any recorded arrival is still inside its [`crate::pulse::FLASH`] window.
    ///
    /// Returns:
    ///     Whether a highlight still has something to draw.
    fn flash_live(&self) -> bool {
        self.flash.values().any(|at| at.elapsed() < FLASH)
    }

    /// Per-tick work of the shared pulse chain: drop finished stamps and dirty the cached table.
    ///
    /// The body is a cached SIBLING view. `cx.notify()` inside the pulse marks this view and its
    /// ancestors, which leaves that child clean and lets GPUI reuse the still-tinted subtree — so
    /// the tint has to be invalidated here or the fade never moves. Pruning first is deliberate:
    /// the tick that drops the last live stamp is the one that must erase the tint, and
    /// [`Self::flash_live`] then ends the chain on the following tick.
    ///
    /// Args:
    ///     cx: View context used to invalidate the cached body.
    fn on_flash_tick(&mut self, cx: &mut Context<Self>) {
        self.flash.retain(|_, at| at.elapsed() < FLASH);
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
                    self.prefs,
                    &self.flash,
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

/// Render the ⚙ button that opens the monitor's display settings.
///
/// The popover owns the click, so this carries no handler of its own: giving it one would fight
/// `MoonPopover`'s trigger for the same press and leave the popup toggling twice.
///
/// An icon rather than a "⚙" label, and lit while the popup is open, for the same reason as the
/// chart strip's gear: only a button with no label takes MoonUI's square path, and every other
/// popup trigger in the terminal shows whether its popup is up.
///
/// Args:
///     open: Whether the settings popup is currently showing.
///
/// Returns:
///     The settings popover's trigger.
fn settings_trigger(open: bool) -> impl IntoElement {
    MoonButton::new("profit-monitor-settings")
        .leading_icon(MoonButtonIconSlot::new("icons/settings.svg"))
        .tooltip(t!("profit_monitor.settings.title").to_string())
        .size(MoonButtonSize::Micro)
        .variant(if open {
            MoonButtonVariant::Blue
        } else {
            MoonButtonVariant::Ghost
        })
        .selected(open)
        .render()
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
///     show_trades: Whether the current width retains the aggregate trade count.
///     palette: Active MoonUI palette.
///     cx: Render context.
///
/// Returns:
///     Explanation and quote chips, with the aggregate trade count when space permits.
fn split_body(
    totals: &moon_core::db::QuoteBreakdown,
    show_trades: bool,
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
        .when(show_trades, |body| {
            body.child(
                div()
                    .text_color(moon(palette.text_soft))
                    .child(t!("profit_monitor.trades_total", n = totals.orders).to_string()),
            )
        })
        .into_any_element()
}

/// Render the responsive monitor table and exact total footer.
///
/// Args:
///     rows: Already grouped and sorted display rows.
///     unit: Comparable exact unit, or `None` for an empty result.
///     width: Current window width.
///     sort: Explicit user-selected ordering, if any.
///     prefs: Display preferences chosen in the ⚙ popup.
///     flash: Live arrival stamps keyed by the core that closed the trade.
///     scroll: Retained vertical-list position.
///     palette: Active MoonUI palette.
///     view: Owning monitor entity receiving sortable-header actions.
///     cx: Application context used for rendering.
///
/// Returns:
///     Fixed header/footer with a vertically scrolling row body.
#[allow(clippy::too_many_arguments)]
fn table(
    rows: Vec<MonitorRow>,
    unit: Option<ProfitUnit>,
    width: f32,
    sort: Option<MonitorSort>,
    prefs: MonitorPrefs,
    flash: &HashMap<CoreId, Instant>,
    scroll: &MoonVirtualListScrollHandle,
    palette: MoonPalette,
    view: Entity<ProfitMonitorView>,
    cx: &App,
) -> AnyElement {
    let layout = MonitorLayout::for_width(width, design::ui_value(cx, 1.0));
    let show_trades = layout.trades;
    let show_win = layout.win_rate;
    let show_average = layout.average_order;
    // Both halves have to agree: the preference asks for the suffix, the width decides whether the
    // column can hold it. Anything narrower would truncate a money value instead of dropping it.
    let show_last = prefs.last_trade && layout.last_trade;
    let profit_width = profit_column_width(show_last);
    let total = rows.iter().fold(MonitorRow::default(), |mut total, row| {
        total.profit += row.profit;
        total.trades += row.trades;
        total.wins += row.wins;
        total.positive_spent += row.positive_spent;
        total.positive_orders += row.positive_orders;
        // The footer's "last trade" is the newest one on screen, so Total answers the same question
        // its rows do rather than summing values from different instants. The core id breaks a tie:
        // folding in list order would otherwise let the footer's money change when the user clicks
        // a different sort column.
        if (row.last_close, row.last_core) > (total.last_close, total.last_core) {
            total.last_profit = row.last_profit;
            total.last_close = row.last_close;
            total.last_core = row.last_core;
        }
        total
    });
    // Resolved once per render rather than inside the row builder — that closure runs for every
    // visible row on every frame, and a lookup there would take the logo cache's global lock at
    // frame rate — and once per distinct EXCHANGE rather than once per row: two hundred cores on
    // one exchange are two hundred identical answers, and the resolver allocates a string for each.
    // One pass over the rows resolves both decorations. Doing either inside the virtual-list item
    // builder would repeat it for every visible row on every frame — and the highlight can drive
    // that at 10 Hz — while a merged Exchange row would pay one hash lookup per core it contains.
    let flashes: Vec<Option<Instant>> = rows
        .iter()
        .map(|row| {
            row.cores
                .iter()
                .filter_map(|core| flash.get(core).copied())
                .max()
        })
        .collect();
    let logos: Vec<Option<Arc<RenderImage>>> = if prefs.exchange_icons {
        let mut resolved: HashMap<&str, Option<Arc<RenderImage>>> = HashMap::new();
        rows.iter()
            .map(|row| {
                let exchange = row.exchange.as_deref()?;
                resolved
                    .entry(exchange)
                    .or_insert_with(|| exchange_logo(exchange))
                    .clone()
            })
            .collect()
    } else {
        Vec::new()
    };
    let header = table_header(
        layout,
        profit_width,
        prefs.exchange_icons,
        sort,
        view,
        palette,
        cx,
    );
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
            let (profit, profit_sign) =
                format_profit(row.profit, row.last_profit.filter(|_| show_last), unit);
            table_row(
                row.name.clone(),
                profit,
                profit_sign,
                format_trade_count(row.trades),
                Some(format_win_rate(row.win_rate())),
                Some(format_amount(row.average_order(), unit)),
                show_trades,
                show_win,
                show_average,
                RowChrome {
                    logo: logos.get(index).cloned().flatten(),
                    logo_gutter: prefs.exchange_icons,
                    flash: flashes.get(index).copied().flatten(),
                    profit_width,
                },
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
    let (total_profit, total_profit_sign) =
        format_profit(total.profit, total.last_profit.filter(|_| show_last), unit);
    let footer = table_row(
        t!("profit_monitor.total").to_string(),
        total_profit,
        total_profit_sign,
        format_trade_count(total.trades),
        Some(format_win_rate(total.win_rate())),
        Some(format_amount(total.average_order(), unit)),
        show_trades,
        show_win,
        show_average,
        RowChrome {
            // The total is not one exchange and never one core's arrival: no logo, no highlight —
            // but it keeps the gutter, or its label stops lining up with the names above it.
            logo: None,
            logo_gutter: prefs.exchange_icons,
            flash: None,
            profit_width,
        },
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
///     layout: Responsive presentation including visible-column selection.
///     profit_width: Profit-column width the data rows are using.
///     logo_gutter: Whether the data rows reserve room for an exchange logo.
///     sort: Current explicit ordering.
///     view: Monitor entity receiving heading clicks.
///     palette: Active MoonUI palette.
///     cx: Render context used for scaled geometry.
///
/// Returns:
///     Fixed-height sortable table header.
#[allow(clippy::too_many_arguments)]
fn table_header(
    layout: MonitorLayout,
    profit_width: f32,
    logo_gutter: bool,
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
            None => cell
                .min_w(design::ui_px(cx, MIN_NAME_COLUMN_WIDTH))
                .flex_1(),
        }
    };

    h_flex()
        .w_full()
        .h(design::fit_h_px(cx, 34.0, 13.0, 8.0))
        .px(design::ui_px(cx, TABLE_HORIZONTAL_PADDING))
        .gap(design::ui_px(cx, TABLE_COLUMN_GAP))
        .bg(moon(palette.table_head))
        .border_b(px(1.0))
        .border_color(moon_alpha(palette.border, 0.7))
        .child(
            sortable(
                "profit-monitor-heading-name",
                t!("profit_monitor.column.name").to_string(),
                MonitorSortColumn::Name,
                None,
                false,
            )
            // Padding rather than a spacer sibling: a sibling would also collect the row's own
            // column gap and overshoot the logo by exactly that much. Without it the heading sits
            // left of every value beneath it once icons are on.
            .when(logo_gutter, |heading| {
                heading.pl(design::ui_px(cx, EXCHANGE_LOGO_SIZE + NAME_LOGO_GAP))
            }),
        )
        .child(sortable(
            "profit-monitor-heading-profit",
            t!("profit_monitor.column.profit").to_string(),
            MonitorSortColumn::Profit,
            Some(profit_width),
            true,
        ))
        .when(layout.trades, |header| {
            header.child(sortable(
                "profit-monitor-heading-trades",
                t!("profit_monitor.column.trades").to_string(),
                MonitorSortColumn::Trades,
                Some(TRADES_COLUMN_WIDTH),
                true,
            ))
        })
        .when(layout.win_rate, |header| {
            header.child(sortable(
                "profit-monitor-heading-win-rate",
                t!("profit_monitor.column.win_rate").to_string(),
                MonitorSortColumn::WinRate,
                Some(WIN_RATE_COLUMN_WIDTH),
                true,
            ))
        })
        .when(layout.average_order, |header| {
            header.child(sortable(
                "profit-monitor-heading-average-order",
                t!("profit_monitor.column.average_order").to_string(),
                MonitorSortColumn::AverageOrder,
                Some(AVERAGE_ORDER_COLUMN_WIDTH),
                true,
            ))
        })
}

/// Per-row decoration that is not one of the row's own numbers.
///
/// Bundled rather than passed as three more positional flags: every one of them is optional and
/// two of them are only ever set for data rows, so a struct is what keeps the Total call honest.
struct RowChrome {
    /// Exchange logo drawn before the name, when enabled and the brand is known.
    logo: Option<Arc<RenderImage>>,
    /// Whether to reserve the logo's width even without one.
    ///
    /// With icons on, a row whose brand is unknown, the Total footer and the sortable heading all
    /// have to start where the logo-bearing rows' text starts — otherwise the Name column no longer
    /// lines up with its own header.
    logo_gutter: bool,
    /// Instant this row's core closed its newest trade, while the highlight is still live.
    flash: Option<Instant>,
    /// Profit-column width selected by the current last-trade decision.
    profit_width: f32,
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
///     show_trades: Whether the current width retains trade count.
///     show_win: Whether the current width retains win rate.
///     show_average: Whether the current width retains average order.
///     chrome: Logo, arrival highlight, and profit-column width.
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
    show_trades: bool,
    show_win: bool,
    show_average: bool,
    chrome: RowChrome,
    palette: MoonPalette,
    cx: &App,
) -> Div {
    let name_tooltip = name.clone();
    let profit_color = profit_sign.pick(
        design::positive_color(palette),
        design::danger_color(palette),
        palette.text,
    );
    let logo_size = design::ui_px(cx, EXCHANGE_LOGO_SIZE);
    // The arrival tint is the News feed's, from the one shared definition: a full-bleed layer
    // declared BEFORE the cells so it sits under the text, and driven by the owner's own stamp
    // rather than by a GPUI animation, which would repaint the whole window at vblank for its
    // entire duration.
    let row = crate::pulse::with_arrival_tint(
        h_flex()
            .w_full()
            .relative()
            .h(design::fit_h_px(cx, 34.0, 13.0, 8.0))
            .px(design::ui_px(cx, TABLE_HORIZONTAL_PADDING))
            .gap(design::ui_px(cx, TABLE_COLUMN_GAP))
            .border_b(px(1.0))
            .border_color(moon_alpha(palette.border, 0.7)),
        palette.table_selected,
        chrome.flash,
    );
    row.child(
        h_flex()
            .id(SharedString::from(format!(
                "profit-monitor-name:{name_tooltip}"
            )))
            .flex_1()
            .min_w(design::ui_px(cx, MIN_NAME_COLUMN_WIDTH))
            .gap(design::ui_px(cx, NAME_LOGO_GAP))
            .overflow_hidden()
            .text_ellipsis()
            .whitespace_nowrap()
            .tooltip(crate::panels::common::text_tooltip(name_tooltip))
            .when_some(chrome.logo.clone(), |element, logo| {
                element.child(
                    img(logo)
                        .flex_none()
                        .w(logo_size)
                        .h(logo_size)
                        .rounded(design::ui_px(cx, 2.0)),
                )
            })
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    // No logo but the gutter is on: pad instead of adding an empty box, so a row
                    // whose brand is unknown still starts its name where its neighbours do without
                    // a second element in the layout tree. Same form the heading uses.
                    .when(chrome.logo.is_none() && chrome.logo_gutter, |text| {
                        text.pl(design::ui_px(cx, EXCHANGE_LOGO_SIZE + NAME_LOGO_GAP))
                    })
                    .child(name),
            ),
    )
    .child(numeric_cell(profit, chrome.profit_width, cx).text_color(moon(profit_color)))
    .when(show_trades, |element| {
        element.child(numeric_cell(trades, TRADES_COLUMN_WIDTH, cx))
    })
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

/// Return the profit column's design-reference width.
///
/// Args:
///     show_last: Whether the cell carries its `total(last)` suffix.
///
/// Returns:
///     Base width, plus the suffix allowance when the suffix is drawn.
fn profit_column_width(show_last: bool) -> f32 {
    if show_last {
        PROFIT_COLUMN_WIDTH + PROFIT_LAST_TRADE_EXTRA
    } else {
        PROFIT_COLUMN_WIDTH
    }
}

/// Format profit with its exact comparable unit, optionally carrying the newest closed trade.
///
/// The suffix goes INSIDE the unit — `-57.11(-0.60) USDT`, not `-57.11 USDT (-0.60)` — so the two
/// amounts read as one measurement in one currency, which is what they are. Both are rounded to
/// the same unit decimals, so the bracket can never claim precision the total does not have.
///
/// The returned sign describes the TOTAL. The suffix is a different trade and may disagree; the
/// cell is coloured by the number it is about.
///
/// Args:
///     value: Projected profit.
///     last: Profit of the newest closed trade, when the suffix is enabled and one exists.
///     unit: Exact quote or percent unit.
///
/// Returns:
///     Signed compact text carrying its unit and the sign represented after display rounding.
fn format_profit(value: f64, last: Option<f64>, unit: Option<ProfitUnit>) -> (String, DeltaSign) {
    let decimals = match unit {
        Some(ProfitUnit::Quote(currency)) => currency.display_decimals(),
        Some(ProfitUnit::Percent) | None => 2,
    };
    let (amount, sign) = moon_core::util::fmt::signed_amount(value, decimals);
    let amount = match last {
        Some(last) => {
            let (last, _) = moon_core::util::fmt::signed_amount(last, decimals);
            format!("{amount}({last})")
        }
        None => amount,
    };
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
///
/// Returns:
///     Nothing; the singleton window is focused or created as a side effect.
pub(crate) fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    open_window(backend, owner, owner_display, true, cx);
}

/// Reopen the monitor at launch because the previous session left it open.
///
/// Separate from [`open`] for one reason: it must NOT activate. `activate_new_window` exists for an
/// explicit user action and its own documentation forbids bulk startup restoration, where each
/// restored window steals the foreground from the one before it — here, from Main.
///
/// Args:
///     backend: Shared terminal state retaining the singleton handle.
///     owner: A window already on screen, used only to choose a display.
///     cx: Application context used to create the window.
pub(crate) fn restore(backend: Entity<Backend>, owner: Option<AnyWindowHandle>, cx: &mut App) {
    open_window(backend, owner, None, false, cx);
}

/// Open or focus the monitor, activating it only for an explicit user action.
///
/// Args:
///     backend: Shared terminal state retaining the singleton handle.
///     owner: Launching window, used only to choose the initial display.
///     owner_display: Display captured by the caller.
///     activate: Whether a newly created window should take the foreground.
///     cx: Application context used to create or activate the window.
fn open_window(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    activate: bool,
    cx: &mut App,
) {
    if let Some(handle) = backend.read(cx).profit_monitor_window {
        // Liveness is probed with an EMPTY update, and the window is raised only for a deliberate
        // action. Activating here regardless would put the restored monitor in front of Main on
        // every launch — the very thing `restore` exists to avoid.
        let alive = if activate {
            handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        } else {
            handle.update(cx, |_, _, _| ()).is_ok()
        };
        if alive {
            mark_open(&backend, cx);
            return;
        }
    }
    // Saved geometry keeps its SIZE unconditionally; only the origin is questioned. A window
    // dragged past the left screen edge has a legal negative x that no display contains, and
    // discarding its size over that would silently resize the monitor on the next launch.
    let saved = backend.read(cx).layout.profit_monitor_window;
    // An origin on an unplugged display is the one that must not survive: it would place a window
    // with no taskbar button off-screen, and because the open flag survives too, every following
    // launch would do it again.
    let origin = saved.filter(|geometry| origin_is_on_a_display(*geometry, cx));
    let bounds = Bounds {
        origin: origin.map_or(point(px(160.0), px(120.0)), |geometry| {
            point(px(geometry.x as f32), px(geometry.y as f32))
        }),
        size: saved.map_or(size(px(720.0), px(520.0)), |geometry| {
            size(px(geometry.w as f32), px(geometry.h as f32))
        }),
    };
    let display_id = crate::window::windowing::saved_or_owner_display_id(
        origin.map(|geometry| point(px(geometry.x as f32), px(geometry.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let options = crate::window::windowing::profit_monitor_window_options(
        t!("profit_monitor.window_title").to_string(),
        WindowBounds::Windowed(bounds),
        display_id,
        Some(size(design::ui_px(cx, MIN_WINDOW_WIDTH), px(320.0))),
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
        mark_open(&backend, cx);
        if activate {
            crate::window::windowing::activate_new_window(handle.into(), cx);
        }
    }
}

/// Return whether a saved window origin still lands on a connected display.
///
/// Args:
///     geometry: Saved window rectangle.
///     cx: Application context used to enumerate displays.
///
/// Returns:
///     Whether some display contains the saved origin. Always `true` on macOS, whose saved global
///     coordinates are not comparable this way — the same exemption `saved_or_owner_display_id`
///     makes for its own containment test.
fn origin_is_on_a_display(geometry: moon_core::config::layout::GeomRect, cx: &mut App) -> bool {
    if cfg!(target_os = "macos") {
        return true;
    }
    let origin = point(px(geometry.x as f32), px(geometry.y as f32));
    cx.displays()
        .into_iter()
        .any(|display| display.bounds().contains(&origin))
}

/// Record that the monitor is open so the next launch reopens it.
///
/// Written only after a window actually exists: a failed `open_window` that still set the flag
/// would make every subsequent startup retry the same failure and log the same error.
///
/// Args:
///     backend: Shared terminal state holding the layout.
///     cx: Application context used to persist.
fn mark_open(backend: &Entity<Backend>, cx: &mut App) {
    backend.update(cx, |backend, _| {
        if backend.layout.profit_monitor_open {
            return;
        }
        backend.layout.profit_monitor_open = true;
        backend.layout_dirty = true;
    });
}
