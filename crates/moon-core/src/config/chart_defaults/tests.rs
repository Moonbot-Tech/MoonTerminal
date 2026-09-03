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

/// Two kinds ship captions of their own — the trade window and a comparison — because both draw
/// something the live default's figures do not describe: a market that stopped moving hours ago,
/// and the same market several times over in panes a third the width. The other two follow Main.
#[test]
fn the_kinds_that_ship_their_own_captions() {
    for kind in ChartTabKind::ALL {
        let ships = matches!(kind, ChartTabKind::Trade | ChartTabKind::Compare);
        assert_eq!(
            kind.builtin_labels().is_some(),
            ships,
            "{kind:?} disagrees about shipping its own captions"
        );
    }
    // Two sets, not one shared by both: they answer different questions and would have been one
    // function if they did not.
    assert_ne!(
        ChartTabKind::Trade.builtin_labels(),
        ChartTabKind::Compare.builtin_labels()
    );
    // Shared and stable: this is read on every settings comparison, and a fresh clone per read
    // would make a panel's signature differ from itself.
    assert!(std::ptr::eq(
        ChartTabKind::Compare.builtin_labels().expect("a set"),
        ChartTabKind::Compare.builtin_labels().expect("a set")
    ));
}

/// A reset walk visits Main FIRST and still visits every kind.
///
/// Resetting Main separates the kinds that follow it — that is what keeps a press on the main chart
/// from moving a kind the reader never ticked — so a kind the same press also resets has to be
/// emptied AFTER that separation, or it keeps a frozen copy of the value being discarded. The order
/// is the whole mechanism, and the caller that walks it lives in another crate.
#[test]
fn a_reset_walk_starts_at_main_and_reaches_every_kind() {
    assert_eq!(ChartTabKind::RESET_ORDER[0], ChartTabKind::Main);
    let mut sorted = ChartTabKind::RESET_ORDER.to_vec();
    let mut all = ChartTabKind::ALL.to_vec();
    sorted.sort_by_key(|k| format!("{k:?}"));
    all.sort_by_key(|k| format!("{k:?}"));
    assert_eq!(
        sorted, all,
        "a reset must reach exactly the kinds that exist"
    );
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
