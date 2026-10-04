//! The bot's inline section menus: what their buttons ask, and the callback data that carries it.
//!
//! Its own `m:` namespace, apart from the report's `r:` and the station's `station:update`. A
//! button that opens a report directly carries a report callback instead; only what needs the
//! bot's state at press time (a preset counted from today, the calendar) lives here.

use chrono::{Datelike, NaiveDate};

use super::report::Preset;
use crate::config::telegram_menu::{MenuItem, ReportBasis, ReportView};
use crate::telegram::notify::AutoReport;

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
    /// A screen or a command of the owner's Control section.
    Control(ControlAction),
}

/// Which cores a Control command addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlTarget {
    /// One core, by its id.
    Core(u64),
    /// Every core the owner sees.
    All,
}

/// A core's run switch, as a Control button names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlSwitch {
    /// Automatic trading: the strategy engine.
    Trading,
    /// Automatic detection.
    AutoDetect,
}

/// What a button of the bot's Control section asks for. A command that can lose money or stop
/// trading everywhere carries `confirmed` — panic sells, cancel all, and starting or stopping all
/// cores (one core's run switch carries the field but does not ask): the first press shows a
/// confirmation, whose button sends the same action confirmed. A command names the state it
/// sets, as a Settings switch does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlAction {
    /// The cores, one page of them, with "all cores".
    Cores(u16),
    /// One core's card.
    Core(u64),
    /// The all-cores card.
    All,
    /// A run switch on one core or all of them.
    Run {
        target: ControlTarget,
        switch: ControlSwitch,
        on: bool,
        confirmed: bool,
    },
    /// Cancel every open order of one core.
    CancelAll { core: u64, confirmed: bool },
    /// Panic-sell every market with an open position, on one core or all of them.
    PanicAll {
        target: ControlTarget,
        confirmed: bool,
    },
    /// Reconnect one core.
    Reconnect(u64),
    /// One core's open positions, a page of them.
    Orders { core: u64, page: u16 },
    /// One open position's card.
    Order { core: u64, uid: u64 },
    /// Panic-sell one order.
    OrderPanic {
        core: u64,
        uid: u64,
        confirmed: bool,
    },
    /// Put an order's coin on a blacklist.
    OrderBan { core: u64, uid: u64, ban: OrderBan },
    /// Ask for a coin to put on the core's own blacklist, or with `lift` to take off it: the
    /// chat's next plain text is the answer.
    AskCoin { core: u64, lift: bool },
    /// One core's strategies, a page of them.
    Strategies { core: u64, page: u16 },
    /// Check or uncheck one strategy, then show the same page again.
    StrategyToggle {
        core: u64,
        id: u64,
        on: bool,
        page: u16,
    },
}

/// Which blacklist an order's coin goes on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderBan {
    /// The core's own list.
    Core,
    /// The `CoinsBlackList` of the strategy that placed the order; a manual order has none.
    Strategy,
    /// The core's temporary list, keyed by the order's market, for this span.
    Temp(crate::config::TempBanSpan),
}

impl ControlAction {
    /// The data after `m:k`.
    fn encode(self) -> String {
        let flag = |on: bool| if on { "1" } else { "0" };
        let target = |target: ControlTarget| match target {
            ControlTarget::Core(id) => id.to_string(),
            ControlTarget::All => "a".into(),
        };
        let switch = |switch: ControlSwitch| match switch {
            ControlSwitch::Trading => "t",
            ControlSwitch::AutoDetect => "d",
        };
        match self {
            Self::Cores(page) => format!(":l:{page}"),
            Self::Core(id) => format!(":c:{id}"),
            Self::All => ":a".into(),
            Self::Run {
                target: t,
                switch: s,
                on,
                confirmed,
            } => format!(
                ":r:{}:{}:{}:{}",
                target(t),
                switch(s),
                flag(on),
                flag(confirmed)
            ),
            Self::CancelAll { core, confirmed } => format!(":x:{core}:{}", flag(confirmed)),
            Self::PanicAll {
                target: t,
                confirmed,
            } => format!(":p:{}:{}", target(t), flag(confirmed)),
            Self::Reconnect(id) => format!(":n:{id}"),
            Self::Orders { core, page } => format!(":o:{core}:{page}"),
            Self::Order { core, uid } => format!(":i:{core}:{uid}"),
            Self::OrderPanic {
                core,
                uid,
                confirmed,
            } => format!(":q:{core}:{uid}:{}", flag(confirmed)),
            Self::OrderBan { core, uid, ban } => {
                let code = match ban {
                    OrderBan::Core => "c".to_string(),
                    OrderBan::Strategy => "s".to_string(),
                    OrderBan::Temp(span) => format!("t{}", span.hours()),
                };
                format!(":b:{core}:{uid}:{code}")
            }
            Self::AskCoin { core, lift } => format!(":w:{core}:{}", flag(lift)),
            Self::Strategies { core, page } => format!(":s:{core}:{page}"),
            Self::StrategyToggle { core, id, on, page } => {
                format!(":g:{core}:{id}:{}:{page}", flag(on))
            }
        }
    }

    /// Decode the parts after `m:k`; anything malformed is `None`.
    fn decode(parts: &[&str]) -> Option<Self> {
        let flag = |code: &str| match code {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        };
        let target = |code: &str| match code {
            "a" => Some(ControlTarget::All),
            id => id.parse().ok().map(ControlTarget::Core),
        };
        let switch = |code: &str| match code {
            "t" => Some(ControlSwitch::Trading),
            "d" => Some(ControlSwitch::AutoDetect),
            _ => None,
        };
        Some(match parts {
            ["l", page] => Self::Cores(page.parse().ok()?),
            ["c", id] => Self::Core(id.parse().ok()?),
            ["a"] => Self::All,
            ["r", t, s, on, confirmed] => Self::Run {
                target: target(t)?,
                switch: switch(s)?,
                on: flag(on)?,
                confirmed: flag(confirmed)?,
            },
            ["x", core, confirmed] => Self::CancelAll {
                core: core.parse().ok()?,
                confirmed: flag(confirmed)?,
            },
            ["p", t, confirmed] => Self::PanicAll {
                target: target(t)?,
                confirmed: flag(confirmed)?,
            },
            ["n", id] => Self::Reconnect(id.parse().ok()?),
            ["o", core, page] => Self::Orders {
                core: core.parse().ok()?,
                page: page.parse().ok()?,
            },
            ["i", core, uid] => Self::Order {
                core: core.parse().ok()?,
                uid: uid.parse().ok()?,
            },
            ["q", core, uid, confirmed] => Self::OrderPanic {
                core: core.parse().ok()?,
                uid: uid.parse().ok()?,
                confirmed: flag(confirmed)?,
            },
            ["w", core, lift] => Self::AskCoin {
                core: core.parse().ok()?,
                lift: flag(lift)?,
            },
            ["s", core, page] => Self::Strategies {
                core: core.parse().ok()?,
                page: page.parse().ok()?,
            },
            ["g", core, id, on, page] => Self::StrategyToggle {
                core: core.parse().ok()?,
                id: id.parse().ok()?,
                on: flag(on)?,
                page: page.parse().ok()?,
            },
            ["b", core, uid, code] => Self::OrderBan {
                core: core.parse().ok()?,
                uid: uid.parse().ok()?,
                ban: match *code {
                    "c" => OrderBan::Core,
                    "s" => OrderBan::Strategy,
                    hours => {
                        let hours: u64 = hours.strip_prefix('t')?.parse().ok()?;
                        OrderBan::Temp(
                            crate::config::TempBanSpan::ALL
                                .into_iter()
                                .find(|span| span.hours() == hours)?,
                        )
                    }
                },
            },
            _ => return None,
        })
    }
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
    /// An automatic report on or off.
    Auto(AutoReport, bool),
    /// The cores' own "trade opened" reports on or off.
    Opened(bool),
    /// The cores' own detect reports on or off.
    Detects(bool),
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
            Self::Auto(kind, on) => format!(":n:a:{}:{}", auto_code(kind), flag(on)),
            Self::Opened(on) => format!(":n:e:o:{}", flag(on)),
            Self::Detects(on) => format!(":n:e:d:{}", flag(on)),
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
            ["n", "a", kind, on] => Self::Auto(
                AutoReport::ALL
                    .into_iter()
                    .find(|k| auto_code(*k) == *kind)?,
                flag(on)?,
            ),
            ["n", "e", "o", on] => Self::Opened(flag(on)?),
            ["n", "e", "d", on] => Self::Detects(flag(on)?),
            _ => return None,
        })
    }
}

/// An automatic report's code in a callback.
fn auto_code(kind: AutoReport) -> &'static str {
    match kind {
        AutoReport::Hourly => "h",
        AutoReport::Today => "t",
        AutoReport::Month => "m",
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
            Self::Control(action) => format!("m:k{}", action.encode()),
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
            ["k", rest @ ..] => ControlAction::decode(rest).map(Self::Control),
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
