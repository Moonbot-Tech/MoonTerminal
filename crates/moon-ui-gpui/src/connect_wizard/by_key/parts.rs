//! Reusable content blocks for the by-key instructions and recovery screens.

use gpui::*;
use moon_ui::{MoonButton, MoonInput, MoonInputState, MoonPalette, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use super::logic::{Action, Footer, Step, firewall_command, footer_actions};
use crate::connect_wizard::render::{Screen, note};
use crate::design;

/// Semantic wash behind a short, readable callout.
#[derive(Clone, Copy)]
pub(super) enum Callout {
    Info,
    Ok,
    Hold,
}

/// Keep normal text ink over a light semantic tint, including in the light theme.
pub(super) fn callout(kind: Callout, title: String, body: String, cx: &App) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let tint = match kind {
        Callout::Info => p.blue,
        Callout::Ok => p.green,
        Callout::Hold => p.amber,
    };
    v_flex()
        .w_full()
        .min_w(px(0.0))
        .gap(design::ui_px(cx, 4.0))
        .p(design::ui_px(cx, 12.0))
        .rounded(design::ui_px(cx, 6.0))
        .bg(rgba_from(tint, 0.12))
        .text_color(rgba_from(p.text, 1.0))
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(body)
}

/// A command wraps within the remaining width while the copy action keeps its natural width.
pub(super) fn code_box(
    port: u16,
    copied: bool,
    on_copy: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    h_flex()
        .id("cw-key-code")
        .debug_selector(|| "cw-key-code".into())
        .w_full()
        .min_w(px(0.0))
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .p(design::ui_px(cx, 10.0))
        .rounded(design::ui_px(cx, 4.0))
        .border(px(1.0))
        .border_color(rgba_from(p.border, 1.0))
        .bg(rgba_from(p.panel, 1.0))
        .child(
            div()
                .id("cw-key-command-text")
                .debug_selector(|| "cw-key-command-text".into())
                .flex_1()
                .min_w(px(0.0))
                .whitespace_normal()
                .font_family(design::mono())
                .text_size(design::t_caption(cx))
                .child(firewall_command(port)),
        )
        .child(
            div()
                .debug_selector(|| "cw-key-copy".into())
                .flex_none()
                .child(
                    MoonButton::new("cw-key-copy")
                        .size(design::CONTROL_TIER)
                        .label(
                            if copied {
                                t!("connect_wizard.by_key.copied")
                            } else {
                                t!("connect_wizard.by_key.copy")
                            }
                            .to_string(),
                        )
                        .on_click(on_copy)
                        .render(),
                ),
        )
}

/// A distinct green line carries the exact result the user should see in Moonbot.
pub(super) fn status_line(text: String, cx: &App) -> impl IntoElement {
    div()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgba_from(MoonPalette::active(cx).green_text, 1.0))
        .child(text)
}

/// One numbered instruction, with optional notes or a command beneath its title.
pub(super) fn numbered_item(
    number: usize,
    title: impl Into<SharedString>,
    details: Vec<AnyElement>,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    h_flex()
        .w_full()
        .min_w(px(0.0))
        .items_start()
        .gap(design::ui_px(cx, 10.0))
        .p(design::ui_px(cx, 10.0))
        .border_b(px(1.0))
        .border_color(rgba_from(p.border, 1.0))
        .child(
            div()
                .flex_none()
                .size(design::ui_px(cx, 20.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba_from(p.accent, 1.0))
                .text_color(rgba_from(p.on_accent, 1.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child(number.to_string()),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.0))
                .gap(design::ui_px(cx, 3.0))
                .child(div().font_weight(FontWeight::MEDIUM).child(title.into()))
                .children(details),
        )
        .into_any_element()
}

/// A translated checklist item with at most one supporting note.
pub(super) fn item(
    number: usize,
    title_key: &'static str,
    note_key: Option<&'static str>,
    cx: &App,
) -> AnyElement {
    numbered_item(
        number,
        t!(title_key),
        note_key
            .into_iter()
            .map(|key| note(t!(key), cx).into_any_element())
            .collect(),
        cx,
    )
}

/// The same amber warning accompanies initial and replacement key port decisions.
pub(super) fn mismatch_callout(key: u16, chosen: u16, cx: &App) -> impl IntoElement {
    callout(
        Callout::Hold,
        t!(
            "connect_wizard.by_key.mismatch_title",
            key = key,
            chosen = chosen
        )
        .to_string(),
        t!("connect_wizard.by_key.mismatch_body", chosen = chosen).to_string(),
        cx,
    )
}

/// Render every by-key footer from the same button definitions in production and tests.
pub(super) fn with_footer(
    mut screen: Screen,
    footer: Footer,
    mismatch: Option<u16>,
    on_action: impl Fn(Action, &ClickEvent, &mut Window, &mut App) + 'static,
) -> Screen {
    let on_action = std::rc::Rc::new(on_action);
    for action in footer_actions(footer, mismatch) {
        let (id, label) = match action {
            Action::Back => ("cw-key-back", t!("connect_wizard.back")),
            Action::Next => (
                "cw-key-next",
                if matches!(footer, Footer::Instructions(Step::Moonbot)) {
                    t!("connect_wizard.by_key.key_copied")
                } else {
                    t!("connect_wizard.next")
                },
            ),
            Action::Connect | Action::SetAddress => {
                ("cw-key-connect", t!("connect_wizard.by_key.connect"))
            }
            Action::AcceptPort => (
                "cw-key-accept",
                t!(
                    "connect_wizard.by_key.connect_with_port",
                    port = mismatch.expect("mismatch action has a port")
                ),
            ),
            Action::Paste => ("cw-key-repaste", t!("connect_wizard.by_key.paste_again")),
            Action::Cancel => ("cw-key-cancel", t!("connect_wizard.cancel")),
            Action::Drop => ("cw-key-drop", t!("connect_wizard.by_key.drop")),
            Action::Retry => ("cw-key-retry", t!("connect_wizard.check_again")),
            Action::NewKey => (
                "cw-key-new-key",
                t!("connect_wizard.by_key.refused_connect"),
            ),
            Action::Again => ("cw-again", t!("connect_wizard.again")),
            Action::Finish => ("cw-finish", t!("connect_wizard.finish")),
        };
        let left = matches!(action, Action::Back | Action::Drop | Action::Again);
        let primary = !left && !matches!(action, Action::Cancel | Action::AcceptPort);
        let on_action = on_action.clone();
        let mut button = MoonButton::new(id)
            .size(design::CONTROL_TIER)
            .label(label.to_string());
        if primary {
            button = button.primary();
        }
        let button = div().debug_selector(move || id.into()).flex_none().child(
            button
                .on_click(move |event, window, cx| on_action(action, event, window, cx))
                .render(),
        );
        screen = if left {
            screen.left(button)
        } else {
            screen.right(button)
        };
    }
    screen
}

/// Shared bordered instruction list; each item owns one fact and its supporting notes.
pub(super) fn numbered_list(items: Vec<AnyElement>, cx: &App) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    v_flex()
        .id("cw-key-steps")
        .debug_selector(|| "cw-key-steps".into())
        .w_full()
        .min_w(px(0.0))
        .border(px(1.0))
        .border_color(rgba_from(p.border, 1.0))
        .rounded(design::ui_px(cx, 6.0))
        .bg(rgba_from(p.panel, 1.0))
        .children(items)
}

/// A full-width labelled input whose width follows the content column.
pub(super) fn input_field(
    id: &'static str,
    label: String,
    state: &Entity<MoonInputState>,
    placeholder: String,
    cx: &App,
) -> impl IntoElement {
    v_flex()
        .debug_selector(|| id.to_owned())
        .w_full()
        .min_w(px(0.0))
        .gap(design::ui_px(cx, 4.0))
        .child(note(label, cx))
        .child(
            MoonInput::new(id)
                .state(state)
                .size(design::INPUT_SIZE)
                .placeholder(placeholder),
        )
}

/// The masked key and paste action share a wrapping row instead of a wide fixed label column.
pub(super) fn key_row(
    key: &Entity<MoonInputState>,
    on_paste: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .min_w(px(0.0))
        .gap(design::ui_px(cx, 4.0))
        .child(note(t!("connect_wizard.by_key.key"), cx))
        .child(
            h_flex()
                .id("cw-key-box")
                .debug_selector(|| "cw-key-box".into())
                .w_full()
                .min_w(px(0.0))
                .flex_wrap()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(
                    div()
                        .debug_selector(|| "cw-key-key".into())
                        .flex_1()
                        .min_w(design::ui_px(cx, 180.0))
                        .child(
                            MoonInput::new("cw-key-key")
                                .state(key)
                                .size(design::INPUT_SIZE)
                                .mask_toggle()
                                .placeholder(
                                    t!("connect_wizard.by_key.key_placeholder").to_string(),
                                ),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "cw-key-paste".into())
                        .flex_none()
                        .child(
                            MoonButton::new("cw-key-paste")
                                .size(design::CONTROL_TIER)
                                .label(t!("connect_wizard.by_key.paste").to_string())
                                .on_click(on_paste)
                                .render(),
                        ),
                ),
        )
}
