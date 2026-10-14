//! Layout of the compact popup's blacklist block in every locale.
//!
//! The block is [`super::compact_blacklist_rows`], the function the General tab renders.

use gpui::{
    App, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Render, Styled,
    VisualTestContext, Window, div, px,
};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::MoonTheme;
use rust_i18n::t;

use crate::design;
use crate::shell::BlacklistReveal;
use crate::test_locale::force;

use super::compact_blacklist_rows;

struct Host;

impl Render for Host {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = "XALPHA, WBTC";
        let coin = "XALPHA";
        let reveal = BlacklistReveal {
            coin,
            alpha: 1.0,
            error: None,
        };
        div().size_full().child(compact_blacklist_rows(
            div()
                .id("bl-flag")
                .flex_none()
                .whitespace_nowrap()
                .child(t!("core_settings.blacklist").to_string()),
            div()
                .id("bl-exclude")
                .flex_none()
                .whitespace_nowrap()
                .child(t!("core_settings.exclude_delta").to_string()),
            div().id("bl-expand").flex_none().child("..."),
            div().id("bl-editor-stub").w_full().child(text),
            text,
            Some(reveal),
            |_, _| {},
            cx,
        ))
    }
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

/// Every locale: the highlighted token and the remove button stay inside the risk-limits block
/// and at least as wide as the text they draw.
#[gpui::test]
fn compact_blacklist_fits_every_locale(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0),
            cx,
        );
    });
    for language in Language::ALL {
        let _locale = force(language.code());
        let window = cx.open_window(gpui::size(px(1600.0), px(700.0)), |_, _| Host);
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        for _ in 0..3 {
            visual.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let at = language.code();
        let row = visual
            .debug_bounds("bl-compact-rows")
            .unwrap_or_else(|| panic!("bl-compact-rows not laid out ({at})"));
        let (token_w, label_w) = visual.update(|_window, app| widths(app));
        let token = visual
            .debug_bounds("coin-bl-token")
            .unwrap_or_else(|| panic!("coin-bl-token not laid out ({at})"));
        let button = visual
            .debug_bounds("bl-compact-remove")
            .unwrap_or_else(|| panic!("bl-compact-remove not laid out ({at})"));
        assert!(
            inside(token, row),
            "token escaped the blacklist block ({at})"
        );
        assert!(
            fits(token, token_w),
            "token is {:.1}px for a {:.1}px string ({at})",
            f32::from(token.size.width),
            token_w
        );
        assert!(
            inside(button, row),
            "remove button escaped the blacklist block ({at})"
        );
        assert!(
            fits(button, label_w),
            "remove button is {:.1}px for a {:.1}px label ({at})",
            f32::from(button.size.width),
            label_w
        );
    }
}

fn widths(cx: &App) -> (f32, f32) {
    let token = design::ui_text_width_zoomed(cx, "XALPHA", design::BODY_TEXT, 700.0, true);
    let label = t!("coin_bl_places.remove_coin", coin = "XALPHA").to_string();
    let label_w = design::ui_text_width_zoomed(cx, &label, design::BODY_TEXT, 400.0, false);
    (token, label_w)
}

/// The General tab must keep drawing this block inside the amber frame.
#[test]
fn general_tab_draws_the_blacklist_block_inside_the_amber_frame() {
    let source = include_str!("../general.rs");
    let body = braced_body(source, "pub(super) fn general_tab(");
    assert!(body.contains("compact_blacklist_rows("));
    assert!(body.contains("\"core-frame-risks-body\""));
    assert!(body.contains("moon_alpha(p.amber, alpha)"));
}

fn braced_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("expected `{signature}` in the general tab"));
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
