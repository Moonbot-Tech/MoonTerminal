use std::collections::BTreeSet;

use super::*;
use crate::feed::types::core_settings::tests::wire_default as base;

/// Sections the table covers, as `(field prefix, struct name)` — the two spellings the source and
/// the keys use for one area.
const COVERED: &[(&str, &str)] = &[
    ("signals", "SignalsSettings"),
    ("auto_start", "AutoStartSettings"),
    ("btc_blink", "BtcBlinkSettings"),
    ("general", "GeneralSettings"),
    ("order_rules", "OrderRulesSettings"),
    ("gestures", "GestureSettings"),
    ("special", "SpecialSettings"),
    ("telegram", "TelegramSettings"),
    ("auto_buy", "AutoBuySettings"),
    ("interface", "InterfaceSettings"),
];

/// The `pub` fields of one struct as the source spells them.
fn source_fields(source: &str, struct_name: &str) -> BTreeSet<String> {
    let header = format!("pub struct {struct_name} {{");
    let start = source
        .find(&header)
        .unwrap_or_else(|| panic!("{struct_name} is not declared in core_settings.rs"));
    let body = &source[start + header.len()..];
    let end = body.find("\n}").expect("struct body must close");
    body[..end]
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("pub ")?;
            let (name, _) = rest.split_once(':')?;
            Some(name.trim().to_string())
        })
        .collect()
}

fn field(key: &str) -> &'static CoreField {
    &CORE_FIELDS[index_of(key).unwrap_or_else(|| panic!("{key} is in the table"))]
}

/// The one guarantee the table exists for: a field added to a covered section without an entry
/// here would silently escape every diff and every bulk apply. Read from the source rather than
/// asserted by count, so the failure names the field.
#[test]
fn table_names_every_field_of_every_covered_section() {
    let source = include_str!("../../core_settings.rs");
    for (prefix, struct_name) in COVERED {
        let expected = source_fields(source, struct_name);
        assert!(
            !expected.is_empty(),
            "{struct_name} parsed with no fields — the parser fell out of step with the source"
        );
        let listed: BTreeSet<String> = CORE_FIELDS
            .iter()
            .filter_map(|f| f.key.strip_prefix(&format!("{prefix}.")))
            .map(str::to_string)
            .collect();
        assert_eq!(
            listed, expected,
            "table entries for {prefix} differ from the source"
        );
    }
}

/// An entry outside the covered sections is a field the window could count and copy without a
/// page to draw it on — exactly what the table's coverage is meant to bound.
#[test]
fn table_has_no_entry_outside_the_covered_sections() {
    for field in CORE_FIELDS {
        let (prefix, _) = field.key.split_once('.').expect("keys are section.field");
        assert!(
            COVERED.iter().any(|(p, _)| *p == prefix),
            "{} lies outside the covered sections",
            field.key
        );
    }
}

#[test]
fn keys_are_unique() {
    let mut seen = BTreeSet::new();
    for field in CORE_FIELDS {
        assert!(seen.insert(field.key), "{} is listed twice", field.key);
    }
    assert_eq!(index_of("no.such_field"), None);
}

/// Each entry's mask names its own area and nothing else — the section line of the macro is what
/// spells both, so this is the check that the two builders it was given agree.
#[test]
fn every_entry_masks_exactly_its_own_area() {
    for field in CORE_FIELDS {
        let expected = match field.area {
            CoreConfigArea::Signals => FieldMask::EMPTY.with_signals(),
            CoreConfigArea::AutoStart => FieldMask::EMPTY.with_auto_start(),
            CoreConfigArea::BtcBlink => FieldMask::EMPTY.with_btc_blink(),
            CoreConfigArea::General => FieldMask::EMPTY.with_general(),
            CoreConfigArea::OrderRules => FieldMask::EMPTY.with_order_rules(),
            CoreConfigArea::Gestures => FieldMask::EMPTY.with_gestures(),
            CoreConfigArea::Special => FieldMask::EMPTY.with_special(),
            CoreConfigArea::Telegram => FieldMask::EMPTY.with_telegram(),
            CoreConfigArea::AutoBuy => FieldMask::EMPTY.with_auto_buy(),
            CoreConfigArea::Interface => FieldMask::EMPTY.with_interface(),
            other => panic!("{} claims area {other:?}", field.key),
        };
        assert_eq!(field.mask, expected, "{} masks the wrong area", field.key);
    }
}

/// `copy_into` moves exactly one field: after it the two agree on that field and on nothing else
/// they did not already agree on.
#[test]
fn copy_moves_one_field_and_differs_agrees() {
    let a = base();
    let mut b = base();
    b.general.take_profit_pct += 1.0;
    b.special.log_level += 1;
    let tp = field("general.take_profit_pct");
    assert!(tp.differs(&a, &b));
    assert_eq!(changed_fields(&a, &b).len(), 2);

    let mut into = a.clone();
    tp.copy_into(&b, &mut into);
    assert!(!tp.differs(&b, &into));
    // Only that field moved: the log level still says what `a` said.
    assert_eq!(into.special.log_level, a.special.log_level);
    assert_eq!(changed_fields(&a, &into), changed_fields(&a, &b)[..1]);
}

/// Strings and lists go through the same two functions as the scalars.
#[test]
fn heap_fields_copy_and_compare() {
    let a = base();
    let mut b = base();
    b.telegram.pump_channels.push("@chan".to_string());
    b.general.blacklist_text = "BTC,ETH".to_string();
    let changed = changed_fields(&a, &b);
    let keys: Vec<&str> = changed.iter().map(|&i| CORE_FIELDS[i].key).collect();
    assert_eq!(
        keys,
        vec!["general.blacklist_text", "telegram.pump_channels"]
    );
    let mut into = a.clone();
    for &i in &changed {
        CORE_FIELDS[i].copy_into(&b, &mut into);
    }
    assert_eq!(into, b);
    assert_eq!(
        CORE_FIELDS[changed[1]].show(&b),
        FieldValue::List(vec!["@chan".to_string()])
    );
}

/// Differences across a selection are against the FIRST configuration, and one configuration
/// differs from nothing.
#[test]
fn differing_fields_compares_everyone_against_the_first() {
    let a = base();
    let mut b = base();
    b.general.take_profit_on = !a.general.take_profit_on;
    let mut c = base();
    c.interface.hide_pnl = !a.interface.hide_pnl;
    assert!(differing_fields(&[&a]).is_empty());
    assert!(differing_fields(&[]).is_empty());
    let keys: Vec<&str> = differing_fields(&[&a, &b, &c])
        .iter()
        .map(|&i| CORE_FIELDS[i].key)
        .collect();
    assert_eq!(keys, vec!["general.take_profit_on", "interface.hide_pnl"]);
}

/// The mask for a set of fields is the union of their sections' masks.
#[test]
fn mask_for_fields_unions_areas() {
    let tp = index_of("general.take_profit_pct").expect("in table");
    let blink = index_of("btc_blink.blink_btc").expect("in table");
    assert_eq!(
        mask_for_fields(&[tp, blink]),
        FieldMask::EMPTY.with_general().with_btc_blink()
    );
    assert_eq!(mask_for_fields(&[]), FieldMask::EMPTY);
}

/// Scalars show with their type kept, so the UI can caption a flag rather than print `true`, and
/// an `f32` leaf keeps its width rather than printing its widened binary value.
#[test]
fn show_keeps_the_type() {
    let mut a = base();
    a.general.take_profit_on = true;
    a.general.take_profit_pct = 2.5;
    a.general.trailing_pct = 0.3;
    a.special.log_level = 3;
    assert_eq!(
        field("general.take_profit_on").show(&a),
        FieldValue::Bool(true)
    );
    assert_eq!(
        field("general.take_profit_pct").show(&a),
        FieldValue::Float(2.5)
    );
    assert_eq!(
        field("general.trailing_pct").show(&a),
        FieldValue::Float32(0.3)
    );
    assert_eq!(field("special.log_level").show(&a), FieldValue::Int(3));
}

/// Exactly the four short move gestures are derived, and only while the mirror is on.
#[test]
fn only_short_move_gestures_are_derived_and_only_under_the_mirror() {
    let mut on = base();
    on.gestures.same_hotkeys_for_move = true;
    let mut off = base();
    off.gestures.same_hotkeys_for_move = false;
    let derived: Vec<&str> = CORE_FIELDS
        .iter()
        .filter(|f| f.is_derived(&on))
        .map(|f| f.key)
        .collect();
    assert_eq!(
        derived,
        vec![
            "gestures.short_buy_move_click",
            "gestures.short_sell_move_click",
            "gestures.short_buy_move_click_2",
            "gestures.short_sell_move_click_2",
        ]
    );
    assert!(CORE_FIELDS.iter().all(|f| !f.is_derived(&off)));
}

/// The perturbed twin disagrees on EVERY table field — the property a control probe relies on to
/// find the field a setter writes whichever value one base already holds.
#[test]
fn perturbed_disagrees_on_every_field() {
    let a = base();
    let b = perturbed(&a);
    assert_eq!(changed_fields(&a, &b).len(), CORE_FIELDS.len());
    // And it is a fixed point of nothing: perturbing twice moves on again rather than back.
    let c = perturbed(&b);
    assert_eq!(changed_fields(&b, &c).len(), CORE_FIELDS.len());
}

/// A probe finds the field a setter writes even when one base already holds the value written —
/// the case a single-base probe misses — and a setter that writes two fields names both.
#[test]
fn probe_finds_the_fields_a_setter_writes() {
    let mut cfg = base();
    cfg.general.take_profit_on = true;
    let bases = ProbeBases::new(&cfg);
    // Writes the value the first base already holds.
    let found = bases.fields_written_by(&|c| c.general.take_profit_on = true);
    assert_eq!(
        found,
        vec![index_of("general.take_profit_on").expect("in table")]
    );
    // A radio option: two flags at once.
    let found = bases.fields_written_by(&|c| {
        c.auto_buy.look_full_link_cbd = true;
        c.auto_buy.advanced_filter_clipboard = false;
    });
    assert_eq!(
        found,
        vec![
            index_of("auto_buy.look_full_link_cbd").expect("in table"),
            index_of("auto_buy.advanced_filter_clipboard").expect("in table"),
        ]
    );
    // A dead row's setter writes nothing.
    assert!(bases.fields_written_by(&|_| {}).is_empty());
    // A long gesture's setter mirrors the short on whichever base has the mirror on — a copy, not
    // a write: only the long is reported. The mirror flag's own setter reports the flag alone
    // for the same reason.
    let found = bases.fields_written_by(&|c| {
        c.gestures
            .set_move_gesture(crate::feed::MoveRow::OpenPrimary, false, 9)
    });
    assert_eq!(
        found,
        vec![index_of("gestures.buy_move_click").expect("in table")]
    );
    let found = bases.fields_written_by(&|c| c.gestures.set_same_hotkeys(true));
    assert_eq!(
        found,
        vec![index_of("gestures.same_hotkeys_for_move").expect("in table")]
    );
}
