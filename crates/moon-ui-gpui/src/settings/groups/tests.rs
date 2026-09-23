//! Strip order and sub-page memory. The expected pages are the Settings grouping itself.

use super::*;

/// The strip is five buttons, General last, and the three single pages have no inner switch.
#[test]
fn the_strip_is_five_buttons_with_general_last() {
    assert_eq!(
        TabGroup::ALL,
        [
            TabGroup::Connections,
            TabGroup::Telegram,
            TabGroup::Hotkeys,
            TabGroup::Interface,
            TabGroup::General,
        ]
    );
    assert_eq!(
        TabGroup::ALL.map(TabGroup::id),
        ["Подключения", "Telegram", "Хоткеи", "Интерфейс", "Общие"]
    );
    for group in [TabGroup::Connections, TabGroup::Telegram, TabGroup::Hotkeys] {
        assert!(!group.has_switch());
        assert_eq!(group.pages().len(), 1);
    }
}

/// Interface opens Interface, then Lines, then Badges. Lines is that group's second page.
#[test]
fn interface_opens_interface_lines_and_badges() {
    assert_eq!(
        TabGroup::Interface.pages(),
        &[Tab::Interface, Tab::Lines, Tab::Badges]
    );
    assert!(TabGroup::Interface.has_switch());
    assert_eq!(Tab::Lines.group(), TabGroup::Interface);
    assert_eq!(Tab::Badges.group(), TabGroup::Interface);
}

/// General opens General, then Storage, then Trade sounds.
#[test]
fn general_opens_general_storage_and_trade_sounds() {
    assert_eq!(
        TabGroup::General.pages(),
        &[Tab::General, Tab::Storage, Tab::TradeSounds]
    );
    assert!(TabGroup::General.has_switch());
    assert_eq!(Tab::Storage.group(), TabGroup::General);
    assert_eq!(Tab::TradeSounds.group(), TabGroup::General);
}

/// A window opened on Lines shows the Interface group on Lines and leaves General on its first page.
#[test]
fn opening_on_lines_selects_the_interface_group_and_the_lines_page() {
    let memory = SubpageMemory::for_initial(Tab::Lines);
    assert_eq!(memory.page(TabGroup::Interface), Tab::Lines);
    assert_eq!(memory.page(TabGroup::General), Tab::General);
    assert_eq!(memory.page(TabGroup::Connections), Tab::Connections);
}

/// A window opened on Trade sounds shows the General group there.
#[test]
fn opening_on_trade_sounds_selects_the_general_group() {
    let memory = SubpageMemory::for_initial(Tab::TradeSounds);
    assert_eq!(memory.page(TabGroup::General), Tab::TradeSounds);
    assert_eq!(memory.page(TabGroup::Interface), Tab::Interface);
}

/// Leaving a group and coming back restores the sub-page chosen while the window was open.
#[test]
fn a_subpage_is_remembered_until_the_window_picks_another() {
    let mut memory = SubpageMemory::for_initial(Tab::Connections);
    assert_eq!(memory.page(TabGroup::Interface), Tab::Interface);
    assert_eq!(memory.page(TabGroup::General), Tab::General);
    memory.remember(Tab::Badges);
    memory.remember(Tab::Storage);
    memory.remember(Tab::Hotkeys);
    assert_eq!(memory.page(TabGroup::Interface), Tab::Badges);
    assert_eq!(memory.page(TabGroup::General), Tab::Storage);
    assert_eq!(memory.page(TabGroup::Hotkeys), Tab::Hotkeys);
}

/// Every page belongs to exactly one strip button.
#[test]
fn every_page_belongs_to_one_group() {
    let pages = [
        Tab::Connections,
        Tab::Telegram,
        Tab::Hotkeys,
        Tab::Interface,
        Tab::Lines,
        Tab::Badges,
        Tab::General,
        Tab::Storage,
        Tab::TradeSounds,
    ];
    for page in pages {
        let owners: Vec<TabGroup> = TabGroup::ALL
            .into_iter()
            .filter(|group| group.pages().contains(&page))
            .collect();
        assert_eq!(owners, vec![page.group()]);
    }
}
