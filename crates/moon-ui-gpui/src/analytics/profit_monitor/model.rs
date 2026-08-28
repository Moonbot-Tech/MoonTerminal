//! Render-free Profit Monitor model and refresh policy.

use std::cmp::Ordering as SortOrdering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Utc};
use chrono_tz::Tz;
use rust_i18n::t;

use super::rows::{LiveContext, MonitorRow};

const MINUTE_MS: u128 = 60_000;
const TRADES_WIDTH: f32 = 390.0;
const STACKED_CONTROLS_WIDTH: f32 = 460.0;
const WIN_RATE_WIDTH: f32 = 620.0;
const STATUS_LABEL_WIDTH: f32 = 700.0;
const AVERAGE_ORDER_WIDTH: f32 = 760.0;

/// Compact monitor-specific period choices.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum MonitorPeriod {
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
    pub(super) const ALL: [Self; 8] = [
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
    pub(super) fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|period| period.id() == id)
    }

    /// Return the stable layout id.
    ///
    /// Returns:
    ///     A short period key.
    pub(super) fn id(self) -> &'static str {
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
    pub(super) fn title(self) -> String {
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
    pub(super) fn range_at(self, now: DateTime<Utc>, zone: Tz) -> (i64, i64) {
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

/// Resolve a saved header-clock zone through the same exact-IANA policy as the visible clock.
///
/// Args:
///     zone_id: Persisted IANA zone id from the shared header clock.
///
/// Returns:
///     Display zone, or UTC when no valid saved selection exists.
pub(super) fn monitor_zone(zone_id: Option<&str>) -> Tz {
    crate::chrome::clock::resolved_header_clock_zone(zone_id)
}

/// Effect of a non-report Backend update on the open monitor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ContextChange {
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
pub(super) struct MonitorLayout {
    /// Whether controls remain on one horizontal line.
    pub(super) inline_controls: bool,
    /// Whether the clock includes seconds.
    pub(super) clock_seconds: bool,
    /// Whether automatic-refresh status includes its text label.
    pub(super) status_label: bool,
    /// Whether trade counts remain visible.
    pub(super) trades: bool,
    /// Whether win rate remains visible.
    pub(super) win_rate: bool,
    /// Whether average order remains visible.
    pub(super) average_order: bool,
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
    ///     boundary. Controls stack and the clock drops seconds below scaled 460. Win rate appears
    ///     at scaled 620, the status label at scaled 700, and Average order at scaled 760. The
    ///     `total(last)` suffix has no tier of its own: `table::profit_column` measures whether it
    ///     fits the room this selection leaves, which is the only question a tier approximated.
    pub(super) fn for_width(width: f32, ui_scale: f32) -> Self {
        Self {
            inline_controls: width >= STACKED_CONTROLS_WIDTH * ui_scale,
            clock_seconds: width >= STACKED_CONTROLS_WIDTH * ui_scale,
            status_label: width >= STATUS_LABEL_WIDTH * ui_scale,
            trades: width >= TRADES_WIDTH * ui_scale,
            win_rate: width >= WIN_RATE_WIDTH * ui_scale,
            average_order: width >= AVERAGE_ORDER_WIDTH * ui_scale,
        }
    }
}

/// Sortable columns exposed by the Profit Monitor header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MonitorSortColumn {
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
    pub(super) fn id(self) -> &'static str {
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
            Self::Name => left.name.to_lowercase().cmp(&right.name.to_lowercase()),
            Self::Profit => left.profit.total_cmp(&right.profit),
            Self::Trades => left.trades.cmp(&right.trades),
            // A row with no denominator sorts as zero rather than dropping out of the ordering:
            // the column has to place every visible row somewhere, and the cell it draws is empty.
            Self::WinRate => left
                .win_rate()
                .unwrap_or(0.0)
                .total_cmp(&right.win_rate().unwrap_or(0.0)),
            Self::AverageOrder => left
                .average_order()
                .unwrap_or(0.0)
                .total_cmp(&right.average_order().unwrap_or(0.0)),
        }
    }
}

/// One explicit user-selected table ordering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MonitorSort {
    /// Primary column.
    pub(super) column: MonitorSortColumn,
    /// Whether the primary comparison is reversed.
    pub(super) descending: bool,
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
    pub(super) fn from_layout(id: &str, descending: bool) -> Option<Self> {
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
pub(super) fn next_sort(current: Option<MonitorSort>, column: MonitorSortColumn) -> MonitorSort {
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
pub(super) fn sort_rows(rows: &mut [MonitorRow], sort: Option<MonitorSort>) {
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
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.primary_core.cmp(&right.primary_core))
    });
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
pub(super) fn context_change(
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

/// Preserve the last known venue while a core is temporarily disconnected.
///
/// A core drops out of the session's venue map the moment its feed goes down, and without this
/// carry-forward a reconnecting core would leave its exchange row for the unidentified one and come
/// back a few seconds later — a visible flicker on a row the user is reading.
///
/// Args:
///     previous: Venue context already represented by the open monitor.
///     sampled: Newly sampled live and configured context.
///
/// Returns:
///     New context with missing venues filled from their last observed values.
pub(super) fn retain_last_known_venues(
    previous: &LiveContext,
    mut sampled: LiveContext,
) -> LiveContext {
    for (core, venue) in &previous.venues {
        sampled.venues.entry(*core).or_insert_with(|| venue.clone());
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
pub(super) fn duration_until_wall_clock_boundary(now: SystemTime, interval_ms: u128) -> Duration {
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
pub(super) fn duration_until_period_refresh(
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
