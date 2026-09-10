//! Message-format regressions for strategy exports and folder batches pasted from MoonBot.

use super::parse;
use crate::strategies::tree::ops::{
    clip_to_text, clipboard_matches_internal, paste_plan, resolve_clipboard,
};
use moon_core::feed::SchemaKind;
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
