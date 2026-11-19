use moon_core::config::auto_figures::{DetectorKind, DetectorParams};

use super::*;

/// The share is by what a row's answer is made of: every calendar kind needs all its bars, every
/// other kind — the formation among them — nine in ten. Mutation: let a calendar kind answer 90
/// (the screener scans a day's high from an incomplete day the chart calls short), or a formation
/// answer 100.
#[test]
fn calendar_rows_need_every_bar_and_the_others_nine_in_ten() {
    for kind in DetectorKind::ALL {
        let params = kind.default_params();
        let calendar = matches!(
            params,
            DetectorParams::PeriodExtreme(_)
                | DetectorParams::PeriodLevel(_)
                | DetectorParams::VolumeProfile(_)
        );
        assert_eq!(history_full(9, 10, &params), !calendar, "{kind:?}");
        assert!(history_full(10, 10, &params), "{kind:?}");
        assert!(!history_full(8, 10, &params), "{kind:?}");
    }
}

/// The boundary is exact in integers: 90 of 100 is full for a sampling row, 899 of 1000 is not.
/// Mutation: compare in `f32` shares (0.9 × 1000 rounds) or with a strict `>`.
#[test]
fn the_sampling_boundary_is_exact() {
    let swing = DetectorKind::Swing.default_params();
    assert!(history_full(90, 100, &swing));
    assert!(!history_full(899, 1000, &swing));
    assert!(history_full(900, 1000, &swing));
}
