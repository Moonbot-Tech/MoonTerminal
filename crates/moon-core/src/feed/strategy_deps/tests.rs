use super::*;

fn values(pairs: &[(&str, &str)]) -> Values {
    pairs
        .iter()
        .map(|(k, v)| (k.to_lowercase(), (*v).to_string()))
        .collect()
}

#[test]
fn every_condition_must_hold() {
    let deps =
        FieldDeps::parse("[deps]\n\"SellShotDistance\" = \"AutoSell=YES;IgnoreSellShot=NO\"\n");
    let on = values(&[("AutoSell", "YES"), ("IgnoreSellShot", "0")]);
    assert!(deps.field_active("SellShotDistance", &on));
    let off = values(&[("AutoSell", "YES"), ("IgnoreSellShot", "YES")]);
    assert!(!deps.field_active("sellshotdistance", &off));
}

/// A condition on a field the kind does not have does not block, and a field with no rule is
/// always in effect.
#[test]
fn an_absent_condition_field_and_a_ruleless_field_are_active() {
    let deps = FieldDeps::parse("\"PriceDownDelay\" = \"AutoSell=YES;PriceDownTimer<>0\"\n");
    assert!(deps.field_active("PriceDownDelay", &values(&[("PriceDownTimer", "1")])));
    assert!(!deps.field_active("PriceDownDelay", &values(&[("PriceDownTimer", "0")])));
    assert!(deps.field_active("Anything", &Values::new()));
}

#[test]
fn numeric_conditions_compare_numbers() {
    let deps = FieldDeps::parse("\"BuyStepKind\" = \"OrdersCount>1\"\n");
    assert!(deps.field_active("BuyStepKind", &values(&[("OrdersCount", "2")])));
    assert!(!deps.field_active("BuyStepKind", &values(&[("OrdersCount", "1")])));
    assert!(!deps.field_active("BuyStepKind", &values(&[("OrdersCount", "many")])));
}

#[test]
fn conditions_of_names_the_fields_a_rule_reads() {
    let deps = FieldDeps::parse("\"StopLoss3\" = \"UseStopLoss=YES;UseStopLoss3=YES\"\n");
    let fields: Vec<&str> = deps.conditions_of("StopLoss3").collect();
    assert_eq!(fields, ["usestoploss", "usestoploss3"]);
    assert_eq!(deps.conditions_of("Nothing").count(), 0);
}

/// The bundled file parses and carries the rules the tuner's warning leans on.
#[test]
fn the_bundled_rules_hold_the_sell_switches() {
    let deps = FieldDeps::bundled();
    let fields: Vec<&str> = deps.conditions_of("SellShotDistance").collect();
    assert!(fields.contains(&"ignoresellshot"), "{fields:?}");
    assert!(
        deps.conditions_of("UseScalpingMode")
            .any(|f| f == "autosell")
    );
}

#[test]
fn booleans_compare_across_spellings() {
    assert!(value_eq("0", "no"));
    assert!(value_eq("True", "yes"));
    assert!(!value_eq("1", "no"));
    assert!(value_eq("Trade", "trade"));
    assert_eq!(as_bool("50"), None);
}

/// MoonShot's ask adjustment moves the take only through the ask branch: with
/// `MShotSellAtLastPrice` off the take is `SellPrice` alone, and the adjustment is not in effect.
#[test]
fn the_ask_adjustment_hangs_on_sell_at_last_price() {
    let deps = FieldDeps::bundled();
    let on = values(&[("MShotSellAtLastPrice", "YES")]);
    assert!(deps.field_active("MShotSellPriceAdjust", &on));
    let off = values(&[("MShotSellAtLastPrice", "0")]);
    assert!(!deps.field_active("MShotSellPriceAdjust", &off));
}
