//! Selection warnings use the displayed precision and flag either direction.

use super::{bias_note, caption};
use moon_core::db::tuner::VarStats;

/// Dropping the read gate would advertise zero usable deals before the tape has answered.
#[test]
fn sample_caption_waits_then_restores_zero_and_positive_counts() {
    let _locale = crate::test_locale::force("en");
    for n in [0, 357] {
        assert_eq!(caption(n, 462, true), "reading the tape…");
    }
    assert_eq!(caption(0, 462, false), "0 usable of 462 closed trades");
    assert_eq!(caption(357, 462, false), "357 usable of 462 closed trades");
}

/// Hiding profitable excluded deals or reversing the bias must both remain visible.
#[test]
fn material_pf_difference_is_symmetric_and_uses_displayed_hundredths() {
    let _locale = crate::test_locale::force("en");
    let all = VarStats {
        n: 405,
        pf: 0.90,
        ..Default::default()
    };
    let mut fit = VarStats {
        n: 155,
        pf: 0.42,
        ..Default::default()
    };
    assert_eq!(caption(155, 405, false), "155 usable of 405 closed trades");
    assert_eq!(
        bias_note(&fit, &all).as_deref(),
        Some("Usable sample worse: PF 0.42 vs 0.90")
    );
    fit.pf = 1.10;
    assert_eq!(
        bias_note(&fit, &all).as_deref(),
        Some("Usable sample better: PF 1.10 vs 0.90")
    );
    fit.pf = 0.71;
    assert!(bias_note(&fit, &all).is_none());
    fit.pf = 0.70;
    assert!(bias_note(&fit, &all).is_some());
    fit.n = 405;
    assert!(bias_note(&fit, &all).is_none());
    fit.n = 0;
    assert!(bias_note(&fit, &all).is_none());
    fit.n = 155;
    fit.pf = f64::NAN;
    assert!(bias_note(&fit, &all).is_none());
}
