use super::*;
use moon_core::feed::StrategyFieldChange;

fn change(name: &str) -> StrategyFieldChange {
    StrategyFieldChange {
        name: name.to_string(),
        sent: "0".to_string(),
        saved: "1".to_string(),
    }
}

/// Dropping the bound, or printing every field, turns the banner into a paragraph the moment a
/// paste disagrees in more than a handful of values. The user then cannot see which value the
/// core actually kept.
#[test]
fn the_suffix_names_the_first_fields_and_counts_the_rest() {
    assert_eq!(adjusted_diff_suffix(&[]), "");
    assert_eq!(
        adjusted_diff_suffix(&[change("buyPrice")]),
        ": buyPrice (0 → 1)"
    );
    let many: Vec<_> = (0..5).map(|i| change(&format!("f{i}"))).collect();
    assert_eq!(
        adjusted_diff_suffix(&many),
        ": f0 (0 → 1), f1 (0 → 1), f2 (0 → 1) +2"
    );
}
