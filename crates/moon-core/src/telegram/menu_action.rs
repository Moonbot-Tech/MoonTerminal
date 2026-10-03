//! The bot's inline section menus: what their buttons ask, and the callback data that carries it.
//!
//! Its own `m:` namespace, apart from the report's `r:` and the station's `station:update`. A
//! button that opens a report directly carries a report callback instead; only what needs the
//! bot's state at press time (a preset counted from today, the calendar) lives here.

use chrono::{Datelike, NaiveDate};

use super::report::Preset;

/// What an inline menu button asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// The Report section: its period buttons.
    Report,
    /// The custom-period screen: presets and a calendar of `month` (the current one when absent),
    /// with `from` picked as the first day when present.
    Custom {
        month: Option<NaiveDate>,
        from: Option<NaiveDate>,
    },
    /// A preset period, counted from today when pressed.
    Preset(Preset),
    /// A calendar cell that does nothing: a weekday caption, a blank, a day out of reach.
    Noop,
}

/// Prefix of every callback in this namespace.
const PREFIX: &str = "m:";

impl MenuAction {
    /// Encode this action, well below Telegram's 64-byte callback limit.
    pub fn callback(&self) -> String {
        match self {
            Self::Report => "m:r".into(),
            Self::Custom { month, from } => {
                let mut data = String::from("m:c");
                if let Some(month) = month.or(*from) {
                    data.push_str(&format!(":{:04}-{:02}", month.year(), month.month()));
                }
                if let Some(from) = from {
                    data.push_str(&format!(":{from}"));
                }
                data
            }
            Self::Preset(Preset::Days7) => "m:p:7".into(),
            Self::Preset(Preset::Days30) => "m:p:30".into(),
            Self::Preset(Preset::LastWeek) => "m:p:w".into(),
            Self::Noop => "m:-".into(),
        }
    }

    /// Decode only this namespace; anything malformed is `None`.
    pub fn parse_callback(value: &str) -> Option<Self> {
        if value.len() > 64 {
            return None;
        }
        let rest = value.strip_prefix(PREFIX)?;
        let parts: Vec<&str> = rest.split(':').collect();
        match parts.as_slice() {
            ["r"] => Some(Self::Report),
            ["-"] => Some(Self::Noop),
            ["p", "7"] => Some(Self::Preset(Preset::Days7)),
            ["p", "30"] => Some(Self::Preset(Preset::Days30)),
            ["p", "w"] => Some(Self::Preset(Preset::LastWeek)),
            ["c"] => Some(Self::Custom {
                month: None,
                from: None,
            }),
            ["c", month] => Some(Self::Custom {
                month: Some(parse_month(month)?),
                from: None,
            }),
            ["c", month, from] => Some(Self::Custom {
                month: Some(parse_month(month)?),
                from: Some(parse_day(from)?),
            }),
            _ => None,
        }
    }
}

/// `YYYY-MM` as the first day of that month, within the years a report can reach.
fn parse_month(value: &str) -> Option<NaiveDate> {
    let (year, month) = value.split_once('-')?;
    if year.len() != 4 || month.len() != 2 {
        return None;
    }
    let date = NaiveDate::from_ymd_opt(year.parse().ok()?, month.parse().ok()?, 1)?;
    (1970..=9999).contains(&date.year()).then_some(date)
}

/// `YYYY-MM-DD`, within the years a report can reach.
fn parse_day(value: &str) -> Option<NaiveDate> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    (1970..=9999).contains(&date.year()).then_some(date)
}

#[cfg(test)]
mod tests;
