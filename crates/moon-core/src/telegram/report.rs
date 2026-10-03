//! Bounded report navigation and civil-time periods shared by chat commands and callbacks.
use crate::config::telegram_menu::ReportBasis;
use crate::util::display_time::{self, LocalBoundary};
use chrono::{Datelike, Days, NaiveDate, Timelike};
use chrono_tz::Tz;

/// Stable exchange identity for report drill-down; an empty scoped roster never means all cores.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReportScope {
    #[default]
    All,
    Unidentified,
    Venue(crate::feed::ExchangeId),
}

/// The widest custom period, in days from its first to its last: one year and a day less.
pub const MAX_SPAN_DAYS: i64 = 366;

/// A ready custom period, counted back from the chat's today in the report zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// The last 7 days, today included.
    Days7,
    /// The last 30 days, today included.
    Days30,
    /// The calendar week before this one, Monday to Sunday.
    LastWeek,
}

impl Preset {
    /// Every preset, in button order.
    pub const ALL: [Self; 3] = [Self::Days7, Self::Days30, Self::LastWeek];

    /// The first and the last day of this preset when today is `today`.
    pub fn dates(self, today: NaiveDate) -> Option<(NaiveDate, NaiveDate)> {
        match self {
            Self::Days7 => Some((today.checked_sub_days(Days::new(6))?, today)),
            Self::Days30 => Some((today.checked_sub_days(Days::new(29))?, today)),
            Self::LastWeek => {
                let monday = today.checked_sub_days(Days::new(u64::from(
                    today.weekday().num_days_from_monday(),
                )))?;
                let from = monday.checked_sub_days(Days::new(7))?;
                Some((from, monday.checked_sub_days(Days::new(1))?))
            }
        }
    }
}

/// A requested period; explicit dates include both calendar days.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Period {
    Hour,
    Today,
    Yesterday,
    Month,
    LastMonth,
    Dates(NaiveDate, NaiveDate),
}

impl Period {
    /// Whether the period can cover more than one calendar day, so a split by days says something.
    pub fn spans_days(&self) -> bool {
        match self {
            Self::Hour | Self::Today | Self::Yesterday => false,
            Self::Month | Self::LastMonth => true,
            Self::Dates(from, to) => from != to,
        }
    }
}

/// One read-only report view, independent of a chat's previous navigation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportRequest {
    pub period: Period,
    pub daily: bool,
    pub by_exchange: bool,
    pub scope: ReportScope,
    pub page: usize,
    /// Frozen UTC bounds for paging; explicit refresh clears these before resolving a preset.
    pub window: Option<(i64, i64)>,
    /// Inline exchange list is expanded; a missing or unknown flag stays collapsed.
    pub exchanges_open: bool,
    /// The chat did not pick a view (a period button or command): the bot's own default view
    /// applies ([`Self::resolve_view`]). Never encoded in a callback — a callback names its view.
    pub follow_view: bool,
    /// The timestamp the period applies to, once a report has been read on it: its paging and view
    /// buttons keep it, so an old message never mixes bases. `None` reads on the bot's basis now.
    pub basis: Option<ReportBasis>,
}

impl ReportRequest {
    /// Construct the first page of a preset.
    pub fn new(period: Period, daily: bool) -> Self {
        Self {
            period,
            daily,
            by_exchange: !daily,
            scope: ReportScope::All,
            page: 0,
            window: None,
            exchanges_open: false,
            follow_view: false,
            basis: None,
        }
    }

    /// The first page of a preset whose view the chat did not pick: a period button or command.
    pub fn preset(period: Period) -> Self {
        Self {
            follow_view: true,
            ..Self::new(period, false)
        }
    }

    /// This request in `view`. A single day has nothing to split by days: it opens by exchanges, as
    /// the report's own buttons never offer days for today.
    pub fn in_view(mut self, view: crate::config::telegram_menu::ReportView) -> Self {
        use crate::config::telegram_menu::ReportView;
        let view = match view {
            ReportView::Days if !self.period.spans_days() => ReportView::Exchanges,
            other => other,
        };
        self.daily = view == ReportView::Days;
        self.by_exchange = view == ReportView::Exchanges;
        self.follow_view = false;
        self
    }

    /// This request in `view` when the chat did not pick one; otherwise unchanged.
    pub fn resolve_view(self, view: crate::config::telegram_menu::ReportView) -> Self {
        if self.follow_view {
            self.in_view(view)
        } else {
            self
        }
    }

    /// Encode a compact callback below Telegram's 64-byte limit.
    pub fn callback(&self) -> String {
        let period = match self.period {
            Period::Hour => "h".into(),
            Period::Today => "t".into(),
            Period::Yesterday => "y".into(),
            Period::Month => "m".into(),
            Period::LastMonth => "l".into(),
            Period::Dates(a, b) => format!("{a},{b}"),
        };
        let view = if self.daily {
            "d"
        } else if self.by_exchange {
            "e"
        } else {
            "c"
        };
        let scope = match self.scope {
            ReportScope::All => String::new(),
            ReportScope::Unidentified => "u".into(),
            ReportScope::Venue(id) => format!("{:x}.{:x}", id.code, id.dex),
        };
        let open = if self.exchanges_open { "k" } else { "" };
        let basis = match self.basis {
            None => "",
            Some(ReportBasis::Close) => "z",
            Some(ReportBasis::Open) => "o",
        };
        let mut encoded = format!("r:{view}{scope}{open}{basis}:{period}:{}", self.page);
        if let Some((from, to)) = self.window {
            encoded.push_str(&format!(":x{from:x},{to:x}"));
        }
        encoded
    }

    /// Decode only this feature's callback namespace and bounded page indices.
    pub fn parse_callback(value: &str) -> Option<Self> {
        if value.len() > 64 {
            return None;
        }
        let parts: Vec<_> = value.split(':').collect();
        if !(4..=5).contains(&parts.len()) || parts[0] != "r" {
            return None;
        }
        let (view, rest) = parts[1].split_at_checked(1)?;
        let daily = match view {
            "d" => true,
            "c" | "e" => false,
            _ => return None,
        };
        // A trailing `o`/`z` is the basis the report was read on (none: the bot's basis now).
        let (rest, basis) = match (rest.strip_suffix('o'), rest.strip_suffix('z')) {
            (Some(stripped), _) => (stripped, Some(ReportBasis::Open)),
            (_, Some(stripped)) => (stripped, Some(ReportBasis::Close)),
            _ => (rest, None),
        };
        // A trailing `k` is the expanded exchange list; any other suffix stays collapsed.
        let (scope_src, exchanges_open) = match rest.strip_suffix('k') {
            Some(stripped) => (stripped, true),
            None => (rest, false),
        };
        let scope = match scope_src {
            "" => ReportScope::All,
            "u" => ReportScope::Unidentified,
            value => {
                let (code, dex) = value.split_once('.')?;
                ReportScope::Venue(crate::feed::ExchangeId {
                    code: u8::from_str_radix(code, 16).ok()?,
                    dex: u32::from_str_radix(dex, 16).ok()?,
                })
            }
        };
        let period = match parts[2] {
            "h" => Period::Hour,
            "t" => Period::Today,
            "y" => Period::Yesterday,
            "m" => Period::Month,
            "l" => Period::LastMonth,
            dates => {
                let (a, b) = dates.split_once(',')?;
                Self::dates(a, b)?.period
            }
        };
        let page = parts[3].parse().ok()?;
        if page > 10_000 {
            return None;
        }
        let window = if let Some(value) = parts.get(4) {
            let (value, radix) = value
                .strip_prefix('x')
                .map(|s| (s, 16))
                .unwrap_or((value, 10));
            let (from, to) = value.split_once(',')?;
            let from = i64::from_str_radix(from, radix).ok()?;
            let to = i64::from_str_radix(to, radix).ok()?;
            if from < 0 || to > 253402300799 || !(0..=367 * 86400).contains(&to.checked_sub(from)?)
            {
                return None;
            }
            Some((from, to))
        } else {
            None
        };
        Some(Self {
            period,
            daily,
            by_exchange: view == "e",
            scope,
            page,
            window,
            exchanges_open,
            follow_view: false,
            basis,
        })
    }

    /// Validate a manual range without accepting reversed dates or unbounded histories.
    pub fn dates(a: &str, b: &str) -> Option<Self> {
        let from = NaiveDate::parse_from_str(a, "%Y-%m-%d").ok()?;
        let to = NaiveDate::parse_from_str(b, "%Y-%m-%d").ok()?;
        Self::span(from, to)
    }

    /// A custom period whose view the chat did not pick, within the same limits as [`Self::dates`].
    pub fn span(from: NaiveDate, to: NaiveDate) -> Option<Self> {
        if from.year() < 1970 || !(0..MAX_SPAN_DAYS).contains(&(to - from).num_days()) {
            return None;
        }
        Some(Self::preset(Period::Dates(from, to)))
    }

    /// Resolve inclusive UTC bounds through the terminal's DST-aware calendar helpers.
    pub fn bounds(&self, now: i64, zone: Tz) -> Option<(i64, i64)> {
        if let Some(window) = self.window {
            return Some(window);
        }
        let local = display_time::at(now, zone)?;
        let today = local.date_naive();
        let month = today.with_day(1)?;
        let (from, to) = match self.period {
            Period::Hour => {
                let local = local.naive_local().with_minute(0)?.with_second(0)?;
                return Some((
                    display_time::unix_from_local(local, zone, LocalBoundary::Lower)?,
                    now,
                ));
            }
            Period::Today => return Some((display_time::day_start(today, zone)?, now)),
            Period::Month => return Some((display_time::day_start(month, zone)?, now)),
            Period::Yesterday => (today.checked_sub_days(Days::new(1))?, today),
            Period::LastMonth => (month.checked_sub_days(Days::new(1))?.with_day(1)?, month),
            Period::Dates(a, b) => (a, b.checked_add_days(Days::new(1))?),
        };
        Some((
            display_time::day_start(from, zone)?,
            display_time::day_start(to, zone)? - 1,
        ))
    }
}

/// An automatic report's latest slot at or before a moment, and the period it reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoWindow {
    /// UTC seconds of the slot: the local hh:00 (hourly, today) or 00:00 (month) it fires at.
    pub at: i64,
    /// Inclusive UTC bounds of the reported period; `to` is the second before `at`.
    pub from: i64,
    pub to: i64,
    /// The preset the period is, for the report's own buttons.
    pub period: Period,
}

/// `kind`'s latest slot at or before `now` in `zone`, and the finished period it reports.
///
/// The hour starts where the local clock reads hh:00, so a zone half an hour off UTC fires at its
/// own hh:00, and the hour before a slot is always 3600 seconds, a daylight-saving change
/// included. A day and a month start at local midnight through [`display_time::day_start`]: a day
/// may last 23 or 25 hours.
///
/// Returns:
///     `None` when the moment cannot be shown in the zone.
pub fn auto_window(
    kind: crate::telegram::notify::AutoReport,
    now: i64,
    zone: Tz,
) -> Option<AutoWindow> {
    use crate::telegram::notify::AutoReport;
    let local = display_time::at(now, zone)?;
    let hour = now - i64::from(local.minute()) * 60 - i64::from(local.second());
    match kind {
        AutoReport::Hourly => Some(AutoWindow {
            at: hour,
            from: hour - 3600,
            to: hour - 1,
            period: Period::Hour,
        }),
        AutoReport::Today => {
            let date = display_time::at(hour, zone)?.date_naive();
            let start = display_time::day_start(date, zone)?;
            // At midnight today has nothing yet: the slot reports the day that just ended.
            let (from, period) = if hour <= start {
                let previous = date.checked_sub_days(Days::new(1))?;
                (display_time::day_start(previous, zone)?, Period::Yesterday)
            } else {
                (start, Period::Today)
            };
            Some(AutoWindow {
                at: hour,
                from,
                to: hour - 1,
                period,
            })
        }
        AutoReport::Month => {
            let today = local.date_naive();
            let at = display_time::day_start(today, zone)?;
            let first = today.with_day(1)?;
            // On the 1st the month just ended is reported whole.
            let (first, period) = if today.day() == 1 {
                let previous = first.checked_sub_days(Days::new(1))?.with_day(1)?;
                (previous, Period::LastMonth)
            } else {
                (first, Period::Month)
            };
            Some(AutoWindow {
                at,
                from: display_time::day_start(first, zone)?,
                to: at - 1,
                period,
            })
        }
    }
}

#[cfg(test)]
mod tests;
