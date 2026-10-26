/// Render the actual heading so a font mismatch cannot invalidate its reserved column width.
struct HeadingProbe;

/// Exercise the production name inside its numeric row slots at the narrow layout width.
struct ThresholdProbe {
    /// Reserve the production range block on wide hosts.
    ranges: bool,
    /// Value projected from the formula, including compact scientific thresholds.
    current: super::NowValue,
}

impl gpui::Render for ThresholdProbe {
    /// Paint the real row with fixed trailing numeric slots so wrapping is measured honestly.
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{InteractiveElement, IntoElement, Styled};
        let p = moon_ui::MoonPalette::active(cx);
        let parts = super::FieldRowParts {
            checkbox: gpui::div().w(gpui::px(16.0)).flex_none().into_any_element(),
            name: super::field_name_el("SellByCustomEMA#1", false, true, None, p)
                .into_any_element(),
            current: super::current_cell_el("SellByCustomEMA#1", true, Some(&self.current), p, cx)
                .into_any_element(),
            variant: gpui::div().into_any_element(),
            clear: gpui::div().w(gpui::px(12.0)).flex_none().into_any_element(),
        };
        let range = self.ranges.then(|| {
            gpui::div()
                .debug_selector(|| "threshold-range".into())
                .w(super::super::ranges::block_w(cx))
                .flex_none()
                .into_any_element()
        });
        super::field_row_el("SellByCustomEMA#1", false, parts, range, p, cx)
    }
}

/// Truncating virtual labels would hide threshold identity in long translations and narrow hosts.
#[gpui::test]
fn custom_ema_threshold_label_fits_all_locales(cx: &mut gpui::TestAppContext) {
    use gpui::px;
    for mode in [
        moon_core::config::UiThemeMode::Light,
        moon_core::config::UiThemeMode::Dark,
        moon_core::config::UiThemeMode::Graphite,
    ] {
        for zoom in [1.0, 1.5] {
            cx.update(|cx| {
                moon_ui::MoonTheme::install_config(
                    crate::startup::moon_theme_config_for_presentation(mode, zoom),
                    cx,
                )
            });
            for language in moon_core::config::Language::ALL {
                let _locale = crate::test_locale::force(language.code());
                for width in [400.0, 687.0] {
                    let window =
                        cx.open_window(gpui::size(px(width), px(240.0)), |_, _| ThresholdProbe {
                            ranges: width > 600.0,
                            current: super::NowValue::Same("0.2".into()),
                        });
                    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                    let name = visual
                        .debug_bounds("an-ticks-name-SellByCustomEMA#1")
                        .unwrap();
                    let row = visual
                        .debug_bounds("an-ticks-field-SellByCustomEMA#1")
                        .unwrap();
                    let needed = visual.update(|_, cx| {
                        crate::design::ui_body_text_width(
                            cx,
                            &super::field_label("SellByCustomEMA#1"),
                            400.0,
                        )
                    });
                    assert!(
                        name.size.width >= px(needed) || name.size.height > px(24.0 * zoom),
                        "{} threshold label did not wrap",
                        language.code()
                    );
                    assert!(name.right() <= row.right(), "threshold name overflow");
                    assert!(
                        name.bottom() <= row.bottom(),
                        "threshold name clipped vertically"
                    );
                }
            }
        }
    }
}

/// Decimal expansion of a scientific threshold must not push the variant or range off-screen.
#[gpui::test]
fn custom_ema_scientific_current_fits_all_locales(cx: &mut gpui::TestAppContext) {
    use gpui::px;
    use std::{collections::HashMap, sync::Arc};
    let own = HashMap::from([(
        (1, 11),
        Arc::new(HashMap::from([(
            "SellByCustomEMA".into(),
            "EMA(5s,1s)>1e-100".into(),
        )])),
    )]);
    let key = "SellByCustomEMA#1";
    let (now, _) = super::super::load::now_values(&[(1, Some(11))], &[key.into()], &own);
    let super::NowValue::Same(value) = &now[key] else {
        panic!("one value")
    };
    assert_eq!(value, "1e-100");
    assert_eq!(value.parse::<f64>().unwrap(), 1e-100);
    for mode in [
        moon_core::config::UiThemeMode::Light,
        moon_core::config::UiThemeMode::Dark,
        moon_core::config::UiThemeMode::Graphite,
    ] {
        for zoom in [1.0, 1.5] {
            cx.update(|cx| {
                moon_ui::MoonTheme::install_config(
                    crate::startup::moon_theme_config_for_presentation(mode, zoom),
                    cx,
                )
            });
            for language in moon_core::config::Language::ALL {
                let _locale = crate::test_locale::force(language.code());
                for width in [400.0, 687.0] {
                    let window =
                        cx.open_window(gpui::size(px(width), px(240.0)), |_, cx| ThresholdProbe {
                            ranges: super::ranges_fit(px(width), cx),
                            current: super::NowValue::Same(value.clone()),
                        });
                    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                    let row = visual
                        .debug_bounds("an-ticks-field-SellByCustomEMA#1")
                        .unwrap();
                    let current = visual
                        .debug_bounds("an-ticks-current-cell-SellByCustomEMA#1")
                        .unwrap();
                    let variant = visual
                        .debug_bounds("an-ticks-variant-cell-SellByCustomEMA#1")
                        .unwrap();
                    let text_width = visual
                        .update(|_, cx| crate::design::mono_body_text_width(cx, value, 400.0));
                    assert!(
                        current.size.width >= px(text_width),
                        "{} text clipped",
                        language.code()
                    );
                    assert!(
                        current.right() <= variant.left(),
                        "current overlaps variant"
                    );
                    assert!(
                        variant.right() <= row.right() && row.right() <= px(width),
                        "variant past edge"
                    );
                    if let Some(range) = visual.debug_bounds("threshold-range") {
                        assert!(range.right() <= row.right(), "range past edge");
                    }
                }
            }
        }
    }
}

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
