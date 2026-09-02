//! Render-free Profit Monitor model and refresh policy.

use std::cmp::Ordering as SortOrdering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Utc};
use chrono_tz::Tz;
use moon_core::session::CoreId;
use rust_i18n::t;

use super::rows::{LiveContext, MonitorRow};

const TRADES_WIDTH: f32 = 390.0;
const STACKED_CONTROLS_WIDTH: f32 = 460.0;
const WIN_RATE_WIDTH: f32 = 620.0;
const STATUS_LABEL_WIDTH: f32 = 700.0;
const AVERAGE_ORDER_WIDTH: f32 = 760.0;

/// Compact monitor-specific period choices.
///
/// Two families, deliberately named apart because a preset that only says "week" or "year"
/// cannot be read: a CALENDAR preset starts at the period's own boundary (Monday, the 1st,
/// January 1st), while a ROLLING one counts a fixed number of days back from today inclusive.
/// The day-only pair and the unbounded `All` round out the set. The menu keeps the families in
/// separate groups for the same reason (see [`Self::GROUPS`]).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum MonitorPeriod {
    /// Current calendar day in the application-wide selected zone.
    #[default]
    Today,
    /// Previous calendar day in the application-wide selected zone.
    Yesterday,
    /// The calendar week from Monday to today.
    CurWeek,
    /// Current calendar month in the application-wide selected zone.
    CurMonth,
    /// The previous selected-zone calendar month as `[previous-month start, current-month start)`.
    LastMonth,
    /// The calendar year from January 1st to today.
    CurYear,
    /// Rolling seven days ending today.
    Days7,
    /// Rolling thirty days ending today.
    Days30,
    /// Rolling three hundred and sixty-five days ending today.
    Days365,
    /// Complete retained report history.
    All,
}

impl MonitorPeriod {
    /// Presets in their stable restore and completeness order.
    pub(super) const ALL: [Self; 10] = [
        Self::Today,
        Self::Yesterday,
        Self::CurWeek,
        Self::CurMonth,
        Self::LastMonth,
        Self::CurYear,
        Self::Days7,
        Self::Days30,
        Self::Days365,
        Self::All,
    ];

    /// The menu's four groups in display order, rendered with a separator between them.
    ///
    /// This is the single declaration of menu membership and grouping, so a variant added without
    /// a home here remains unreachable from the menu rather than being silently appended. [`Self::ALL`]
    /// above stays the restore/completeness authority [`Self::from_id`] iterates; this const
    /// governs menu layout only.
    pub(super) const GROUPS: [&'static [Self]; 4] = [
        &[Self::Today, Self::Yesterday],
        &[
            Self::CurWeek,
            Self::CurMonth,
            Self::LastMonth,
            Self::CurYear,
        ],
        &[Self::Days7, Self::Days30, Self::Days365],
        &[Self::All],
    ];

    /// Restore a persisted monitor period.
    ///
    /// A saved `"m-hour"` (the retired rolling-hour preset) matches nothing in [`Self::ALL`] and
    /// returns `None` here; callers fall back to the default `Today`, the same documented
    /// contract as `analytics/period.rs`'s own unknown-id fallback.
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
    /// The three renamed rolling variants (`Days7`/`Days30`/`Days365`, formerly `Week`/`Month`/
    /// `Year`) keep their old ids: the range semantics are unchanged, so resetting a user's saved
    /// choice over a label rename alone would be pure churn. `CurWeek` and `CurYear` are new
    /// variants and get new ids; `m-hour` is retired with no successor.
    ///
    /// Returns:
    ///     A short period key.
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Today => "m-today",
            Self::Yesterday => "m-yesterday",
            Self::CurWeek => "m-cur-week",
            Self::CurMonth => "m-current-month",
            Self::LastMonth => "m-last-month",
            Self::CurYear => "m-cur-year",
            Self::Days7 => "m-week",
            Self::Days30 => "m-month",
            Self::Days365 => "m-year",
            Self::All => "m-all",
        }
    }

    /// Return the localized button title.
    ///
    /// `CurWeek` borrows `report.period.cur_week` rather than a `profit_monitor`-local key: the
    /// Report panel's calendar-week preset already carries the exact label this needs, and
    /// cross-namespace `t!("report.*")` borrowing from `analytics/*` is already precedented
    /// elsewhere (`analytics/toolbar.rs`, `analytics/tuner/list/mod.rs`).
    ///
    /// Returns:
    ///     User-facing period label.
    pub(super) fn title(self) -> String {
        match self {
            Self::Today => t!("analytics.period.today"),
            Self::Yesterday => t!("analytics.period.yesterday"),
            Self::CurWeek => t!("report.period.cur_week"),
            Self::CurMonth => t!("analytics.period.cur_month"),
            Self::LastMonth => t!("analytics.period.last_month"),
            Self::CurYear => t!("analytics.period.cur_year"),
            Self::Days7 => t!("analytics.period.week"),
            Self::Days30 => t!("analytics.period.month"),
            Self::Days365 => t!("analytics.period.year"),
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
            Self::CurWeek => {
                let week_start = shift(-i64::from(today.weekday().num_days_from_monday()));
                (start(week_start), tomorrow)
            }
            Self::CurMonth => {
                let month_start = today.with_day(1).unwrap_or(today);
                (start(month_start), tomorrow)
            }
            Self::LastMonth => {
                let (prev_month_start_date, cur_month_start_date) =
                    moon_core::util::display_time::prev_and_cur_month_start(today);
                // The upper bound is the current month's own start (exclusive), so a shorter or
                // longer previous month (28/29/30/31 days) needs no explicit length check.
                (start(prev_month_start_date), start(cur_month_start_date))
            }
            Self::CurYear => {
                let year_start = today
                    .with_month(1)
                    .and_then(|date| date.with_day(1))
                    .unwrap_or(today);
                (start(year_start), tomorrow)
            }
            Self::Days7 => (shifted(-6), tomorrow),
            Self::Days30 => (shifted(-29), tomorrow),
            Self::Days365 => (shifted(-364), tomorrow),
            Self::All => (-1, tomorrow),
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
    } else if query_scope_changed(before, after) {
        // A preset flip (or a membership save landing) changes the SQL the scoped query sends —
        // see `scoped_query_core_ids`. A plain `Regroup` over rows read under the OLD filter would
        // keep showing money for cores the new filter no longer includes, so this outranks the
        // generic equality check below and forces a real re-read instead.
        ContextChange::Reload {
            restart_clock: false,
        }
    } else if before != after {
        ContextChange::Regroup
    } else {
        ContextChange::None
    }
}

/// Resolve the exact core-id filter the scoped Profit Monitor query should send.
///
/// Args:
///     live: Current live context — `core_order` is the active preset's membership-filtered
///         display set, `configured_core_ids` is every core `config.servers` names before that
///         filter ran.
///     previously_seen_cores: Every core the last successfully applied `ProfitMonitorSummary` named,
///         carried forward the same way [`retain_last_known_venues`] carries a venue: this window
///         has no cheap way to name every core WITH data short of `db::distinct_cores`'s full-table
///         walk (its own doc says it "walks every core because it must NAME them all"), so it reuses
///         what the last successful query already learned instead. Only entries ABSENT from
///         `live.configured_core_ids` contribute anything — a hidden-but-still-configured core is
///         already covered by `live.core_order` excluding it on purpose.
///
/// Returns:
///     `Vec::new()` when the active preset (if any) hides no configured core — unfiltered, exactly
///     as an absent scope has always meant. Otherwise `core_order` plus every data-only core named
///     in `previously_seen_cores`, through [`crate::workspace::query_core_ids`] so an inclusion list
///     that resolves empty becomes the no-match sentinel rather than "unfiltered".
///
///     **KNOWN LIMITATION, stated exactly rather than optimistically.** The carry-forward can only
///     learn a data-only core from a read that RETURNED it, and a scoped read cannot discover one
///     that is not already in its carry-forward list. So the list fills only once an UNFILTERED
///     read has happened during this view's lifetime — which
///     means a monitor window OPENED while the preset already hides something stays blind to a
///     data-only core until hiding is switched off at least once. `ProfitMonitorView` is
///     constructed fresh on every window open (`window.rs`), so closing and reopening under a
///     hiding preset re-enters that state. Closing this properly needs an independent universe on
///     construction — a `db::distinct_cores`-shaped read, which is what
///     `analytics::toolbar::analytics_core_filter_ids` has and this window does not — and that is
///     deliberately NOT done here: it is one full-table walk added to a window-open path, weighed
///     against a narrow gap, and the honest comment is worth more than a hurried read.
///
///     **And "an unfiltered read" means one that returns a COMPARABLE result.** `moon-core`
///     answers a successful but mixed-currency read with `ProfitScope::Split`, which discards the
///     per-core list before the summary is built, so `ProfitLoadState::data` is `None` there too
///     and nothing seeds. On a fleet whose quotes are not comparable, switching hiding off does
///     NOT recover the core — the gap is permanent for that fleet, not merely until the next
///     toggle. Stated exactly because two earlier drafts of this comment each claimed a recovery
///     that does not happen.
///
///     **That recovery is only real because the caller keeps the list OUTSIDE the load state.**
///     `analytics::toolbar::analytics_core_filter_ids` can document a one-cycle gap because its
///     universe comes from `db::distinct_cores`, independent of any view state. This one does not:
///     if it were read back from `ProfitLoadState`, every scope-narrowing reload would clear it
///     first, the narrowed result would become the next read's universe, and the core would be
///     gone permanently rather than for a cycle. `ProfitMonitorView::seen_data_cores` exists for
///     exactly that reason and accumulates rather than replaces.
pub(super) fn scoped_query_core_ids(
    live: &LiveContext,
    previously_seen_cores: &[CoreId],
) -> Vec<CoreId> {
    if !live.scope_marker().hides_anything() {
        return Vec::new();
    }
    let mut ids = live.core_order.clone();
    ids.extend(
        previously_seen_cores
            .iter()
            .copied()
            .filter(|core| !live.configured_core_ids.contains(core)),
    );
    crate::workspace::query_core_ids(ids, true)
}

/// Whether two live contexts would resolve [`scoped_query_core_ids`] to a different core set.
///
/// Independent of `previously_seen_cores`, which [`scoped_query_core_ids`] also takes: that
/// contribution is driven by the last DATABASE read, not by anything a context sample observes, so
/// it cannot itself flip between two samples and is deliberately left out of this comparison.
///
/// Args:
///     before: Context represented by the currently visible table.
///     after: Newly sampled context.
///
/// Returns:
///     `true` when the scoped query's inclusion list would differ: hiding started or stopped, or
///     the set of cores hidden changed while hiding stayed in effect.
fn query_scope_changed(before: &LiveContext, after: &LiveContext) -> bool {
    let before_hides = before.scope_marker().hides_anything();
    let after_hides = after.scope_marker().hides_anything();
    if before_hides != after_hides {
        return true;
    }
    // `configured_core_ids` belongs here beside `core_order` because BOTH feed the inclusion list:
    // the visible cores come from one, and the carry-forward is whatever the previous read named
    // MINUS the other. A core that was data-only and becomes configured-but-hidden changes nothing
    // about `core_order` — it is hidden, so it never enters it — while silently dropping out of the
    // carry-forward, so the query narrows with no sign here and the table would keep that core's
    // money on screen over rows that no longer include it.
    before_hides
        && (before.core_order != after.core_order
            || before.configured_core_ids != after.configured_core_ids)
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
///     Next local midnight for every bounded preset, or `None` for `All`.
pub(super) fn duration_until_period_refresh(
    period: MonitorPeriod,
    zone: Tz,
    now: SystemTime,
) -> Option<Duration> {
    match period {
        MonitorPeriod::All => None,
        MonitorPeriod::Today
        | MonitorPeriod::Yesterday
        | MonitorPeriod::CurWeek
        | MonitorPeriod::CurMonth
        | MonitorPeriod::LastMonth
        | MonitorPeriod::CurYear
        | MonitorPeriod::Days7
        | MonitorPeriod::Days30
        | MonitorPeriod::Days365 => {
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
