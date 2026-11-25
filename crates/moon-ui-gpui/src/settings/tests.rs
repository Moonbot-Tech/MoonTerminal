//! Regression coverage for the Settings save indicator and page navigation.

use super::draft_dirty;
use moon_core::config::AppConfig;

/// Stable navigation order, including Station AI access after its maintenance page.
const MOCKUP_IDS: [&str; 18] = [
    "cores",
    "tg-bot",
    "tg-notify",
    "tg-msg",
    "tg-core",
    "st-over",
    "st-cores",
    "st-tape",
    "st-maint",
    "st-mcp",
    "appearance",
    "lines",
    "badges",
    "hotkeys",
    "general",
    "security",
    "storage",
    "sounds",
];

/// Catches a renamed, duplicated or reordered page id; element ids and the mockup key off them.
#[test]
fn page_ids_are_unique_ascii_and_match_the_mockup() {
    let ids: Vec<&str> = super::Tab::ALL.iter().map(|t| t.id()).collect();
    assert_eq!(ids, MOCKUP_IDS);
    for id in &ids {
        assert!(id.is_ascii(), "{id}");
        assert_eq!(ids.iter().filter(|other| *other == id).count(), 1, "{id}");
    }
}

/// Catches a missing locale key: rust-i18n falls back to the raw key, which would show up in the
/// navigation as `settings.page.x.title`.
#[test]
fn every_page_and_group_has_text_in_every_language() {
    for lang in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(lang.code());
        for tab in super::Tab::ALL {
            for (what, text) in [
                ("title", tab.title()),
                ("desc", tab.desc()),
                ("keywords", tab.keywords()),
            ] {
                assert!(
                    !text.trim().is_empty() && !text.starts_with("settings."),
                    "{}: {tab:?} {what} = {text:?}",
                    lang.code()
                );
            }
            assert_eq!(tab.keywords(), tab.keywords().to_lowercase(), "{tab:?}");
        }
        for group in super::NavGroup::ALL {
            let title = group.title();
            assert!(
                !title.trim().is_empty() && !title.starts_with("settings."),
                "{}: {group:?} = {title:?}",
                lang.code()
            );
        }
    }
}

/// Catches a field hashed under the wrong page: a theme colour edit must dot Appearance alone.
#[test]
fn theme_colour_edit_dirties_only_appearance() {
    let saved = AppConfig::load(None, false).expect("test config");
    let mut draft = saved.clone();
    draft.theme.dark.bg[0] = draft.theme.dark.bg[0].wrapping_add(1);
    let pages = super::dirty_pages(&saved, &draft);
    assert!(draft_dirty(&saved, &draft));
    assert_eq!(
        pages.iter().collect::<Vec<_>>(),
        vec![super::Tab::Appearance]
    );
}

/// Catches the bot token falling outside TgBot, or a token edit leaking to other pages.
#[test]
fn token_edit_dirties_only_the_bot_page() {
    let saved = AppConfig::load(None, false).expect("test config");
    let mut draft = saved.clone();
    draft.telegram.token = moon_core::config::Secret::new("123:synthetic-token");
    assert!(draft_dirty(&saved, &draft));
    let pages = super::dirty_pages(&saved, &draft);
    assert_eq!(pages.iter().collect::<Vec<_>>(), vec![super::Tab::TgBot]);
}

/// Catches page signatures disagreeing with the whole-draft signature: some page is dirty exactly
/// when the draft is, for edits to page-owned fields and for no edit at all.
#[test]
fn page_dirt_appears_exactly_when_the_draft_is_dirty() {
    let saved = AppConfig::load(None, false).expect("test config");
    let clean = saved.clone();
    assert!(!draft_dirty(&saved, &clean));
    assert_eq!(super::dirty_pages(&saved, &clean).len(), 0);

    let mut edits: Vec<AppConfig> = Vec::new();
    let mut d = saved.clone();
    d.log_to_file = !d.log_to_file;
    edits.push(d);
    let mut d = saved.clone();
    d.telegram.mini_app_enabled = !d.telegram.mini_app_enabled;
    edits.push(d);
    let mut d = saved.clone();
    d.ui_scale += 0.25;
    edits.push(d);
    // The quit question (#1170): left out of either signature, a ticked box would never reach
    // Save, or would mark no page as unsaved.
    let mut d = saved.clone();
    d.confirm_quit = !d.confirm_quit;
    edits.push(d);
    for draft in edits {
        assert!(draft_dirty(&saved, &draft));
        assert_eq!(super::dirty_pages(&saved, &draft).len(), 1);
    }
}

/// MoonBot import has ONE builder, `nav.rs:import_entry`: the sidebar footer, or the Main page
/// header only while the narrow window hides the sidebar — never both on screen. Counts the files
/// building a control from the import caption across the Settings sources.
#[test]
fn moonbot_import_has_one_entry() {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("settings sources") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs")
                && path.file_name().is_some_and(|n| n != "tests.rs")
            {
                let text = std::fs::read_to_string(&path).expect("source");
                out.push((path.display().to_string(), text));
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/settings");
    let mut files = Vec::new();
    walk(&root, &mut files);
    let sites: Vec<&str> = files
        .iter()
        .filter(|(_, text)| text.contains("t!(\"import.button\")"))
        .map(|(path, _)| path.as_str())
        .collect();
    assert_eq!(
        sites.len(),
        1,
        "MoonBot import is built in {sites:?}; only the sidebar footer may"
    );
    assert!(
        sites[0].ends_with("nav.rs"),
        "the one import entry is {sites:?}"
    );
}

use std::rc::Rc;

use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, VisualTestContext,
    Window, div, px,
};
use moon_ui::MoonInputState;

use super::nav::{NavCounts, nav_model, nav_sidebar};
use super::{Tab, TabSet};

/// Headless Settings navigation fixture used when no Backend entity is available.
struct SettingsNavBench {
    query: Entity<MoonInputState>,
    active: Tab,
    counts: NavCounts,
}

impl Render for SettingsNavBench {
    /// Draw the production Settings navigation model and sidebar.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = nav_model("", TabSet::default(), &self.counts);
        assert_eq!(model.items.len(), Tab::ALL.len());
        div().size_full().child(nav_sidebar(
            &model,
            self.active,
            &self.query,
            Rc::new(|_, _, _| {}),
            Rc::new(|_, _| {}),
            cx,
        ))
    }
}

/// Build and draw one navigation fixture, returning the number of visited pages.
fn draw_settings_nav(cx: &mut gpui::TestAppContext, config: &AppConfig) -> VisualTestContext {
    let counts = NavCounts {
        cores: config.servers.len() as u32,
        ..NavCounts::default()
    };
    let window = cx.open_window(gpui::size(px(860.0), px(620.0)), |window, cx| {
        SettingsNavBench {
            query: cx.new(|cx| MoonInputState::new(window, cx)),
            active: Tab::Cores,
            counts,
        }
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| {
        let _ = window.draw(cx);
    });
    visual
}

/// Time config draft construction, icon discovery, and the first production navigation draw.
#[gpui::test]
#[ignore = "bench: S-smooth"]
fn bench_s_settings_open(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    cx.update(moon_ui::init);
    let config = crate::test_support::fixtures::app_config(20);
    crate::test_support::bench::measure(
        "bench_s_settings_open stimulus=cold_open_first_draw servers=20 partial_(no_Backend_wiring)",
        3,
        || {
            let draft = super::apply::reverted(&config);
            assert_eq!(draft.servers.len(), 20);
            let icons = super::IconSet::discover();
            let mut visual = draw_settings_nav(cx, &draft);
            visual.update(|window, _| window.remove_window());
            (draft, icons)
        },
    );
}

/// Time a warm visit to every Settings page through the production navigation model and draw.
#[gpui::test]
#[ignore = "bench: S-smooth"]
fn bench_s_settings_page_switch_all(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    cx.update(moon_ui::init);
    let config = crate::test_support::fixtures::app_config(20);
    let window = cx.open_window(gpui::size(px(860.0), px(620.0)), |window, cx| {
        SettingsNavBench {
            query: cx.new(|cx| MoonInputState::new(window, cx)),
            active: Tab::Cores,
            counts: NavCounts {
                cores: 20,
                ..NavCounts::default()
            },
        }
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    crate::test_support::bench::measure(
        "bench_s_settings_page_switch_all stimulus=warm_all_pages pages=18 partial_(navigation_only_no_Backend)",
        3,
        || {
            for tab in Tab::ALL {
                window
                    .update(&mut visual, |view, _, cx| {
                        view.active = tab;
                        cx.notify();
                    })
                    .expect("Settings navigation fixture");
                visual.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                assert_eq!(super::dirty_pages(&config, &config).len(), 0);
            }
            Tab::ALL.len()
        },
    );
    visual.update(|window, _| window.remove_window());
}
