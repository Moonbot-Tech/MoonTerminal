//! Settings previews preserve unrelated state and expose the same exact paths as ActiveLib.

use super::*;

/// An unrelated field may change between preview and dispatch; a touched field may not.
#[test]
fn preview_is_scoped_to_exact_touched_fields() {
    let mut config = SharedConfig::default();
    let changes = BTreeMap::from([("trading.log_level".into(), json!(3))]);
    let original = preview(&config, &changes).unwrap();
    config.trading.fixed_trade_balance = 123.456789;
    assert_eq!(preview(&config, &changes).unwrap(), original);
    config.trading.log_level = 2;
    assert_ne!(
        preview(&config, &changes).unwrap()["before"],
        original["before"]
    );
    config.trading.log_level = 3;
    assert!(matches(&config, &original["after"]).unwrap());
}

/// Returned schemas retain service read-only markers and fixed-array lengths.
#[test]
fn field_catalog_resolves_types_and_current_values() {
    let config = SharedConfig::default();
    let field = read(&config, Some("ui.markets_table_config.col_vis"), 0).unwrap();
    assert_eq!(field["fields"][0]["schema"]["minItems"], 41);
    assert_eq!(field["fields"][0]["value"].as_array().unwrap().len(), 41);
    let version = read(&config, Some("config_version"), 0).unwrap();
    assert_eq!(version["fields"][0]["schema"]["readOnly"], true);
    assert!(read(&config, Some("signals.unknown_tail"), 0).is_err());
}
