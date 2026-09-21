//! Figure-tool menu width: the open list has to show every localized name in full.

use moon_core::config::UiThemeMode;
use moon_ui::{MoonButtonSize, MoonMenuSize, MoonSize, MoonTheme};

use super::{PICKER_W, fig_tool_menu_design_width, fig_tool_menu_labels, fig_tool_menu_width};

/// `fig_tool_menu_design_width` returning the unscaled pixel budget, or dropping the trigger
/// floor, hands `menu_width_scaled` a width it then scales again or a menu narrower than the
/// button that opened it.
///
/// The user-visible consequence is a tool name ending in `…`, or a menu that sits inside the
/// trigger. One design pixel under the result misses the label budget or [`PICKER_W`].
#[test]
fn design_width_is_the_smallest_step_that_covers_content_and_trigger() {
    let cases = [
        (30.0_f32, 1.0_f32),
        (254.0, 1.0),
        (400.0, 1.25),
        (10.0, 0.8),
        (254.0, 2.0),
    ];
    for (content, scale) in cases {
        let width = fig_tool_menu_design_width(content, scale);
        let rendered = width * scale;
        assert!(
            rendered + 1e-3 >= content,
            "content {content} at scale {scale} rendered {rendered}"
        );
        assert!(
            width + 1e-3 >= PICKER_W,
            "width {width} is under the trigger design width {PICKER_W}"
        );
        let shorter = width - 1.0;
        assert!(
            shorter * scale < content || shorter < PICKER_W,
            "width {width} is more than one design px above both budgets \
             (content {content}, trigger {PICKER_W}, scale {scale})"
        );
    }
}

/// A non-finite zoom must not produce a NaN menu width that MoonUI would then treat as empty.
#[test]
fn design_width_rejects_a_non_finite_zoom() {
    let width = fig_tool_menu_design_width(80.0, f32::NAN);
    assert!(width.is_finite());
    assert!(width * 0.25 + 1e-3 >= 80.0);
}

/// `fig_tool_menu_width` measuring on the font channel, at the wrong tier, or passing
/// already-zoomed pixels into `menu_width_scaled` clips the longest tool name.
///
/// The user-visible consequence is the figure-tool menu reading `Про…` / `Фибоначчи…` /
/// `Горизонтал…` again. The theme here is Md at UI scale 1.25, so a width taken at Sm's 14px
/// or multiplied by that scale a second time misses the bound.
#[gpui::test]
fn menu_width_covers_every_active_locale_name_without_scaling_twice(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0)
                .with_ui_scale(1.25)
                .with_tier(MoonSize::Md),
            cx,
        );
    });

    let (en_text, en_width) = measure(cx);
    rust_i18n::set_locale("ru");
    let (ru_text, ru_width) = measure(cx);
    rust_i18n::set_locale("es");
    let (es_text, es_width) = measure(cx);

    assert!(
        (ru_text - en_text).abs() > 8.0 && (ru_text - es_text).abs() > 8.0,
        "ru/en/es tool names must differ enough to move the menu \
         (ru {ru_text}, en {en_text}, es {es_text})"
    );
    assert_eq!(
        (ru_width > en_width),
        (ru_text > en_text),
        "menu width must follow the active locale's widest label"
    );
    assert_eq!((es_width > en_width), (es_text > en_text));
}

/// Measure the active locale's widest row and the menu width the dropdown will be given.
fn measure(cx: &mut gpui::TestAppContext) -> (f32, f32) {
    cx.update(|cx| {
        let tokens = MoonTheme::active_tokens(cx);
        let tier = match MoonMenuSize::from_theme(&tokens) {
            MoonMenuSize::Tier(tier) => tier,
            MoonMenuSize::Custom { .. } => MoonSize::Sm,
        };
        let row = tier.control_metrics();
        let widest = fig_tool_menu_labels()
            .iter()
            .map(|label| crate::design::ui_text_width_zoomed(cx, label, row.font_size, 600.0, true))
            .fold(0.0_f32, f32::max);
        // MoonUI `dropdown/layout.rs`: outer pad, unscaled border, row pad, check column, two gaps.
        let chrome = crate::design::ui_value(cx, 4.0) * 2.0
            + 2.0
            + crate::design::ui_value(cx, row.pad_x) * 2.0
            + crate::design::ui_value(cx, 12.0)
            + crate::design::ui_value(cx, row.gap) * 2.0;
        let button_font = MoonButtonSize::density(cx)
            .tier()
            .unwrap_or(crate::design::CONTROL_TIER)
            .control_metrics()
            .font_size;
        let trigger = PICKER_W * crate::design::font_value(cx, button_font) / button_font.max(1.0);
        let scale = crate::design::ui_value(cx, 1.0);
        assert!(
            (scale - 1.25).abs() < 1e-3,
            "ui scale {scale} did not reach the menu measurement"
        );
        let width = fig_tool_menu_width(cx);
        assert!(
            width + 0.05 >= PICKER_W,
            "menu design width {width} is under the trigger {PICKER_W}"
        );
        let rendered = width * scale;
        let budget = (widest + chrome).max(trigger);
        assert!(
            rendered + 0.05 >= widest + chrome,
            "menu {rendered}px does not cover label {widest} + chrome {chrome}"
        );
        assert!(
            rendered + 0.05 >= trigger,
            "menu {rendered}px is narrower than the trigger {trigger}"
        );
        // Ceil-to-design-px adds less than one zoom step, plus the one rendered pixel of slack.
        // A second zoom multiplies the whole budget by `scale` and misses this ceiling.
        assert!(
            rendered <= budget + 1.0 + scale + 1.0,
            "menu {rendered}px is wider than the measured row ({budget}); the width was scaled twice"
        );
        (widest, width)
    })
}
