//! Filter and marker coverage for the Settings navigation model.

use super::{NavCount, NavCounts, nav_model, picker_label, tab_icon};
use crate::settings::{Tab, TabSet};

fn tabs(model: &super::NavModel) -> Vec<Tab> {
    model.items.iter().map(|item| item.tab).collect()
}

/// Catches a filter that matches only exact case or only titles: a mixed-case title fragment and
/// a mixed-case keyword must both find their page.
#[test]
fn filter_matches_title_substring_and_keyword_ignoring_case() {
    let counts = NavCounts::default();
    {
        let _locale = crate::test_locale::force("en");
        let by_title = nav_model("  thEME AND  ", TabSet::default(), &counts);
        assert_eq!(tabs(&by_title), vec![Tab::Appearance]);

        let by_keyword = nav_model("SQLite", TabSet::default(), &counts);
        assert_eq!(tabs(&by_keyword), vec![Tab::Storage]);
    }
    let _locale = crate::test_locale::force("ru");
    let ru = nav_model("ГОРЯЧ", TabSet::default(), &counts);
    assert_eq!(tabs(&ru), vec![Tab::Hotkeys]);
}

/// Catches a filter that falls back to every page when nothing matches; the sidebar then shows
/// its empty-state line instead of a list.
#[test]
fn unmatched_query_yields_no_items() {
    let _locale = crate::test_locale::force("en");
    let model = nav_model(
        "zzqqxx-no-such-setting",
        TabSet::default(),
        &NavCounts::default(),
    );
    assert!(model.items.is_empty());
}

/// Catches a page left out of the navigation: every one of the 17 pages is offered, and a search
/// names the page it is about.
#[test]
fn every_page_is_offered() {
    let _locale = crate::test_locale::force("en");
    let all = nav_model("", TabSet::default(), &NavCounts::default());
    for tab in Tab::ALL {
        assert!(tabs(&all).contains(&tab), "{tab:?} is not offered");
    }
    let title = Tab::StCores.title();
    let station = nav_model(&title, TabSet::default(), &NavCounts::default());
    assert!(tabs(&station).contains(&Tab::StCores));
}

/// Catches markers lost between the memo and the drawn entries: dirty pages keep their dot,
/// counts land on their own pages only, and a zero conflict count draws nothing.
#[test]
fn dirty_flags_and_counts_reach_their_pages() {
    let _locale = crate::test_locale::force("en");
    let mut dirty = TabSet::default();
    dirty.insert(Tab::Appearance);
    dirty.insert(Tab::Hotkeys);
    let counts = NavCounts {
        cores: 12,
        hotkey_conflicts: 3,
        station_diverging: None,
    };
    let model = nav_model("", dirty, &counts);
    for item in &model.items {
        assert_eq!(
            item.dirty,
            matches!(item.tab, Tab::Appearance | Tab::Hotkeys),
            "{:?}",
            item.tab
        );
        let want = match item.tab {
            Tab::Cores => Some(NavCount::Plain(12)),
            Tab::Hotkeys => Some(NavCount::Warn(3)),
            _ => None,
        };
        assert_eq!(item.count, want, "{:?}", item.tab);
    }
    let hotkeys = model.items.iter().find(|i| i.tab == Tab::Hotkeys).unwrap();
    assert!(picker_label(hotkeys).ends_with(" • (3)"));

    let quiet = nav_model("", TabSet::default(), &NavCounts::default());
    let hotkeys = quiet.items.iter().find(|i| i.tab == Tab::Hotkeys).unwrap();
    assert_eq!(hotkeys.count, None);
    assert!(!hotkeys.dirty);
}

/// Catches a sidebar icon path MoonUI does not ship, which draws an empty slot before the page
/// name: every page's icon must load from the app's asset source.
#[test]
fn every_page_icon_is_a_shipped_asset() {
    use gpui::AssetSource as _;
    for tab in Tab::ALL {
        let path = tab_icon(tab);
        assert!(
            matches!(moon_ui::MoonAssets.load(path), Ok(Some(_))),
            "{tab:?}: {path} is not in MoonUI's assets"
        );
    }
}
