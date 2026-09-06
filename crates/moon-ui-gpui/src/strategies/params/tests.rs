//! Unit tests for strategy-field label lookup.

use super::field_keys;

/// `params.rs::field_keys`: dropping an exact arm or weakening its case-sensitive guard
/// would make a known field fall back to its raw identifier or accept a schema spelling we do not
/// localize, leaving traders with an untranslated label or an invented match.
#[test]
fn field_labels_are_exact_and_fail_closed() {
    assert_eq!(
        field_keys("AutoBuy"),
        Some((Some("strat.field.AutoBuy"), Some("strat.label.AutoBuy")))
    );
    assert_eq!(field_keys("autobuy"), None);
    assert_eq!(
        field_keys("SilentNoCharts"),
        Some((None, Some("strat.label.SilentNoCharts")))
    );
    assert_eq!(field_keys("silentnocharts"), None);
}
