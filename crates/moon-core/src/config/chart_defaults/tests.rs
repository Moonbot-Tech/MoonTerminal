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

/// The trade window is a kind with no TABS: it is opened from the Report and lives outside the tab
/// strip and `charts.json` alike, so a walk that writes a setting into every matching tab has
/// nothing to visit for it and must stop at storing the default.
#[test]
fn the_tab_kinds_are_every_kind_but_the_trade_window() {
    let expected: Vec<ChartTabKind> = ChartTabKind::ALL
        .into_iter()
        .filter(|kind| *kind != ChartTabKind::Trade)
        .collect();
    assert_eq!(ChartTabKind::TAB_KINDS.to_vec(), expected);
}

/// Only the trade window ships captions of its own; every other kind follows the Main default.
#[test]
fn only_the_trade_window_ships_its_own_captions() {
    for kind in ChartTabKind::ALL {
        assert_eq!(
            kind.builtin_labels().is_some(),
            kind == ChartTabKind::Trade,
            "{kind:?} disagrees about shipping its own captions"
        );
    }
}

/// The runtime classifier never produces the trade window: that kind is set by the window itself,
/// which is neither detached-with-a-tab nor comparing.
#[test]
fn the_classifier_never_answers_trade() {
    for detached in [false, true] {
        for comparing in [false, true] {
            assert_ne!(ChartTabKind::of(detached, comparing), ChartTabKind::Trade);
        }
    }
}
