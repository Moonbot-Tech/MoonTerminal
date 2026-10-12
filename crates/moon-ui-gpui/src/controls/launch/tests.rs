//! Explicit imports: the parent re-exports `gpui::*`, whose own `test` shadows the built-in
//! attribute.

use super::{LAUNCHER_BORDER_W, LAUNCHER_PAD_X, launcher_label_width};
use moon_core::config::UiThemeMode;
use moon_ui::MoonButtonSize;

/// `controls/launch.rs:launcher_label_width` must budget the same tier font and gap it draws.
///
/// Breakage: measuring launcher text with the old font channel or a stale icon gap makes the
/// window buttons overlap their labels before the row can shed a caption.
#[gpui::test]
fn launcher_label_width_matches_the_drawn_tier_geometry(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        moon_ui::MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0),
            cx,
        );
    });
    let (actual, expected) = cx.update(|cx| {
        let tier = crate::design::CONTROL_TIER;
        let metrics = tier.control_metrics();
        let text =
            crate::design::ui_text_width_zoomed(cx, "Settings", metrics.font_size, 500.0, false);
        let chrome =
            crate::design::ui_value(cx, MoonButtonSize::tier_icon_size(tier) + metrics.gap)
                + crate::design::ui_value(cx, LAUNCHER_PAD_X) * 2.0
                + LAUNCHER_BORDER_W;
        (
            launcher_label_width(cx, "Settings"),
            (text + chrome).max(crate::design::glyph_btn_w(cx)),
        )
    });
    assert_eq!(
        actual, expected,
        "launcher budget must equal drawn geometry"
    );
}
