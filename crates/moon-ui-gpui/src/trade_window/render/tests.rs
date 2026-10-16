//! Headless layout coverage of the production trade-detail titlebar.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AppContext, Bounds, Context, IntoElement, ParentElement, Pixels, Render, Styled, Window, div,
    px,
};
use moon_core::config::UiThemeMode;
use moon_ui::{MoonButtonSegment, MoonDropdown, MoonPalette, MoonSize};

use super::trade_header;

/// Supply inert settings/scale triggers to the real header, without a backend or trade fetch.
struct HeaderHarness {
    width: f32,
    boxes: Rc<RefCell<Vec<Bounds<Pixels>>>>,
}

impl Render for HeaderHarness {
    /// Lay out the real header with a long title and the shared icon settings trigger.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let boxes = self.boxes.clone();
        let settings = crate::panels::common::popup_gear_trigger_dense(
            "header-settings",
            rust_i18n::t!("trade_window.settings.tip").to_string(),
            false,
        )
        .into_any_element();
        // The scale slot is the same MoonUI trigger size and width as controls::scale; handlers
        // and menu contents do not participate in the closed titlebar's layout.
        let p = MoonPalette::active(cx);
        let scale = MoonDropdown::new("header-scale")
            .trigger_width_scaled(58.0)
            .trigger_size(MoonSize::Xs.into())
            .segment(MoonButtonSegment::new("\u{1f50d}").color(p.text_muted))
            .segment(MoonButtonSegment::new(
                rust_i18n::t!("toolbar.scale_auto_short").to_string(),
            ))
            .into_any_element();
        div().w(px(self.width)).child(
            trade_header(
                "UNIT_TRADE_LONG_NAME ".repeat(20),
                false,
                settings,
                scale,
                cx,
            )
            .on_children_prepainted(move |bounds, _, _| *boxes.borrow_mut() = bounds),
        )
    }
}

/// Removing title truncation or fixed control sizing clips/overlaps buttons at minimum width.
/// Exercise all locales and themes, including the 360-content-pixel width at 200% UI zoom.
#[gpui::test]
fn trade_header_controls_fit_every_locale(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    for locale in ["ru", "en", "es", "uk", "tr", "pt", "vi"] {
        let _locale = crate::test_locale::force(locale);
        for mode in [
            UiThemeMode::Dark,
            UiThemeMode::Light,
            UiThemeMode::Graphite,
            UiThemeMode::LightExperimental,
            UiThemeMode::DarkExperimental,
        ] {
            cx.update(|cx| {
                moon_ui::MoonTheme::install_config(
                    crate::startup::moon_theme_config_for_presentation(mode, 1.0),
                    cx,
                )
            });
            for width in [360.0, 720.0, 1920.0] {
                let boxes = Rc::new(RefCell::new(Vec::new()));
                let sink = boxes.clone();
                let handle = cx.add_window(move |_, _| HeaderHarness { width, boxes: sink });
                cx.update_window(handle.into(), |_, window, cx| {
                    window.draw(cx).clear();
                })
                .unwrap();
                let boxes = boxes.borrow();
                let custom_controls = crate::design::show_custom_window_controls();
                assert_eq!(boxes.len(), if custom_controls { 5 } else { 4 });
                for bounds in boxes.iter() {
                    assert!(
                        bounds.origin.x >= px(0.0) && bounds.right() <= px(width + 0.5),
                        "{locale}: child clipped at width {width}: {bounds:?}"
                    );
                }
                for pair in boxes.windows(2) {
                    assert!(
                        pair[0].right() <= pair[1].origin.x,
                        "{locale}: titlebar children overlap at width {width}"
                    );
                }
                assert!(boxes[0].size.width > px(0.0), "title must remain readable");
                // Three 25px native controls with two 2px gaps, independently specified by
                // MoonUI's frame contract; dropping maximize or shrinking the group fails.
                if custom_controls {
                    assert_eq!(boxes[4].size.width, px(79.0));
                }
                cx.update_window(handle.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }
}
