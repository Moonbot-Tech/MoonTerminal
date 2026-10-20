//! Verdict-color regression shared by filter, time and the displayed holdout baseline.

use moon_core::db::metrics::Tally;

/// Reproduce Entry/Exit's body-sized ancestor while comparing the real caption with a caption reference.
struct CaptionProbe;

impl gpui::Render for CaptionProbe {
    /// Render the production baseline beside an independently sized full-caption text node.
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{InteractiveElement, ParentElement, Styled};
        moon_ui::v_flex()
            .w_full()
            .text_size(crate::design::t_body(cx))
            .child(super::fact_line(
                &Tally::default(),
                moon_ui::MoonPalette::active(cx),
                cx,
            ))
            .child(
                moon_ui::h_flex().child(
                    gpui::div()
                        .debug_selector(|| "split-caption-reference".into())
                        .flex_none()
                        .whitespace_nowrap()
                        .font_family(crate::design::ui_font())
                        .text_size(crate::design::t_caption(cx))
                        .child(rust_i18n::t!("analytics.tuner.split_fact_holdout").to_string()),
                ),
            )
    }
}

/// Inheriting Entry/Exit's larger body band would clip the baseline caption in every wide panel.
#[gpui::test]
fn baseline_caption_uses_its_measured_typography(cx: &mut gpui::TestAppContext) {
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
        let window = cx.open_window(gpui::size(px(540.0), px(300.0)), |_, _| CaptionProbe);
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        for _ in 0..3 {
            visual.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let caption = visual.debug_bounds("split-caption").unwrap();
        let reference = visual.debug_bounds("split-caption-reference").unwrap();
        assert_eq!(
            caption.size.height,
            reference.size.height,
            "{} inherited the body font",
            language.code()
        );
        assert!(
            caption.size.width + px(0.5) >= reference.size.width,
            "{} clips the full caption",
            language.code()
        );
    }
}

/// Reverting to profit-sign coloring would advertise positive but inferior unseen profit as green.
#[gpui::test]
fn positive_below_fact_is_danger(cx: &mut gpui::TestAppContext) {
    let mut holdout = Tally::default();
    holdout.push(5.0);
    let mut fact = Tally::default();
    fact.push(6.0);
    for theme in [
        moon_core::config::UiThemeMode::Light,
        moon_core::config::UiThemeMode::Dark,
        moon_core::config::UiThemeMode::Graphite,
    ] {
        cx.update(|cx| {
            moon_ui::MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(theme, 1.0),
                cx,
            );
            let p = moon_ui::MoonPalette::active(cx);
            assert_eq!(
                super::holdout_color(holdout.profit, Some(fact.profit), p),
                crate::design::danger_color(p)
            );
            assert_eq!(
                super::holdout_color(holdout.profit, None, p),
                crate::design::positive_color(p)
            );
            assert_eq!(
                super::holdout_color(fact.profit, Some(holdout.profit), p),
                crate::design::positive_color(p)
            );
            assert_eq!(
                super::holdout_color(-5.0, Some(-6.0), p),
                crate::design::positive_color(p)
            );
            assert_eq!(
                super::holdout_color(-5.0, None, p),
                crate::design::danger_color(p)
            );
        });
    }
}
