//! Period and tabs of the "Analytics" window: period presets (`Period`) with their ranges and
//! persist keys, the list of tabs (`Tab`) and the date helpers shared by the pages.

use chrono::Datelike as _;
use chrono_tz::Tz;
use moon_core::db::analytics::{ANALYTICS_HORIZON_SECS, ANALYTICS_MAX_SPAN_SECS};
use rust_i18n::t;

use crate::controls::date_range;

/// Window tabs. The placeholders (Coins/Leverage) will come back as the stages are implemented.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tab {
    Summary,
    Strategies,
    /// Profit calendar (Day / Month / Year).
    Calendar,
}

impl Tab {
    /// Tab order: Summary → Strategy tuning → Calendar (last, behind a rule).
    ///
    /// The first two answer "how did it go" through the window's own filters and period bar; the
    /// Calendar answers "when", browsing time by its own navigation and hiding that period bar
    /// entirely. `tabs_bar` draws the boundary between the two kinds, so this order is what the
    /// rule divides — reordering here moves the rule with it.
    pub(super) const ALL: [Tab; 3] = [Tab::Summary, Tab::Strategies, Tab::Calendar];
    pub(super) fn id(self) -> &'static str {
        match self {
            Tab::Summary => "an-summary",
            Tab::Strategies => "an-strategies",
            Tab::Calendar => "an-calendar",
        }
    }
    pub(super) fn title(self) -> String {
        match self {
            Tab::Summary => t!("analytics.tab.summary"),
            Tab::Strategies => t!("analytics.tab.strategies"),
            Tab::Calendar => t!("analytics.tab.calendar"),
        }
        .to_string()
    }
}

/// Period presets resolved as civil dates in the selected header-clock zone.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Period {
    Today,
    Yesterday,
    Week,
    /// The current selected-zone calendar month from its first civil day.
    CurMonth,
    /// The current selected-zone calendar year from January 1 of that civil year.
    CurYear,
    /// A rolling 30 days.
    Month,
    /// A rolling 365 days.
    Year,
    All,
    /// An arbitrary range from the "from"/"to" fields: `[from, to)` in UTC unix seconds;
    /// from = -1 — "from" is unset (the whole history up to "to").
    ///
    /// Both bounds carry a clock time, not only a day: the fields are `MoonDateTimePicker`s and
    /// resolve to whole minutes, so `to` is the exclusive end of the minute the user picked.
    Custom(i64, i64),
}

impl Period {
    pub(super) const ALL: [Period; 8] = [
        Period::Today,
        Period::Yesterday,
        Period::Week,
        Period::CurMonth,
        Period::Month,
        Period::CurYear,
        Period::Year,
        Period::All,
    ];
    /// Restore a period from its persisted selection id.
    ///
    /// Args:
    ///     id: Persisted preset id or custom-period encoding.
    ///
    /// Returns:
    ///     Matching period, or `None` for an unknown or invalid persisted value.
    pub(super) fn from_id(id: &str) -> Option<Period> {
        let now = moon_core::util::now_unix_ms_i64() / 1000;
        Self::from_id_at(id, now)
    }

    /// [`from_id`](Self::from_id) at a pinned instant — the same pinned-instant shape
    /// [`range`](Self::range)/[`range_at`](Self::range_at) already use, so tests can validate
    /// deterministically.
    ///
    /// A persisted custom range is REJECTED, not clamped, when it cannot have come from the
    /// picker. There is no epoch floor on `f` — no closed trade predates it, but flooring a
    /// PICKER value the way moon-core floors a data-derived one would turn a genuinely old `f`
    /// into a silently different range instead of a named error, and the core's own
    /// `clamp_period` is the boundary that actually protects the read. `f == -1` (the documented
    /// "from unset" sentinel) is always accepted and skips the span check below, since it defers
    /// the real lower bound to whatever the core resolves. Otherwise: `t` must exceed `f.max(0)`,
    /// sit below `ANALYTICS_HORIZON_SECS`, and the span `t - f` must not exceed
    /// `ANALYTICS_MAX_SPAN_SECS` — a picker can never produce a range that wide, so refusing one
    /// loses no genuine user intent. `to` stays deliberately unbounded against `now` — the picker
    /// lets a user choose a future end date, and that already persists and works today (SQL
    /// simply finds no future rows), so bounding it here would be a regression rather than a
    /// hardening. `now` is therefore unused inside this rule; it stays a parameter only to keep
    /// this function's shape symmetric with `range`/`range_at`. `None` here falls through to the
    /// same `unwrap_or(Period::CurMonth)` sites an unknown id already does, with no new fallback
    /// of its own.
    ///
    /// That fallback is SILENT by design, and deliberately not a `PeriodOutOfRange` alert. A
    /// rejected value here is a discarded PREFERENCE, not a failed read: the period bar renders
    /// `active_period()`, so the window shows "current month" and reads the current month — the
    /// label and the data never disagree, and nothing is presented under a range the user did not
    /// choose. It is the same contract `dock_persist::DOCK_VERSION` already applies to an
    /// incompatible saved layout. Surfacing an error instead would make a corrupt file open a
    /// window that is useless until the user edits the very period the file broke.
    pub(super) fn from_id_at(id: &str, _now: i64) -> Option<Period> {
        if let Some(rest) = id.strip_prefix("p-custom:") {
            let (f, t) = rest.split_once(':')?;
            let f: i64 = f.parse().ok()?;
            let t: i64 = t.parse().ok()?;
            let not_inverted = t > f.max(0);
            let under_horizon = t < ANALYTICS_HORIZON_SECS;
            let span_ok = f == -1 || t.saturating_sub(f) <= ANALYTICS_MAX_SPAN_SECS;
            return (not_inverted && under_horizon && span_ok).then_some(Period::Custom(f, t));
        }
        Period::ALL.into_iter().find(|p| p.id() == id)
    }
    pub(super) fn id(self) -> &'static str {
        match self {
            Period::Today => "p-today",
            Period::Yesterday => "p-yesterday",
            Period::Week => "p-week",
            Period::CurMonth => "p-cur-month",
            Period::CurYear => "p-cur-year",
            Period::Month => "p-month",
            Period::Year => "p-year",
            Period::All => "p-all",
            Period::Custom(..) => "p-custom",
        }
    }
    /// The string persisted in layout: for Custom the bounds are encoded into the id.
    pub(super) fn persist_id(self) -> String {
        match self {
            Period::Custom(f, t) => format!("p-custom:{f}:{t}"),
            p => p.id().to_string(),
        }
    }
    /// Format the localized period label in the selected display zone.
    ///
    /// Args:
    ///     zone: Selected IANA display zone used by custom bound labels.
    ///
    /// Returns:
    ///     Localized preset name or formatted custom range.
    pub(super) fn title(self, zone: Tz) -> String {
        match self {
            Period::Today => t!("analytics.period.today"),
            Period::Yesterday => t!("analytics.period.yesterday"),
            Period::Week => t!("analytics.period.week"),
            Period::CurMonth => t!("analytics.period.cur_month"),
            Period::CurYear => t!("analytics.period.cur_year"),
            Period::Month => t!("analytics.period.month"),
            Period::Year => t!("analytics.period.year"),
            Period::All => t!("analytics.period.all"),
            Period::Custom(f, t) => {
                let a = if f < 0 {
                    "—".to_string()
                } else {
                    fmt_minute(f, zone)
                };
                // `t` is exclusive, so the label names the last minute INSIDE the range.
                return format!(
                    "{a} – {}",
                    fmt_minute((t - date_range::MINUTE).max(f.max(0)), zone)
                );
            }
        }
        .to_string()
    }
    /// Resolve this period to absolute `[from, to)` bounds using civil calendar dates.
    ///
    /// Args:
    ///     zone: Selected IANA display zone.
    ///
    /// Returns:
    ///     UTC Unix-second bounds; `from = -1` means the whole history.
    pub(super) fn range(self, zone: Tz) -> (i64, i64) {
        let now = moon_core::util::now_unix_ms_i64() / 1000;
        self.range_at(now, zone)
    }

    /// Resolve this period at a pinned instant for deterministic civil-boundary handling.
    ///
    /// Args:
    ///     now: Current UTC Unix timestamp in seconds.
    ///     zone: Selected IANA display zone.
    ///
    /// Returns:
    ///     UTC Unix-second bounds; `from = -1` means the whole history.
    fn range_at(self, now: i64, zone: Tz) -> (i64, i64) {
        let today = moon_core::util::display_time::date(now, zone).unwrap_or_default();
        let day0 = moon_core::util::display_time::day_start(today, zone).unwrap_or(now);
        let tomorrow = moon_core::util::display_time::day_start(
            moon_core::util::display_time::shift_date(today, 1),
            zone,
        )
        .unwrap_or(now);
        let shifted =
            |days| moon_core::util::display_time::shift_day_start(day0, days, zone).unwrap_or(day0);
        match self {
            Period::Today => (day0, tomorrow),
            Period::Yesterday => (shifted(-1), day0),
            Period::Week => (shifted(-6), tomorrow),
            Period::CurMonth => {
                // Resolve the current civil month's first date through the selected zone.
                let start = today
                    .with_day(1)
                    .and_then(|day| moon_core::util::display_time::day_start(day, zone))
                    .unwrap_or(day0);
                (start, tomorrow)
            }
            Period::CurYear => {
                // January 1 of today's civil year in the selected zone, exclusive tomorrow.
                let start = today
                    .with_month(1)
                    .and_then(|date| date.with_day(1))
                    .and_then(|day| moon_core::util::display_time::day_start(day, zone))
                    .unwrap_or(day0);
                (start, tomorrow)
            }
            Period::Month => (shifted(-29), tomorrow),
            Period::Year => (shifted(-364), tomorrow),
            Period::All => (-1, tomorrow),
            Period::Custom(f, t) => (f, t),
        }
    }
}

/// The period whose bounds the shared "from"/"to" fields must show for one tab.
///
/// The window carries a period per tab under its own persisted key, so a reopened window must
/// seed those fields from the tab it actually opens on — otherwise the tuner filters by a custom
/// range while the fields sit empty, and the filter reads as absent. Mirrors
/// `AnalyticsView::active_period`, which every LATER sync already goes through.
///
/// Args:
///     tab: Tab the window opens on.
///     summary: Period persisted for Summary (and Calendar), if any.
///     strat: Period persisted for Strategy tuning, if any.
///
/// Returns:
///     That tab's persisted period, or `None` when it was never persisted.
pub(super) fn seed_period(
    tab: Tab,
    summary: Option<Period>,
    strat: Option<Period>,
) -> Option<Period> {
    match tab {
        Tab::Strategies => strat,
        _ => summary,
    }
}

/// Return the selected-zone civil date for an absolute instant.
///
/// Args:
///     secs: UTC Unix seconds.
///     zone: Selected IANA display zone.
///
/// Returns:
///     Civil date, or `None` outside chrono's range.
pub(super) fn day_of_secs(secs: i64, zone: Tz) -> Option<chrono::NaiveDate> {
    moon_core::util::display_time::date(secs, zone)
}

/// Resolve the first real instant of one selected-zone civil date.
///
/// Args:
///     d: Civil date.
///     zone: Selected IANA display zone.
///
/// Returns:
///     UTC Unix seconds, or zero when the date cannot be resolved.
pub(super) fn secs_of_day(d: chrono::NaiveDate, zone: Tz) -> i64 {
    moon_core::util::display_time::day_start(d, zone).unwrap_or(0)
}

/// Resolve a civil date only when that date actually exists in the selected zone.
///
/// The shared gap policy advances a fully skipped historical date to the next real instant. Month
/// cells need to distinguish that case so the following day's aggregate is not displayed twice.
///
/// Args:
///     d: Civil date represented by one calendar cell.
///     zone: Selected IANA display zone.
///
/// Returns:
///     First real instant of `d`, or `None` when the date is skipped or out of range.
pub(super) fn exact_secs_of_day(d: chrono::NaiveDate, zone: Tz) -> Option<i64> {
    moon_core::util::display_time::exact_day_start(d, zone)
}

/// Format an absolute instant as `DD.MM.YY` in the selected zone.
///
/// Args:
///     secs: UTC Unix seconds.
///     zone: Selected IANA display zone.
///
/// Returns:
///     Civil date label, or an empty string outside chrono's range.
pub(super) fn fmt_day(secs: i64, zone: Tz) -> String {
    day_of_secs(secs, zone)
        .map(|d| d.format("%d.%m.%y").to_string())
        .unwrap_or_default()
}

/// Format an absolute instant as `DD.MM.YY HH:MM` in the selected zone.
///
/// Args:
///     secs: UTC Unix seconds.
///     zone: Selected IANA display zone.
///
/// Returns:
///     Civil date-time label, or an empty string outside chrono's range.
pub(super) fn fmt_minute(secs: i64, zone: Tz) -> String {
    date_range::dt_of_secs(secs, zone)
        .map(|d| d.format("%d.%m.%y %H:%M").to_string())
        .unwrap_or_default()
}

/// Build the `[from, to)` bounds of a custom period out of the two picked field values.
///
/// Args:
///     from: Lower bound as picked, or `None` while the field is empty.
///     to: Upper bound as picked, or `None` while the field is empty.
///     tomorrow: Midnight after today, used when the upper field is empty.
///     zone: Selected IANA display zone used to resolve both civil picker values.
///
/// Returns:
///     `(from, to)` in UTC unix seconds, `from = -1` for unbounded history. The upper bound is
///     exclusive and ends the picked minute, so an equal pair spans that one minute rather than
///     an empty range. An empty upper field never lands before the lower one, which would make
///     a bound picked past today select nothing.
pub(super) fn custom_bounds(
    from: Option<chrono::NaiveDateTime>,
    to: Option<chrono::NaiveDateTime>,
    tomorrow: i64,
    zone: Tz,
) -> (i64, i64) {
    let lower = from
        .and_then(|dt| date_range::secs_of_dt(dt, zone, date_range::Bound::From))
        .unwrap_or(-1);
    let upper = to
        .and_then(|dt| date_range::secs_of_dt(dt, zone, date_range::Bound::To))
        .map(date_range::exclusive_end)
        .unwrap_or_else(|| tomorrow.max(lower + date_range::MINUTE));
    (lower, upper)
}

#[cfg(test)]
mod tests;
