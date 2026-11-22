use super::{SECTION_TITLE, laid_out, put_manual_on_buttons, schema_of, written_kinds};
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

/// Writes `text` to a temporary file named after `tag`, reads it back as written kinds and removes it.
fn read_written(tag: &str, text: &str) -> Option<Vec<crate::feed::SchemaKind>> {
    let path = std::env::temp_dir().join(format!("recorded-{tag}-{}.json", std::process::id()));
    std::fs::write(&path, text).expect("a temporary file");
    let written = written_kinds(&path);
    let _ = std::fs::remove_file(&path);
    written
}

/// Catches a written editor losing a recorded value — a field the file does not list for its kind
/// would vanish from the editor while the strategy still holds it — a listed field being added twice
/// or its written widget replaced by the one read off the value, one kind's editor being handed to
/// another, and a kind the file names but no strategy holds going missing (the new-strategy dialog
/// reads its fields from the schema).
#[test]
fn written_kinds_keep_every_recorded_field_under_its_own_kind() {
    let written = read_written(
        "kinds",
        r#"{"kinds":[
            {"ordinal":18,"name":"Spread","sections":[
                {"title":"Main","fields":[{"name":"A","type":"String","ui":"Combo","picklist":["x"],"default":"x"}]},
                {"title":"Stops","fields":[{"name":"B","type":"Int32","ui":"Edit"}]}]},
            {"ordinal":12,"name":"Manual","sections":[
                {"title":"Main","fields":[{"name":"M","type":"Bool","ui":"Checkbox"}]}]},
            {"ordinal":2,"name":"DropsDetection","sections":[]}]}"#,
    )
    .expect("three kinds");

    let schema = laid_out(
        &[
            row(18, "Spread", &[("A", "5"), ("C", "Yes")]),
            row(12, "Manual", &[("M", "Yes"), ("N", "2")]),
            row(20, "MoonHook", &[("H", "1")]),
        ],
        &written,
    );
    let ordinals: Vec<u8> = schema.kinds.iter().map(|kind| kind.ordinal).collect();
    assert_eq!(
        ordinals,
        [18, 12, 2, 20],
        "every written kind, then the unwritten recorded one"
    );
    let names = |ordinal: u8, at: usize| -> Vec<&str> {
        let kind = schema
            .kinds
            .iter()
            .find(|kind| kind.ordinal == ordinal)
            .expect("kind");
        kind.sections[at]
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect()
    };
    let spread = &schema.kinds[0].sections;
    assert_eq!(spread[0].title, "Main");
    assert_eq!(names(18, 0), ["A"]);
    assert_eq!(spread[0].fields[0].ui, SchemaFieldUi::Combo);
    assert_eq!(names(18, 1), ["B", "C"], "the unlisted one goes last");
    assert_eq!(
        names(12, 0),
        ["M", "N"],
        "a Manual field stays on the Manual editor"
    );
    assert!(
        schema.kinds[2].sections.is_empty(),
        "nothing recorded, nothing added"
    );
    assert_eq!(
        names(20, 0),
        ["H"],
        "a kind the file lacks keeps its derived editor"
    );
}

/// Catches a file that cannot be read, or one in the old one-editor-for-all shape, taking the editor
/// down with it: the run falls back to the derived schema, which only `None` selects.
#[test]
fn an_unreadable_old_or_empty_file_gives_no_kinds() {
    assert!(read_written("empty", r#"{"kinds":[]}"#).is_none());
    assert!(read_written("old", r#"[{"title":"Main","fields":[]}]"#).is_none());
    assert!(read_written("junk", "not json").is_none());
    let absent = std::env::temp_dir().join(format!("recorded-absent-{}.json", std::process::id()));
    assert!(written_kinds(&absent).is_none());
}

/// Catches a button that names something no button may fire — a Spread or a MoonHook on the header,
/// where a click places a manual order on it — and a shown slot past the core's strategies, which
/// draws an empty button that fires nothing.
#[test]
fn only_manual_strategies_go_on_buttons_and_only_their_slots_show() {
    let (_, mut config) = crate::feed::live::default_core_settings();
    put_manual_on_buttons(
        &mut config,
        &[
            row(18, "Spread", &[]),
            StrategyRow {
                name: "Scalp".into(),
                ..row(12, "Manual", &[])
            },
            StrategyRow {
                name: "Bounce".into(),
                ..row(20, "MoonHook", &[])
            },
            StrategyRow {
                name: "Swing".into(),
                ..row(12, "Manual", &[])
            },
        ],
    );
    let buttons = &config.manual.strat_buttons;
    assert!(buttons.use_buttons);
    assert_eq!(&config.manual.strat_names[..3], ["Scalp", "Swing", ""]);
    assert_eq!(&buttons.show_button[..3], [true, true, false]);
}

/// Catches a core with no manual strategy switching its buttons on: the header would then hide the
/// picker behind a row of empty buttons.
#[test]
fn a_core_without_manual_strategies_keeps_its_buttons_off() {
    let (_, mut config) = crate::feed::live::default_core_settings();
    let before = config.manual.clone();
    put_manual_on_buttons(&mut config, &[row(18, "Spread", &[])]);
    assert_eq!(config.manual, before);
}
