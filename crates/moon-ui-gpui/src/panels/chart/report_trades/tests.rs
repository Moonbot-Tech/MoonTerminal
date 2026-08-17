//! Regression tests for durable-history asynchronous request identity.

use super::history_result_is_current;

/// Removing the sequence check lets a slower Report scope A overwrite newer scope B on the same
/// tab; removing the target check lets core A history land after the tab moves to core B.
#[test]
fn stale_history_results_require_both_latest_sequence_and_exact_target() {
    let core_a = (7, "BTCUSDT".to_string());
    let core_b = (8, "BTCUSDT".to_string());

    assert!(!history_result_is_current(1, 2, &core_a, Some(&core_a)));
    assert!(!history_result_is_current(2, 2, &core_a, Some(&core_b)));
    assert!(history_result_is_current(2, 2, &core_b, Some(&core_b)));
}

use super::{EmulatorAdmission, admitted_emulator_kinds};

/// Collapsing "both checkboxes off" onto `None`, or letting either narrowing override the other,
/// must fail here.
///
/// The chart's two trade-kind checkboxes and the Report scope's own emulator predicate are
/// independent narrowings of one set, so the answer is their conjunction. Two failures are cheap to
/// write and both put trades back on the chart that the user just excluded: `(false, false)` mapped to
/// `All` shows EVERY trade at the moment the user asked for none, and answering `Only(x)` against a
/// scope pinned to the other kind re-admits exactly the rows the Report scope had filtered out.
#[test]
fn chart_trade_kind_checkboxes_intersect_the_report_scope_emulator_filter() {
    use EmulatorAdmission::{All, Nothing, Only};

    // No Report scope predicate: the checkboxes decide alone.
    assert_eq!(admitted_emulator_kinds(true, true, None), All);
    assert_eq!(admitted_emulator_kinds(true, false, None), Only(false));
    assert_eq!(admitted_emulator_kinds(false, true, None), Only(true));
    assert_eq!(
        admitted_emulator_kinds(false, false, None),
        Nothing,
        "both boxes off must admit NOTHING, never everything"
    );

    // Scope pinned to REAL only (`Some(false)`): emulator can never come back.
    assert_eq!(
        admitted_emulator_kinds(true, true, Some(false)),
        Only(false)
    );
    assert_eq!(
        admitted_emulator_kinds(true, false, Some(false)),
        Only(false)
    );
    assert_eq!(admitted_emulator_kinds(false, true, Some(false)), Nothing);
    assert_eq!(admitted_emulator_kinds(false, false, Some(false)), Nothing);

    // Scope pinned to EMULATOR only (`Some(true)`): real can never come back.
    assert_eq!(admitted_emulator_kinds(true, true, Some(true)), Only(true));
    assert_eq!(
        admitted_emulator_kinds(true, false, Some(true)),
        Nothing,
        "a real-only checkbox against an emulator-only scope admits nothing, not real trades"
    );
    assert_eq!(admitted_emulator_kinds(false, true, Some(true)), Only(true));
    assert_eq!(admitted_emulator_kinds(false, false, Some(true)), Nothing);
}
