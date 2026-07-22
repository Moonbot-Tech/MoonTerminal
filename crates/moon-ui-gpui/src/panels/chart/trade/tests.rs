// Do not use `super::*`: the parent re-exports GPUI's `test` attribute macro through `gpui::*`,
// which would shadow the built-in `#[test]` and make it expand recursively.
use super::hover_probe_due;

/// Pins the MouseMove hot-path thresholds enforced by `hover_probe_due`.
///
/// Plausible breakage: changing `trade.rs::hover_probe_due` to accept sub-threshold jitter or miss
/// an exact boundary would either rescan every order line on redundant moves or delay hover at the
/// specified one-pixel X and half-pixel Y thresholds.
#[test]
fn hover_probe_threshold_matches_delphi() {
    // The first visit always probes.
    assert!(hover_probe_due(None, (10.0, 10.0)));
    // Sub-threshold mouse-move jitter does not probe again.
    assert!(!hover_probe_due(Some((10.0, 10.0)), (10.0, 10.0)));
    assert!(!hover_probe_due(Some((10.0, 10.0)), (10.9, 10.4)));
    // Movement at either the one-pixel X or half-pixel Y boundary probes in both directions.
    assert!(hover_probe_due(Some((10.0, 10.0)), (11.0, 10.0)));
    assert!(hover_probe_due(Some((10.0, 10.0)), (10.0, 10.5)));
    assert!(hover_probe_due(Some((10.0, 10.0)), (9.0, 10.0)));
    assert!(hover_probe_due(Some((10.0, 10.0)), (10.0, 9.5)));
}
