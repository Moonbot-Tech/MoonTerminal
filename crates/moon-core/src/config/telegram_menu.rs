//! The Telegram bot's own menu and report preferences: one set per bot, wherever it runs.
//!
//! It travels inside [`TelegramConfig`](super::TelegramConfig): the terminal saves it in
//! `servers.enc`. A station's bot runs these defaults until its own copy is delivered to it. A
//! file may be read by an older or newer build than the one that wrote it, so nothing here fails a
//! load:
//! an item id or a value this build does not know is dropped or read as the default, and a menu
//! is always normalized on the way in ([`BotMenu::normalized`]).
//!
//! Items are addressed by stable ids ([`MenuItem::id`]); captions come from the locale. Renaming
//! an id breaks the reply keyboards users already have installed.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One button the bot's menu can hold: an action or a section with its own inline menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuItem {
    Today,
    Yesterday,
    Month,
    LastMonth,
    /// The current month split by days.
    Daily,
    /// The custom-period screen: presets and a calendar.
    Custom,
    Help,
    /// The station's status; a station owner's only.
    Status,
    /// The Report section: an inline menu of periods under one message.
    Report,
    /// The bot's own settings, under one message; the owner's only.
    Settings,
    /// Commands to the cores — run switches, orders, strategies, blacklists; the owner's only.
    /// Hidden until the owner shows it: trading from the chat is a deliberate choice, and on a
    /// station it costs the full feed profile.
    Control,
}

impl MenuItem {
    /// Every item, in the order a picker lists them.
    pub const ALL: [Self; 11] = [
        Self::Report,
        Self::Today,
        Self::Yesterday,
        Self::Month,
        Self::LastMonth,
        Self::Daily,
        Self::Custom,
        Self::Help,
        Self::Status,
        Self::Settings,
        Self::Control,
    ];

    /// Stable id: the saved value and the suffix of the item's locale keys
    /// (`telegram.button_{id}`). Never rename one.
    pub fn id(self) -> &'static str {
        match self {
            Self::Today => "today",
            Self::Yesterday => "yesterday",
            Self::Month => "month",
            Self::LastMonth => "lastmonth",
            Self::Daily => "daily",
            Self::Custom => "custom",
            Self::Help => "help",
            Self::Status => "status",
            Self::Report => "report",
            Self::Settings => "settings",
            Self::Control => "control",
        }
    }

    /// The item with this id, if this build knows it.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|item| item.id() == id)
    }
}

impl Serialize for MenuItem {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.id())
    }
}

/// The Report section's inline menu: a fixed layout, every period shown.
pub const REPORT_SECTION: [&[MenuItem]; 3] = [
    &[MenuItem::Today, MenuItem::Yesterday],
    &[MenuItem::Month, MenuItem::LastMonth],
    &[MenuItem::Daily, MenuItem::Custom],
];

/// One button in a row: the item and whether it is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct MenuEntry {
    pub item: MenuItem,
    pub show: bool,
}

impl MenuEntry {
    /// A shown entry.
    pub fn shown(item: MenuItem) -> Self {
        Self { item, show: true }
    }

    /// A hidden entry.
    pub fn hidden(item: MenuItem) -> Self {
        Self { item, show: false }
    }
}

/// Telegram's widest inline keyboard row; a longer saved row is split.
pub const MAX_ROW: usize = 8;

/// The bot's menu: the reply keyboard as rows of buttons. The Report section is fixed
/// ([`REPORT_SECTION`]).
///
/// The keyboard always lists each item exactly once, hidden ones included, so a settings editor
/// shows the whole set and a renderer only filters ([`Self::visible`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct BotMenu {
    pub keyboard: Vec<Vec<MenuEntry>>,
}

impl Default for BotMenu {
    /// The layout the bot had before the menu became configurable — periods and Help — with the
    /// station's Status and the owner's Settings on a row of their own; everything else that came
    /// with the configurable menu hidden.
    fn default() -> Self {
        use MenuEntry as E;
        use MenuItem::*;
        Self {
            keyboard: vec![
                vec![E::shown(Today), E::shown(Yesterday), E::shown(Help)],
                vec![E::shown(Month), E::shown(LastMonth)],
                vec![E::shown(Status), E::shown(Settings), E::hidden(Control)],
                vec![E::hidden(Report), E::hidden(Daily), E::hidden(Custom)],
            ],
        }
    }
}

impl BotMenu {
    /// Whether the keyboard shows `item`.
    pub fn shows(&self, item: MenuItem) -> bool {
        self.keyboard
            .iter()
            .flatten()
            .any(|entry| entry.item == item && entry.show)
    }

    /// The shown keyboard items that `keep` admits, row by row; rows left empty are dropped.
    ///
    /// Args:
    ///     keep: The caller's filter — what the chat's role and the host allow.
    pub fn visible(&self, keep: impl Fn(MenuItem) -> bool) -> Vec<Vec<MenuItem>> {
        self.keyboard
            .iter()
            .map(|row| {
                row.iter()
                    .filter(|entry| entry.show && keep(entry.item))
                    .map(|entry| entry.item)
                    .collect::<Vec<_>>()
            })
            .filter(|row| !row.is_empty())
            .collect()
    }

    /// The keyboard entries in order, each with whether it starts a row: the shape an editor
    /// works on, where rows can never be left empty.
    pub fn flat(&self) -> Vec<(MenuEntry, bool)> {
        self.keyboard
            .iter()
            .flat_map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(index, entry)| (*entry, index == 0))
            })
            .collect()
    }

    /// Replace the keyboard rows with `flat`, cut where an entry starts a row (the first always
    /// does) and where a row is full ([`MAX_ROW`]).
    ///
    /// Returns:
    ///     Whether the rows changed: joining an entry onto a full row does not.
    fn set_flat(&mut self, flat: Vec<(MenuEntry, bool)>) -> bool {
        let mut rows: Vec<Vec<MenuEntry>> = Vec::new();
        for (entry, starts) in flat {
            match rows.last_mut() {
                Some(row) if !starts && row.len() < MAX_ROW => row.push(entry),
                _ => rows.push(vec![entry]),
            }
        }
        let changed = self.keyboard != rows;
        self.keyboard = rows;
        changed
    }

    /// Move `item` one place earlier (`up`) or later: within its row, or across the edge into the
    /// next row, which keeps its place.
    ///
    /// Returns:
    ///     Whether anything moved; the first entry cannot go up nor the last one down.
    pub fn move_item(&mut self, item: MenuItem, up: bool) -> bool {
        let mut flat = self.flat();
        let Some(at) = flat.iter().position(|(entry, _)| entry.item == item) else {
            return false;
        };
        let other = if up { at.checked_sub(1) } else { Some(at + 1) };
        let Some(other) = other.filter(|&other| other < flat.len()) else {
            return false;
        };
        // The entries swap; the row starts stay where they are.
        let (a, b) = (flat[at].0, flat[other].0);
        flat[at].0 = b;
        flat[other].0 = a;
        self.set_flat(flat)
    }

    /// Start a new row at `item` (`starts`), or join it to the row before.
    ///
    /// Returns:
    ///     Whether anything changed; the first entry always starts a row.
    pub fn set_row_start(&mut self, item: MenuItem, starts: bool) -> bool {
        let mut flat = self.flat();
        let Some(at) = flat.iter().position(|(entry, _)| entry.item == item) else {
            return false;
        };
        if at == 0 || flat[at].1 == starts {
            return false;
        }
        flat[at].1 = starts;
        self.set_flat(flat)
    }

    /// Show or hide `item`.
    ///
    /// Returns:
    ///     Whether anything changed.
    pub fn set_shown(&mut self, item: MenuItem, show: bool) -> bool {
        let entry = self
            .keyboard
            .iter_mut()
            .flatten()
            .find(|entry| entry.item == item);
        match entry {
            Some(entry) if entry.show != show => {
                entry.show = show;
                true
            }
            _ => false,
        }
    }

    /// This menu made valid: repeats are dropped, rows wider than [`MAX_ROW`] are split, empty
    /// rows are removed, and each item missing from the keyboard — one a newer build added — is
    /// appended on a row at the end, shown or hidden as the default layout has it.
    pub fn normalized(mut self) -> Self {
        self.keyboard = normalize_keyboard(std::mem::take(&mut self.keyboard));
        self
    }
}

/// See [`BotMenu::normalized`].
fn normalize_keyboard(rows: Vec<Vec<MenuEntry>>) -> Vec<Vec<MenuEntry>> {
    let allowed = &MenuItem::ALL;
    let mut seen = Vec::new();
    let mut out: Vec<Vec<MenuEntry>> = Vec::new();
    for row in rows {
        let kept: Vec<MenuEntry> = row
            .into_iter()
            .filter(|entry| {
                let fresh = allowed.contains(&entry.item) && !seen.contains(&entry.item);
                if fresh {
                    seen.push(entry.item);
                }
                fresh
            })
            .collect();
        out.extend(kept.chunks(MAX_ROW).map(<[MenuEntry]>::to_vec));
    }
    let missing: Vec<MenuEntry> = allowed
        .iter()
        .filter(|item| !seen.contains(item))
        .map(|&item| MenuEntry {
            item,
            show: BotMenu::default()
                .keyboard
                .iter()
                .flatten()
                .any(|entry| entry.item == item && entry.show),
        })
        .collect();
    out.extend(missing.chunks(MAX_ROW).map(<[MenuEntry]>::to_vec));
    out
}

/// How a saved entry reads: the item as a raw id, so one this build does not know is dropped
/// instead of failing the whole file.
#[derive(Deserialize)]
struct RawEntry {
    item: String,
    #[serde(default = "shown_by_default")]
    show: bool,
}

/// An entry saved without `show` is shown.
fn shown_by_default() -> bool {
    true
}

/// How a saved menu reads before it is normalized. A `report` level saved by an earlier build,
/// when the Report section was configurable, is ignored: the section is fixed now.
#[derive(Deserialize)]
struct RawMenu {
    #[serde(default)]
    keyboard: Option<Vec<Vec<RawEntry>>>,
}

impl<'de> Deserialize<'de> for BotMenu {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = RawMenu::deserialize(d)?;
        let keyboard = match raw.keyboard {
            None => Self::default().keyboard,
            Some(rows) => rows
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .filter_map(|entry| {
                            MenuItem::from_id(&entry.item).map(|item| MenuEntry {
                                item,
                                show: entry.show,
                            })
                        })
                        .collect()
                })
                .collect(),
        };
        Ok(Self { keyboard }.normalized())
    }
}

/// The view a report opens in when the chat did not pick one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ReportView {
    #[default]
    Exchanges,
    Cores,
    Days,
}

impl ReportView {
    /// Every view, in picker order.
    pub const ALL: [Self; 3] = [Self::Exchanges, Self::Cores, Self::Days];

    /// Stable saved id.
    pub fn id(self) -> &'static str {
        match self {
            Self::Exchanges => "exchanges",
            Self::Cores => "cores",
            Self::Days => "days",
        }
    }
}

/// Which timestamp a bot report's period applies to. The bot's own word for
/// [`crate::db::PeriodBasis`], which carries no saved form.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ReportBasis {
    /// By close time, as the terminal's Report by default.
    #[default]
    Close,
    /// By open time.
    Open,
}

impl ReportBasis {
    /// Every basis, in picker order.
    pub const ALL: [Self; 2] = [Self::Close, Self::Open];

    /// Stable saved id.
    pub fn id(self) -> &'static str {
        match self {
            Self::Close => "close",
            Self::Open => "open",
        }
    }

    /// The database filter's basis.
    pub fn period_basis(self) -> crate::db::PeriodBasis {
        match self {
            Self::Close => crate::db::PeriodBasis::CloseDate,
            Self::Open => crate::db::PeriodBasis::OpenDate,
        }
    }
}

/// Serialize and read a small closed set through its stable id; an id this build does not know
/// reads as the default.
macro_rules! id_serde {
    ($ty:ty) => {
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.id())
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let id = String::deserialize(d)?;
                Ok(Self::ALL
                    .into_iter()
                    .find(|value| value.id() == id)
                    .unwrap_or_default())
            }
        }
    };
}

id_serde!(ReportView);
id_serde!(ReportBasis);

/// The bot's own settings: its menu and how its reports read. Every field defaults, so a
/// configuration saved before they existed loads with the bot as it was.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BotSettings {
    /// The view a report opens in.
    #[serde(default)]
    pub report_view: ReportView,
    /// Which timestamp report periods apply to.
    #[serde(default)]
    pub period_basis: ReportBasis,
    /// The menu. Kept last: TOML writes a table after the plain values.
    #[serde(default)]
    pub menu: BotMenu,
}

#[cfg(test)]
mod tests;
