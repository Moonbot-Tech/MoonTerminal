use super::*;

/// Protects `layout.rs:WindowLayout::max_core_uid` from dropping active Main-core references.
///
/// The plausible edit is folding only `header_ticker` into the high-water mark. A deleted core
/// selected in a Main header could then have its UID reissued, rebinding the saved group selection
/// to an unrelated new core after restart.
#[test]
fn active_trade_core_selection_raises_uid_floor() {
    let mut layout = WindowLayout {
        header_ticker: Some(HeaderTicker {
            core_uid: 7,
            market: "BTCUSDT".to_string(),
        }),
        ..WindowLayout::default()
    };
    layout
        .active_trade_core_by_group
        .insert("default".to_string(), 42);

    assert_eq!(layout.max_core_uid(), Some(42));
}

/// A hand-written tuner seed must never cost the user the rest of their layout.
///
/// `layout.toml` is one document with no schema version, so a field that rejects its stored value
/// discards EVERY other setting in the file and the next save writes those defaults back over it.
/// The seed is the field most likely to be edited by hand.
///
/// Breakage this pins: dropping `deserialize_with = "de_lenient_seed"` from
/// `layout.rs:analytics_tuner_seed`, or narrowing the helper to strings only. The bare number
/// below — the intuitive way to write a seed — would then fail the whole document, and the window
/// geometry asserted alongside it would come back as the default.
#[test]
fn a_hand_written_seed_cannot_discard_the_saved_layout() {
    // `analytics_period` stands in for the rest of the file: if the document is rejected, it is
    // the one that visibly reverts.
    for (written, expected) in [
        ("analytics_tuner_seed = \"123\"", Some("123")),
        // Bare integer: not a String, and the natural way to type a seed.
        ("analytics_tuner_seed = 123", Some("123")),
        // Beyond what a TOML integer holds, so it can only ever arrive as text.
        (
            "analytics_tuner_seed = \"18446744073709551615\"",
            Some("18446744073709551615"),
        ),
        // Shapes with no reading as a seed at all — accepted, then ignored.
        ("analytics_tuner_seed = true", None),
        ("analytics_tuner_seed = 1.5", None),
        ("analytics_tuner_seed = -1", None),
        ("analytics_tuner_seed = [1, 2]", None),
        ("", None),
    ] {
        let doc = format!("analytics_period = \"p-cur-month\"\n{written}\n");
        let decoded: WindowLayout = toml::from_str(&doc)
            .unwrap_or_else(|e| panic!("{written:?} must not fail the whole document: {e}"));

        assert_eq!(
            decoded.analytics_tuner_seed.as_deref(),
            expected,
            "{written:?} was read as the wrong seed"
        );
        assert_eq!(
            decoded.analytics_period.as_deref(),
            Some("p-cur-month"),
            "{written:?} discarded a neighbouring setting"
        );
    }
}

/// The tuner's numeric search settings carry the same hazard as its seed, and the same guard.
///
/// Breakage this pins: dropping `deserialize_with = "de_lenient_u32"` from
/// `layout.rs:analytics_tuner_{iters,edges,train}`. A depth copied in as `"64"`, or a train share
/// written as the fraction `0.7` rather than a percentage, would fail the whole document — the
/// neighbouring setting asserted below would come back as the default, and the next save would
/// write that default over the user's real layout.
#[test]
fn a_hand_written_search_setting_cannot_discard_the_saved_layout() {
    for (written, iters, edges, train) in [
        ("analytics_tuner_iters = 200", Some(200), None, None),
        // Quoted, which is how a value arrives when copied out of another file.
        ("analytics_tuner_edges = \"64\"", None, Some(64), None),
        ("analytics_tuner_train = 70", None, None, Some(70)),
        // The intuitive mistake: a SHARE where a percentage is stored.
        ("analytics_tuner_train = 0.7", None, None, None),
        // Shapes with no reading as a count at all — accepted, then ignored.
        ("analytics_tuner_iters = -5", None, None, None),
        ("analytics_tuner_edges = true", None, None, None),
        ("analytics_tuner_train = [70]", None, None, None),
    ] {
        let doc = format!("analytics_period = \"p-cur-month\"\n{written}\n");
        let decoded: WindowLayout = toml::from_str(&doc)
            .unwrap_or_else(|e| panic!("{written:?} must not fail the whole document: {e}"));

        assert_eq!(decoded.analytics_tuner_iters, iters, "{written:?}: iters");
        assert_eq!(decoded.analytics_tuner_edges, edges, "{written:?}: edges");
        assert_eq!(decoded.analytics_tuner_train, train, "{written:?}: train");
        assert_eq!(
            decoded.analytics_period.as_deref(),
            Some("p-cur-month"),
            "{written:?} discarded a neighbouring setting"
        );
    }
}

/// Protects `layout.rs:WindowLayout::active_trade_core_by_group` across application restarts.
///
/// The plausible edit is marking the field `#[serde(skip)]`. The selector would appear sticky
/// during one process but silently return to the first core after saving and reloading layout.toml.
#[test]
fn active_trade_core_selection_survives_toml_round_trip() {
    let mut layout = WindowLayout::default();
    layout
        .active_trade_core_by_group
        .insert("Binance Futures".to_string(), 42);

    let encoded = toml::to_string(&layout).expect("WindowLayout must serialize to TOML");
    let decoded: WindowLayout =
        toml::from_str(&encoded).expect("serialized WindowLayout must deserialize");

    assert_eq!(
        decoded.active_trade_core_by_group.get("Binance Futures"),
        Some(&42)
    );
}

/// Analytics notice state and liquidation attribution must not reach `layout.toml`.
///
/// The round trip starts from a document carrying both stale keys because serializing a default
/// cannot detect an optional persisted field whose value is `None`.
///
/// Plausible edit this catches: persisting dismissal state could suppress the only warning that
/// money is missing from every Analytics figure, while a per-user attribution flag could make
/// two installations assign the same liquidation differently.
#[test]
fn analytics_notice_and_attribution_are_not_persisted() {
    let carrying = "analytics_undated_hidden_n = 12\nanalytics_attribute_liq = true\n";
    let layout: WindowLayout = toml::from_str(carrying).expect("stale keys must be ignored");
    let text = toml::to_string(&layout).expect("layout must serialize");
    for banned in ["undated", "attribute_liq"] {
        assert!(
            !text.contains(banned),
            "`{banned}` must not survive a load-and-save round trip:\n{text}"
        );
    }
}

/// Stale Analytics keys must not make the whole layout document fail.
///
/// `WindowLayout` is deserialized as ONE value, so a rejected key would cost every window
/// position, column width, and detached-window slot in a file that still carries those keys.
#[test]
fn a_stale_key_from_an_older_build_still_loads() {
    let stale = "analytics_undated_hidden_n = 12\n\
                 analytics_attribute_liq = true\n\
                 analytics_profit_percent = true\n";
    let layout: WindowLayout = toml::from_str(stale).expect("a stale key must be ignored");
    assert!(
        layout.analytics_profit_percent,
        "the keys that DO still exist must survive beside the ignored ones"
    );
}

/// A config written before these keys existed must keep behaving exactly as it did.
///
/// Every existing `layout.toml` is "a document without them", so this is the upgrade path itself,
/// and the oracle is an empty document rather than a restated literal.
///
/// Breakage this pins, twice over. Declaring `analytics_tuner_fields` as a bare `Vec<String>`
/// instead of `Option<Vec<String>>` makes an existing config deserialize to an EMPTY selection,
/// which the tuner reads as "the user unchecked everything" — every field comes up unchecked on
/// the first launch after the update. Flipping `analytics_hist_collapsed` to mean "expanded"
/// makes its `false` default fold the distribution card shut for every existing user, with
/// nothing on screen to explain why.
#[test]
fn a_config_without_the_new_analytics_keys_keeps_todays_behaviour() {
    let decoded: WindowLayout = toml::from_str("analytics_period = \"p-cur-month\"\n")
        .expect("a document without the new keys must still load");

    assert_eq!(
        decoded.analytics_tuner_fields, None,
        "an absent key must read as 'never saved', not as an empty selection"
    );
    assert!(
        !decoded.analytics_hist_collapsed,
        "the distribution card must open expanded for every existing config"
    );
    assert!(
        !decoded.analytics_tuner_compose,
        "automatic composition must stay OFF for every existing config: it is a different, \
         slower search, and no update should silently change what a familiar button does"
    );
    assert_eq!(
        decoded.analytics_strat_sort, None,
        "an older config has no saved strategy-list sort"
    );
}

/// Strategy-list sorting must survive a restart without making `layout.toml` fragile.
///
/// The round trip proves both the stable column key and direction. The malformed cases assert
/// the neighbouring period because the real failure is not merely losing sort: without
/// `de_lenient`, one hand-edited tuple can reject the whole layout and later overwrite every
/// window position with defaults.
///
/// Breakage this pins: removing `deserialize_with = "de_lenient"` from
/// `layout.rs:analytics_strat_sort`. The first malformed value below would reject the document
/// instead of preserving the period and treating sort as unset.
#[test]
fn strategy_sort_survives_restart_without_endangering_layout() {
    let saved = WindowLayout {
        analytics_strat_sort: Some(("core".to_string(), false)),
        ..WindowLayout::default()
    };
    let encoded = toml::to_string(&saved).expect("the layout must serialize");
    let decoded: WindowLayout = toml::from_str(&encoded).expect("its own output must load");
    assert_eq!(
        decoded.analytics_strat_sort,
        Some(("core".to_string(), false)),
        "column key and ascending direction must both survive"
    );

    for written in [
        "\"core\"",
        "5",
        "true",
        "[\"core\"]",
        "[\"core\", false, true]",
        "[\"core\", 7]",
    ] {
        let doc = format!("analytics_period = \"p-cur-month\"\nanalytics_strat_sort = {written}\n");
        let decoded: WindowLayout = toml::from_str(&doc)
            .unwrap_or_else(|error| panic!("{written} must not reject the layout: {error}"));
        assert_eq!(
            decoded.analytics_period.as_deref(),
            Some("p-cur-month"),
            "{written}: the rest of the layout must survive"
        );
        assert_eq!(
            decoded.analytics_strat_sort, None,
            "{written}: an unusable sort value must be ignored"
        );
    }
}

/// Current strategy-column masks survive a restart without making `layout.toml` fragile.
///
/// Breakage this pins: removing `de_lenient` from `analytics_strat_cols_modes2`. A malformed
/// hand-edited mask would reject the complete layout and discard the neighbouring period.
#[test]
fn current_strategy_column_masks_cannot_discard_the_saved_layout() {
    let saved = WindowLayout {
        analytics_strat_cols_modes2: Some(StratColsByMode {
            filter: 3,
            coins: 7,
            time: 11,
        }),
        ..WindowLayout::default()
    };
    let encoded = toml::to_string(&saved).expect("the layout must serialize");
    let decoded: WindowLayout = toml::from_str(&encoded).expect("its own output must load");
    let masks = decoded
        .analytics_strat_cols_modes2
        .expect("current masks survive");
    assert_eq!((masks.filter, masks.coins, masks.time), (3, 7, 11));

    for written in ["17", "true", "[1, 2]", "{ filter = \"bad\" }"] {
        let doc = format!(
            "analytics_period = \"p-cur-month\"\nanalytics_strat_cols_modes2 = {written}\n"
        );
        let decoded: WindowLayout = toml::from_str(&doc)
            .unwrap_or_else(|error| panic!("{written} must not reject the layout: {error}"));
        assert_eq!(decoded.analytics_period.as_deref(), Some("p-cur-month"));
    }
}

/// The composition switch must survive a round trip and a hand-edited value.
///
/// `layout.toml` is one schema-less document holding every window's geometry, so the hazard is
/// not this key reading wrong — it is this key rejecting the DOCUMENT. The oracle is the
/// neighbouring `analytics_period`, whose survival proves the salvage rather than restating it.
///
/// Breakage this pins: declaring `layout.rs:analytics_tuner_compose` as a plain `bool` instead of
/// going through `de_lenient_bool`. A single quoted `analytics_tuner_compose = "true"` — the
/// intuitive way to hand-edit it — would then reject the whole file, and the next save would
/// write default geometry over every window position and column width the user had arranged.
#[test]
fn a_hand_edited_composition_switch_never_costs_the_layout() {
    let saved = WindowLayout {
        analytics_tuner_compose: true,
        ..WindowLayout::default()
    };
    let encoded = toml::to_string(&saved).expect("the layout must serialize");
    let decoded: WindowLayout = toml::from_str(&encoded).expect("its own output must load back");
    assert!(
        decoded.analytics_tuner_compose,
        "an enabled composition switch must survive its own round trip"
    );

    for written in ["\"true\"", "\"TRUE\"", "\"yes\"", "17", "[1, 2]"] {
        let doc =
            format!("analytics_period = \"p-cur-month\"\nanalytics_tuner_compose = {written}\n");
        let decoded: WindowLayout = toml::from_str(&doc)
            .unwrap_or_else(|e| panic!("{written} must not reject the document: {e}"));
        assert_eq!(
            decoded.analytics_period.as_deref(),
            Some("p-cur-month"),
            "{written}: the rest of the layout must survive an unexpected value"
        );
    }
    let quoted: WindowLayout =
        toml::from_str("analytics_tuner_compose = \"true\"\n").expect("a quoted bool must load");
    assert!(
        quoted.analytics_tuner_compose,
        "a quoted true must still mean true, not fall back to the default"
    );
}

/// A hand-written field list must never cost the user the rest of their layout.
///
/// Same hazard as the seed above: `layout.toml` is one schema-less document, and this key sits in
/// the same hand-edited tuner block. The intuitive typo is a bare string.
///
/// Breakage this pins: dropping `deserialize_with = "de_lenient"` from
/// `layout.rs:analytics_tuner_fields`. A single `analytics_tuner_fields = "lev"` would then reject
/// the whole document, and the next save would write default geometry over every window position
/// and column width in the file — the `analytics_period` asserted alongside is the visible proof.
#[test]
fn a_hand_written_field_list_cannot_discard_the_saved_layout() {
    for (written, expected) in [
        (
            "analytics_tuner_fields = [\"lev\", \"dmark\"]",
            Some(vec!["lev".to_string(), "dmark".to_string()]),
        ),
        // Unchecking everything is a real state and must round-trip as itself.
        ("analytics_tuner_fields = []", Some(Vec::new())),
        // Shapes with no reading as a list of ids — accepted, then ignored.
        ("analytics_tuner_fields = \"lev\"", None),
        ("analytics_tuner_fields = 5", None),
        ("analytics_tuner_fields = true", None),
        ("analytics_tuner_fields = [1, 2]", None),
        ("", None),
    ] {
        let doc = format!("analytics_period = \"p-cur-month\"\n{written}\n");
        let decoded: WindowLayout = toml::from_str(&doc)
            .unwrap_or_else(|e| panic!("{written:?} must not fail the whole document: {e}"));

        assert_eq!(
            decoded.analytics_tuner_fields, expected,
            "{written:?} must read as {expected:?}"
        );
        assert_eq!(
            decoded.analytics_period.as_deref(),
            Some("p-cur-month"),
            "{written:?} must not take the rest of the layout down with it"
        );
    }
}

/// Standalone Report geometry must survive restart without making `layout.toml` fragile.
///
/// Breakage this pins: marking `layout.rs:WindowLayout::report_window` as skipped would lose the
/// user's window placement on restart, while removing its `de_lenient` deserializer would let one
/// malformed rectangle reject every neighbouring layout preference.
#[test]
fn report_window_geometry_survives_restart_without_endangering_layout() {
    let saved = WindowLayout {
        report_window: Some(GeomRect {
            x: 120,
            y: 180,
            w: 1640,
            h: 1100,
        }),
        ..WindowLayout::default()
    };
    let encoded = toml::to_string(&saved).expect("the layout must serialize");
    let decoded: WindowLayout = toml::from_str(&encoded).expect("its own output must load");
    let geometry = decoded
        .report_window
        .expect("standalone Report geometry must survive a save-and-reload cycle");
    assert_eq!(
        (geometry.x, geometry.y, geometry.w, geometry.h),
        (120, 180, 1640, 1100)
    );

    for written in ["\"wide\"", "17", "true", "[120, 180, 1640]", "{ x = 120 }"] {
        let doc = format!("analytics_period = \"p-cur-month\"\nreport_window = {written}\n");
        let decoded: WindowLayout = toml::from_str(&doc)
            .unwrap_or_else(|error| panic!("{written} must not reject the layout: {error}"));
        assert_eq!(
            decoded.analytics_period.as_deref(),
            Some("p-cur-month"),
            "{written}: malformed Report geometry discarded a neighbouring setting"
        );
        assert!(
            decoded.report_window.is_none(),
            "{written}: unusable Report geometry must be ignored"
        );
    }
}

/// The header clock's compatibility offset must survive as the seed city migration reads.
///
/// Breakage this pins: deleting `layout.rs:header_clock_offset_min` as dead code once
/// `header_clock_zone` exists. Layouts without a zone need the offset to recover their clock
/// selection, while layouts with a zone keep it updated as a fixed-offset compatibility mirror.
#[test]
fn a_pre_city_clock_choice_survives_as_the_migration_seed() {
    let doc = "analytics_period = \"p-cur-month\"\nheader_clock_offset_min = 120\n";
    let decoded: WindowLayout = toml::from_str(doc).expect("a legacy layout still loads");

    assert_eq!(decoded.header_clock_offset_min, 120, "the seed was dropped");
    assert!(
        decoded.header_clock_zone.is_none(),
        "a legacy layout has no zone yet; migration is what supplies it"
    );
}

/// A hand-written clock zone of the wrong type must cost that one key, not the whole document.
///
/// Breakage this pins: dropping `deserialize_with = "de_lenient"` from
/// `layout.rs:header_clock_zone`. `layout.toml` is hand-editable and holds every window's
/// geometry, so a zone written as a bare number would take all of it down.
#[test]
fn a_mistyped_clock_zone_cannot_discard_the_saved_layout() {
    for (written, expected) in [
        (
            "header_clock_zone = \"Europe/Warsaw\"",
            Some("Europe/Warsaw"),
        ),
        ("header_clock_zone = 42", None),
        ("header_clock_zone = true", None),
        ("header_clock_zone = [\"Europe/Warsaw\"]", None),
        ("", None),
    ] {
        let doc = format!("analytics_period = \"p-cur-month\"\n{written}\n");
        let decoded: WindowLayout = toml::from_str(&doc)
            .unwrap_or_else(|e| panic!("{written:?} must not fail the whole document: {e}"));

        assert_eq!(
            decoded.header_clock_zone.as_deref(),
            expected,
            "{written:?} was read as the wrong zone"
        );
        assert_eq!(
            decoded.analytics_period.as_deref(),
            Some("p-cur-month"),
            "{written:?} discarded a neighbouring setting"
        );
    }
}
