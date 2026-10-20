/// Render the actual heading so a font mismatch cannot invalidate its reserved column width.
struct HeadingProbe;

impl gpui::Render for HeadingProbe {
    /// Use the production heading at the shared default width.
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        super::current_heading(cx)
    }
}

/// Reverting the measured width or typography would clip the localized current-value heading.
#[gpui::test]
fn polish_current_heading_fits_every_locale(cx: &mut gpui::TestAppContext) {
    use gpui::px;
    cx.update(|cx| {
        moon_ui::MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(
                moon_core::config::UiThemeMode::Light,
                1.0,
            ),
            cx,
        )
    });
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let window = cx.open_window(gpui::size(px(687.0), px(200.0)), |_, _| HeadingProbe);
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let bounds = visual.debug_bounds("ticks-current-heading").unwrap();
        let needed = visual.update(|_, cx| {
            crate::design::ui_body_text_width(
                cx,
                &rust_i18n::t!("analytics.ticks.pick.col_now"),
                400.0,
            )
        });
        assert!(
            bounds.size.width >= px(needed),
            "{} current heading clipped",
            language.code()
        );
    }
}
