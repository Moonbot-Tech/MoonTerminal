//! Regression tests for the Core Status alert-configuration popover metrics.

use crate::panels::core_status::config_popup::WarnCfgMetrics;

/// `config_popup.rs:WarnCfgMetrics::resolve` must retain `ui.max(font_w)` for horizontal sizing;
/// simplifying it to `font_w` lets UI-scaled checkboxes, steppers, and the play button overflow
/// their columns when the UI scale is wider than the font scale.
#[test]
fn ui_scale_controls_set_the_horizontal_extent() {
    let metrics = WarnCfgMetrics::resolve(1.5, 1.0, 9.0, 11.0, 39.0);

    // 636 is the independently specified base row width (eight columns plus seven gaps), and the
    // popover adds 16px of slack before the same UI-or-font scale is applied.
    assert!(
        (metrics.row_w() - 954.0).abs() < 0.01,
        "the UI-dominant row must be 636 * 1.5 px, not font-width-sized"
    );
    assert!(
        (metrics.content_w() - 978.0).abs() < 0.01,
        "the popover content must retain its independently specified 16px scaled slack"
    );
}

/// `config_popup.rs:WarnCfgMetrics::resolve` must retain `.max(action_h)` in `ctrl_h`; dropping it
/// clips the 32px Action-sized sound dropdown at Font-slider +6 inside a 28px control band.
#[test]
fn font_deltas_keep_the_control_band_as_tall_as_the_action_dropdown() {
    let cases = [
        // (font-width scale, caption px, body px, Action height, row px, content px, caption band,
        //  control band). These are derived from the published font-slider contract, not from the
        //  metrics implementation: 636px row plus 16px slack at delta 0, then font-width scaling.
        (1.0, 9.0, 11.0, 26.0, 636.0, 652.0, 12.0, 28.0),
        (
            13.0 / 11.0,
            11.0,
            13.0,
            28.0,
            751.63635,
            770.5455,
            14.3,
            28.0,
        ),
        (
            17.0 / 11.0,
            15.0,
            17.0,
            32.0,
            982.9091,
            1007.63635,
            19.5,
            32.0,
        ),
    ];

    for (font_w, cap_px, body_px, action_h, row_w, content_w, cap_h, ctrl_h) in cases {
        let metrics = WarnCfgMetrics::resolve(1.0, font_w, cap_px, body_px, action_h);

        assert!(
            (metrics.row_w() - row_w).abs() < 0.01,
            "row width must fit its columns"
        );
        assert!(
            (metrics.content_w() - content_w).abs() < 0.01,
            "content width must fit the row and its slack"
        );
        assert!(
            (metrics.cap_h - cap_h).abs() < 0.01,
            "caption band must fit its font line box"
        );
        assert!(
            (metrics.ctrl_h - ctrl_h).abs() < 0.01,
            "control band must fit the Action dropdown at every required font delta"
        );
    }
}
