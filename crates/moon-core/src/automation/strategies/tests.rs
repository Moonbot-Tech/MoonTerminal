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

/// One strategy's part of an edit, setting `fields`.
fn part(id: u64, fields: &[(&str, Value)]) -> StrategyFieldEdit {
    StrategyFieldEdit {
        id,
        fields: fields
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect(),
        checked: None,
        path: None,
    }
}

/// An edit of `parts`, not applied to orders.
fn edit(parts: Vec<StrategyFieldEdit>) -> Action {
    Action::StrategyEdit {
        edits: parts,
        apply_to_orders: false,
    }
}

/// Renaming through an edit of `StrategyName` is the second way to the same duplicate.
#[test]
fn edit_refuses_renaming_into_a_taken_name() {
    let mut full = core_with_manual();
    full.push(named(2, StrategyKind::MOON_HOOK, "Fast - hook"));
    let action = edit(vec![part(2, &[("StrategyName", json!("Fast - 1"))])]);
    let error = refusal(transform(&mut full, &empty_schema(), &action, None));
    assert!(error.contains("already used"), "{error}");
}

/// The refusal of one entry names that strategy and says the batch went nowhere: the client has
/// to know which of hundreds to fix, and that the others were not sent either.
#[test]
fn a_refused_entry_names_its_strategy_and_sends_nothing() {
    let mut full = core_with_manual();
    full.push(named(2, StrategyKind::MOON_HOOK, "Fast - hook"));
    let before = full.clone();
    let action = edit(vec![
        part(2, &[("StrategyName", json!("Fast - 1"))]),
        part(1, &[]),
    ]);
    let error = match apply(&mut full, Some(&empty_schema()), &action, &json!({}), 1) {
        Ok(_) => panic!("expected a refusal"),
        // `to_string`, as the Station and the feed pass the error on: the cause must survive it.
        Err(error) => error.to_string(),
    };
    assert!(error.contains("strategy 2"), "{error}");
    assert!(error.contains("already used"), "{error}");
    assert!(error.contains("nothing was sent"), "{error}");
    assert_eq!(full, before, "a refused batch must leave the set as it was");
}

/// A strategy listed twice would let the second entry silently overwrite the first.
#[test]
fn a_strategy_listed_twice_is_refused() {
    let error = refusal(transform(
        &mut core_with_manual(),
        &empty_schema(),
        &edit(vec![part(1, &[]), part(1, &[])]),
        None,
    ));
    assert!(error.contains("listed twice"), "{error}");
}

/// An empty edit and one past the bound are refused before anything is built.
#[test]
fn a_batch_is_bounded_on_both_sides() {
    let mut full = core_with_manual();
    let empty = refusal(transform(&mut full, &empty_schema(), &edit(vec![]), None));
    assert!(empty.contains("1..500"), "{empty}");
    let many = (0..=MAX_BATCH as u64).map(|id| part(id, &[])).collect();
    let error = refusal(transform(&mut full, &empty_schema(), &edit(many), None));
    assert!(error.contains("1..500"), "{error}");
}

/// The revision stamp moves on every send; an edit that changes nothing else is no change.
#[test]
fn rows_differing_only_in_revision_hold_the_same_strategy() {
    let one = named(1, StrategyKind::MANUAL, "A");
    let mut stamped = one.clone();
    stamped.strategy_ver += 1;
    stamped.last_date += 5;
    assert!(same_content(&row(&one), &row(&stamped)));
    assert!(!same_content(
        &row(&one),
        &row(&named(1, StrategyKind::MANUAL, "B"))
    ));
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

/// What goes on the wire is read from the preview: a strategy marked unchanged must not be sent
/// and its revision must not be watched, or the client is told one thing and the core gets another.
#[test]
fn only_changed_strategies_are_sent_in_the_order_listed() {
    let preview = json!({"strategies":[
        {"id":"9","unchanged":false},{"id":"4","unchanged":true},{"id":"2","unchanged":false}]});
    assert_eq!(changed_ids(&preview), vec![9, 2]);
}
