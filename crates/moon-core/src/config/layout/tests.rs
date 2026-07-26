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
