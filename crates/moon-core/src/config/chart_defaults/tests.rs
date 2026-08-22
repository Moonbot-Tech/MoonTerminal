use super::*;

/// The anchor lock outranks the place: a comparison torn off into its own window is still a
/// comparison, and handing it the window default would dress it as something it is not.
#[test]
fn the_lock_outranks_the_window() {
    assert_eq!(ChartTabKind::of(false, false), ChartTabKind::Main);
    assert_eq!(ChartTabKind::of(true, false), ChartTabKind::AddTo);
    assert_eq!(ChartTabKind::of(true, true), ChartTabKind::Compare);
    assert_eq!(ChartTabKind::of(false, true), ChartTabKind::Compare);
}

/// A profile that never split its defaults holds nothing here, and "nothing" has to mean "follow
/// Main" rather than "the built-in default" — otherwise the first launch after the split would
/// redress every window.
#[test]
fn an_untouched_profile_holds_nothing() {
    let empty = ChartTabDefaults::default();
    assert!(empty.candle_view.is_none());
    assert!(empty.chart_graphics.is_none());
    assert!(empty.chart_labels.is_none());
}

/// A hand-edited file states nonsense in one place and the rest of the layout must survive it.
#[test]
fn a_broken_table_costs_only_itself() {
    #[derive(serde::Deserialize)]
    struct Doc {
        keep: u32,
        #[serde(default, deserialize_with = "ChartTabDefaults::de_lenient")]
        defaults: ChartTabDefaults,
    }

    let doc: Doc = toml::from_str("keep = 7\ndefaults = \"nonsense\"\n").expect("the file loads");
    assert_eq!(doc.keep, 7);
    assert_eq!(doc.defaults, ChartTabDefaults::default());

    // And a table that IS usable arrives intact.
    let doc: Doc = toml::from_str("keep = 7\n[defaults.candle_view]\n").expect("the file loads");
    assert!(doc.defaults.candle_view.is_some());
}
