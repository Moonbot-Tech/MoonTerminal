//! Scope and save rules for the By-filter Detect group.
//!
//! The grid itself is a GPUI tree this binary crate cannot click. These tests drive the
//! decisions the grid, the master checkbox, and Save call.

use super::{
    changes_by_target, changes_for_kind, changes_remain, drops_only_scope, master_bit, offer_field,
};
use moon_core::db::analytics::GroupStat;
use moon_core::db::tuner::FieldClass;

/// Restoring the old no-holdout early return would hide material growth in the default mode.
#[test]
fn polish_default_full_history_summary_shows_material_staleness() {
    let split = super::state::SearchSplit {
        train: Default::default(),
        holdout: None,
        composed: None,
        compose_skipped: None,
    };
    let stats = |n| {
        vec![moon_core::db::tuner::VarStats {
            n,
            ..Default::default()
        }]
    };
    let baseline = super::super::freshness::SampleSnapshot::from_count(100, 100);
    assert!(!split.should_show_summary(baseline.materially_changed(Some(&stats(101)), 100)));
    assert!(split.should_show_summary(baseline.materially_changed(Some(&stats(105)), 100)));
}

fn row(key: &str, kind: &str) -> GroupStat {
    GroupStat {
        key: key.to_string(),
        kind: kind.to_string(),
        ..GroupStat::default()
    }
}

fn sample() -> Vec<GroupStat> {
    vec![
        row("7@1", "DropsDetection"),
        row("8@1", "MoonShot"),
        row("9@2", "Drops"),
        row("10@1", "PumpDetection"),
        row("11@1", ""),
    ]
}

/// A scope that is not entirely Drops must not count as Drops-only.
///
/// The oracle is the kind text on each loaded row, not the function's own boolean.
///
/// Breakage this pins: treating an empty selection as "no strategies" and then accepting
/// the vacuous case, or accepting the schema spelling `Drops` only when the export spelling
/// `DropsDetection` is what the row actually stores. Either one shows xPriceDelta on a
/// MoonShot scope, and Save then writes `DropsPriceDelta` into that strategy.
#[test]
fn a_scope_is_drops_only_only_when_every_strategy_is_drops() {
    let rows = sample();
    assert!(!drops_only_scope(&[], &[]));
    assert!(!drops_only_scope(&rows, &[]), "the loaded list is mixed");
    assert!(!drops_only_scope(&rows, &[(7, Some(1)), (8, Some(1))]));
    assert!(!drops_only_scope(&rows, &[(8, Some(1))]));
    assert!(!drops_only_scope(&rows, &[(10, Some(1))]));
    assert!(!drops_only_scope(&rows, &[(11, Some(1))]));
    assert!(
        !drops_only_scope(&rows, &[(99, Some(1))]),
        "a selected key the list does not contain is not Drops"
    );
    assert!(
        !drops_only_scope(&rows, &[(7, None)]),
        "a bare id does not match a per-core row"
    );

    assert!(drops_only_scope(&rows, &[(7, Some(1))]));
    assert!(drops_only_scope(&rows, &[(9, Some(2))]));
    assert!(drops_only_scope(&rows, &[(7, Some(1)), (9, Some(2))]));

    let spelled = vec![row("1@1", " dropsdetection "), row("2@1", "DROPS")];
    assert!(drops_only_scope(&spelled, &[]));
    let moon = vec![row("1@1", "MoonShot")];
    assert!(!drops_only_scope(&moon, &[]));
}

/// Save must not emit a Detect parameter for a strategy that is not Drops.
///
/// The oracle is the input list with `DropsPriceDelta` removed by hand for the non-Drops
/// kinds, and left in place for both Drops spellings. Other parameters stay on every kind.
///
/// Breakage this pins: `changes_for_kind` matching the parameter name as a literal that
/// drifts from `FIELDS`, or treating only `DropsDetection` as Drops so a schema-kind row
/// loses a Min the user set on a Drops strategy.
#[test]
fn save_keeps_a_detect_parameter_only_for_a_drops_strategy() {
    let changes = vec![
        ("DropsPriceDelta".to_string(), "3".to_string()),
        ("MinLeverage".to_string(), "2".to_string()),
        ("Comment".to_string(), "note".to_string()),
    ];
    let kept = vec![
        ("MinLeverage".to_string(), "2".to_string()),
        ("Comment".to_string(), "note".to_string()),
    ];
    assert_eq!(changes_for_kind(&changes, Some("MoonShot")), kept);
    assert_eq!(changes_for_kind(&changes, Some("PumpDetection")), kept);
    assert_eq!(changes_for_kind(&changes, Some("")), kept);
    assert_eq!(changes_for_kind(&changes, None), kept);
    assert_eq!(changes_for_kind(&changes, Some("DropsDetection")), changes);
    assert_eq!(changes_for_kind(&changes, Some("Drops")), changes);
    assert_eq!(changes_for_kind(&changes, Some(" drops ")), changes);

    let (preview, per_target) =
        changes_by_target(&changes, &[Some("DropsDetection"), Some("MoonShot")]);
    assert!(
        preview.is_empty(),
        "a mixed selection has no shared list: the dialog would show it on every target"
    );
    let per_target = per_target.expect("a mixed selection writes one list per target");
    assert_eq!(per_target[0], changes);
    assert_eq!(per_target[1], kept);
    assert!(changes_remain(&preview, Some(&per_target)));

    let (preview, per_target) =
        changes_by_target(&changes, &[Some("Drops"), Some("DropsDetection")]);
    assert!(
        per_target.is_none(),
        "identical lists stay on the shared payload"
    );
    assert_eq!(preview, changes);

    let (preview, per_target) = changes_by_target(&changes, &[Some("MoonShot")]);
    assert!(per_target.is_none());
    assert_eq!(preview, kept);

    let only_detect = vec![("DropsPriceDelta".to_string(), "3".to_string())];
    let (preview, per_target) =
        changes_by_target(&only_detect, &[Some("MoonShot"), Some("PumpDetection")]);
    assert!(per_target.is_none());
    assert!(preview.is_empty());
    assert!(!changes_remain(&preview, per_target.as_deref()));
}

/// The Detect group is the only one the scope can hide.
#[test]
fn a_detect_field_is_offered_only_on_a_drops_scope() {
    assert!(!offer_field(FieldClass::Detect, false));
    assert!(offer_field(FieldClass::Detect, true));
    for class in [
        FieldClass::Base,
        FieldClass::Filter,
        FieldClass::Volume,
        FieldClass::Delta,
        FieldClass::Ping,
    ] {
        assert!(
            offer_field(class, false),
            "{class:?} stays on a mixed scope"
        );
    }
}

/// The master switch must not tick a hidden Detect field, and must clear one when it turns all off.
///
/// The four corners are the oracle: on/off crossed with offered/hidden, plus an unmapped field.
#[test]
fn the_master_switch_leaves_a_hidden_detect_field_unticked() {
    assert!(master_bit(false, true, true, true));
    assert!(!master_bit(false, true, true, false));
    assert!(
        master_bit(true, true, true, false),
        "all-on does not clear a hidden tick the user already had"
    );
    assert!(!master_bit(true, false, true, false));
    assert!(!master_bit(false, true, false, true));
}

/// The grid must consult the offer decision. Driving the GPUI tree is not available here.
///
/// Breakage this pins: drawing every `FIELDS` row again, which puts the Detect group back on
/// a MoonShot scope while the pure predicate still says it is hidden.
#[test]
fn the_fields_grid_skips_a_field_the_scope_does_not_offer() {
    let source = include_str!("mod.rs").replace("\r\n", "\n");
    let body = source
        .split_once("fn filter_table(")
        .expect("filter_table")
        .1
        .split("\n    fn ")
        .next()
        .expect("filter_table body");
    assert!(
        body.contains("if !offer_field(field.class, drops_only)"),
        "the row loop must skip a field the scope does not offer"
    );
    assert!(
        body.contains("all_mapped_enabled(drops_only)"),
        "the master tick must use the same scope the rows do"
    );
}
