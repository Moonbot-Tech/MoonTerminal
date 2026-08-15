//! Persistence, lenient-decoding, and UID high-water regressions for workspace layout fields.

use super::*;

/// Protects all workspace maps as a restart-stable, backwards-compatible layout contract.
///
/// Plausible breakage: marking a map as skipped/default-only makes a saved Auto workspace silently
/// return to Classic, Overview, or Report after restart. Literal TOML is independent of the
/// serializer and therefore catches a matching encoder/decoder mistake.
#[test]
fn legacy_and_current_toml_restore_workspace_state() {
    let legacy: WindowLayout = toml::from_str("analytics_period = \"p-cur-month\"\n")
        .expect("legacy layout must remain readable");
    assert_eq!(
        legacy
            .workspace_mode_by_group
            .get("desk")
            .copied()
            .unwrap_or_default(),
        WorkspaceMode::Classic
    );
    assert_eq!(legacy.auto_workspace_core_by_group.get("desk"), None);
    assert_eq!(legacy.auto_workspace_tab_by_group.get("desk"), None);

    let current = "analytics_period = \"p-cur-month\"\n\
                   [workspace_mode_by_group]\n\
                   desk = \"auto-trading\"\n\
                   [auto_workspace_core_by_group]\n\
                   desk = 73\n\
                   [auto_workspace_tab_by_group]\n\
                   desk = \"Assets\"\n";
    let decoded: WindowLayout =
        toml::from_str(current).expect("current workspace layout must load");
    assert_eq!(
        decoded.workspace_mode_by_group.get("desk"),
        Some(&WorkspaceMode::AutoTrading)
    );
    assert_eq!(decoded.auto_workspace_core_by_group.get("desk"), Some(&73));
    assert_eq!(
        decoded
            .auto_workspace_tab_by_group
            .get("desk")
            .map(String::as_str),
        Some("Assets")
    );

    let encoded = toml::to_string(&decoded).expect("workspace layout must serialize");
    assert!(encoded.contains("desk = \"auto-trading\""));
    assert!(encoded.contains("desk = \"Assets\""));
}

/// Protects the Auto selection as a durable UID high-water reference.
///
/// Plausible breakage: dropping this map from `WindowLayout::max_core_uid` allows a deleted core's
/// UID to be reused and binds the saved workspace selection to an unrelated server.
#[test]
fn workspace_core_contributes_to_uid_high_water_mark() {
    let mut layout = WindowLayout::default();
    layout
        .active_trade_core_by_group
        .insert("desk".to_string(), 11);
    layout
        .auto_workspace_core_by_group
        .insert("desk".to_string(), 97);

    assert_eq!(layout.max_core_uid(), Some(97));
}

/// Protects the rest of the schema-less layout from malformed workspace preferences.
///
/// Plausible breakage: replacing either lenient map reader with ordinary deserialization makes a
/// hand-edited wrong type reject every saved window position and column width in the document.
#[test]
fn malformed_workspace_fields_do_not_discard_other_layout() {
    for written in [
        "workspace_mode_by_group = 17",
        "workspace_mode_by_group = [\"auto-trading\"]",
        "auto_workspace_core_by_group = \"desk\"",
        "auto_workspace_core_by_group = [73]",
        "auto_workspace_tab_by_group = \"Report\"",
        "auto_workspace_tab_by_group = [\"Report\"]",
    ] {
        let doc = format!("analytics_period = \"p-cur-month\"\n{written}\n");
        let decoded: WindowLayout = toml::from_str(&doc)
            .unwrap_or_else(|error| panic!("{written} must not reject the layout: {error}"));
        assert_eq!(decoded.analytics_period.as_deref(), Some("p-cur-month"));
        assert!(decoded.workspace_mode_by_group.is_empty());
        assert!(decoded.auto_workspace_core_by_group.is_empty());
        assert!(decoded.auto_workspace_tab_by_group.is_empty());
    }

    let unknown = "analytics_period = \"p-cur-month\"\n\
                   [workspace_mode_by_group]\n\
                   desk = \"future-preset\"\n";
    let decoded: WindowLayout = toml::from_str(unknown).expect("unknown mode must remain readable");
    assert_eq!(
        decoded.workspace_mode_by_group.get("desk"),
        Some(&WorkspaceMode::Classic)
    );
    assert_eq!(decoded.analytics_period.as_deref(), Some("p-cur-month"));
}

/// Protects the global Auto rail width as a lenient, bounded restart preference.
///
/// Plausible breakage: replacing the custom decoder with ordinary `f32` deserialization makes a
/// quoted or malformed width discard every saved window position, while omitting the clamp can
/// restore a rail that consumes the complete workspace.
#[test]
fn auto_workspace_rail_width_defaults_decodes_and_clamps() {
    let legacy: WindowLayout = toml::from_str("analytics_period = \"p-cur-month\"\n")
        .expect("legacy layout must remain readable");
    // The accepted UX contract is independent of the production default constant.
    assert_eq!(legacy.auto_workspace_rail_width(), 340.0);

    for (written, expected) in [
        ("480.5", 480.5),
        ("\"300\"", 300.0),
        ("1", 52.0),
        ("900", 560.0),
        ("\"wide\"", 340.0),
        ("true", 340.0),
    ] {
        let doc =
            format!("analytics_period = \"p-cur-month\"\nauto_workspace_rail_width = {written}\n");
        let decoded: WindowLayout = toml::from_str(&doc)
            .unwrap_or_else(|error| panic!("{written} must not reject the layout: {error}"));
        assert_eq!(decoded.analytics_period.as_deref(), Some("p-cur-month"));
        assert_eq!(decoded.auto_workspace_rail_width(), expected);
    }
}

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

/// Protects `layout.rs:WindowLayout::max_core_uid` from dropping the recent-coins history.
///
/// The plausible edit is tidying the iterator chain and dropping the `.chain(self.recent_coins
/// ...)` link, reasoning the header ticker and active-trade selections already cover "referenced
/// UIDs". A core referenced only by a market it was once opened on (never pinned as the active
/// trade core or the header ticker) would then stop raising the high-water mark, letting a
/// deleted core's UID be reissued to a new server, which inherits the deleted core's trades and
/// P&L.
#[test]
fn recent_coins_history_also_raises_the_uid_floor() {
    let mut layout = WindowLayout {
        header_ticker: Some(HeaderTicker {
            core_uid: 3,
            market: "BTCUSDT".to_string(),
        }),
        ..WindowLayout::default()
    };
    layout
        .active_trade_core_by_group
        .insert("default".to_string(), 5);
    // Referenced ONLY here: a market opened once from the coin-search dropdown on a core that was
    // never made the header ticker or the active trade selection for any group.
    layout.recent_coins = Some(vec![HeaderTicker {
        core_uid: 99,
        market: "ETHUSDT".to_string(),
    }]);

    assert_eq!(layout.max_core_uid(), Some(99));
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

/// `layout.rs:WindowLayout::table_sorts` must preserve every context and both directions.
///
/// Mutation: mark the field skipped or decode every direction as ascending. A table would appear
/// sticky in one process but reopen on its default column or with the arrow reversed.
#[test]
fn table_sort_preferences_survive_toml_round_trip() {
    let mut layout = WindowLayout::default();
    layout.table_sorts.insert(
        "orders-table:dock".to_string(),
        TableSortPreference {
            column: "pnl".to_string(),
            ascending: false,
        },
    );
    layout.table_sorts.insert(
        "core-status-table:win".to_string(),
        TableSortPreference {
            column: "server".to_string(),
            ascending: true,
        },
    );

    let encoded = toml::to_string(&layout).expect("WindowLayout must serialize to TOML");
    let decoded: WindowLayout =
        toml::from_str(&encoded).expect("serialized WindowLayout must deserialize");

    assert_eq!(decoded.table_sorts, layout.table_sorts);
}

/// `layout.rs:de_table_sort_map` must discard one malformed entry without losing valid siblings.
///
/// Mutation: deserialize the `HashMap<String, TableSortPreference>` as one all-or-nothing value.
/// The malformed Assets entry would then empty the map, and Orders would also forget its saved
/// descending PnL sort even though that neighbouring entry is valid.
#[test]
fn one_malformed_table_sort_does_not_erase_valid_siblings_or_layout() {
    let doc = r#"
analytics_period = "p-cur-month"

[table_sorts."orders-table:dock"]
column = "pnl"
ascending = false

[table_sorts."assets-table:win"]
column = 7
ascending = "up"
"#;
    let decoded: WindowLayout =
        toml::from_str(doc).expect("one malformed table sort must not reject the layout");

    assert_eq!(decoded.analytics_period.as_deref(), Some("p-cur-month"));
    assert_eq!(
        decoded.table_sorts.get("orders-table:dock"),
        Some(&TableSortPreference {
            column: "pnl".to_string(),
            ascending: false,
        })
    );
    assert!(!decoded.table_sorts.contains_key("assets-table:win"));
}

/// `layout.rs:de_table_sort_map` must tolerate a wrong outer value like every hand-edited map.
///
/// Mutation: remove the lenient outer `Stored::Other` arm. A typo on `table_sorts` would reject
/// all window geometry and the neighbouring period instead of merely resetting table sorts.
#[test]
fn malformed_table_sort_map_cannot_reject_the_layout() {
    let decoded: WindowLayout =
        toml::from_str("analytics_period = \"p-cur-month\"\ntable_sorts = 5\n")
            .expect("a malformed table_sorts value must not fail the complete document");

    assert_eq!(decoded.analytics_period.as_deref(), Some("p-cur-month"));
    assert!(decoded.table_sorts.is_empty());
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

/// Re-opening the same market repeatedly must not flood the recent-coins history.
///
/// Breakage this pins: `layout.rs:WindowLayout::push_recent_coin` dropping the
/// `entries.retain(...)` dedup pass, reasoning that `insert(0, ..)` already puts the newest entry
/// first. Without the dedup, re-opening one coin over and over fills every slot up to
/// [`WindowLayout::RECENT_COINS_CAP`] with copies of that one market and pushes every other
/// recently opened market out of the history entirely.
#[test]
fn reopening_the_same_market_repeatedly_does_not_flood_the_history() {
    let mut layout = WindowLayout::default();
    assert!(layout.push_recent_coin(1, "ETHUSDT"));
    for _ in 0..(WindowLayout::RECENT_COINS_CAP * 2) {
        // Moving it to the front each time is a no-op once it is already on top, so this must
        // stop reporting a change (and stop growing the list) after the very first call.
        layout.push_recent_coin(1, "BTCUSDT");
        layout.push_recent_coin(1, "ETHUSDT");
    }

    let entries = layout.recent_coins.expect("entries were pushed");
    assert_eq!(
        entries.len(),
        2,
        "repeated re-opens of two markets must leave exactly those two entries, not one per call: {:?}",
        entries.iter().map(|e| &e.market).collect::<Vec<_>>()
    );
    assert_eq!(
        entries[0].market, "ETHUSDT",
        "the most recently opened market leads"
    );
    assert_eq!(entries[1].market, "BTCUSDT");
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

/// `layout.rs:WindowLayout::profit_monitor_*` must survive restart and ignore malformed hand edits;
/// removing any lenient deserializer makes this assertion red and lets one widget preference erase
/// every neighbouring saved window position.
#[test]
fn profit_monitor_preferences_round_trip_without_endangering_layout() {
    let saved = WindowLayout {
        profit_monitor_window: Some(GeomRect {
            x: 240,
            y: 160,
            w: 720,
            h: 520,
        }),
        profit_monitor_period: Some("m-week".to_string()),
        profit_monitor_group: Some("core".to_string()),
        profit_monitor_sort: Some(("trades".to_string(), true)),
        profit_monitor_open: true,
        profit_monitor_exchange_icons: Some(false),
        profit_monitor_last_trade: Some(true),
        profit_monitor_flash: Some(false),
        profit_monitor_core_filter: Some(false),
        profit_monitor_group_sections: Some(false),
        profit_monitor_idle_cores: Some(true),
        ..WindowLayout::default()
    };
    let encoded = toml::to_string(&saved).expect("the layout must serialize");
    let decoded: WindowLayout = toml::from_str(&encoded).expect("its own output must load");
    let geometry = decoded
        .profit_monitor_window
        .expect("monitor geometry must survive restart");
    assert_eq!(
        (geometry.x, geometry.y, geometry.w, geometry.h),
        (240, 160, 720, 520)
    );
    assert_eq!(decoded.profit_monitor_period.as_deref(), Some("m-week"));
    assert_eq!(decoded.profit_monitor_group.as_deref(), Some("core"));
    assert_eq!(
        decoded.profit_monitor_sort,
        Some(("trades".to_string(), true))
    );
    assert!(
        decoded.profit_monitor_open,
        "a monitor left open must reopen after restart"
    );
    // Each preference round-trips on its own: a shared writer that saved all of them from one edit
    // would pass a test that only checked the one just changed.
    assert_eq!(decoded.profit_monitor_exchange_icons, Some(false));
    assert_eq!(decoded.profit_monitor_last_trade, Some(true));
    assert_eq!(decoded.profit_monitor_flash, Some(false));
    assert_eq!(decoded.profit_monitor_core_filter, Some(false));
    assert_eq!(decoded.profit_monitor_group_sections, Some(false));
    assert_eq!(decoded.profit_monitor_idle_cores, Some(true));

    // The display flags are booleans, so `true` is a VALID value there and cannot double as garbage.
    for written in ["17", "\"maybe\"", "[1, 2]", "{ x = 240 }"] {
        let doc = format!(
            "analytics_period = \"p-cur-month\"\nprofit_monitor_open = {written}\nprofit_monitor_exchange_icons = {written}\nprofit_monitor_last_trade = {written}\nprofit_monitor_flash = {written}\nprofit_monitor_core_filter = {written}\nprofit_monitor_group_sections = {written}\nprofit_monitor_idle_cores = {written}\n"
        );
        let decoded: WindowLayout = toml::from_str(&doc)
            .unwrap_or_else(|error| panic!("{written} must not reject the layout: {error}"));
        assert_eq!(decoded.analytics_period.as_deref(), Some("p-cur-month"));
        assert!(!decoded.profit_monitor_open);
        assert!(decoded.profit_monitor_exchange_icons.is_none());
        assert!(decoded.profit_monitor_last_trade.is_none());
        assert!(decoded.profit_monitor_flash.is_none());
        assert!(decoded.profit_monitor_core_filter.is_none());
        assert!(decoded.profit_monitor_group_sections.is_none());
        assert!(decoded.profit_monitor_idle_cores.is_none());
    }

    for written in ["true", "17", "[240, 160, 720]", "{ x = 240 }"] {
        let doc = format!(
            "analytics_period = \"p-cur-month\"\nprofit_monitor_window = {written}\nprofit_monitor_period = {written}\nprofit_monitor_group = {written}\nprofit_monitor_sort = {written}\n"
        );
        let decoded: WindowLayout = toml::from_str(&doc)
            .unwrap_or_else(|error| panic!("{written} must not reject the layout: {error}"));
        assert_eq!(decoded.analytics_period.as_deref(), Some("p-cur-month"));
        assert!(decoded.profit_monitor_window.is_none());
        assert!(decoded.profit_monitor_period.is_none());
        assert!(decoded.profit_monitor_group.is_none());
        assert!(decoded.profit_monitor_sort.is_none());
    }
}

/// `layout.rs:WindowLayout::strategies_*` must preserve explicit choices and ignore malformed
/// hand edits; replacing either optional lenient field with a bare boolean would lose the
/// absent-versus-disabled distinction or discard neighboring layout preferences on restart.
#[test]
fn strategies_preferences_round_trip_without_endangering_layout() {
    let saved = WindowLayout {
        strategies_group_by_venue: Some(false),
        strategies_active_only: Some(true),
        analytics_period: Some("p-cur-month".to_string()),
        ..WindowLayout::default()
    };
    let encoded = toml::to_string(&saved).expect("the layout must serialize");
    let decoded: WindowLayout = toml::from_str(&encoded).expect("its own output must load");
    assert_eq!(decoded.strategies_group_by_venue, Some(false));
    assert_eq!(decoded.strategies_active_only, Some(true));
    assert_eq!(decoded.analytics_period.as_deref(), Some("p-cur-month"));

    let absent: WindowLayout = toml::from_str("analytics_period = \"p-cur-month\"\n")
        .expect("a layout written before Strategies preferences must remain readable");
    assert_eq!(absent.strategies_group_by_venue, None);
    assert_eq!(absent.strategies_active_only, None);

    for written in ["17", "\"maybe\"", "[true]", "{ enabled = true }"] {
        let doc = format!(
            "analytics_period = \"p-cur-month\"\nstrategies_group_by_venue = {written}\nstrategies_active_only = {written}\n"
        );
        let decoded: WindowLayout = toml::from_str(&doc)
            .unwrap_or_else(|error| panic!("{written} must not reject the layout: {error}"));
        assert_eq!(
            decoded.analytics_period.as_deref(),
            Some("p-cur-month"),
            "{written}: a malformed Strategies preference discarded a neighboring setting"
        );
        assert_eq!(decoded.strategies_group_by_venue, None);
        assert_eq!(decoded.strategies_active_only, None);
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

/// A hand-written clock zone of the wrong type must preserve the document without looking absent.
///
/// Breakage this pins: replacing `de_clock_zone` with generic `de_lenient` turns malformed present
/// values into `None`; startup then mistakes the profile for first run and overwrites it from the
/// operating system. Removing lenient deserialization entirely would discard the whole layout.
#[test]
fn a_mistyped_clock_zone_cannot_discard_the_saved_layout() {
    for (written, expected) in [
        (
            "header_clock_zone = \"Europe/Warsaw\"",
            Some("Europe/Warsaw"),
        ),
        ("header_clock_zone = 42", Some("")),
        ("header_clock_zone = true", Some("")),
        ("header_clock_zone = [\"Europe/Warsaw\"]", Some("")),
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

/// A `layout.toml` written before the API-key axis existed must still load, keep every axis the
/// user had tuned, and default the new one to ON with its 7-day horizon.
///
/// The literal is copied verbatim from a real pre-change config, not from a freshly serialized
/// one — a round trip through today's writer would contain the new keys and prove nothing.
#[test]
fn a_config_without_the_api_key_axis_still_loads() {
    let old = "\
[warn_axes]
cpu = true
mem = true
conn = true
ping = true
exch = true

[warn_params.cpu]
chart = true
sound = \"ringout\"
pct = 70
hold = 5

[warn_params.exch]
chart = true
sound = \"ringout\"
yellow = 2
red = 30
window = 15
hold = 3
";

    let decoded: WindowLayout = toml::from_str(old).expect("an old config must still parse");

    assert!(decoded.warn_axes.api, "a new axis defaults to on");
    assert_eq!(decoded.warn_params.api.days, 7);
    assert_eq!(decoded.warn_params.api.sound, None);
    // The axes the user had already tuned survive untouched.
    assert_eq!(decoded.warn_params.cpu.pct, 70);
    assert_eq!(decoded.warn_params.exch.red, 30);
    assert_eq!(decoded.warn_params.exch.sound.as_deref(), Some("ringout"));
}

/// A `layout.toml` `report_filters` entry with a wrong-typed member must lose only THAT member,
/// keep its correctly-typed neighbours in the same entry, ignore an unknown extra key, and never
/// take the rest of the schema-less layout document down with it.
///
/// Breakage this pins: replacing `deserialize_with = "de_lenient"` on any `ReportFilterPrefs`
/// field (`side`/`kind`/`deleted_only`/`period`/`period_overview`/`strategy_name_mask`) with plain
/// deserialization.
/// Because
/// `report_filters` itself is ALSO read leniently as a whole map, a plain field does not fail this
/// call to `toml::from_str` — it instead collapses the WHOLE `report_filters` map to empty (every
/// host context's stored filters, not just the malformed one), which the second assertion below
/// (the entry's own survival) catches: an "all-or-nothing" map fallback is exactly the coarsening
/// the same mutation names.
#[test]
fn a_malformed_report_filter_member_defaults_alone_without_costing_the_layout() {
    let entry_id = "report-filters:dock";
    // One member malformed per case; the other five stay well-typed and non-default so their
    // survival is a real assertion, not a comparison against a value that defaults the same way.
    let cases: [(&str, &str); 6] = [
        (
            "side",
            "side = 5\nkind = \"real\"\ndeleted_only = true\nperiod = \"rp-cur-month\"\nperiod_overview = \"rp-today\"\nstrategy_name_mask = \"EMA_\"\n",
        ),
        (
            "kind",
            "side = \"long\"\nkind = [\"real\"]\ndeleted_only = true\nperiod = \"rp-cur-month\"\nperiod_overview = \"rp-today\"\nstrategy_name_mask = \"EMA_\"\n",
        ),
        (
            "deleted_only",
            "side = \"long\"\nkind = \"real\"\ndeleted_only = \"not-a-bool\"\nperiod = \"rp-cur-month\"\nperiod_overview = \"rp-today\"\nstrategy_name_mask = \"EMA_\"\n",
        ),
        (
            "period",
            "side = \"long\"\nkind = \"real\"\ndeleted_only = true\nperiod = 42\nperiod_overview = \"rp-today\"\nstrategy_name_mask = \"EMA_\"\n",
        ),
        (
            "period_overview",
            "side = \"long\"\nkind = \"real\"\ndeleted_only = true\nperiod = \"rp-cur-month\"\nperiod_overview = 42\nstrategy_name_mask = \"EMA_\"\n",
        ),
        (
            "strategy_name_mask",
            "side = \"long\"\nkind = \"real\"\ndeleted_only = true\nperiod = \"rp-cur-month\"\nperiod_overview = \"rp-today\"\nstrategy_name_mask = [\"EMA_\"]\n",
        ),
    ];

    for (bad_field, body) in cases {
        let doc = format!(
            "analytics_period = \"p-cur-month\"\n[report_filters.\"{entry_id}\"]\n{body}future_unknown_key = \"surprise\"\n"
        );
        let decoded: WindowLayout = toml::from_str(&doc).unwrap_or_else(|e| {
            panic!("{bad_field}: a malformed report-filter member must not fail the whole document: {e}")
        });
        assert_eq!(
            decoded.analytics_period.as_deref(),
            Some("p-cur-month"),
            "{bad_field}: a malformed report-filter member discarded a neighbouring layout setting"
        );
        let prefs = decoded.report_filters.get(entry_id).unwrap_or_else(|| {
            panic!("{bad_field}: the entry itself must survive its own malformed member")
        });

        match bad_field {
            "side" => {
                assert_eq!(prefs.side, None, "malformed side must default to None");
                assert_eq!(
                    prefs.kind.as_deref(),
                    Some("real"),
                    "a well-typed neighbour must survive"
                );
                assert_eq!(
                    prefs.deleted_only,
                    Some(true),
                    "a well-typed neighbour must survive"
                );
                assert_eq!(
                    prefs.period.as_deref(),
                    Some("rp-cur-month"),
                    "a well-typed neighbour must survive"
                );
            }
            "kind" => {
                assert_eq!(
                    prefs.side.as_deref(),
                    Some("long"),
                    "a well-typed neighbour must survive"
                );
                assert_eq!(prefs.kind, None, "malformed kind must default to None");
                assert_eq!(
                    prefs.deleted_only,
                    Some(true),
                    "a well-typed neighbour must survive"
                );
                assert_eq!(
                    prefs.period.as_deref(),
                    Some("rp-cur-month"),
                    "a well-typed neighbour must survive"
                );
            }
            "deleted_only" => {
                assert_eq!(
                    prefs.side.as_deref(),
                    Some("long"),
                    "a well-typed neighbour must survive"
                );
                assert_eq!(
                    prefs.kind.as_deref(),
                    Some("real"),
                    "a well-typed neighbour must survive"
                );
                assert_eq!(
                    prefs.deleted_only, None,
                    "malformed deleted_only must default to None"
                );
                assert_eq!(
                    prefs.period.as_deref(),
                    Some("rp-cur-month"),
                    "a well-typed neighbour must survive"
                );
            }
            "period" => {
                assert_eq!(
                    prefs.side.as_deref(),
                    Some("long"),
                    "a well-typed neighbour must survive"
                );
                assert_eq!(
                    prefs.kind.as_deref(),
                    Some("real"),
                    "a well-typed neighbour must survive"
                );
                assert_eq!(
                    prefs.deleted_only,
                    Some(true),
                    "a well-typed neighbour must survive"
                );
                assert_eq!(prefs.period, None, "malformed period must default to None");
            }
            "period_overview" => {
                assert_eq!(
                    prefs.period.as_deref(),
                    Some("rp-cur-month"),
                    "a malformed overview period must not drop the legacy period"
                );
                assert_eq!(
                    prefs.period_overview, None,
                    "malformed period_overview must default to None"
                );
            }
            "strategy_name_mask" => {
                assert_eq!(
                    prefs.side.as_deref(),
                    Some("long"),
                    "a well-typed neighbour must survive"
                );
                assert_eq!(
                    prefs.kind.as_deref(),
                    Some("real"),
                    "a well-typed neighbour must survive"
                );
                assert_eq!(
                    prefs.deleted_only,
                    Some(true),
                    "a well-typed neighbour must survive"
                );
                assert_eq!(
                    prefs.period.as_deref(),
                    Some("rp-cur-month"),
                    "a well-typed neighbour must survive"
                );
            }
            _ => unreachable!(),
        }
        if bad_field != "period_overview" {
            assert_eq!(
                prefs.period_overview.as_deref(),
                Some("rp-today"),
                "a well-typed overview period must survive its malformed neighbour"
            );
        }
        if bad_field == "strategy_name_mask" {
            assert_eq!(
                prefs.strategy_name_mask, None,
                "a malformed mask must default alone"
            );
        } else {
            assert_eq!(
                prefs.strategy_name_mask.as_deref(),
                Some("EMA_"),
                "a well-typed mask must survive its malformed neighbour"
            );
        }
    }

    let prefs = super::ReportFilterPrefs {
        side: Some("short".to_string()),
        kind: Some("emu".to_string()),
        deleted_only: Some(true),
        period: Some("rp-cur-week".to_string()),
        period_overview: Some("rp-today".to_string()),
        strategy_name_mask: Some("EMA_%\\".to_string()),
    };
    let encoded = toml::to_string(&prefs).expect("serialize Report filters");
    let decoded: super::ReportFilterPrefs =
        toml::from_str(&encoded).expect("deserialize Report filters");
    assert_eq!(decoded, prefs, "both period buckets must round-trip");

    let legacy: super::ReportFilterPrefs = toml::from_str(
        "side = \"long\"\nperiod = \"rp-cur-year\"\nstrategy_name_mask = \"EMA_\"\n",
    )
    .expect("deserialize legacy Report filters");
    assert_eq!(legacy.period.as_deref(), Some("rp-cur-year"));
    assert_eq!(legacy.period_overview, None);

    // One level up, the salvage is coarser by design: an entry that is not a table at all takes
    // the whole `report_filters` map down to empty, never the rest of the layout document.
    let doc = "analytics_period = \"p-cur-month\"\nreport_filters = 5\n";
    let decoded: WindowLayout = toml::from_str(doc).unwrap_or_else(|e| {
        panic!("a malformed report_filters map must not fail the whole document: {e}")
    });
    assert_eq!(decoded.analytics_period.as_deref(), Some("p-cur-month"));
    assert!(decoded.report_filters.is_empty());
}
