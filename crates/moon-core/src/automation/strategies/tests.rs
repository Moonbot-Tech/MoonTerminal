//! Machine-facing values must not inherit the UI's six-decimal formatting loss.

use super::*;

/// UI precision and JavaScript number limits must not change model-visible strategy values.
#[test]
fn exact_values_survive_unrelated_strategy_fields_and_javascript_integer_limits() {
    assert_eq!(
        value(&FieldValue::Double(0.000000123456789)),
        json!(0.000000123456789)
    );
    assert_eq!(
        value(&FieldValue::UInt64(18_446_744_073_709_551_615)),
        json!("18446744073709551615")
    );
    assert_eq!(
        value(&FieldValue::Int64(-9_223_372_036_854_775_808)),
        json!("-9223372036854775808")
    );
}

/// A schema with no kinds and no fields: the name guard runs before anything reads the schema.
fn empty_schema() -> StrategySchema {
    StrategySchema {
        format_version: 0,
        kinds: Vec::new(),
        fields: Vec::new(),
    }
}

/// One retained strategy of `kind` carrying `name` as its `StrategyName`.
fn named(id: u64, kind: StrategyKind, name: &str) -> StrategySnapshot {
    let mut fields = moonproto::StrategyFields::new();
    fields.insert("StrategyName", FieldValue::String(name.to_string()));
    StrategySnapshot::new(id, 1, 0, false, kind, "", fields)
}

/// The core set from the field report: a Manual strategy already named `Fast - 1`.
fn core_with_manual() -> Vec<StrategySnapshot> {
    vec![named(1, StrategyKind::MANUAL, "Fast - 1")]
}

/// The error text a refusal carries, or a panic naming what came back instead.
fn refusal(result: Result<Value>) -> String {
    match result {
        Ok(preview) => panic!("expected a refusal, got preview {preview}"),
        Err(error) => format!("{error:#}"),
    }
}

/// A MoonHook created under a name a Manual already carries is refused: `UseHookStrategy` names
/// its hook by name, and the core then resolved the Manual's parameters instead of the hook's.
#[test]
fn create_refuses_a_name_another_strategy_on_the_core_carries() {
    let mut full = core_with_manual();
    let action = Action::StrategyCreate {
        strategy_kind: StrategyKind::MOON_HOOK.ordinal(),
        name: "Fast - 1".to_string(),
        path: String::new(),
        fields: Default::default(),
    };
    let error = refusal(transform(&mut full, &empty_schema(), &action, None));
    assert!(error.contains("already used"), "{error}");
    assert_eq!(full.len(), 1, "a refused create must not land in the set");
}

/// Renaming through an edit of `StrategyName` is the second way to the same duplicate.
#[test]
fn edit_refuses_renaming_into_a_taken_name() {
    let mut full = core_with_manual();
    full.push(named(2, StrategyKind::MOON_HOOK, "Fast - hook"));
    let action = Action::StrategyEdit {
        id: 2,
        fields: [("StrategyName".to_string(), json!("Fast - 1"))].into(),
        checked: None,
        path: None,
        apply_to_orders: false,
    };
    let error = refusal(transform(&mut full, &empty_schema(), &action, None));
    assert!(error.contains("already used"), "{error}");
}

/// Names differing only in surrounding whitespace read as one name, so they collide too.
#[test]
fn surrounding_whitespace_does_not_make_a_name_free() {
    assert!(ensure_name_free(&core_with_manual(), None, " Fast - 1 ").is_err());
}

/// A strategy's own name never blocks it, so an edit of an already-duplicated pair stays possible.
#[test]
fn a_strategy_does_not_collide_with_itself() {
    assert!(ensure_name_free(&core_with_manual(), Some(1), "Fast - 1").is_ok());
}

/// Case does not make a name free: whether the core's lookup folds case is unknown.
#[test]
fn a_name_differing_in_case_collides() {
    assert!(ensure_name_free(&core_with_manual(), None, "FAST - 1").is_err());
}

/// A genuinely different name passes, so the guard is not refusing every create.
#[test]
fn a_different_name_is_free() {
    assert!(ensure_name_free(&core_with_manual(), None, "Fast - hook").is_ok());
}
