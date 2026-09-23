//! Top buttons of the Settings window, and which page each one opens.
//!
//! Five buttons are visible. Interface and General each remember one sub-page for as long as
//! the window stays open; that memory is not written to config.

use super::Tab;

/// One button on the Settings tab strip, in strip order.
///
/// General is last on purpose: the strip reads Connections, Telegram, Hotkeys, Interface,
/// then General.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::settings) enum TabGroup {
    Connections,
    Telegram,
    Hotkeys,
    Interface,
    General,
}

impl TabGroup {
    /// Strip order. General is the last button.
    pub(in crate::settings) const ALL: [TabGroup; 5] = [
        Self::Connections,
        Self::Telegram,
        Self::Hotkeys,
        Self::Interface,
        Self::General,
    ];

    const INTERFACE_PAGES: [Tab; 3] = [Tab::Interface, Tab::Lines, Tab::Badges];
    const GENERAL_PAGES: [Tab; 3] = [Tab::General, Tab::Storage, Tab::TradeSounds];

    /// Pages this button can show, in inner-switch order.
    ///
    /// A button with one page has no inner switch. The first page is the default.
    ///
    /// Returns:
    ///     The pages, with the default first.
    pub(in crate::settings) fn pages(self) -> &'static [Tab] {
        match self {
            Self::Connections => &[Tab::Connections],
            Self::Telegram => &[Tab::Telegram],
            Self::Hotkeys => &[Tab::Hotkeys],
            Self::Interface => &Self::INTERFACE_PAGES,
            Self::General => &Self::GENERAL_PAGES,
        }
    }

    /// Whether this button draws the inner switch under the strip.
    ///
    /// Returns:
    ///     `true` for Interface and General.
    pub(in crate::settings) fn has_switch(self) -> bool {
        self.pages().len() > 1
    }

    /// Stable button id, shared with the lead page so an existing control keeps its identity.
    ///
    /// Returns:
    ///     The untranslated id used by `MoonButton::new`.
    pub(in crate::settings) fn id(self) -> &'static str {
        self.pages()[0].id()
    }

    /// Localized caption of the lead page. Group buttons reuse those existing labels.
    ///
    /// Returns:
    ///     The caption drawn on the strip.
    pub(in crate::settings) fn title(self) -> String {
        self.pages()[0].title()
    }

    /// Element id of this group's inner switch.
    ///
    /// Returns:
    ///     A stable id. Single-page groups do not render a switch, so their id is unused.
    pub(in crate::settings) fn segment_id(self) -> &'static str {
        match self {
            Self::Interface => "settings-interface-segments",
            Self::General => "settings-general-segments",
            Self::Connections | Self::Telegram | Self::Hotkeys => "settings-segments",
        }
    }
}

impl Tab {
    /// Group whose button shows this page.
    ///
    /// Returns:
    ///     The strip button that owns the page.
    pub(in crate::settings) fn group(self) -> TabGroup {
        match self {
            Tab::Connections => TabGroup::Connections,
            Tab::Telegram => TabGroup::Telegram,
            Tab::Hotkeys => TabGroup::Hotkeys,
            Tab::Interface | Tab::Lines | Tab::Badges => TabGroup::Interface,
            Tab::General | Tab::Storage | Tab::TradeSounds => TabGroup::General,
        }
    }
}

/// Last Interface and General sub-page chosen while this Settings window is open.
///
/// Not persisted. Opening the window on a grouped page selects that page; every other group
/// starts on its first page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::settings) struct SubpageMemory {
    interface: Tab,
    general: Tab,
}

impl SubpageMemory {
    /// Memory for a window created on `page`.
    ///
    /// Args:
    ///     page: The page `open_on_tab` asked to show.
    ///
    /// Returns:
    ///     Memory whose grouped slot matches `page` when `page` is grouped, and whose other
    ///     slot is the group's first page.
    pub(in crate::settings) fn for_initial(page: Tab) -> Self {
        let mut memory = Self {
            interface: Tab::Interface,
            general: Tab::General,
        };
        memory.remember(page);
        memory
    }

    /// Page to show when `group`'s button is pressed.
    ///
    /// Args:
    ///     group: The strip button that was pressed.
    ///
    /// Returns:
    ///     The remembered sub-page, or the group's only page.
    pub(in crate::settings) fn page(self, group: TabGroup) -> Tab {
        match group {
            TabGroup::Connections => Tab::Connections,
            TabGroup::Telegram => Tab::Telegram,
            TabGroup::Hotkeys => Tab::Hotkeys,
            TabGroup::Interface => self.interface,
            TabGroup::General => self.general,
        }
    }

    /// Remember `page` when it belongs to Interface or General.
    ///
    /// A page from any other group leaves both slots as they are.
    ///
    /// Args:
    ///     page: The page now on screen.
    pub(in crate::settings) fn remember(&mut self, page: Tab) {
        match page {
            Tab::Interface | Tab::Lines | Tab::Badges => self.interface = page,
            Tab::General | Tab::Storage | Tab::TradeSounds => self.general = page,
            Tab::Connections | Tab::Telegram | Tab::Hotkeys => {}
        }
    }
}

#[cfg(test)]
mod tests;
