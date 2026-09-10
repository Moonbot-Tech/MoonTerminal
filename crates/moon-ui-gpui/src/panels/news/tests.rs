//! Regression tests for News workspace scoping.

use super::scoped_chart_candidates;

/// Production wiring must treat WorkspaceRevision as authoritative rather than trusting the
/// collision-prone folded news signature, and scope navigation must not mark history fresh.
#[test]
fn workspace_revision_rebuilds_news_scope_without_fresh_history() {
    let src = include_str!("mod.rs");
    let observer = src
        .split("let workspace_revision = backend.read(cx).workspace_revision();")
        .nth(1)
        .and_then(|tail| tail.split(".detach();").next())
        .expect("workspace observer must exist");
    assert!(observer.contains("this.rebuild_workspace_scope(b);"));
    assert!(!observer.contains("cache_sig != Some(sig)"));

    let rebuild = src
        .split("fn rebuild_workspace_scope(")
        .nth(1)
        .and_then(|tail| tail.split("\n    }").next())
        .expect("authoritative scope rebuild must exist");
    assert!(rebuild.contains("self.cached = Rc::new(self.collect(b));"));
    assert!(rebuild.contains("self.catalog = self.collect_catalog(b);"));
    assert!(rebuild.contains("self.flash.clear();"));
    assert!(!rebuild.contains("fresh"));
}

/// Removing the scope predicate from `news/mod.rs:scoped_chart_candidates` would expose the hidden
/// core's exact-coin hit to chart navigation from a selected Auto core.
#[test]
fn chart_candidates_exclude_out_of_scope_cores() {
    let candidates = vec![
        (7, "BTCUSDT".to_string(), "hidden".to_string()),
        (9, "BTCUSDT".to_string(), "visible".to_string()),
        (9, "BTCUSDC".to_string(), "visible".to_string()),
    ];

    assert_eq!(
        scoped_chart_candidates(candidates, &[9]),
        vec![(9, "BTCUSDT".to_string(), "visible".to_string())]
    );
}

/// Removing the live authority checks from `news/mod.rs:NewsView::open_coin` would let a retained
/// picker item open its old core after the user selected a different Auto workspace core.
#[test]
fn delayed_coin_navigation_revalidates_live_workspace_authority() {
    let source = include_str!("mod.rs");
    let navigation = source
        .split("fn open_coin(")
        .nth(1)
        .and_then(|tail| tail.split("// ---- footer").next())
        .expect("coin navigation implementation must exist");

    assert_eq!(
        navigation
            .matches("workspace_action_allows_core(Some(&group), core)")
            .count(),
        2,
        "both immediate and retained-picker navigation must re-read live authority"
    );
    assert!(navigation.contains("let group = group.clone();"));
}

/// Cards and chart marks share tag_color: custom RGB must survive both theme changes and persisted
/// settings reloads, while legacy symbolic keys continue to follow the selected theme.
#[test]
fn saved_custom_tag_color_is_fixed_while_symbolic_color_follows_theme() {
    let settings: moon_core::config::NewsTagSettings = serde_json::from_str(
        r##"{"colors":{"custom":"#12aBcD","legacy":"red","unknown":"future-key"}}"##,
    )
    .expect("valid persisted tag colors");
    for config in [
        moon_ui::MoonThemeConfig::moon_terminal(),
        moon_ui::MoonThemeConfig::moon_light(),
    ] {
        let palette = moon_ui::MoonTheme::from_config(config).palette;
        assert_eq!(
            super::tag_color("CUSTOM", &settings, palette),
            Some(0x12abcd)
        );
        assert_eq!(
            super::tag_color("legacy", &settings, palette),
            Some(palette.red)
        );
        assert_eq!(super::tag_color("unknown", &settings, palette), None);
        assert_eq!(super::tag_color("neutral", &settings, palette), None);
    }
}
