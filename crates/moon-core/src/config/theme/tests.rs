use super::*;
use crate::config::{HotkeysConfig, OrdersStyleSet};

/// Round trip: copied tab text pastes back exactly.
#[test]
fn share_roundtrip() {
    let mut set = ChartThemeSet::default();
    set.dark.bg = [1, 2, 3];
    set.light.grid = [7, 8, 9];
    let text = set.to_share_string().unwrap();
    let parsed = ChartThemeSet::parse_share(&text, &ChartThemeSet::default()).unwrap();
    assert_eq!(parsed, set);
}

/// Old flat theme.toml → dark (light remains current).
#[test]
fn share_flat_legacy_goes_dark() {
    let mut flat = ChartTheme::default();
    flat.bg = [10, 20, 30];
    let text = toml::to_string_pretty(&flat).unwrap();
    let mut current = ChartThemeSet::default();
    current.light.bg = [200, 200, 200];
    let parsed = ChartThemeSet::parse_share(&text, &current).unwrap();
    assert_eq!(parsed.dark, flat);
    assert_eq!(parsed.light, current.light);
}

/// Foreign files (orders.toml/hotkeys.toml/garbage) do NOT parse as a theme, and vice versa.
#[test]
fn share_rejects_foreign_files() {
    let orders = OrdersStyleSet::default().to_share_string().unwrap();
    let hotkeys = HotkeysConfig::default().to_share_string().unwrap();
    let theme = ChartThemeSet::default().to_share_string().unwrap();
    let cur_t = ChartThemeSet::default();
    let cur_o = OrdersStyleSet::default();

    assert!(ChartThemeSet::parse_share(&orders, &cur_t).is_none());
    assert!(ChartThemeSet::parse_share(&hotkeys, &cur_t).is_none());
    assert!(OrdersStyleSet::parse_share(&theme, &cur_o).is_none());
    assert!(OrdersStyleSet::parse_share(&hotkeys, &cur_o).is_none());
    assert!(HotkeysConfig::parse_share(&theme).is_none());
    assert!(HotkeysConfig::parse_share(&orders).is_none());
    assert!(ChartThemeSet::parse_share("не toml вовсе {", &cur_t).is_none());

    // Each parser accepts its own file.
    assert!(OrdersStyleSet::parse_share(&orders, &cur_o).is_some());
    assert!(HotkeysConfig::parse_share(&hotkeys).is_some());
}
