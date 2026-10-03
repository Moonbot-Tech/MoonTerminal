//! The bot's inline section menus: what their buttons ask, and the callback data that carries it.
//!
//! Its own `m:` namespace, apart from the report's `r:` and the station's `station:update`. A
//! button that opens a report directly carries a report callback instead; only what needs the
//! bot's state at press time (a preset counted from today, the calendar) lives here.

use chrono::{Datelike, NaiveDate};

use super::report::Preset;
use crate::config::telegram_menu::{MenuItem, ReportBasis, ReportView};

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
    /// A screen or a switch of the owner's Settings section.
    Settings(SettingsAction),
}

/// What a button of the bot's Settings section asks for: a screen, or one switch that saves at
/// once and shows its screen again. A switch names the state it sets, not "the other one": a
/// press on an old message, or a second tap, then cannot undo what the user meant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsAction {
    /// The section itself.
    Root,
    /// Which buttons the keyboard shows.
    Buttons,
    /// Show (`true`) or hide one keyboard button.
    ShowButton(MenuItem, bool),
    /// The view reports open in.
    View,
    SetView(ReportView),
    /// The time a period counts trades by.
    Basis,
    SetBasis(ReportBasis),
    /// Switch the Mini App on or off.
    MiniApp(bool),
    /// This chat's notifications.
    Notify,
    /// Closed-trade cards on or off.
    Trades(bool),
    /// Core down/back notices on or off.
    Down(bool),
    /// How long a core must stay down before the notice, in minutes.
    DownAfter(u16),
    /// The daily summary on or off.
    Daily(bool),
    /// The hour picker of the daily summary.
    DailyHours,
    /// The hour the daily summary is sent at.
    DailyHour(u8),
    /// The station's status, opened from the section: it leads back to it.
    StationStatus,
}

impl SettingsAction {
    /// The data after `m:s`, empty for the section itself.
    fn encode(self) -> String {
        let flag = |on: bool| if on { "1" } else { "0" };
        match self {
            Self::Root => String::new(),
            Self::Buttons => ":b".into(),
            // `k` is the keyboard, kept from when the Report section had buttons of its own.
            Self::ShowButton(item, show) => format!(":b:k:{}:{}", item.id(), flag(show)),
            Self::View => ":v".into(),
            Self::SetView(view) => format!(":v:{}", view.id()),
            Self::Basis => ":p".into(),
            Self::SetBasis(basis) => format!(":p:{}", basis.id()),
            Self::MiniApp(on) => format!(":m:{}", flag(on)),
            Self::Notify => ":n".into(),
            Self::Trades(on) => format!(":n:t:{}", flag(on)),
            Self::Down(on) => format!(":n:o:{}", flag(on)),
            Self::DownAfter(minutes) => format!(":n:d:{minutes}"),
            Self::Daily(on) => format!(":n:y:{}", flag(on)),
            Self::DailyHours => ":n:h".into(),
            Self::DailyHour(hour) => format!(":n:h:{hour}"),
            Self::StationStatus => ":st".into(),
        }
    }

    /// Decode the parts after `m:s`; anything malformed is `None`.
    fn decode(parts: &[&str]) -> Option<Self> {
        let flag = |code: &str| match code {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        };
        Some(match parts {
            [] => Self::Root,
            ["b"] => Self::Buttons,
            ["b", "k", id, show] => Self::ShowButton(MenuItem::from_id(id)?, flag(show)?),
            ["v"] => Self::View,
            ["v", id] => Self::SetView(ReportView::ALL.into_iter().find(|v| v.id() == *id)?),
            ["p"] => Self::Basis,
            ["p", id] => Self::SetBasis(ReportBasis::ALL.into_iter().find(|b| b.id() == *id)?),
            ["m", on] => Self::MiniApp(flag(on)?),
            ["st"] => Self::StationStatus,
            ["n"] => Self::Notify,
            ["n", "t", on] => Self::Trades(flag(on)?),
            ["n", "o", on] => Self::Down(flag(on)?),
            ["n", "d", minutes] => {
                let minutes: u16 = minutes.parse().ok()?;
                (1..=1440)
                    .contains(&minutes)
                    .then_some(Self::DownAfter(minutes))?
            }
            ["n", "y", on] => Self::Daily(flag(on)?),
            ["n", "h"] => Self::DailyHours,
            ["n", "h", hour] => {
                let hour: u8 = hour.parse().ok()?;
                (hour < 24).then_some(Self::DailyHour(hour))?
            }
            _ => return None,
        })
    }
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
            Self::Settings(action) => format!("m:s{}", action.encode()),
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
            ["s", rest @ ..] => SettingsAction::decode(rest).map(Self::Settings),
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
