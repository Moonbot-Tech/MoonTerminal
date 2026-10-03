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

/// A configuration saved before the menu existed loads with the bot as it was.
#[test]
fn an_old_servers_file_loads_with_the_old_menu() {
    let file: ServersFile = toml::from_str(OLD_SERVERS_TOML).unwrap();
    assert_eq!(file.telegram.bot, BotSettings::default());
    assert_eq!(file.telegram.authorized_chat_ids, vec![7, 8]);
    assert_eq!(file.telegram.owner_chat_id, Some(7));
    assert!(file.telegram.mini_app_enabled);
    assert_eq!(file.telegram.chat_access[0].core_uids, vec![3]);
    let keyboard = file
        .telegram
        .bot
        .menu
        .visible(MenuLevel::Keyboard, |_| true);
    assert_eq!(
        keyboard,
        vec![
            vec![Today, Yesterday, Help],
            vec![Month, LastMonth],
            vec![Status],
        ]
    );
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
            report: vec![vec![MenuEntry::shown(Custom)]],
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
            "keyboard": [[{"item": "control"}, {"item": "today"}], [{"item": "help", "show": false}]]
        }
    }"#;
    let bot: BotSettings = serde_json::from_str(json).unwrap();
    assert_eq!(bot.report_view, ReportView::Exchanges);
    assert_eq!(bot.period_basis, ReportBasis::Open);
    assert_eq!(bot.menu.keyboard[0], vec![MenuEntry::shown(Today)]);
    assert_eq!(bot.menu.keyboard[1], vec![MenuEntry::hidden(Help)]);
    // A level the file did not carry keeps its default.
    assert_eq!(bot.menu.report, BotMenu::default().report);
    let back: BotSettings = serde_json::from_str(&serde_json::to_string(&bot).unwrap()).unwrap();
    assert_eq!(back, bot);
}

/// Normalizing keeps every allowed item exactly once and nothing the level does not allow.
#[test]
fn normalizing_repairs_a_level() {
    let wide: Vec<MenuEntry> = MenuItem::ALL.into_iter().map(MenuEntry::shown).collect();
    let menu = BotMenu {
        keyboard: vec![Vec::new(), wide, vec![MenuEntry::hidden(Today)]],
        report: vec![vec![
            MenuEntry::shown(Report),
            MenuEntry::shown(Month),
            MenuEntry::shown(Month),
        ]],
    }
    .normalized();
    assert_eq!(
        menu.keyboard.len(),
        2,
        "empty row dropped, ten split as 8 + 2"
    );
    assert_eq!(menu.keyboard[0].len(), MAX_ROW);
    assert_eq!(menu.keyboard[1].len(), 2);
    assert!(
        menu.keyboard.iter().flatten().all(|e| e.show),
        "the repeated hidden Today is dropped, the first shown one stays"
    );
    assert_eq!(menu.report[0], vec![MenuEntry::shown(Month)]);
    let rest: Vec<MenuItem> = menu.report[1..].iter().flatten().map(|e| e.item).collect();
    assert_eq!(rest, vec![Today, Yesterday, LastMonth, Daily, Custom]);
    assert!(menu.report[1..].iter().flatten().all(|e| !e.show));
    for level in [MenuLevel::Keyboard, MenuLevel::Report] {
        let mut items: Vec<MenuItem> = menu.rows(level).iter().flatten().map(|e| e.item).collect();
        items.sort_by_key(|item| item.id());
        let mut allowed = level.allowed().to_vec();
        allowed.sort_by_key(|item| item.id());
        assert_eq!(items, allowed);
    }
}

/// Every id reads back as its item, and no two items share one.
#[test]
fn ids_are_stable_and_distinct() {
    for item in MenuItem::ALL {
        assert_eq!(MenuItem::from_id(item.id()), Some(item));
    }
    assert_eq!(MenuItem::from_id("control"), None);
    assert_eq!(LastMonth.id(), "lastmonth");
    assert_eq!(MiniApp.id(), "miniapp");
}

/// The items of a level, row by row.
fn layout(menu: &BotMenu, level: MenuLevel) -> Vec<Vec<MenuItem>> {
    menu.rows(level)
        .iter()
        .map(|row| row.iter().map(|e| e.item).collect())
        .collect()
}

/// Moving swaps neighbours and crosses a row edge without moving the edge; the ends stay put.
#[test]
fn moving_swaps_neighbours_across_rows() {
    let mut menu = BotMenu::default();
    let level = MenuLevel::Report;
    // [Today, Yesterday] [Month, LastMonth] [Daily, Custom]
    assert!(menu.move_item(level, Month, true));
    assert_eq!(
        layout(&menu, level),
        vec![
            vec![Today, Month],
            vec![Yesterday, LastMonth],
            vec![Daily, Custom]
        ]
    );
    assert!(!menu.move_item(level, Today, true));
    assert!(!menu.move_item(level, Custom, false));
    assert!(menu.move_item(level, Today, false));
    assert_eq!(layout(&menu, level)[0], vec![Month, Today]);
}

/// A row starts and joins at an item; the first item always starts one, and no row is empty.
#[test]
fn row_starts_split_and_join() {
    let mut menu = BotMenu::default();
    let level = MenuLevel::Report;
    assert!(menu.set_row_start(level, Yesterday, true));
    assert_eq!(
        layout(&menu, level),
        vec![
            vec![Today],
            vec![Yesterday],
            vec![Month, LastMonth],
            vec![Daily, Custom]
        ]
    );
    assert!(menu.set_row_start(level, Month, false));
    assert!(menu.set_row_start(level, Daily, false));
    assert_eq!(
        layout(&menu, level),
        vec![
            vec![Today],
            vec![Yesterday, Month, LastMonth, Daily, Custom]
        ]
    );
    assert!(!menu.set_row_start(level, Today, false));
    assert!(!menu.set_row_start(level, Month, false), "already joined");
    assert!(menu.rows(level).iter().all(|row| !row.is_empty()));
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
    assert!(menu.set_shown(MenuLevel::Keyboard, Report, true));
    assert!(!menu.set_shown(MenuLevel::Keyboard, Report, true));
    assert_eq!(
        menu.visible(MenuLevel::Keyboard, |_| true).last(),
        Some(&vec![Report])
    );
}

/// Joining a button onto a full row changes nothing, and says so: the tick does not lie.
#[test]
fn joining_a_full_row_is_no_change() {
    let mut menu = BotMenu::default();
    let level = MenuLevel::Keyboard;
    for item in MenuItem::ALL {
        menu.set_row_start(level, item, false);
    }
    // Ten buttons: a full row of eight and two more that cannot join it.
    assert_eq!(menu.rows(level).len(), 2);
    let ninth = menu.rows(level)[1][0].item;
    let before = menu.clone();
    assert!(!menu.set_row_start(level, ninth, false));
    assert_eq!(menu, before);
}
