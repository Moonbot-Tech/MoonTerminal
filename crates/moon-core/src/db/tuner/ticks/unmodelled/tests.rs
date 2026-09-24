use super::*;
use crate::db::tuner::ticks::params::{StrategyValues, unmodelled_rule};

fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn keys(found: &[UnmodelledField]) -> Vec<&'static str> {
    found.iter().map(|f| f.key).collect()
}

/// A strategy the model covers raises nothing; the fields the dump leaves out are at default.
#[test]
fn a_plain_strategy_raises_nothing() {
    let v = values(&[
        ("SellPrice", "1.5"),
        ("IgnoreSellShot", "YES"),
        ("SellShotDistance", "0.1"),
        ("IgnoreSellSpread", "True"),
        ("AutoSell", "True"),
        ("UseScalpingMode", "YES"),
    ]);
    assert!(unmodelled_fields(&v, &HashMap::new(), &FieldDeps::bundled()).is_empty());
}

#[test]
fn switched_on_fields_come_with_value_and_section() {
    let v = values(&[
        ("UseBV_SV_Stop", "YES"),
        ("PanicSellDelisted", "YES"),
        ("SellByFilters", "30"),
        ("IgnoreSellSpread", "NO"),
    ]);
    let found = unmodelled_fields(&v, &HashMap::new(), &FieldDeps::bundled());
    assert_eq!(
        keys(&found),
        [
            "UseBV_SV_Stop",
            "PanicSellDelisted",
            "SellByFilters",
            "IgnoreSellSpread"
        ]
    );
    assert_eq!(found[2].value, "30");
    assert_eq!(found[2].section, ParamSection::SellOrder);
    assert_eq!(found[1].section, ParamSection::Stops);
    assert_eq!(found[3].rule, Some(UnmodelledRule::SellSpread));
    assert_eq!(found[0].rule, None, "a warning, not a rule");
}

/// The stop ladder is modelled, and the liquidation guards and the grid's fixed stop are left out
/// on purpose (the developer's call, 2026-09-24): none of them raises the warning.
#[test]
fn the_ladder_and_the_left_out_stop_fields_raise_nothing() {
    let v = values(&[
        ("UseSecondStop", "YES"),
        ("UseStopLoss3", "YES"),
        ("DontSellBelowLiq", "True"),
        ("StopAboveLiq", "50"),
        ("StopLossFixed", "YES"),
    ]);
    assert!(unmodelled_fields(&v, &HashMap::new(), &FieldDeps::bundled()).is_empty());
}

/// The live schema's default wins over the one written here: a value AT it raises nothing.
#[test]
fn the_schema_default_decides_what_is_changed() {
    let v = values(&[("SellByFilters", "50")]);
    let defaults: HashMap<String, f64> = [("sellbyfilters".to_string(), 50.0)].into();
    assert!(unmodelled_fields(&v, &defaults, &FieldDeps::bundled()).is_empty());
    // A schema that says the sell is off by default fills `AutoSell` for the rule.
    let v = values(&[("SellByFilters", "30")]);
    let off: HashMap<String, f64> = [("autosell".to_string(), 0.0)].into();
    assert!(unmodelled_fields(&v, &off, &FieldDeps::bundled()).is_empty());
}

/// A field whose dependency rule does not hold is not in effect, as the Strategies window greys
/// it out: `SellByFilters` means nothing with `AutoSell` off.
#[test]
fn a_field_under_a_switch_that_is_off_is_not_in_effect() {
    let v = values(&[("AutoSell", "NO"), ("SellByFilters", "30")]);
    assert!(
        !keys(&unmodelled_fields(
            &v,
            &HashMap::new(),
            &FieldDeps::bundled()
        ))
        .contains(&"SellByFilters")
    );
    let on = values(&[("AutoSell", "YES"), ("SellByFilters", "30")]);
    assert_eq!(
        keys(&unmodelled_fields(
            &on,
            &HashMap::new(),
            &FieldDeps::bundled()
        )),
        ["SellByFilters"]
    );
}

/// `UseScalpingMode` acts only under a 1 % `SellPrice`; SellShot only with a distance.
#[test]
fn the_extra_conditions_hold_the_field_back() {
    let deps = FieldDeps::bundled();
    let wide = values(&[("UseScalpingMode", "YES"), ("SellPrice", "1.2")]);
    assert!(unmodelled_fields(&wide, &HashMap::new(), &deps).is_empty());
    let tight = values(&[("UseScalpingMode", "YES"), ("SellPrice", "0,5")]);
    assert_eq!(
        keys(&unmodelled_fields(&tight, &HashMap::new(), &deps)),
        ["UseScalpingMode"]
    );
    let still = values(&[("IgnoreSellShot", "NO"), ("SellShotDistance", "0")]);
    assert!(unmodelled_fields(&still, &HashMap::new(), &deps).is_empty());
    let shot = values(&[("IgnoreSellShot", "NO"), ("SellShotDistance", "0.5")]);
    let found = unmodelled_fields(&shot, &HashMap::new(), &deps);
    assert_eq!(keys(&found), ["IgnoreSellShot"]);
    assert_eq!(found[0].rule, Some(UnmodelledRule::SellShot));
}

/// `AutoSell` is watched the other way round: NO is what the model does not have.
#[test]
fn auto_sell_off_is_raised() {
    let v = values(&[("AutoSell", "NO")]);
    let found = unmodelled_fields(&v, &HashMap::new(), &FieldDeps::bundled());
    assert_eq!(keys(&found), ["AutoSell"]);
    assert_eq!(found[0].rule, Some(UnmodelledRule::NoAutoSell));
}

/// The warning and the verdict agree: every strategy the model does not judge
/// (`params::unmodelled_rule`) raises the field of that rule.
#[test]
fn every_unmodelled_rule_is_raised() {
    let cases = [
        values(&[("AutoSell", "NO")]),
        values(&[("IgnoreSellShot", "NO"), ("SellShotDistance", "0.1")]),
        values(&[("IgnoreSellSpread", "NO")]),
    ];
    let defaults = HashMap::new();
    for v in cases {
        let rule = unmodelled_rule(&StrategyValues {
            values: &v,
            defaults: &defaults,
        })
        .expect("a rule the model does not have");
        let found = unmodelled_fields(&v, &defaults, &FieldDeps::bundled());
        assert!(
            found.iter().any(|f| f.rule == Some(rule)),
            "{v:?} -> {found:?}"
        );
    }
}

/// Every field a watched rule's condition reads is fetched, spelled as the dump spells it — a
/// condition left unread would answer on its absence, which does not block.
#[test]
fn the_condition_fields_are_fetched() {
    let deps = FieldDeps::bundled();
    let fetched: Vec<String> = watched_keys()
        .into_iter()
        .map(|k| k.to_ascii_lowercase())
        .collect();
    for w in WATCHED {
        for field in deps.conditions_of(w.key) {
            assert!(
                fetched.iter().any(|k| k == field),
                "{} depends on {field}, which is never read",
                w.key
            );
        }
    }
}

/// A watched field is outside the model: none of them is a field the models act on.
#[test]
fn no_watched_field_is_read_by_the_model() {
    for w in WATCHED {
        assert!(
            !crate::db::tuner::ticks::params::is_model_only(w.key)
                && !crate::db::tuner::ticks::TICK_PARAMS
                    .iter()
                    .any(|p| p.key == w.key),
            "{} is read by the model",
            w.key
        );
    }
}

/// Each watched field sits in the section `assets/param_deps.toml` files it under — the section
/// the dialog names.
#[test]
fn every_watched_field_sits_in_its_section() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/param_deps.toml");
    let text = std::fs::read_to_string(&path).expect("param_deps.toml");
    let mut section = "";
    let mut filed: HashMap<String, String> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(title) = line
            .strip_prefix("# === ")
            .and_then(|s| s.strip_suffix(" ==="))
        {
            section = title;
        } else if let Some(rest) = line.strip_prefix('"') {
            if let Some(end) = rest.find('"') {
                filed.insert(rest[..end].to_string(), section.to_string());
            }
        }
    }
    for w in WATCHED {
        assert_eq!(
            filed.get(w.key).map(String::as_str),
            Some(w.section.schema_title()),
            "{}",
            w.key
        );
    }
}

/// A schema section of `(name, default)` fields.
fn schema(title: &str, fields: &[(&str, Option<&str>)]) -> crate::feed::SchemaSection {
    crate::feed::SchemaSection {
        title: title.to_string(),
        fields: fields
            .iter()
            .map(|(name, default)| crate::feed::SchemaField {
                name: (*name).to_string(),
                type_name: "Double".to_string(),
                ui: crate::feed::SchemaFieldUi::Edit,
                picklist: Vec::new(),
                default: default.map(str::to_string),
            })
            .collect(),
    }
}

/// The grid's reading: a field is in use when its rule holds and it is off its default. A dump
/// that leaves SellSpread's switch out keeps the section off (`IgnoreSellSpread` defaults to
/// YES), so its distance, though stored, is not in use; the stop's per cent under a stop left on
/// at default is.
#[test]
fn a_field_is_in_use_when_its_rule_holds_and_it_is_off_its_default() {
    let kind = [
        schema(
            "Sell order",
            &[
                ("HODLmode", Some("NO")),
                ("AutoSell", Some("YES")),
                ("SellPrice", Some("1")),
                ("SellDelay", Some("0")),
            ],
        ),
        schema(
            "Sell order\\SellSpread",
            &[
                ("IgnoreSellSpread", Some("YES")),
                ("SellSpreadDistance", Some("0.5")),
            ],
        ),
        schema(
            "Stops",
            &[
                ("UseStopLoss", Some("YES")),
                ("StopLoss", Some("-5")),
                ("UseSecondStop", Some("NO")),
                ("SecondStopLoss", None),
            ],
        ),
    ];
    let v = values(&[
        ("SellPrice", "1.5"),
        ("SellDelay", "0.0"),
        ("SellSpreadDistance", "0.8"),
        ("StopLoss", "-3"),
        ("SecondStopLoss", "-1"),
    ]);
    let used = fields_in_use(&kind, &v, &FieldDeps::bundled());
    assert_eq!(used, ["sellprice", "stoploss"]);
    // Switched on, SellSpread's switch and its distance are both in use; the second stop's per
    // cent stays out behind its switch, which is off by default.
    let mut on = v.clone();
    on.insert("IgnoreSellSpread".into(), "NO".into());
    let used = fields_in_use(&kind, &on, &FieldDeps::bundled());
    assert_eq!(
        used,
        [
            "sellprice",
            "ignoresellspread",
            "sellspreaddistance",
            "stoploss"
        ]
    );
    // A stop switched off takes its per cent out with it.
    on.insert("UseStopLoss".into(), "NO".into());
    let used = fields_in_use(&kind, &on, &FieldDeps::bundled());
    assert!(used.contains(&"usestoploss".to_string()), "{used:?}");
    assert!(!used.contains(&"stoploss".to_string()), "{used:?}");
}

/// The Delta Modifiers tab acts as a whole: `AddPriceBug` with no modifier applying the sum uses
/// nothing, and neither does a modifier with no term; the two together use the tab.
#[test]
fn the_delta_tab_is_in_use_only_when_a_modifier_applies_a_term() {
    let kind = [schema(
        "Delta Modifiers",
        &[
            ("SellModifier", Some("0")),
            ("StopLossModifier", Some("0")),
            ("MaxModifier", Some("0")),
            ("AddPriceBug", Some("0")),
            ("Add1minDelta", Some("0")),
        ],
    )];
    let deps = FieldDeps::bundled();
    let term = values(&[("AddPriceBug", "0.2"), ("MaxModifier", "5")]);
    assert!(fields_in_use(&kind, &term, &deps).is_empty());
    let modifier = values(&[("SellModifier", "0.3")]);
    assert!(fields_in_use(&kind, &modifier, &deps).is_empty());
    let both = values(&[
        ("AddPriceBug", "0.2"),
        ("SellModifier", "0.3"),
        ("MaxModifier", "5"),
    ]);
    assert_eq!(
        fields_in_use(&kind, &both, &deps),
        ["sellmodifier", "maxmodifier", "addpricebug"]
    );
}
