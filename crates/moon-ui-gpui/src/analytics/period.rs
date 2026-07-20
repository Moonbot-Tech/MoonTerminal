//! Period and tabs of the "Analytics" window: period presets (`Period`) with their ranges and
//! persist keys, the list of tabs (`Tab`) and the date helpers shared by the pages.

use rust_i18n::t;

/// Window tabs. The placeholders (Coins/Leverage) will come back as the stages are implemented.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tab {
    Summary,
    Strategies,
    /// Profit calendar (Day / Month / Year).
    Calendar,
}

impl Tab {
    // Tab order: Summary → Calendar → Strategy tuning (last).
    pub(super) const ALL: [Tab; 3] = [Tab::Summary, Tab::Calendar, Tab::Strategies];
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

/// Period presets (UTC days, same as in the "Report" panel).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Period {
    Today,
    Yesterday,
    Week,
    /// The current calendar month (from the 1st, UTC).
    CurMonth,
    /// A rolling 30 days.
    Month,
    Year,
    All,
    /// An arbitrary range from the "from"/"to" fields: `[from, to)` in UTC unix seconds;
    /// from = -1 — "from" is unset (the whole history up to "to").
    Custom(i64, i64),
}

impl Period {
    pub(super) const ALL: [Period; 7] = [
        Period::Today,
        Period::Yesterday,
        Period::Week,
        Period::CurMonth,
        Period::Month,
        Period::Year,
        Period::All,
    ];
    /// A preset by its id (the selection persisted in layout); None — unknown id.
    pub(super) fn from_id(id: &str) -> Option<Period> {
        if let Some(rest) = id.strip_prefix("p-custom:") {
            let (f, t) = rest.split_once(':')?;
            return Some(Period::Custom(f.parse().ok()?, t.parse().ok()?));
        }
        Period::ALL.into_iter().find(|p| p.id() == id)
    }
    pub(super) fn id(self) -> &'static str {
        match self {
            Period::Today => "p-today",
            Period::Yesterday => "p-yesterday",
            Period::Week => "p-week",
            Period::CurMonth => "p-cur-month",
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
    pub(super) fn title(self) -> String {
        match self {
            Period::Today => t!("analytics.period.today"),
            Period::Yesterday => t!("analytics.period.yesterday"),
            Period::Week => t!("analytics.period.week"),
            Period::CurMonth => t!("analytics.period.cur_month"),
            Period::Month => t!("analytics.period.month"),
            Period::Year => t!("analytics.period.year"),
            Period::All => t!("analytics.period.all"),
            Period::Custom(f, t) => {
                let a = if f < 0 { "—".to_string() } else { fmt_day(f) };
                return format!("{a} – {}", fmt_day((t - 86_400).max(f.max(0))));
            }
        }
        .to_string()
    }
    /// The `[from, to)` bounds in UTC unix seconds; from = -1 → the whole history.
    pub(super) fn range(self) -> (i64, i64) {
        let now = moon_core::util::now_unix_ms_i64() / 1000;
        let day0 = now.div_euclid(86_400) * 86_400;
        let tomorrow = day0 + 86_400;
        match self {
            Period::Today => (day0, tomorrow),
            Period::Yesterday => (day0 - 86_400, day0),
            Period::Week => (tomorrow - 7 * 86_400, tomorrow),
            Period::CurMonth => {
                // The 1st of the current month: "YYYY-MM" from the DB formatter + "-01".
                let ym = moon_core::db::fmt_unix(now);
                let start = moon_core::db::parse_ymd(&format!("{}-01", &ym[..7.min(ym.len())]))
                    .unwrap_or(day0);
                (start, tomorrow)
            }
            Period::Month => (tomorrow - 30 * 86_400, tomorrow),
            Period::Year => (tomorrow - 365 * 86_400, tomorrow),
            Period::All => (-1, tomorrow),
            Period::Custom(f, t) => (f, t),
        }
    }
}

/// unix seconds → UTC date (for the "from"/"to" calendars and the heatmap).
pub(super) fn day_of_secs(secs: i64) -> Option<chrono::NaiveDate> {
    chrono::DateTime::from_timestamp(secs, 0).map(|d| d.date_naive())
}

/// UTC date → unix seconds of that day's midnight.
pub(super) fn secs_of_day(d: chrono::NaiveDate) -> i64 {
    d.and_hms_opt(0, 0, 0)
        .map(|dt| dt.and_utc().timestamp())
        .unwrap_or(0)
}

/// "dd.mm.yy" for the range field labels and the heatmap info.
pub(super) fn fmt_day(secs: i64) -> String {
    day_of_secs(secs)
        .map(|d| d.format("%d.%m.%y").to_string())
        .unwrap_or_default()
}
