//! Bounded report navigation and civil-time periods shared by chat commands and callbacks.
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
        let mut encoded = format!("r:{view}{scope}:{period}:{}", self.page);
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
        let (view, scope) = parts[1].split_at_checked(1)?;
        let daily = match view {
            "d" => true,
            "c" | "e" => false,
            _ => return None,
        };
        let scope = match scope {
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
        })
    }

    /// Validate a manual range without accepting reversed dates or unbounded histories.
    pub fn dates(a: &str, b: &str) -> Option<Self> {
        let from = NaiveDate::parse_from_str(a, "%Y-%m-%d").ok()?;
        let to = NaiveDate::parse_from_str(b, "%Y-%m-%d").ok()?;
        if from.year() < 1970 || !(0..366).contains(&(to - from).num_days()) {
            return None;
        }
        Some(Self::new(Period::Dates(from, to), false))
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

#[cfg(test)]
mod tests;
