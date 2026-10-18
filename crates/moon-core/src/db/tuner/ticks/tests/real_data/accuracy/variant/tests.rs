//! Effective-setting treatment controls must not inherit an omitted non-default knob.

use super::*;

/// A sparse B with trailing absent must switch A's non-default trailing back to the fallback.
#[test]
fn absent_partner_knob_removes_source_nondefault() {
    let a = HashMap::from([
        ("UseTrailing".into(), "YES".into()),
        ("ModelInput".into(), "kept".into()),
    ]);
    let b = HashMap::from([
        ("SellPrice".into(), "3".into()),
        ("ModelInput".into(), "partner".into()),
    ]);
    let laid = effective_overlay(&a, &b);
    assert!(!laid.contains_key("UseTrailing"));
    assert_eq!(laid.get("SellPrice").map(String::as_str), Some("3"));
    assert_eq!(laid.get("ModelInput").map(String::as_str), Some("kept"));
}
