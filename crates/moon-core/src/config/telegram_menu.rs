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
    MiniApp,
    /// The Report section: an inline menu of periods under one message.
    Report,
}

impl MenuItem {
    /// Every item, in the order a picker lists them.
    pub const ALL: [Self; 10] = [
        Self::Report,
        Self::Today,
        Self::Yesterday,
        Self::Month,
        Self::LastMonth,
        Self::Daily,
        Self::Custom,
        Self::Help,
        Self::Status,
        Self::MiniApp,
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
            Self::MiniApp => "miniapp",
            Self::Report => "report",
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

/// Which level of the menu a row list is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuLevel {
    /// The persistent reply keyboard under the chat.
    Keyboard,
    /// The Report section's inline menu.
    Report,
}

impl MenuLevel {
    /// The items this level may hold, in default picker order.
    pub fn allowed(self) -> &'static [MenuItem] {
        match self {
            Self::Keyboard => &MenuItem::ALL,
            Self::Report => &[
                MenuItem::Today,
                MenuItem::Yesterday,
                MenuItem::Month,
                MenuItem::LastMonth,
                MenuItem::Daily,
                MenuItem::Custom,
            ],
        }
    }
}

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

/// The bot's menu: the reply keyboard and the Report section, each as rows of buttons.
///
/// Every level always lists each of its allowed items exactly once, hidden ones included, so a
/// settings editor shows the whole set and a renderer only filters ([`Self::visible`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct BotMenu {
    pub keyboard: Vec<Vec<MenuEntry>>,
    pub report: Vec<Vec<MenuEntry>>,
}

impl Default for BotMenu {
    /// The layout the bot had before the menu became configurable: periods and Help, and the
    /// station's Status on a row of its own; everything new hidden.
    fn default() -> Self {
        use MenuEntry as E;
        use MenuItem::*;
        Self {
            keyboard: vec![
                vec![E::shown(Today), E::shown(Yesterday), E::shown(Help)],
                vec![E::shown(Month), E::shown(LastMonth)],
                vec![E::shown(Status)],
                vec![
                    E::hidden(Report),
                    E::hidden(Daily),
                    E::hidden(Custom),
                    E::hidden(MiniApp),
                ],
            ],
            report: vec![
                vec![E::shown(Today), E::shown(Yesterday)],
                vec![E::shown(Month), E::shown(LastMonth)],
                vec![E::shown(Daily), E::shown(Custom)],
            ],
        }
    }
}

impl BotMenu {
    /// The rows of `level`.
    pub fn rows(&self, level: MenuLevel) -> &[Vec<MenuEntry>] {
        match level {
            MenuLevel::Keyboard => &self.keyboard,
            MenuLevel::Report => &self.report,
        }
    }

    /// The rows of `level`, for an editor.
    pub fn rows_mut(&mut self, level: MenuLevel) -> &mut Vec<Vec<MenuEntry>> {
        match level {
            MenuLevel::Keyboard => &mut self.keyboard,
            MenuLevel::Report => &mut self.report,
        }
    }

    /// The shown items of `level` that `keep` admits, row by row; rows left empty are dropped.
    ///
    /// Args:
    ///     keep: The caller's filter — what the chat's role and the host allow.
    pub fn visible(&self, level: MenuLevel, keep: impl Fn(MenuItem) -> bool) -> Vec<Vec<MenuItem>> {
        self.rows(level)
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

    /// The entries of `level` in order, each with whether it starts a row: the shape an editor
    /// works on, where rows can never be left empty.
    pub fn flat(&self, level: MenuLevel) -> Vec<(MenuEntry, bool)> {
        self.rows(level)
            .iter()
            .flat_map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(index, entry)| (*entry, index == 0))
            })
            .collect()
    }

    /// Replace `level`'s rows with `flat`, cut where an entry starts a row (the first always does)
    /// and where a row is full ([`MAX_ROW`]).
    ///
    /// Returns:
    ///     Whether the rows changed: joining an entry onto a full row does not.
    fn set_flat(&mut self, level: MenuLevel, flat: Vec<(MenuEntry, bool)>) -> bool {
        let mut rows: Vec<Vec<MenuEntry>> = Vec::new();
        for (entry, starts) in flat {
            match rows.last_mut() {
                Some(row) if !starts && row.len() < MAX_ROW => row.push(entry),
                _ => rows.push(vec![entry]),
            }
        }
        let changed = *self.rows(level) != rows;
        *self.rows_mut(level) = rows;
        changed
    }

    /// Move `item` one place earlier (`up`) or later on `level`: within its row, or across the
    /// edge into the next row, which keeps its place.
    ///
    /// Returns:
    ///     Whether anything moved; the first entry cannot go up nor the last one down.
    pub fn move_item(&mut self, level: MenuLevel, item: MenuItem, up: bool) -> bool {
        let mut flat = self.flat(level);
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
        self.set_flat(level, flat)
    }

    /// Start a new row at `item` (`starts`), or join it to the row before.
    ///
    /// Returns:
    ///     Whether anything changed; the first entry always starts a row.
    pub fn set_row_start(&mut self, level: MenuLevel, item: MenuItem, starts: bool) -> bool {
        let mut flat = self.flat(level);
        let Some(at) = flat.iter().position(|(entry, _)| entry.item == item) else {
            return false;
        };
        if at == 0 || flat[at].1 == starts {
            return false;
        }
        flat[at].1 = starts;
        self.set_flat(level, flat)
    }

    /// Show or hide `item` on `level`.
    ///
    /// Returns:
    ///     Whether anything changed.
    pub fn set_shown(&mut self, level: MenuLevel, item: MenuItem, show: bool) -> bool {
        let entry = self
            .rows_mut(level)
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

    /// This menu made valid: on every level, items the level does not allow and repeats are
    /// dropped, rows wider than [`MAX_ROW`] are split, empty rows are removed, and each allowed
    /// item missing from the level is appended, hidden, on a row of its own kind at the end.
    pub fn normalized(mut self) -> Self {
        for level in [MenuLevel::Keyboard, MenuLevel::Report] {
            let rows = std::mem::take(self.rows_mut(level));
            *self.rows_mut(level) = normalize_level(rows, level);
        }
        self
    }
}

/// See [`BotMenu::normalized`].
fn normalize_level(rows: Vec<Vec<MenuEntry>>, level: MenuLevel) -> Vec<Vec<MenuEntry>> {
    let allowed = level.allowed();
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
        .map(|&item| MenuEntry::hidden(item))
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

/// How a saved menu reads before it is normalized.
#[derive(Deserialize)]
struct RawMenu {
    #[serde(default)]
    keyboard: Option<Vec<Vec<RawEntry>>>,
    #[serde(default)]
    report: Option<Vec<Vec<RawEntry>>>,
}

impl<'de> Deserialize<'de> for BotMenu {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = RawMenu::deserialize(d)?;
        let defaults = Self::default();
        let level = |rows: Option<Vec<Vec<RawEntry>>>, fallback: Vec<Vec<MenuEntry>>| match rows {
            None => fallback,
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
        Ok(Self {
            keyboard: level(raw.keyboard, defaults.keyboard),
            report: level(raw.report, defaults.report),
        }
        .normalized())
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
