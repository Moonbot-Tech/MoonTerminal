//! Message-format regressions for strategy exports and folder batches pasted from MoonBot.

use super::{expand_magnitude, parse};
use crate::strategies::tree::ops::{
    clip_to_text, clipboard_matches_internal, paste_plan, resolve_clipboard,
};
use moon_core::feed::{SchemaField, SchemaFieldUi, SchemaKind, SchemaSection};
use std::collections::HashSet;

/// Reduced reproduction of the user's standalone MoonHook export, retaining literal field syntax.
const HOOK: &str = "##Begin_Strategy
Active=0
FVersion=12
StrategyName=HOOK_01 - SHARP SPIKE REBOUNDS [DJM5KYPJ]
LastEditDate=2026-09-10 12:00
SignalType=MoonHook
EmulatorMode=NO
SilentNoCharts=YES
HookDetectDepth=1.6
CoinsWhiteList=
StopLoss=-0.55
Comment=literal \\n and a=b
##End_Strategy";

/// Reduced reproduction of the second strategy in the user's folder export.
const DROPS: &str = "##Begin_Strategy
Active=0
StrategyName=DROPS_01 - SHARP SPIKE REBOUNDS [WNA7QX76]
Comment=Каскадный сброс 3.2% за 6с
SignalType=DropsDetection
DropsMaxTime=6
DropsPriceDelta=3.2
MaxPosition=0
##End_Strategy";

/// Destination schema exposes protocol ordinals separately from display names.
fn kinds() -> Vec<SchemaKind> {
    vec![
        SchemaKind {
            ordinal: 20,
            name: "MoonHook".into(),
            sections: vec![],
        },
        SchemaKind {
            ordinal: 2,
            name: "Drops".into(),
            sections: vec![],
        },
    ]
}

/// Literal values must survive external parsing rather than taking the internal unescape path.
#[test]
fn standalone_export_keeps_kind_name_and_literal_fields() {
    let clip = resolve_clipboard(None, Some(&HOOK.replace('\n', "\r\n")), &kinds()).unwrap();
    assert_eq!(clip.len(), 1);
    assert_eq!(clip[0].kind_ordinal, 20);
    assert_eq!(clip[0].name, "HOOK_01 - SHARP SPIKE REBOUNDS [DJM5KYPJ]");
    assert!(clip[0].rel_path.is_empty());
    assert_eq!(clip[0].src, None);
    for (key, value) in [
        ("StopLoss", "-0.55"),
        ("CoinsWhiteList", ""),
        ("Comment", "literal \\n and a=b"),
        ("SilentNoCharts", "YES"),
    ] {
        assert!(clip[0].fields.contains(&(key.into(), value.into())));
    }
}

/// A slash surrounded by spaces belongs to the folder name; both strategies land beneath it.
#[test]
fn folder_export_pastes_both_strategies_under_the_named_folder() {
    let text =
        format!("#Begin_Folder SPIKE / SHARP SPIKE REBOUNDS LLM\n{HOOK}\n{DROPS}\n#End_Folder ");
    let clip = resolve_clipboard(None, Some(&text), &kinds()).unwrap();
    assert_eq!(clip.len(), 2);
    assert_eq!(clip[1].kind_ordinal, 2);
    assert_eq!(clip[1].rel_path, ["SPIKE / SHARP SPIKE REBOUNDS LLM"]);
    let plan = paste_plan(&clip, &["Destination".into()], &HashSet::new());
    assert_eq!(plan.len(), 2);
    for item in &plan {
        assert_eq!(
            item.folder_path,
            "Destination/SPIKE / SHARP SPIKE REBOUNDS LLM"
        );
        assert_eq!(item.insert_after, None);
    }
    assert!(
        plan[1]
            .fields
            .contains(&("Comment".into(), "Каскадный сброс 3.2% за 6с".into()))
    );
}

/// Nested folder boundaries and message code fences must not become fields or lose siblings.
#[test]
fn fenced_nested_folders_preserve_each_strategy_placement() {
    let text = format!(
        "\u{feff}```ini\n#Begin_Folder Parent\n#Begin_Folder Child\n{HOOK}\n#End_Folder\n{DROPS}\n#End_Folder\n```"
    );
    let clip = parse(&text, &kinds()).unwrap();
    assert_eq!(clip[0].rel_path, ["Parent", "Child"]);
    assert_eq!(clip[1].rel_path, ["Parent"]);
}

/// Never create a partial folder or silently infer a kind when a message was truncated or unknown.
#[test]
fn malformed_batch_never_returns_its_valid_prefix() {
    for text in [
        format!("{HOOK}\n##Begin_Strategy\nStrategyName=unfinished"),
        format!("#Begin_Folder missing_end\n{HOOK}"),
        format!("{HOOK}\n#End_Folder"),
        format!(
            "{HOOK}\n{}",
            DROPS.replace("DropsDetection", "UnknownFutureType")
        ),
        HOOK.replace("StrategyName=", "MissingName="),
        HOOK.replace("Active=0", "Active=0\nActive=1"),
    ] {
        assert!(
            parse(&text, &kinds()).is_none(),
            "accepted malformed batch: {text}"
        );
    }
}

/// A fresh external copy supersedes retained local text; an unchanged local copy keeps its anchor.
#[test]
fn external_clipboard_supersedes_old_local_copy_without_losing_unchanged_anchors() {
    let mut local = parse(HOOK, &kinds()).unwrap();
    local[0].src = Some((7, 42));
    let external = resolve_clipboard(Some(&local), Some(DROPS), &kinds()).unwrap();
    assert_eq!(
        external[0].name,
        "DROPS_01 - SHARP SPIKE REBOUNDS [WNA7QX76]"
    );
    assert_eq!(external[0].src, None);
    let own_text = clip_to_text(&local).replace('\n', "\r\n");
    assert!(clipboard_matches_internal(Some(&local), Some(&own_text)));
    assert!(!clipboard_matches_internal(Some(&local), Some(DROPS)));
    assert!(!clipboard_matches_internal(
        Some(&local),
        Some("unrelated message")
    ));
    assert_eq!(
        resolve_clipboard(Some(&local), Some(&own_text), &kinds()),
        Some(local.clone())
    );
    assert_eq!(
        resolve_clipboard(Some(&local), None, &kinds()),
        Some(local.clone())
    );
    assert!(resolve_clipboard(Some(&local), Some("unrelated message"), &kinds()).is_none());
    assert!(resolve_clipboard(Some(&local), Some("##Begin_Strategy"), &kinds()).is_none());
}

/// The volume and delta lines of the user's MainShotL export, exactly as MoonBot's grid printed
/// them — the eleven fields the core's parser refused on 2026-09-17 — beside a string field that
/// happens to look like one of them.
const SHOT: &str = "##Begin_Strategy
StrategyName=MainShotL
SignalType=MoonShot
MinVolume=1400000
MaxVolume=100000M
MaxHourlyVolume=1000M
MinHourlyVolFast=30k
MaxHourlyVolFast=1000000M
Delta_3h_Max=1E11k
Delta2_Max=20.00k
Delta3_Max=1000000.00k
Delta_BTC_24_Max=5E08k
Delta_Market_Max=1E09k
MinuteVolDeltaMax=62.00
Comment=10k
Unknown_Field=10k
##End_Strategy";

fn field(name: &str, type_name: &str) -> SchemaField {
    SchemaField {
        name: name.into(),
        type_name: type_name.into(),
        ui: SchemaFieldUi::Edit,
        picklist: vec![],
        default: None,
    }
}

/// A MoonShot kind whose schema types the fields the way the live core does.
fn shot_kinds() -> Vec<SchemaKind> {
    vec![SchemaKind {
        ordinal: 6,
        name: "MoonShot".into(),
        sections: vec![SchemaSection {
            title: "Filters / Volume".into(),
            fields: vec![
                field("MinVolume", "Int64"),
                field("MaxVolume", "Int64"),
                field("MaxHourlyVolume", "Int64"),
                field("MinHourlyVolFast", "Int64"),
                field("MaxHourlyVolFast", "Int64"),
                field("Delta_3h_Max", "Double"),
                field("Delta2_Max", "Double"),
                field("Delta3_Max", "Double"),
                field("Delta_BTC_24_Max", "Double"),
                field("Delta_Market_Max", "Double"),
                field("MinuteVolDeltaMax", "Double"),
                field("Comment", "String"),
            ],
        }],
    }]
}

/// Every suffixed number lands as the value the core echoed after the same export was pasted into
/// MoonBot itself; a string field and a field the schema does not know keep their text.
#[test]
fn grid_magnitude_suffixes_expand_to_the_core_values() {
    let clip = parse(SHOT, &shot_kinds()).unwrap();
    for (key, value) in [
        ("MinVolume", "1400000"),
        ("MaxVolume", "100000000000"),
        ("MaxHourlyVolume", "1000000000"),
        ("MinHourlyVolFast", "30000"),
        ("MaxHourlyVolFast", "1000000000000"),
        ("Delta_3h_Max", "100000000000000"),
        ("Delta2_Max", "20000"),
        ("Delta3_Max", "1000000000"),
        ("Delta_BTC_24_Max", "500000000000"),
        ("Delta_Market_Max", "1000000000000"),
        ("MinuteVolDeltaMax", "62.00"),
        ("Comment", "10k"),
        ("Unknown_Field", "10k"),
    ] {
        assert!(
            clip[0].fields.contains(&(key.into(), value.into())),
            "{key}: {:?}",
            clip[0].fields.iter().find(|(k, _)| k == key)
        );
    }
}

/// The expansion is exact decimal arithmetic, and anything that is not a number with a suffix is
/// left for the field parser to refuse.
#[test]
fn magnitude_expansion_is_exact_and_refuses_non_numbers() {
    for (raw, expected) in [
        ("0.1k", "100"),
        ("1.5k", "1500"),
        ("-1.5k", "-1500"),
        ("1,5k", "1500"),
        ("0.0001k", "0.1"),
        ("1E-5k", "0.01"),
        ("1.2345k", "1234.5"),
        ("0k", "0"),
        ("-0k", "0"),
        (" 7M ", "7000000"),
        ("2.5M", "2500000"),
        ("12K", "12000"),
    ] {
        assert_eq!(expand_magnitude(raw).as_deref(), Some(expected), "{raw}");
    }
    for raw in [
        "",
        "k",
        "M",
        "1400000",
        "62.00",
        "YES",
        "abck",
        "1.2.3k",
        "1Ek",
        "1e400k",
        "1E-2147483648k",
        "10m",
        "1Gk",
        "--1k",
        "1k k",
    ] {
        assert_eq!(expand_magnitude(raw), None, "{raw}");
    }
}
