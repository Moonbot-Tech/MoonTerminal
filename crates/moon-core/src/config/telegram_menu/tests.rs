use super::*;
use crate::config::schema::ServersFile;
use MenuItem::*;

/// `servers.enc` exactly as a build before the menu wrote it.
const OLD_SERVERS_TOML: &str = r#"
[[servers]]
uid = 3
name = "core"
key = "k"

[telegram]
token = "t"
authorized_chat_ids = [7, 8]
owner_chat_id = 7
mini_app_enabled = true

[[telegram.chat_access]]
chat_id = 8
name = "viewer"
core_uids = [3]
"#;

/// A configuration saved before the menu existed loads with the bot as it was: the owner's
/// Control, which came later, stays hidden until shown.
#[test]
fn an_old_servers_file_loads_with_the_old_menu() {
    let file: ServersFile = toml::from_str(OLD_SERVERS_TOML).unwrap();
    assert_eq!(file.telegram.bot, BotSettings::default());
    assert_eq!(file.telegram.authorized_chat_ids, vec![7, 8]);
    assert_eq!(file.telegram.owner_chat_id, Some(7));
    assert!(file.telegram.mini_app_enabled);
    assert_eq!(file.telegram.chat_access[0].core_uids, vec![3]);
    let keyboard = file.telegram.bot.menu.visible(|_| true);
    assert_eq!(
        keyboard,
        vec![
            vec![Today, Yesterday, Help],
            vec![Month, LastMonth],
            vec![Status, Settings],
        ]
    );
    assert!(!file.telegram.bot.menu.shows(Control));
}

/// A configured bot survives the TOML round trip `servers.enc` takes.
#[test]
fn settings_survive_the_servers_file_round_trip() {
    let mut file: ServersFile = toml::from_str(OLD_SERVERS_TOML).unwrap();
    file.telegram.bot = BotSettings {
        report_view: ReportView::Days,
        period_basis: ReportBasis::Open,
        menu: BotMenu {
            keyboard: vec![
                vec![MenuEntry::shown(Report), MenuEntry::shown(Help)],
                vec![MenuEntry::hidden(Today)],
            ],
        }
        .normalized(),
    };
    let text = toml::to_string(&file).unwrap();
    let back: ServersFile = toml::from_str(&text).unwrap();
    assert_eq!(back.telegram.bot, file.telegram.bot);
    assert_eq!(back.telegram.authorized_chat_ids, vec![7, 8]);
}

/// What a newer build may have written does not fail a load: an unknown item is dropped, an
/// unknown view reads as the default, and an entry without `show` is shown.
#[test]
fn unknown_ids_degrade_instead_of_failing() {
    let json = r#"{
        "report_view": "weekly",
        "period_basis": "open",
        "future_field": 1,
        "menu": {
            "keyboard": [[{"item": "future_item"}, {"item": "today"}], [{"item": "help", "show": false}]]
        }
    }"#;
    let bot: BotSettings = serde_json::from_str(json).unwrap();
    assert_eq!(bot.report_view, ReportView::Exchanges);
    assert_eq!(bot.period_basis, ReportBasis::Open);
    assert_eq!(bot.menu.keyboard[0], vec![MenuEntry::shown(Today)]);
    assert_eq!(bot.menu.keyboard[1], vec![MenuEntry::hidden(Help)]);
    let back: BotSettings = serde_json::from_str(&serde_json::to_string(&bot).unwrap()).unwrap();
    assert_eq!(back, bot);
}

/// A menu saved while the Report section was configurable and the keyboard had a Mini App button
/// loads: the section's own rows are ignored, the Mini App entry is dropped, the rest stays.
#[test]
fn a_menu_with_the_retired_report_level_and_mini_app_loads() {
    let json = r#"{
        "menu": {
            "keyboard": [
                [{"item": "today", "show": true}, {"item": "miniapp", "show": true}],
                [{"item": "help", "show": false}]
            ],
            "report": [[{"item": "custom", "show": false}], [{"item": "today", "show": true}]]
        }
    }"#;
    let bot: BotSettings = serde_json::from_str(json).unwrap();
    assert_eq!(bot.menu.keyboard[0], vec![MenuEntry::shown(Today)]);
    assert_eq!(bot.menu.keyboard[1], vec![MenuEntry::hidden(Help)]);
    assert_eq!(
        bot.menu.keyboard.iter().flatten().count(),
        MenuItem::ALL.len()
    );
    let text = serde_json::to_string(&bot).unwrap();
    assert!(!text.contains("\"report\":"), "{text}");
    assert!(!text.contains("miniapp"), "{text}");
}

/// The Report section is fixed: every period once, in three rows.
#[test]
fn the_report_section_lists_every_period_once() {
    let items: Vec<MenuItem> = REPORT_SECTION
        .iter()
        .flat_map(|row| row.iter().copied())
        .collect();
    assert_eq!(
        items,
        vec![Today, Yesterday, Month, LastMonth, Daily, Custom]
    );
}

/// Normalizing keeps every item exactly once.
#[test]
fn normalizing_repairs_the_keyboard() {
    let wide: Vec<MenuEntry> = MenuItem::ALL.into_iter().map(MenuEntry::shown).collect();
    let menu = BotMenu {
        keyboard: vec![Vec::new(), wide, vec![MenuEntry::hidden(Today)]],
    }
    .normalized();
    assert_eq!(
        menu.keyboard.len(),
        2,
        "empty row dropped, eleven split as 8 + 3"
    );
    assert_eq!(menu.keyboard[0].len(), MAX_ROW);
    assert_eq!(menu.keyboard[1].len(), 3);
    assert!(
        menu.keyboard.iter().flatten().all(|e| e.show),
        "the repeated hidden Today is dropped, the first shown one stays"
    );
    let mut items: Vec<MenuItem> = menu.keyboard.iter().flatten().map(|e| e.item).collect();
    items.sort_by_key(|item| item.id());
    let mut all = MenuItem::ALL.to_vec();
    all.sort_by_key(|item| item.id());
    assert_eq!(items, all);
}

/// Every id reads back as its item, and no two items share one.
#[test]
fn ids_are_stable_and_distinct() {
    for item in MenuItem::ALL {
        assert_eq!(MenuItem::from_id(item.id()), Some(item));
    }
    assert_eq!(MenuItem::from_id("future_item"), None);
    assert_eq!(Control.id(), "control");
    assert_eq!(MenuItem::from_id("miniapp"), None);
    assert_eq!(LastMonth.id(), "lastmonth");
}

/// The keyboard's items, row by row.
fn layout(menu: &BotMenu) -> Vec<Vec<MenuItem>> {
    menu.keyboard
        .iter()
        .map(|row| row.iter().map(|e| e.item).collect())
        .collect()
}

/// Moving swaps neighbours and crosses a row edge without moving the edge; the ends stay put.
#[test]
fn moving_swaps_neighbours_across_rows() {
    let mut menu = BotMenu::default();
    // [Today, Yesterday, Help] [Month, LastMonth] [Status, Settings] [Report, Daily, Custom]
    assert!(menu.move_item(Month, true));
    assert_eq!(
        layout(&menu)[..2],
        [vec![Today, Yesterday, Month], vec![Help, LastMonth]]
    );
    assert!(!menu.move_item(Today, true));
    assert!(!menu.move_item(Custom, false));
    assert!(menu.move_item(Today, false));
    assert_eq!(layout(&menu)[0], vec![Yesterday, Today, Month]);
}

/// A row starts and joins at an item; the first item always starts one, and no row is empty.
#[test]
fn row_starts_split_and_join() {
    let mut menu = BotMenu::default();
    assert!(menu.set_row_start(Yesterday, true));
    assert_eq!(
        layout(&menu)[..3],
        [vec![Today], vec![Yesterday, Help], vec![Month, LastMonth]]
    );
    assert!(menu.set_row_start(Month, false));
    assert_eq!(layout(&menu)[1], vec![Yesterday, Help, Month, LastMonth]);
    assert!(!menu.set_row_start(Today, false));
    assert!(!menu.set_row_start(Month, false), "already joined");
    assert!(menu.keyboard.iter().all(|row| !row.is_empty()));
    assert_eq!(
        menu.clone().normalized(),
        menu,
        "an edited menu is already valid"
    );
}

/// Showing and hiding flips one entry and reports a real change only.
#[test]
fn showing_flips_one_entry() {
    let mut menu = BotMenu::default();
    assert!(menu.set_shown(Report, true));
    assert!(!menu.set_shown(Report, true));
    assert_eq!(menu.visible(|_| true).last(), Some(&vec![Report]));
}

/// Joining a button onto a full row changes nothing, and says so: the tick does not lie.
#[test]
fn joining_a_full_row_is_no_change() {
    let mut menu = BotMenu::default();
    for item in MenuItem::ALL {
        menu.set_row_start(item, false);
    }
    // Ten buttons: a full row of eight and two more that cannot join it.
    assert_eq!(menu.keyboard.len(), 2);
    let ninth = menu.keyboard[1][0].item;
    let before = menu.clone();
    assert!(!menu.set_row_start(ninth, false));
    assert_eq!(menu, before);
}

/// A saved menu from before an item existed gets it as the default layout has it: Settings
/// shown, an item hidden by default hidden.
#[test]
fn a_new_item_takes_its_default_visibility() {
    let menu = BotMenu {
        keyboard: vec![vec![MenuEntry::shown(Today)]],
    }
    .normalized();
    let entry = |item| {
        *menu
            .keyboard
            .iter()
            .flatten()
            .find(|e| e.item == item)
            .unwrap()
    };
    assert!(entry(Settings).show);
    assert!(entry(Status).show);
    assert!(!entry(Report).show);
}
