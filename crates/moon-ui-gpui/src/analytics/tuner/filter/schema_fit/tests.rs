use std::collections::{HashMap, HashSet};

use moon_core::feed::{
    SchemaField, SchemaFieldUi, SchemaKind, SchemaSection, StrategyRow, StrategySchemaModel,
};

use super::{fit_to_schema, shown_fields};

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn names(list: &[&str]) -> HashSet<String> {
    list.iter().map(|n| n.to_ascii_lowercase()).collect()
}

/// What a fit on a spot core produced before #1160: the zero-off leverage and mark bounds, a
/// real spot bound, and the flags the save cleared for them.
fn spot_fit() -> Vec<(String, String)> {
    pairs(&[
        ("MaxLeverage", "0"),
        ("MarkPriceMin", "0"),
        ("MarkPriceMax", "0"),
        ("BinancePriceBug", "1.5"),
        ("IgnoreBase", "NO"),
        ("IgnorePing", "NO"),
        ("IgnoreFilters", "NO"),
        ("Comment", "stamp"),
    ])
}

/// A spot schema without leverage and mark bounds: those leave the save, and so does the
/// `IgnoreBase` they alone cleared. The price-bug bound and the flags it needs stay.
#[test]
fn a_spot_target_loses_the_futures_bounds_and_the_flag_they_cleared() {
    let spot = names(&[
        "BinancePriceBug",
        "IgnoreBase",
        "IgnorePing",
        "IgnoreFilters",
        "Comment",
    ]);
    let fitted = fit_to_schema(&spot_fit(), Some(&spot), &HashMap::new());
    assert_eq!(
        fitted,
        pairs(&[
            ("BinancePriceBug", "1.5"),
            ("IgnorePing", "NO"),
            ("IgnoreFilters", "NO"),
            ("Comment", "stamp"),
        ])
    );
}

/// A flag the user staged on a subheader is their request, not a consequence of a bound.
#[test]
fn a_staged_flag_stays_when_its_bounds_leave() {
    let spot = names(&[
        "BinancePriceBug",
        "IgnoreBase",
        "IgnorePing",
        "IgnoreFilters",
    ]);
    let staged = HashMap::from([("IgnoreBase", false)]);
    let fitted = fit_to_schema(&spot_fit(), Some(&spot), &staged);
    assert!(fitted.contains(&("IgnoreBase".to_string(), "NO".to_string())));
    assert!(!fitted.iter().any(|(k, _)| k == "MaxLeverage"));
}

/// Leverage alone on spot: nothing is left to write, so the stamp must not claim a save.
#[test]
fn a_save_left_with_only_its_stamp_writes_nothing() {
    let changes = pairs(&[
        ("MaxLeverage", "0"),
        ("IgnoreBase", "NO"),
        ("IgnoreFilters", "NO"),
        ("Comment", "stamp"),
    ]);
    let spot = names(&["IgnoreBase", "IgnoreFilters", "Comment"]);
    assert!(fit_to_schema(&changes, Some(&spot), &HashMap::new()).is_empty());
}

/// A futures schema shows every bound, and a target nobody can judge keeps the list as it is.
#[test]
fn a_target_that_shows_everything_or_cannot_be_judged_is_unchanged() {
    let futures = names(&[
        "MaxLeverage",
        "MarkPriceMin",
        "MarkPriceMax",
        "BinancePriceBug",
        "IgnoreBase",
        "IgnorePing",
        "IgnoreFilters",
    ]);
    assert_eq!(
        fit_to_schema(&spot_fit(), Some(&futures), &HashMap::new()),
        spot_fit()
    );
    assert_eq!(
        fit_to_schema(&spot_fit(), None, &HashMap::new()),
        spot_fit()
    );
}

fn field(name: &str) -> SchemaField {
    SchemaField {
        name: name.to_string(),
        type_name: "Double".to_string(),
        ui: SchemaFieldUi::Edit,
        picklist: Vec::new(),
        default: None,
    }
}

fn schema(kinds: &[(u8, &[&str])]) -> StrategySchemaModel {
    StrategySchemaModel {
        kinds: kinds
            .iter()
            .map(|(ordinal, fields)| SchemaKind {
                ordinal: *ordinal,
                name: String::new(),
                sections: vec![SchemaSection {
                    title: String::new(),
                    fields: fields.iter().map(|n| field(n)).collect(),
                }],
            })
            .collect(),
    }
}

fn row(kind_ordinal: u8) -> StrategyRow {
    StrategyRow {
        id: 1,
        name: String::new(),
        kind: String::new(),
        kind_ordinal,
        folder_path: String::new(),
        checked: false,
        is_short: false,
        fields: Vec::new(),
    }
}

/// The names come from the strategy's own kind; a copy without a schema or a row is unknown.
#[test]
fn shown_fields_reads_the_kind_of_every_copy() {
    let spot = schema(&[(6, &["BinancePriceBug"]), (16, &["MinVolume"])]);
    let futures = schema(&[(6, &["MaxLeverage", "BinancePriceBug"])]);
    let moonshot = row(6);
    assert_eq!(
        shown_fields([(Some(&spot), Some(&moonshot))]),
        Some(names(&["BinancePriceBug"]))
    );
    assert_eq!(
        shown_fields([
            (Some(&spot), Some(&moonshot)),
            (Some(&futures), Some(&moonshot))
        ]),
        Some(names(&["BinancePriceBug", "MaxLeverage"]))
    );
    assert_eq!(shown_fields([(None, Some(&moonshot))]), None);
    assert_eq!(shown_fields([(Some(&spot), None)]), None);
    assert_eq!(shown_fields(std::iter::empty()), None);
}
