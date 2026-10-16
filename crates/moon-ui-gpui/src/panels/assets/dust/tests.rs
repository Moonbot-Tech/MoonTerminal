//! Dust eligibility, one-shot confirmation dispatch, and localized headless layout regressions.

use super::{DustConfirmation, core_header, dust_button, dust_target};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    AppContext, Bounds, Context, IntoElement, ParentElement, Pixels, Render, Styled, Window, div,
    px,
};
use moon_core::config::UiThemeMode;
use moon_core::venue::CoreVenue;

/// Widening support by exchange family would expose futures or undocumented Gate conversion.
#[test]
fn only_documented_spot_venue_offers_dust() {
    assert_eq!(dust_target(None), None);
    for code in 0..=255 {
        let venue = CoreVenue::identify(code, "", None);
        assert_eq!(
            dust_target(Some(&venue)),
            (code == 3).then_some("BNB"),
            "platform {code}"
        );
    }
}

/// Removing the one-shot guard or using a mutable selected core sends duplicate/wrong commands.
#[test]
fn confirming_sends_exactly_the_captured_core_once() {
    let confirmation = DustConfirmation {
        core: 7,
        consumed: Cell::new(false),
    };
    let mut sent = Vec::new();
    assert!(sent.is_empty(), "opening a confirmation must not submit");
    for _ in 0..2 {
        confirmation
            .confirm(true, |core| {
                sent.push(core);
                Ok::<_, ()>(())
            })
            .unwrap();
    }
    assert_eq!(sent, [7]);
}

/// Independent wiring contract from issue 658: Yes must pass the captured id to the existing
/// dust command, not a balance refresh or a no-op. The behavioral test above checks the guard;
/// this binary-crate source check catches changing the production callback behind that guard.
#[test]
fn yes_wires_the_one_shot_guard_to_the_session_dust_command() {
    let source = include_str!("../dust.rs");
    let yes = source
        .split_once("MoonButton::new(\"assets-dust-yes\")")
        .expect("the confirmation must have a Yes button")
        .1;
    // Ignore whitespace so rustfmt cannot change the contract being checked.
    let yes: String = yes.split_whitespace().collect();
    assert!(yes.contains("letauthorized=panel.dust_authorized(core,app);"));
    assert!(yes.contains(
        "confirmation.confirm(authorized,|id|{panel.backend.read(app).session.convert_dust(id)})"
    ));
    assert_eq!(source.matches(".convert_dust(").count(), 1);
}

/// A stale scope or offline core must not dispatch even when an old Yes callback fires.
#[test]
fn stale_confirmation_and_failed_send_do_not_retry() {
    let confirmation = DustConfirmation {
        core: 7,
        consumed: Cell::new(false),
    };
    confirmation
        .confirm(false, |_| -> Result<(), ()> {
            panic!("stale confirmation sent")
        })
        .unwrap();
    assert_eq!(confirmation.confirm(true, |_| Err("closed")), Err("closed"));
    confirmation
        .confirm(true, |_| -> Result<(), &str> {
            panic!("failed command retried")
        })
        .unwrap();
}

/// Render the production header under a constrained parent and capture both children's boxes.
struct HeaderHarness {
    width: f32,
    boxes: Rc<RefCell<Vec<Bounds<Pixels>>>>,
}

impl Render for HeaderHarness {
    /// Layout production widgets, keeping the text/button relationship visible to the assertion.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let boxes = self.boxes.clone();
        div().w(px(self.width)).child(
            core_header(
                "CORE_A".into(),
                "Binance Spot".into(),
                dust_button(7, "BNB"),
                cx,
            )
            .on_children_prepainted(move |bounds, _, _| *boxes.borrow_mut() = bounds),
        )
    }
}

/// Fixed English widths or allowing the action to shrink clips a translation in narrow docks.
/// This drives the actual MoonUI button and flex layout with every shipped locale and theme.
#[gpui::test]
fn localized_dust_label_fits_narrow_and_wide_headers(cx: &mut gpui::TestAppContext) {
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
            for scale in [1.0, 1.5] {
                cx.update(|cx| {
                    moon_ui::MoonTheme::install_config(
                        crate::startup::moon_theme_config_for_presentation(mode, scale),
                        cx,
                    )
                });
                for width in [240.0, 640.0] {
                    let boxes = Rc::new(RefCell::new(Vec::new()));
                    let sink = boxes.clone();
                    let window = cx.add_window(move |_, _| HeaderHarness { width, boxes: sink });
                    let label_width = cx
                        .update_window(window.into(), |_, window, cx| {
                            window.draw(cx).clear();
                            let text = rust_i18n::t!("assets.dust.button", token = "BNB");
                            crate::design::ui_text_width_zoomed(
                                cx,
                                &text,
                                moon_ui::MoonSize::Xs.control_metrics().font_size,
                                400.0,
                                false,
                            )
                        })
                        .unwrap();
                    let boxes = boxes.borrow();
                    assert_eq!(boxes.len(), 2);
                    let button = boxes[1];
                    assert!(
                        f32::from(button.size.width) > label_width,
                        "{locale}: button must contain its complete label"
                    );
                    assert!(
                        f32::from(button.right()) <= width,
                        "{locale}: action must remain within narrow header"
                    );
                    assert!(
                        button.origin.y >= boxes[0].bottom() || button.origin.x >= boxes[0].right(),
                        "{locale}: name must not overlap action"
                    );
                }
            }
        }
    }
}
