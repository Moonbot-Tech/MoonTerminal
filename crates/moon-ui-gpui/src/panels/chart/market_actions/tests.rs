//! Chart-caption regression coverage for the MoonUI tier-segment boundary.

use moon_core::config::UiThemeMode;
use moon_ui::MoonTheme;

/// The chart draws its captions at device density while the action segments are GPUI elements
/// under the window content zoom, so a segment must be handed the caption size divided by that
/// zoom to land on the same device pixels. Dividing by the token scale (always 1 now) or by
/// nothing puts the labels off their measured rectangles at every zoom but 100%.
#[gpui::test]
fn chart_action_labels_land_on_the_chart_caption_at_every_zoom(cx: &mut gpui::TestAppContext) {
    for zoom in [0.75, 1.0, 1.5] {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, zoom),
                cx,
            );
            for size in [8.0, 15.0, 32.0] {
                let (font, line) = super::action_label_metrics(cx, size);
                assert!((font * zoom - size).abs() < 0.0001, "zoom {zoom}: font");
                assert!(
                    (line * zoom - (size + 4.0)).abs() < 0.0001,
                    "zoom {zoom}: line"
                );
            }
        });
    }
}

/// The auto-figures eye draws two DIFFERENT icons, and both ship in MoonUI's assets.
///
/// Breaks on: an icon path MoonUI does not ship (the button would draw an empty square), or both
/// states mapping to one icon (the button would stop saying which state the pane is in).
#[test]
fn the_eye_draws_a_shipped_icon_for_each_state() {
    use gpui::AssetSource as _;
    let (shown, hidden) = (super::eye_icon(true), super::eye_icon(false));
    assert_ne!(shown, hidden, "the two states draw different icons");
    for path in [shown, hidden] {
        assert!(
            matches!(moon_ui::MoonAssets.load(path), Ok(Some(_))),
            "{path} is not in MoonUI's assets"
        );
    }
}
