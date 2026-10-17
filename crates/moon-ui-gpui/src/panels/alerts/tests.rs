//! Signature regressions for translated alert rows.

use super::{alerts_should_rebuild, locale_data_sig};
use crate::panels::common::GateStep;

/// Rebuilding resolved rows each second wastes work; failing to retry fallback labels freezes them.
#[test]
fn repaint_rebuilds_only_with_unresolved_coin() {
    assert!(!alerts_should_rebuild(false, GateStep::Repaint, false));
    assert!(alerts_should_rebuild(false, GateStep::Repaint, true));
    assert!(!alerts_should_rebuild(false, GateStep::Idle, true));
    assert!(alerts_should_rebuild(true, GateStep::Idle, false));
    assert!(alerts_should_rebuild(false, GateStep::Rebuild, false));
}

/// Omitting the locale strands tool labels in the previous language on an idle account.
#[test]
fn locale_change_moves_data_sig() {
    let en = locale_data_sig(17, "en");
    assert_ne!(en, locale_data_sig(17, "ru"));
    assert_ne!(en, locale_data_sig(18, "en"));
    assert_eq!(en, locale_data_sig(17, "en"));
}
