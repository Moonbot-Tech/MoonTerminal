//! Chart-caption regression coverage for the MoonUI tier-segment boundary.

use moon_core::config::{UiDensity, UiThemeMode};
use moon_ui::MoonTheme;

/// Restoring the legacy inverse makes captions shrink by the density delta inside their measured
/// rectangles. Assert the actual tier forward transform preserves both glyph and line-box sizes.
#[gpui::test]
fn chart_action_labels_match_measured_sizes_at_every_density_and_zoom(
    cx: &mut gpui::TestAppContext,
) {
    for density in [UiDensity::Compact, UiDensity::Standard, UiDensity::Large] {
        for zoom in [0.75, 1.0, 1.5] {
            cx.update(|cx| {
                MoonTheme::install_config(
                    crate::startup::moon_theme_config_for_presentation(
                        UiThemeMode::Dark,
                        density,
                        zoom,
                    ),
                    cx,
                );
                let tokens = MoonTheme::active_tokens(cx);
                for size in [8.0, 15.0, 32.0] {
                    let (font, line) = super::action_label_metrics(cx, size);
                    assert!((tokens.ui(font) - size).abs() < 0.0001);
                    assert!((tokens.ui(line) - (size + 4.0)).abs() < 0.0001);
                }
            });
        }
    }
}
