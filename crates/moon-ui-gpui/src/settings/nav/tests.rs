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

/// Text the sidebar's MoonUI items size as `text_sm` comes out at the body tier inside the
/// sidebar's rem scope: as wide as the same word measured at [`crate::design::t_body`], the size
/// the main window's lists draw with.
///
/// Breaks on: the sidebar losing its rem scope, or `nav_rem` aiming at the caption tier again —
/// the labels shrink back below the rest of the app.
#[gpui::test]
fn sidebar_item_text_draws_at_the_body_tier(cx: &mut gpui::TestAppContext) {
    use gpui::{InteractiveElement, IntoElement, ParentElement, Render, Styled, div};

    struct Probe;
    impl Render for Probe {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().flex().items_start().child(crate::design::rem_scope(
                super::nav_rem(cx),
                div()
                    .debug_selector(|| "nav-probe-label".into())
                    .font_family(crate::design::ui_font())
                    .text_sm()
                    .child(WORD),
            ))
        }
    }
    const WORD: &str = "Notifications";

    cx.update(moon_ui::init);
    let window = cx.add_window(|_, _| Probe);
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let drawn = visual
        .debug_bounds("nav-probe-label")
        .expect("probe label laid out");
    let body = visual.update(|_, cx| crate::design::ui_body_text_width(cx, WORD, 400.0));
    let drawn = f32::from(drawn.size.width);
    assert!(
        (drawn - body).abs() <= 1.0,
        "sidebar text is {drawn} px wide, the body tier draws it {body} px wide"
    );
}

/// No sidebar label truncates, in any language: the widest page name at the body tier plus the
/// item's chrome and the widest suffix fits the sidebar.
///
/// The chrome is MoonUI's, in rems: the sidebar's `px_3` on both sides, the item's `p_2` on both
/// sides, its `size_4` icon and the `gap_x_2` after it, then the suffix's own `gap_x_2`. The
/// suffix is the unsaved dot and a three-digit count beside it.
///
/// Breaks on: a longer translation, or a larger sidebar text tier, without the width growing.
#[gpui::test]
fn no_sidebar_label_truncates(cx: &mut gpui::TestAppContext) {
    use crate::design;
    use moon_core::config::Language;

    cx.update(moon_ui::init);
    cx.update(|cx| {
        let rem = f32::from(super::nav_rem(cx));
        let suffix = 0.5 * rem
            + design::ui_value(cx, 6.0) * 2.0
            + design::ui_text_width_zoomed(cx, "999", design::BODY_TEXT - 2.0, 400.0, true);
        let chrome = (1.5 + 1.0 + 1.0 + 0.5) * rem + 1.0;
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            let width = super::nav_w(cx);
            for tab in Tab::ALL {
                let title = tab.title().to_string();
                let need = design::ui_body_text_width(cx, &title, 400.0) + chrome + suffix;
                assert!(
                    need <= width,
                    "{title:?} needs {need} px of a {width} px sidebar ({})",
                    language.code()
                );
            }
        }
    });
}

/// The search field scales with the sidebar: its box is one item row tall and its text sits on
/// the body tier, not MoonInput's fixed small size.
#[gpui::test]
fn sidebar_search_field_matches_the_item_row(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    cx.update(|cx| {
        let row = f32::from(super::nav_rem(cx)) * super::ITEM_ROW_REMS;
        let moon_ui::MoonInputSize::Custom {
            height, font_size, ..
        } = super::nav_search_size(cx)
        else {
            panic!("the search field uses a custom size");
        };
        let drawn = crate::design::ui_value(cx, height);
        assert!(
            (drawn - row).abs() <= 0.5,
            "field {drawn} px, item row {row} px"
        );
        let text = crate::design::font_value(cx, font_size);
        let body = f32::from(crate::design::t_body(cx));
        assert!(
            (text - body).abs() <= 0.5,
            "field text {text} px, body {body} px"
        );
    });
}
