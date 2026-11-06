//! Shared command-window chrome using MoonUI controls and active palette tokens.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonButton, MoonButtonVariant, MoonPalette, MoonSpinner, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use super::log_lines::{self, Kind};
use super::pages::{Button, Tone};
use super::view::CoreCmdsView;
use crate::design;

/// Map semantic state to the active theme's legible foreground token.
fn tone_color(tone: Tone, p: MoonPalette) -> u32 {
    match tone {
        Tone::Ok => p.green_text,
        Tone::Bad => p.red_text,
        Tone::Run => p.accent,
        Tone::Warn => p.amber,
    }
}

/// Semantic state panel, with a spinner only while a server operation is running.
pub(super) fn state_box(tone: Tone, title: String, why: String, cx: &App) -> impl IntoElement {
    #[cfg(test)]
    {
        super::tests::record(
            "cc-state-title".into(),
            title.clone(),
            14.0,
            FontWeight::SEMIBOLD,
        );
        if !why.is_empty() {
            super::tests::record("cc-state-why".into(), why.clone(), 13.0, FontWeight::NORMAL);
        }
    }
    let p = MoonPalette::active(cx);
    let color = tone_color(tone, p);
    v_flex()
        .w_full()
        .gap(px(8.0))
        .p(px(12.0))
        .rounded(px(6.0))
        .bg(rgba_from(color, 0.11))
        .child(
            h_flex()
                .items_center()
                .gap(px(10.0))
                .when(tone == Tone::Run, |row| row.child(MoonSpinner::new()))
                .child(
                    div()
                        .debug_selector(|| "cc-state-title".to_owned())
                        .flex_1()
                        .min_w_0()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgba_from(color, 1.0))
                        .child(title),
                ),
        )
        .when(!why.is_empty(), |root| {
            root.child(
                div()
                    .debug_selector(|| "cc-state-why".to_owned())
                    .w_full()
                    .child(why),
            )
        })
}

/// Palette-owned confirmation panel; the Stop action uses the danger variant.
pub(super) fn confirm_box(
    danger: bool,
    title: String,
    why: String,
    buttons: Vec<Button>,
    id: usize,
    cx: &mut Context<CoreCmdsView>,
) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let color = if danger { p.red_text } else { p.amber };
    #[cfg(test)]
    {
        super::tests::record(
            "cc-confirm-title".into(),
            title.clone(),
            13.0,
            FontWeight::SEMIBOLD,
        );
        super::tests::record(
            "cc-confirm-why".into(),
            why.clone(),
            13.0,
            FontWeight::NORMAL,
        );
    }
    v_flex()
        .w_full()
        .p(px(12.0))
        .gap(px(8.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(rgba_from(color, 1.0))
        .bg(rgba_from(color, 0.10))
        .child(
            div()
                .debug_selector(|| "cc-confirm-title".to_owned())
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            div()
                .debug_selector(|| "cc-confirm-why".to_owned())
                .w_full()
                .child(why),
        )
        .child(buttons_row(buttons, &format!("cc-confirm-{id}"), cx))
}

/// Facts use a shared key column; values own the remaining width and can wrap independently.
pub(super) fn kv_block(rows: Vec<(&'static str, String)>, cx: &App) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let key_width = rows
        .iter()
        .map(|(key, _)| design::ui_body_text_width(cx, &t!(*key), 400.0))
        .fold(0.0, f32::max);
    v_flex()
        .w_full()
        .gap(px(4.0))
        .p(px(12.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(rgba_from(p.border, 1.0))
        .bg(rgba_from(p.panel, 1.0))
        .children(rows.into_iter().map(|(key, value)| {
            h_flex()
                .w_full()
                .items_start()
                .gap(px(18.0))
                .child(
                    div()
                        .flex_none()
                        .w(px(key_width))
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child(t!(key).to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(design::mono())
                        .child(value),
                )
        }))
}

/// Display a bounded log with faint timestamps and semantic warning/error emphasis.
pub(super) fn mono_block(lines: Vec<String>, cx: &App) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    v_flex()
        .id("cc-log")
        .w_full()
        .max_h(px(260.0))
        .overflow_y_scroll()
        .p(px(10.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(rgba_from(p.border, 1.0))
        .bg(rgba_from(p.panel, 1.0))
        .font_family(design::mono())
        .text_size(px(12.0))
        .line_height(px(18.0))
        .children(lines.into_iter().map(|line| {
            let (stamp, kind, message) = log_lines::classify(&line);
            let color = match kind {
                Kind::Error => p.red_text,
                Kind::Warn => p.amber,
                Kind::Plain => p.text,
            };
            let prefix = stamp.map(|s| format!("{s} ")).unwrap_or_default();
            let text = format!("{prefix}{message}");
            let mut runs = vec![];
            if !prefix.is_empty() {
                runs.push(TextRun {
                    len: prefix.len(),
                    color: rgba_from(p.text_faint, 1.0).into(),
                    ..TextRun::default()
                });
            }
            runs.push(TextRun {
                len: message.len(),
                color: rgba_from(color, 1.0).into(),
                ..TextRun::default()
            });
            div()
                .w_full()
                .flex_none()
                .child(StyledText::new(text).with_runs(runs))
        }))
}

/// Use the standard MoonUI button for focus, keyboard activation, hover, and disabled behavior.
pub(super) fn button(spec: Button, id: String, cx: &mut Context<CoreCmdsView>) -> AnyElement {
    let action = spec.action;
    div()
        .flex_none()
        .debug_selector(|| id.clone())
        .child(
            MoonButton::new(SharedString::from(id.clone()))
                .size(design::CONTROL_TIER)
                .mono(false)
                .label(t!(spec.key).to_string())
                .variant(if spec.danger {
                    MoonButtonVariant::Danger
                } else if spec.primary {
                    MoonButtonVariant::Solid
                } else {
                    MoonButtonVariant::Neutral
                })
                .disabled(!spec.enabled)
                .on_click(
                    cx.listener(move |this, _, window, cx| this.act(action.clone(), window, cx)),
                )
                .render(),
        )
        .into_any_element()
}

/// Wrapping rows keep translated action labels inside their available width.
pub(super) fn buttons_row(
    buttons: Vec<Button>,
    prefix: &str,
    cx: &mut Context<CoreCmdsView>,
) -> Div {
    h_flex()
        .w_full()
        .items_center()
        .flex_wrap()
        .gap(px(8.0))
        .children(
            buttons
                .into_iter()
                .enumerate()
                .map(|(i, spec)| button(spec, format!("{prefix}-{i}"), cx))
                .collect::<Vec<_>>(),
        )
}

/// Hint and actions share a footer; narrow windows put the hint on its own row.
pub(super) fn footer(
    hint: Option<String>,
    buttons: Vec<Button>,
    narrow: bool,
    cx: &mut Context<CoreCmdsView>,
) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let buttons = buttons_row(buttons, "cc-footer-button", cx)
        .justify_end()
        .when(!narrow, |row| row.w_auto().flex_none());
    div()
        .debug_selector(|| "cc-footer".to_owned())
        .flex()
        .flex_none()
        .w_full()
        .when(narrow, |root| root.flex_col())
        .items_center()
        .justify_end()
        .gap(px(8.0))
        .px(px(16.0))
        .py(px(10.0))
        .bg(rgba_from(p.shell_high, 1.0))
        .border_t_1()
        .border_color(rgba_from(p.border, 1.0))
        .when_some(hint, |root, hint| {
            root.child(
                div()
                    .min_w_0()
                    .when(!narrow, |hint| hint.flex_1())
                    .text_size(px(12.0))
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(hint),
            )
        })
        .child(buttons)
}
