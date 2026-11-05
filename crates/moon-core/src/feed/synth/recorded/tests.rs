use super::{SECTION_TITLE, laid_out, schema_of, written_sections};
use crate::feed::{SchemaFieldUi, StrategyRow};

/// A strategy row with the given kind and fields; the rest does not reach the schema.
fn row(kind_ordinal: u8, kind: &str, fields: &[(&str, &str)]) -> StrategyRow {
    StrategyRow {
        id: 1,
        name: "S".to_string(),
        kind: kind.to_string(),
        kind_ordinal,
        folder_path: String::new(),
        checked: false,
        is_short: false,
        fields: fields
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
    }
}

/// Catches a kind's field going missing because only its first strategy was read — the editor
/// would then hide a value the second strategy holds — and two kinds merging into one, which would
/// offer one kind's fields on the other's strategies.
#[test]
fn every_kind_gets_every_field_its_strategies_carry_once() {
    let schema = schema_of(&[
        row(18, "Spread", &[("Active", "1"), ("Comment", "x")]),
        row(18, "Spread", &[("Active", "2"), ("AllowedDrop", "-95")]),
        row(3, "Drops", &[("Active", "1")]),
    ]);
    assert_eq!(schema.kinds.len(), 2);
    assert_eq!(schema.kinds[0].ordinal, 18);
    assert_eq!(schema.kinds[0].name, "Spread");
    assert_eq!(schema.kinds[0].sections[0].title, SECTION_TITLE);
    let names: Vec<&str> = schema.kinds[0].sections[0]
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    assert_eq!(names, ["Active", "Comment", "AllowedDrop"]);
    assert_eq!(schema.kinds[1].sections[0].fields.len(), 1);
}

/// Catches a numeric field typed as text — `field_text_is_valid` would accept anything in it, and
/// the memo rule would draw it as a multi-line box — and an integer-valued one typed as an integer,
/// which then refuses a fraction the real field takes.
#[test]
fn a_fields_type_is_read_off_its_value() {
    let schema = schema_of(&[row(
        1,
        "K",
        &[("A", "Yes"), ("B", "-3"), ("C", "0.25"), ("D", "BTC,ETH")],
    )]);
    let found: Vec<(&str, SchemaFieldUi)> = schema.kinds[0].sections[0]
        .fields
        .iter()
        .map(|field| (field.type_name.as_str(), field.ui))
        .collect();
    assert_eq!(
        found,
        [
            ("Bool", SchemaFieldUi::Checkbox),
            ("Double", SchemaFieldUi::Edit),
            ("Double", SchemaFieldUi::Edit),
            ("String", SchemaFieldUi::Edit),
        ]
    );
}

/// Catches a written section losing a recorded value — a field the file does not list would vanish
/// from the editor while the strategy still holds it — and a listed field being added twice, or its
/// written widget replaced by the one read off the value.
#[test]
fn written_sections_keep_every_recorded_field() {
    let path = std::env::temp_dir().join(format!("recorded-schema-{}.json", std::process::id()));
    std::fs::write(
        &path,
        r#"[{"title":"Main","fields":[{"name":"A","type":"String","ui":"Combo","picklist":["x"],"default":"x"}]},
            {"title":"Stops","fields":[{"name":"B","type":"Int32","ui":"Edit"}]}]"#,
    )
    .expect("a temporary file");
    let written = written_sections(&path).expect("two sections");
    let _ = std::fs::remove_file(&path);

    let schema = laid_out(&[row(18, "Spread", &[("A", "5"), ("C", "Yes")])], &written);
    let sections = &schema.kinds[0].sections;
    let names = |at: usize| -> Vec<&str> {
        sections[at]
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect()
    };
    assert_eq!(sections[0].title, "Main");
    assert_eq!(names(0), ["A"]);
    assert_eq!(sections[0].fields[0].ui, SchemaFieldUi::Combo);
    assert_eq!(names(1), ["B", "C"], "the unlisted one goes last");
}

/// Catches a file that cannot be read taking the editor down with it: the run falls back to the
/// derived schema, which only `None` selects.
#[test]
fn an_unreadable_or_empty_file_gives_no_sections() {
    let path = std::env::temp_dir().join(format!("recorded-empty-{}.json", std::process::id()));
    std::fs::write(&path, "[]").expect("a temporary file");
    assert!(written_sections(&path).is_none());
    std::fs::write(&path, "not json").expect("a temporary file");
    assert!(written_sections(&path).is_none());
    let _ = std::fs::remove_file(&path);
    assert!(written_sections(&path).is_none());
}
