//! Layout of a strategy coins-blacklist row in every locale.
//!
//! The strip and the remove button are the functions [`super::field_row`] calls. The 180px label
//! pitch stays in `field_row`; this row uses the same pitch so a long remove label is measured
//! against the room that pitch leaves.

use gpui::{
    AnyElement, App, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels,
    Render, Styled, VisualTestContext, Window, div, px,
};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::{MoonTheme, h_flex, v_flex};
use rust_i18n::t;

use crate::design;
use crate::test_locale::force;

use super::strategy_blacklist_remove;
use crate::controls::coin_token::coin_token_strip;

struct Host {
    compact: bool,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(strategy_row(self.compact, cx))
    }
}

fn strategy_row(compact: bool, cx: &App) -> AnyElement {
    let text = "XALPHA, WBTC";
    let coin = "XALPHA";
    let size = if compact {
        design::t_caption(cx)
    } else {
        design::t_body(cx)
    };
    let max_width = compact.then(|| design::font_w_px(cx, 160.0));
    let strip = coin_token_strip(text, coin, size, max_width, cx);
    let button = strategy_blacklist_remove("coins", coin, None, compact, |_, _| {}, cx);
    let value = if compact {
        h_flex()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .child(strip)
            .child(div().flex_1().min_w_0().child(text))
            .into_any_element()
    } else {
        v_flex()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .gap(px(2.0))
            .child(strip)
            .child(text)
            .into_any_element()
    };
    h_flex()
        .id("field-row-bl")
        .debug_selector(|| "field-row-bl".into())
        .w_full()
        .items_start()
        .gap(design::ui_px(cx, 14.0))
        .pl(px(8.0))
        .pr_2()
        .child(
            v_flex()
                .id("field-label-bl")
                .w(design::font_w_px(cx, 180.0))
                .flex_none()
                .min_w_0()
                .child("CoinsBlackList"),
        )
        .child(value)
        .child(button)
        .into_any_element()
}

fn inside(child: Bounds<Pixels>, parent: Bounds<Pixels>) -> bool {
    child.origin.x + px(0.5) >= parent.origin.x
        && child.right() <= parent.right() + px(0.5)
        && child.origin.y + px(0.5) >= parent.origin.y
        && child.bottom() <= parent.bottom() + px(0.5)
}

fn fits(bounds: Bounds<Pixels>, width: f32) -> bool {
    f32::from(bounds.size.width) + 0.1 >= width
}

/// Every locale, both row pitches: the token and the remove button stay inside the row, and the
/// button is at least as wide as its label.
#[gpui::test]
fn strategy_blacklist_row_fits_every_locale(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0),
            cx,
        );
    });
    for language in Language::ALL {
        let _locale = force(language.code());
        for compact in [false, true] {
            let window = cx.open_window(gpui::size(px(1600.0), px(700.0)), |_, _| Host { compact });
            let mut visual = VisualTestContext::from_window(window.into(), cx);
            for _ in 0..3 {
                visual.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
            let at = format!("{} compact={compact}", language.code());
            let row = visual
                .debug_bounds("field-row-bl")
                .unwrap_or_else(|| panic!("field-row-bl not laid out ({at})"));
            let (token_w, label_w) = visual.update(|_window, app| widths(app, compact));
            let token = visual
                .debug_bounds("coin-bl-token")
                .unwrap_or_else(|| panic!("coin-bl-token not laid out ({at})"));
            let button = visual
                .debug_bounds("coin-bl-remove-wrap-coins")
                .unwrap_or_else(|| panic!("coin-bl-remove-wrap-coins not laid out ({at})"));
            assert!(inside(token, row), "token escaped the field row ({at})");
            assert!(
                fits(token, token_w),
                "token is {:.1}px for a {:.1}px string ({at})",
                f32::from(token.size.width),
                token_w
            );
            assert!(
                inside(button, row),
                "remove button escaped the field row ({at})"
            );
            assert!(
                fits(button, label_w),
                "remove button is {:.1}px for a {:.1}px label ({at})",
                f32::from(button.size.width),
                label_w
            );
        }
    }
}

fn widths(cx: &App, compact: bool) -> (f32, f32) {
    let size = if compact {
        design::BODY_TEXT - 2.0
    } else {
        design::BODY_TEXT
    };
    let token = design::ui_text_width_zoomed(cx, "XALPHA", size, 700.0, true);
    let label = t!("coin_bl_places.remove_coin", coin = "XALPHA").to_string();
    let label_w = design::ui_text_width_zoomed(cx, &label, design::BODY_TEXT, 400.0, false);
    (token, label_w)
}

/// `field_row` keeps the 180px label cell and draws the reveal through the shared helpers.
#[test]
fn field_row_keeps_the_label_pitch_and_the_reveal_helpers() {
    let source = include_str!("../editors.rs");
    let body = braced_body(source, "pub(in crate::strategies) fn field_row(");
    assert!(
        body.contains(".w(design::font_w_px(cx, 180.0))"),
        "the label pitch stays inside field_row"
    );
    assert!(body.contains("coin_token_strip("));
    assert!(body.contains("strategy_blacklist_remove("));
}

fn braced_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("expected `{signature}` in editors source"));
    let open = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("expected opening brace after `{signature}`"));
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced brace after `{signature}`");
}
