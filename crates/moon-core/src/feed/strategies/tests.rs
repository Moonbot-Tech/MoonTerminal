use super::*;

/// `0` in KeepInChart/AddToChart is a MEANING, so garbage has to yield `None` — and with it the
/// caller's default — rather than quietly folding to zero and reading as one of those meanings.
#[test]
fn garbage_is_none_not_zero() {
    assert_eq!(field_num(&FieldValue::Double(f64::NAN)), None);
    assert_eq!(field_num(&FieldValue::Single(f32::NAN)), None);
    assert_eq!(field_num(&FieldValue::Double(f64::INFINITY)), None);
    assert_eq!(field_num(&FieldValue::Int32(-1)), None);
    assert_eq!(field_num(&FieldValue::Int64(-1)), None);
    assert_eq!(field_num(&FieldValue::String("60".into())), None);
}

/// Too large is garbage too: neither wrapping modulo 2^32, which would fake a `0`, nor
/// saturating, which would open AddToChart tab number 4294967295.
#[test]
fn oversized_is_none_neither_wraps_nor_saturates() {
    assert_eq!(field_num(&FieldValue::UInt64(1u64 << 32)), None);
    assert_eq!(field_num(&FieldValue::Int64(1i64 << 32)), None);
    assert_eq!(field_num(&FieldValue::Double(1e30)), None);
    // The upper bound itself is a value, not garbage.
    assert_eq!(
        field_num(&FieldValue::UInt64(u32::MAX as u64)),
        Some(u32::MAX)
    );
}

#[test]
fn plain_values_pass_through() {
    assert_eq!(field_num(&FieldValue::Int32(0)), Some(0));
    assert_eq!(field_num(&FieldValue::Int32(60)), Some(60));
    assert_eq!(field_num(&FieldValue::Double(60.0)), Some(60));
    assert_eq!(field_num(&FieldValue::Bool(true)), Some(1));
}

/// A Russian keyboard types the decimal separator as a COMMA. `parse::<f64>` rejects "0,5", and
/// `unwrap_or(0.0)` used to turn that into a silent zero which the core then answered with its own
/// default — the user saw a parameter he never typed. Reported live on MShotPriceMin/MShotPrice.
#[test]
fn a_decimal_comma_reads_as_a_dot() {
    assert_eq!(
        fv_from_str(Some(&FieldValue::Double(7.0)), None, "0,5"),
        Some(FieldValue::Double(0.5))
    );
    // Also when only the schema knows the type, which is the path for a field the core omitted
    // because it still holds its default.
    assert_eq!(
        fv_from_str(None, Some(StrategyFieldType::Single), " 1,25 "),
        Some(FieldValue::Single(1.25))
    );
}

/// The reason the zero was worse than a refusal: it is a VALID parameter. "no distance", "no
/// stop", "no size" all read as deliberate to the core, so text that is not a number must leave
/// the field alone instead.
#[test]
fn unparsable_text_is_refused_not_zeroed() {
    for text in ["", "  ", "0.5%", "0 5", "abc", "-", "1,2,3"] {
        assert_eq!(
            fv_from_str(Some(&FieldValue::Double(7.0)), None, text),
            None,
            "{text:?} must not become a number"
        );
    }
    // A fraction is not an integer: truncating it would send a value the user did not type.
    assert_eq!(fv_from_str(Some(&FieldValue::Int32(3)), None, "2.5"), None);
}

/// Out of range is refused rather than wrapped: `300 as u8` used to reach the core as 44, and
/// a byte field holding 44 is indistinguishable from one the user set to 44 on purpose.
#[test]
fn out_of_range_is_refused_not_wrapped() {
    assert_eq!(fv_from_str(Some(&FieldValue::Byte(1)), None, "300"), None);
    assert_eq!(fv_from_str(Some(&FieldValue::Word(1)), None, "70000"), None);
    assert_eq!(
        fv_from_str(Some(&FieldValue::Int32(1)), None, "3000000000"),
        None
    );
    assert_eq!(fv_from_str(Some(&FieldValue::UInt32(1)), None, "-1"), None);
    // The boundary itself is a value.
    assert_eq!(
        fv_from_str(Some(&FieldValue::Byte(1)), None, "255"),
        Some(FieldValue::Byte(255))
    );
}

/// A checkbox has two states and a string field takes any text, so neither can refuse: only the
/// numeric conversions gained a way to say no.
#[test]
fn bool_and_string_stay_total() {
    assert_eq!(
        fv_from_str(Some(&FieldValue::Bool(true)), None, "nonsense"),
        Some(FieldValue::Bool(false))
    );
    assert_eq!(
        fv_from_str(Some(&FieldValue::Bool(false)), None, "Yes"),
        Some(FieldValue::Bool(true))
    );
    assert_eq!(
        fv_from_str(None, None, "Last1hDelta"),
        Some(FieldValue::String("Last1hDelta".into()))
    );
}

/// Text that parses perfectly well and still is not a value of the field: `1e400` becomes `inf`,
/// `1e-400` a real `0.0`, and an f64 that survives its own type underflows to `0.0f32` on the way
/// into a Single field. The zeros are the silent zero this function exists to stop; the infinity is
/// a threshold that would compare false to everything the core measures against it.
#[test]
fn a_number_that_parses_but_is_not_a_value_is_refused() {
    // Overflow: `1e400` parses to `inf`, and `1e300` survives f64 only to overflow f32.
    assert_eq!(
        fv_from_str(None, Some(StrategyFieldType::Double), "1e400"),
        None
    );
    assert_eq!(
        fv_from_str(None, Some(StrategyFieldType::Single), "1e300"),
        None
    );
    assert_eq!(
        fv_from_str(None, Some(StrategyFieldType::Double), "1e-400"),
        None
    );
    assert_eq!(
        fv_from_str(None, Some(StrategyFieldType::Single), "1e-60"),
        None
    );
    // A zero the user did type stays a zero, however it is spelled.
    assert_eq!(
        fv_from_str(None, Some(StrategyFieldType::Double), "0,000"),
        Some(FieldValue::Double(0.0))
    );
    assert_eq!(
        fv_from_str(None, Some(StrategyFieldType::Single), "-0.0"),
        Some(FieldValue::Single(-0.0))
    );
}

/// The panel's marker and the sender must agree, or a row reads as accepted and is then dropped on
/// the way out. This compares the two ACROSS both of the sender's branches — the schema type and
/// the type of the value the core last sent — because `field_text_is_valid` can only consult the
/// first, and a check that walked one branch would agree with itself by construction.
#[test]
fn the_ui_check_agrees_with_the_conversion_on_both_branches() {
    let cases: [(&str, StrategyFieldType, FieldValue); 6] = [
        ("Double", StrategyFieldType::Double, FieldValue::Double(1.0)),
        ("Single", StrategyFieldType::Single, FieldValue::Single(1.0)),
        ("Int32", StrategyFieldType::Int32, FieldValue::Int32(1)),
        ("Byte", StrategyFieldType::Byte, FieldValue::Byte(1)),
        ("Word", StrategyFieldType::Word, FieldValue::Word(1)),
        ("UInt64", StrategyFieldType::UInt64, FieldValue::UInt64(1)),
    ];
    for text in [
        "0,5", "0.5", "0,5%", "2.5", "300", "70000", "-1", "", "abc", "1e-400", "12",
    ] {
        for (name, stype, existing) in &cases {
            let marker = field_text_is_valid(name, text);
            assert_eq!(
                marker,
                fv_from_str(None, Some(*stype), text).is_some(),
                "{name} {text:?}: marker disagrees with the schema-type branch"
            );
            assert_eq!(
                marker,
                fv_from_str(Some(existing), None, text).is_some(),
                "{name} {text:?}: marker disagrees with the stored-value branch"
            );
        }
    }
    // A string field takes anything, and so does a type name this build does not know.
    assert!(field_text_is_valid("String", "anything at all"));
    assert!(field_text_is_valid("Unknown", ""));
}

/// A `SoundKind` value is a file name, not an entry in a list this crate keeps.
///
/// Plausible breakage: reinstating a whitelist of the embedded stems here would turn a user's own
/// `MYSOUND.wav` — legitimately named by a Moonbot strategy — back into "no sound field", which the
/// caller then reads as the schema default. The player is the one place that knows which names
/// exist, and it reports a missing one rather than swallowing it.
#[test]
fn a_sound_kind_is_normalized_not_filtered() {
    assert_eq!(sound_stem("BABYTOY"), Some("babytoy".into()));
    assert_eq!(sound_stem(" ding1.wav "), Some("ding1".into()));
    assert_eq!(sound_stem("MySound.WAV"), Some("mysound".into()));
    assert_eq!(
        sound_stem("NotEmbedded"),
        Some("notembedded".into()),
        "an unknown name must travel to the player, which reports it; dropping it here is silent"
    );
}

/// `NONE` and an empty value both mean silence, in either spelling Moonbot uses.
#[test]
fn none_and_empty_mean_silence() {
    assert_eq!(sound_stem("NONE"), None);
    assert_eq!(sound_stem("none"), None);
    assert_eq!(sound_stem(""), None);
    assert_eq!(sound_stem("   "), None);
    assert_eq!(
        sound_stem(".wav"),
        None,
        "an extension with no stem names nothing"
    );
}

/// Bypassing shared normalization makes archive-prefixed strategy alerts miss the catalog.
#[test]
fn strategy_sound_ignores_archive_folders() {
    assert_eq!(sound_stem("sounds/hook"), Some("hook".into()));
    assert_eq!(sound_stem(r"sounds\hook"), Some("hook".into()));
    assert_eq!(sound_stem("sounds/"), None);
}

/// A snapshot whose only interesting state is the fields a test names.
///
/// Id 9 makes the identifier fallback `strat 9`, so a test that expected a real name and got the
/// fallback cannot pass by accident.
fn snapshot_with(pairs: &[(&str, FieldValue)]) -> StrategySnapshot {
    let mut fields = StrategyFields::new();
    for (name, value) in pairs {
        fields.insert(*name, value.clone());
    }
    StrategySnapshot::new(
        9,
        1,
        0,
        false,
        moonproto::StrategyKind::MOON_SHOT,
        "",
        fields,
    )
}

/// An omitted timing field is the caller's default, and the three callers do not share one.
///
/// `KeepAlert` waits a minute, `AddToChart` means "do not add", and `KeepInChart` waits a minute
/// only when no schema can say that the omitted field was the zero meaning "keep forever".
/// Collapsing the three to one number would open a chart tab for a strategy that asked for none,
/// or drop a detect alert immediately.
#[test]
fn missing_timing_fields_take_their_own_defaults() {
    let params = alert_params(&snapshot_with(&[]), None);
    assert!(!params.sound_alert);
    assert_eq!(params.sound_name, None);
    assert_eq!(params.keep_alert_secs, 60);
    assert_eq!(params.add_to_chart, 0);
    assert_eq!(params.keep_in_chart_secs, 60);
}

/// Zero stored on the strategy is a meaning — "keep forever", "do not add" — not a missing field.
///
/// The server omits a field that still equals its schema default, so a zero that DID arrive has
/// already been judged different from that default. Treating it as absent would replace a chart
/// the user pinned with the sixty-second fallback.
#[test]
fn a_stored_zero_stays_zero() {
    let params = alert_params(
        &snapshot_with(&[
            ("KeepAlert", FieldValue::Int32(0)),
            ("KeepInChart", FieldValue::UInt32(0)),
            ("AddToChart", FieldValue::UInt32(2)),
        ]),
        None,
    );
    assert_eq!(params.keep_alert_secs, 0);
    assert_eq!(params.keep_in_chart_secs, 0);
    assert_eq!(params.add_to_chart, 2);
}

/// A present field that cannot be read falls back to the caller's default, never to zero.
///
/// Zero in `KeepInChart` and `AddToChart` is itself a meaning. Folding NaN, a negative, or a
/// string onto that meaning would pin a chart forever or refuse to add one because a value failed
/// to parse.
#[test]
fn garbage_in_a_present_timing_field_uses_the_caller_default() {
    let params = alert_params(
        &snapshot_with(&[
            ("KeepAlert", FieldValue::Int32(-1)),
            ("KeepInChart", FieldValue::Double(f64::NAN)),
            ("AddToChart", FieldValue::String("4".into())),
        ]),
        None,
    );
    assert_eq!(params.keep_alert_secs, 60);
    assert_eq!(params.keep_in_chart_secs, 60);
    assert_eq!(
        params.add_to_chart, 0,
        "text in a numeric field is not parsed as that number"
    );
}

/// The sound the strategy names plays even when `SoundAlert` is off.
///
/// The flag decides whether an alert is raised. The stem is a separate field, and dropping it
/// whenever the flag is false would silence a strategy that named `DING1` the moment that flag
/// is the one the server bothered to send.
#[test]
fn a_named_sound_plays_while_the_alert_flag_is_off() {
    let params = alert_params(
        &snapshot_with(&[
            ("SoundAlert", FieldValue::Bool(false)),
            ("SoundKind", FieldValue::String("sounds/DING1.wav".into())),
        ]),
        None,
    );
    assert!(!params.sound_alert);
    assert_eq!(params.sound_name, Some("ding1".into()));
}

/// No strategy behind a detect prints nothing. A strategy that exists is never nameless.
///
/// An alert is a drawn chart object and has no snapshot; an empty card label is how a caption
/// tells those apart from a strategy the core simply did not name. The unnamed strategy still
/// has an id, including when the only characters in its name are invisible.
#[test]
fn an_absent_detect_is_nameless_and_a_blank_strategy_uses_its_id() {
    assert_eq!(detect_strat_name(None), "");
    assert_eq!(
        detect_strat_name(Some(&snapshot_with(&[]))),
        "strat 9",
        "a missing StrategyName is the identifier, not a blank caption"
    );
    assert_eq!(
        detect_strat_name(Some(&snapshot_with(&[(
            "StrategyName",
            FieldValue::String(String::new())
        )]))),
        "strat 9"
    );
    assert_eq!(
        detect_strat_name(Some(&snapshot_with(&[(
            "StrategyName",
            FieldValue::String("\u{200b}\u{202e}".into()),
        )]))),
        "strat 9",
        "bidi and zero-width marks are not a name"
    );
    assert_eq!(
        strat_display_name(&snapshot_with(&[(
            "StrategyName",
            FieldValue::String("   ".into())
        )])),
        "strat 9"
    );
}

/// Leading blanks are removed before the length cut, and a newline does not survive into the caption.
///
/// The cut is [`crate::feed::DETECT_STRAT_NAME_KEEP`] characters of the trimmed name. Cutting the
/// raw string first spends that budget on the padding, and a name that is only blanks inside the
/// window then falls through to `strat <id>` — the answer reserved for a strategy that sent no
/// name at all. A newline is a control character, so it becomes a space; joining the two words
/// would rename the strategy.
#[test]
fn the_strategy_name_is_trimmed_before_the_length_cut() {
    let keep = crate::feed::DETECT_STRAT_NAME_KEEP;
    let padded = format!("{}Moon", " ".repeat(keep));
    assert_eq!(
        detect_strat_name(Some(&snapshot_with(&[(
            "StrategyName",
            FieldValue::String(padded),
        )]))),
        "Moon",
        "padding wider than the cut must not erase the name"
    );
    let long = format!("{}{}", " ".repeat(keep), "B".repeat(keep + 10));
    assert_eq!(
        detect_strat_name(Some(&snapshot_with(&[(
            "StrategyName",
            FieldValue::String(long),
        )]))),
        "B".repeat(keep)
    );
    assert_eq!(
        detect_strat_name(Some(&snapshot_with(&[(
            "StrategyName",
            FieldValue::String("  Foo\nBar  ".into()),
        )]))),
        "Foo Bar"
    );
}

/// Trimmed from the user's MoonBot export. The service keys and the mixed-case `SellPrice` are
/// the ones the create path used to forward verbatim.
const PASTE_SAMPLE: &str = "\
#Begin_Folder DIPBUY - LONG REBOUND AFTER DROPS LLM
##Begin_Strategy
Active=0
FVersion=12
StrategyName=DROPS_02 - LONG REBOUND AFTER DROPS [94HQE5E7]
LastEditDate=2026-09-25 12:00
SignalType=DropsDetection
DropsMaxTime=90
buyPrice=0.3
SellPrice=2.2
OrderSize=300
MaxPing=600
##End_Strategy
#End_Folder
";

/// Split one MoonBot strategy block into the pairs the create command receives.
///
/// Not the product parser. That lives in the UI crate and is what turns `DropsDetection` into the
/// Drops kind. This only reads the `Key=Value` lines the builder is handed afterwards.
fn moonbot_field_pairs(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line == "##Begin_Strategy" {
            inside = true;
            continue;
        }
        if line == "##End_Strategy" {
            break;
        }
        if !inside {
            continue;
        }
        let (key, value) = line.split_once('=').expect("sample field");
        pairs.push((key.to_string(), value.to_string()));
    }
    pairs
}

fn known(
    name: &str,
    type_id: StrategyFieldType,
    default_value: Option<FieldValue>,
    visible_ordinals: &[u8],
) -> KnownStrategyField {
    KnownStrategyField {
        name: name.to_string(),
        type_id,
        default_value,
        visible_ordinals: visible_ordinals.to_vec(),
    }
}

/// Sending `Active` and `FVersion` makes every MoonBot paste come back Adjusted: the core drops a
/// name its schema does not have, the echo lacks it, and `field_matches` returns false. Mapping
/// `SellPrice` onto `sellPrice` is the same failure one step earlier — the writer never finds the
/// mixed-case key, so the value the user typed is dropped too.
#[test]
fn pasted_sample_sends_only_schema_fields_in_schema_spelling() {
    let pairs = moonbot_field_pairs(PASTE_SAMPLE);
    assert!(
        pairs
            .iter()
            .any(|(key, value)| key == "Active" && value == "0")
    );
    assert!(pairs.iter().any(|(key, _)| key == "FVersion"));
    assert!(
        pairs
            .iter()
            .any(|(key, value)| key == "SignalType" && value == "DropsDetection")
    );
    let drops = StrategyKind::DROPS.ordinal();
    let schema = vec![
        known("StrategyName", StrategyFieldType::String, None, &[drops]),
        known("SignalType", StrategyFieldType::String, None, &[drops]),
        known(
            "buyPrice",
            StrategyFieldType::Double,
            Some(FieldValue::Double(0.0)),
            &[drops],
        ),
        known(
            "sellPrice",
            StrategyFieldType::Double,
            Some(FieldValue::Double(0.0)),
            &[drops],
        ),
        known(
            "orderSize",
            StrategyFieldType::Int32,
            Some(FieldValue::Int32(0)),
            &[drops],
        ),
        known(
            "DropsMaxTime",
            StrategyFieldType::Int32,
            Some(FieldValue::Int32(0)),
            &[drops],
        ),
        // Present in the schema and in the paste, but hidden from this kind.
        known("MaxPing", StrategyFieldType::Int32, None, &[99]),
    ];
    let fields = fields_from_known(&schema, StrategyKind::DROPS, &pairs, 1, "create strategy 1");
    assert!(
        fields.get("Active").is_none(),
        "service key Active was sent"
    );
    assert!(
        fields.get("FVersion").is_none(),
        "service key FVersion was sent"
    );
    assert!(
        fields.get("LastEditDate").is_none(),
        "a key the schema does not know was sent"
    );
    assert!(
        fields.get("MaxPing").is_none(),
        "a field hidden from this kind was sent"
    );
    assert_eq!(
        fields.get("SignalType"),
        Some(&FieldValue::String("DropsDetection".into()))
    );
    assert_eq!(
        fields.get("StrategyName"),
        Some(&FieldValue::String(
            "DROPS_02 - LONG REBOUND AFTER DROPS [94HQE5E7]".into()
        ))
    );
    assert_eq!(fields.get("buyPrice"), Some(&FieldValue::Double(0.3)));
    assert_eq!(fields.get("sellPrice"), Some(&FieldValue::Double(2.2)));
    assert!(fields.get("SellPrice").is_none());
    assert_eq!(fields.get("orderSize"), Some(&FieldValue::Int32(300)));
    assert!(fields.get("OrderSize").is_none());
    assert_eq!(fields.get("DropsMaxTime"), Some(&FieldValue::Int32(90)));

    let mixed = fields_from_known(
        &schema,
        StrategyKind::DROPS,
        &[("BuyPrice".into(), "0.2".into())],
        1,
        "create strategy 2",
    );
    assert_eq!(mixed.get("buyPrice"), Some(&FieldValue::Double(0.2)));
    assert!(mixed.get("BuyPrice").is_none());

    // No schema yet: there is no list to filter against, and dropping every key would create a
    // strategy of defaults.
    let untyped = fields_from_text(None, StrategyKind::DROPS, &pairs, 1, "create strategy 3");
    assert!(untyped.get("Active").is_some());
}

fn snap(
    checked: bool,
    kind: StrategyKind,
    path: &str,
    fields: &[(&str, FieldValue)],
) -> StrategySnapshot {
    let mut built = StrategyFields::new();
    for (name, value) in fields {
        built.insert(*name, value.clone());
    }
    StrategySnapshot::new(1, 12, 5, checked, kind, path, built)
}

/// A paste that comes back Adjusted used to say only that the core saved something else. The note
/// has to name the one field that actually differs, and it must not name a field the echo omitted
/// because the value was already the schema default — that omission is how the core stores a
/// default, and reporting it would flag every untouched field.
#[test]
fn an_adjustment_names_the_field_that_differs_and_ignores_a_default() {
    let drops = StrategyKind::DROPS.ordinal();
    let known = vec![
        known(
            "buyPrice",
            StrategyFieldType::Double,
            Some(FieldValue::Double(0.0)),
            &[drops],
        ),
        known(
            "stopLoss",
            StrategyFieldType::Double,
            Some(FieldValue::Double(-1.0)),
            &[drops],
        ),
        known(
            "keepAlert",
            StrategyFieldType::Int32,
            Some(FieldValue::Int32(60)),
            &[drops],
        ),
        known("orderSize", StrategyFieldType::Int32, None, &[drops]),
    ];
    let desired = snap(
        false,
        StrategyKind::DROPS,
        "folder",
        &[
            ("buyPrice", FieldValue::Double(0.2)),
            ("stopLoss", FieldValue::Double(-1.0)),
            ("orderSize", FieldValue::Int32(0)),
        ],
    );
    let echo = snap(
        false,
        StrategyKind::DROPS,
        "folder",
        &[
            ("buyPrice", FieldValue::Double(0.3)),
            ("keepAlert", FieldValue::Int32(60)),
        ],
    );
    let changes = field_changes_against(&known, &desired, &echo);
    assert_eq!(
        changes,
        vec![StrategyFieldChange {
            name: "buyPrice".to_string(),
            sent: "0.2".to_string(),
            saved: "0.3".to_string(),
        }]
    );

    let bumped = snap(
        false,
        StrategyKind::DROPS,
        "folder",
        &[("orderSize", FieldValue::Int32(5))],
    );
    let omitted = snap(false, StrategyKind::DROPS, "folder", &[]);
    let implicit = field_changes_against(&known, &bumped, &omitted);
    assert_eq!(
        implicit,
        vec![StrategyFieldChange {
            name: "orderSize".to_string(),
            sent: "5".to_string(),
            saved: "0".to_string(),
        }]
    );

    let checked = snap(true, StrategyKind::DROPS, "folder", &[]);
    assert_eq!(
        field_changes_against(&[], &checked, &omitted)
            .iter()
            .map(|change| change.name.as_str())
            .collect::<Vec<_>>(),
        vec!["checked"]
    );
    let moved = snap(false, StrategyKind::DROPS, "other", &[]);
    assert_eq!(
        field_changes_against(&[], &omitted, &moved)
            .iter()
            .map(|change| change.name.as_str())
            .collect::<Vec<_>>(),
        vec!["path"]
    );
    let waves = snap(false, StrategyKind::WAVES, "folder", &[]);
    let kind_change = field_changes_against(&[], &omitted, &waves);
    assert_eq!(kind_change.len(), 1);
    assert_eq!(kind_change[0].name, "kind");
    assert_eq!(kind_change[0].sent, "Drops");
    assert_eq!(kind_change[0].saved, "Waves");
}
