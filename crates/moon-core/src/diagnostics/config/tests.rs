//! Rules for reading `diagnostics.toml`, and the drift guards between the file's documentation and
//! the struct it parses into.

use super::*;
use crate::diagnostics::template;

/// The oracle for "every switch is documented": what the struct serializes to, which is derived
/// from the struct's own fields and not from the table under test.
fn fields_of_default() -> Vec<(String, String)> {
    let text = toml::to_string(&DiagCfg::default()).expect("defaults serialize");
    let table: toml::Table = text.parse().expect("serialized defaults re-parse");
    let mut out = Vec::new();
    for (section, item) in &table {
        let Some(keys) = item.as_table() else {
            continue;
        };
        for key in keys.keys() {
            out.push((section.clone(), key.clone()));
        }
    }
    out
}

#[test]
fn the_rendered_template_parses_back_into_the_defaults() {
    let parsed = parse(&template::render()).expect("the shipped template must be valid TOML");
    assert_eq!(
        parsed,
        DiagCfg::default(),
        "a default written in template::KEYS disagrees with the Default impl, so a fresh install \
         would behave differently from one with no file at all"
    );
}

#[test]
fn every_field_of_the_config_is_documented_in_the_template() {
    let documented: Vec<(String, String)> = template::KEYS
        .iter()
        .map(|k| (k.section.to_string(), k.key.to_string()))
        .collect();
    for field in fields_of_default() {
        assert!(
            documented.contains(&field),
            "{}.{} has no entry in template::KEYS: it would be missing from a fresh file and \
             never appended to an existing one",
            field.0,
            field.1
        );
    }
}

#[test]
fn every_documented_key_exists_in_the_config() {
    let fields = fields_of_default();
    for key in template::KEYS {
        let pair = (key.section.to_string(), key.key.to_string());
        assert!(
            fields.contains(&pair),
            "template documents {}.{}, which the config struct does not have: the file would \
             advertise a switch that does nothing",
            key.section,
            key.key
        );
    }
}

#[test]
fn a_broken_file_is_an_error_rather_than_a_partial_read() {
    assert!(parse("[log]\nbalances = ").is_err());
}

#[test]
fn missing_keys_fall_back_to_defaults() {
    let cfg = parse("[log]\nbalances = true\n").expect("partial files are valid");
    assert!(cfg.log.balances);
    assert!(!cfg.log.kline_cache, "an absent key must keep its default");
    assert_eq!(cfg.limits.log_ring_lines, DEFAULT_RING_LINES);
}

#[test]
fn the_ring_size_is_clamped_to_something_survivable() {
    let zero = parse("[limits]\nlog_ring_lines = 0\n").expect("valid");
    assert_eq!(
        zero.limits.log_ring_lines, MIN_RING_LINES,
        "a zero ring makes the Log panel permanently empty, which reads as a broken application"
    );
    let huge = parse("[limits]\nlog_ring_lines = 999999999\n").expect("valid");
    assert_eq!(huge.limits.log_ring_lines, MAX_RING_LINES);
}

#[test]
fn the_environment_only_ever_turns_a_switch_on() {
    let mut cfg = DiagCfg::default();
    cfg.channels.render = true;
    // The variable is absent, and the file says on. A CI run that sets nothing must not silently
    // disable what the file asked for.
    apply_env(&mut cfg, |_| None);
    assert!(cfg.channels.render);

    let mut cfg = DiagCfg::default();
    apply_env(&mut cfg, |v| {
        (v == "MOON_RENDER_DIAG").then(|| "1".to_string())
    });
    assert!(cfg.channels.render, "the variable must enable the channel");
}

#[test]
fn presence_alone_enables_even_with_a_falsy_value() {
    let mut cfg = DiagCfg::default();
    apply_env(&mut cfg, |v| {
        (v == "MOON_DETECT_DIAG").then(|| "0".to_string())
    });
    assert!(
        cfg.channels.detect,
        "every one of these variables has always enabled on presence; a run relying on \
         MOON_DETECT_DIAG=0 must keep working"
    );
}

#[test]
fn the_render_variable_also_enables_the_market_channel() {
    let mut cfg = DiagCfg::default();
    apply_env(&mut cfg, |v| {
        (v == "MOON_RENDER_DIAG").then(|| "1".to_string())
    });
    assert!(
        cfg.channels.markets,
        "market::source::market_diag_enabled accepted either variable before this module existed"
    );
}

#[test]
fn an_empty_order_selector_from_the_environment_follows_everything() {
    let mut cfg = DiagCfg::default();
    apply_env(&mut cfg, |v| (v == "MOON_ORDER_DIAG").then(String::new));
    assert_eq!(
        cfg.channels.orders, "1",
        "an empty value matched every market in the old selector; mapping it to the empty string \
         would instead read as 'off'"
    );
}

#[test]
fn an_order_selector_from_the_environment_is_kept_verbatim() {
    let mut cfg = DiagCfg::default();
    apply_env(&mut cfg, |v| {
        (v == "MOON_ORDER_DIAG").then(|| "GateF/BTC".to_string())
    });
    assert_eq!(cfg.channels.orders, "GateF/BTC");
}

#[test]
fn all_switches_off_is_reported_as_nothing_active() {
    assert!(!DiagCfg::default().any_active());
    assert_eq!(DiagCfg::default().active_summary(), None);
}

#[test]
fn a_string_switch_counts_as_active_and_carries_its_value() {
    let mut cfg = DiagCfg::default();
    cfg.channels.orders = "GateF/BTC".to_string();
    assert!(cfg.any_active());
    let summary = cfg.active_summary().expect("active");
    assert!(
        summary.contains("channels.orders=GateF/BTC"),
        "the warning has to say WHICH market is followed; {summary}"
    );
}

#[test]
fn a_bare_filter_string_counts_as_active() {
    let mut cfg = DiagCfg::default();
    cfg.log.filter = "moon_core::db=debug".to_string();
    assert!(
        cfg.any_active(),
        "a directive string raises the log level as surely as a flag; leaving it out of the \
         warning would let the loudest switch of all stay silent"
    );
}

#[test]
fn whitespace_is_not_a_switch() {
    let mut cfg = DiagCfg::default();
    cfg.channels.orders = "   ".to_string();
    cfg.log.filter = "  ".to_string();
    assert!(!cfg.any_active());
}
