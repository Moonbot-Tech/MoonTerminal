//! PIN-row interaction and narrow-window layout in every shipped locale.

use gpui::{
    AppContext, Bounds, Context, InteractiveElement, IntoElement, Modifiers, ParentElement, Pixels,
    Render, Styled, VisualTestContext, Window, div, px,
};
use moon_core::{
    config::{Language, UiThemeMode},
    feed::CoreTelegramState,
};
use moon_ui::{MoonPalette, MoonTheme, Root, h_flex, v_flex};
use rust_i18n::t;

use crate::{
    core_expert::{MIN_SIZE, remote::RemoteBlock},
    design,
};

use super::{RemoteCtx, pin};

const PIN: &str = "739182";

/// Render the production PIN row in the narrowest column its real three-column page can give it.
struct Host {
    pin: Option<&'static str>,
}

impl Render for Host {
    /// Reserve the expert rail's maximum share of the minimum window, then lay out three equal
    /// remote-page columns with the production PIN row in the middle one.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = CoreTelegramState {
            settings_supported: true,
            channel_pin: self.pin.map(str::to_string),
            ..Default::default()
        };
        let block = RemoteBlock::default();
        let remote = RemoteCtx {
            block: &block,
            snapshot: Some(&snapshot),
            fresh: true,
            track: None,
            core_name: None,
            selected_cores: 1,
        };
        h_flex()
            .size_full()
            .child(div().flex_none().w(px(MIN_SIZE.0 * 0.45)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .px(design::ui_px(cx, design::HEADER_PAD_X))
                    .child(
                        h_flex()
                            .w_full()
                            .items_start()
                            .gap(design::ui_px(cx, 10.0))
                            .child(v_flex().flex_1().min_w_0())
                            .child(
                                v_flex()
                                    .debug_selector(|| "exp-sp-pin-test-column".to_string())
                                    .flex_1()
                                    .min_w_0()
                                    .child(pin(&remote, MoonPalette::active(cx), cx)),
                            )
                            .child(v_flex().flex_1().min_w_0()),
                    ),
            )
    }
}

/// Open and paint the PIN host at the expert window's real minimum dimensions.
fn open_pin_window(cx: &mut gpui::TestAppContext, pin: Option<&'static str>) -> VisualTestContext {
    let handle = cx.open_window(gpui::size(px(MIN_SIZE.0), px(MIN_SIZE.1)), |window, cx| {
        let host = cx.new(|_| Host { pin });
        Root::new(host, window, cx)
    });
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    for _ in 0..3 {
        visual.update(|window, app| {
            let _ = window.draw(app);
        });
    }
    visual
}

/// Whether one laid-out rectangle stays inside another, allowing only fractional-pixel rounding.
fn inside(child: Bounds<Pixels>, parent: Bounds<Pixels>) -> bool {
    child.origin.x + px(0.5) >= parent.origin.x
        && child.right() <= parent.right() + px(0.5)
        && child.origin.y + px(0.5) >= parent.origin.y
        && child.bottom() <= parent.bottom() + px(0.5)
}

/// A wide translation or missing flex constraint would push the PIN or its copy action out of the
/// narrowest column at the window minimum.
#[gpui::test]
fn pin_and_copy_fit_every_locale_at_minimum_window_width(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    cx.update(|app| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            app,
        )
    });
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let mut visual = open_pin_window(cx, Some(PIN));
        let at = language.code();
        let column = visual
            .debug_bounds("exp-sp-pin-test-column")
            .unwrap_or_else(|| panic!("PIN column not laid out ({at})"));
        let row = visual
            .debug_bounds("exp-sp-pin-row")
            .unwrap_or_else(|| panic!("PIN row not laid out ({at})"));
        let value = visual
            .debug_bounds("exp-sp-pin-value")
            .unwrap_or_else(|| panic!("PIN value not laid out ({at})"));
        let button = visual
            .debug_bounds("exp-sp-copy-pin")
            .unwrap_or_else(|| panic!("copy button not laid out ({at})"));
        assert!(inside(row, column), "PIN row escaped its column ({at})");
        assert!(inside(value, row), "PIN escaped its row ({at})");
        assert!(
            inside(button, row),
            "copy button escaped its row ({at}): column {column:?}, row {row:?}, value {value:?}, button {button:?}"
        );
        assert!(
            value.right() <= button.origin.x + px(0.5),
            "PIN and copy button overlap ({at})"
        );
        let required_button_width = visual.update(|_window, app| {
            design::ui_caption_text_width(app, &t!("core_cmds.common.copy"), 400.0) + 2.0 * 4.0
        });
        assert!(
            f32::from(button.size.width) + 0.5 >= required_button_width,
            "copy label is clipped ({at}): button {}, required {required_button_width}",
            f32::from(button.size.width)
        );
        visual.update(|window, _| window.remove_window());
    }
}

/// Missing and empty snapshots must offer no copy control whose payload could be blank.
#[gpui::test]
fn absent_pin_has_no_copy_button(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    let _locale = crate::test_locale::force("en");
    for supplied in [None, Some("")] {
        let mut visual = open_pin_window(cx, supplied);
        assert!(
            visual.debug_bounds("exp-sp-pin-row").is_some(),
            "dash placeholder row missing"
        );
        assert!(
            visual.debug_bounds("exp-sp-copy-pin").is_none(),
            "copy offered without a real PIN"
        );
        visual.update(|window, _| window.remove_window());
    }
}

/// Clicking the production MoonButton must put the exact snapshot text, not a placeholder or a
/// reformatted value, on the clipboard and confirm the action through the existing notification layer.
#[gpui::test]
fn copy_button_writes_the_exact_pin(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    let _locale = crate::test_locale::force("en");
    let mut visual = open_pin_window(cx, Some(PIN));
    let button = visual
        .debug_bounds("exp-sp-copy-pin")
        .expect("copy button with a real PIN");
    visual.simulate_click(button.center(), Modifiers::none());
    let copied = visual
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("PIN copied to clipboard");
    assert_eq!(copied, PIN);
    let notifications =
        visual.update(|window, app| Root::read(window, app).notification_count(app));
    assert_eq!(notifications, 1, "copy gave no success notification");
    visual.update(|window, _| window.remove_window());
}
