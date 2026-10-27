//! The move window's layout: the form, the server's key to confirm, and the move as it runs.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonCheckbox, MoonInput, MoonInputState, MoonPalette, MoonWindowFrame, h_flex,
    rgba_from, v_flex,
};
use rust_i18n::t;

use super::CoreMoveView;
use crate::backend::core_move::{Command, MovePhase};
use crate::design;
use crate::settings::telegram::server_bot::progress::StationProgress;

/// Window header height in unscaled pixels, matching the other tool windows.
const HEADER_H: f32 = 30.0;
/// Label column width in unscaled pixels.
const LABEL_W: f32 = 150.0;
/// Field width in unscaled pixels.
const FIELD_W: f32 = 260.0;

impl Render for CoreMoveView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let chrome_width = f32::from(window.viewport_size().width);
        let phase = self.phase(cx);
        let lines = self
            .backend
            .read(cx)
            .core_moves
            .get(self.uid)
            .map(|job| job.lines.clone())
            .unwrap_or_default();
        let mut body = v_flex()
            .id("core-move-body")
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_y_scroll()
            .p(design::ui_px(cx, 20.0))
            .gap(design::ui_px(cx, 10.0))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(design::t_title(cx))
                    .child(t!("core_move.title", name = &self.name).to_string()),
            );
        body = match &phase {
            None => self.form_view(body, cx),
            Some(phase) => self.phase_view(body, phase, cx),
        };
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .text_color(rgba_from(p.red_text, 1.0))
                    .child(error.clone()),
            );
        }
        if !lines.is_empty() {
            body = body.child(StationProgress {
                status: None,
                lines,
                scroll: self.log_scroll.clone(),
            });
        }
        v_flex()
            .size_full()
            .relative()
            .bg(rgba_from(p.shell, 1.0))
            .font_family(design::ui_font())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .text_color(rgba_from(p.text, 1.0))
            .child(header(&self.name, p, cx))
            .child(body)
            .child(
                MoonWindowFrame::tool("core-move-window-frame-hit", chrome_width)
                    .controls(moon_ui::MoonWindowFrameControls::MinimizeMaximizeClose)
                    .header_height(HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}

impl CoreMoveView {
    /// The form: the server, its login, where the core goes on it.
    fn form_view(&self, body: Stateful<Div>, cx: &Context<Self>) -> Stateful<Div> {
        body.child(note(t!("core_move.form.intro").to_string(), cx))
            .child(field(
                "core-move-host",
                t!("core_move.form.host"),
                &self.host,
                false,
                cx,
            ))
            .child(field(
                "core-move-login",
                t!("core_move.form.login"),
                &self.login,
                false,
                cx,
            ))
            .child(
                MoonCheckbox::new("core-move-by-key")
                    .checked(self.by_key)
                    .label(t!("core_move.form.by_key").to_string())
                    .on_change(cx.listener(|this, v: &bool, _, cx| {
                        this.by_key = *v;
                        cx.notify();
                    })),
            )
            .when(self.by_key, |s| {
                s.child(
                    h_flex()
                        .flex_wrap()
                        .items_center()
                        .gap(design::ui_px(cx, 10.0))
                        .child(field(
                            "core-move-key",
                            t!("core_move.form.key_path"),
                            &self.key_path,
                            false,
                            cx,
                        ))
                        .child(
                            MoonButton::new("core-move-key-browse")
                                .size(design::CONTROL_TIER)
                                .label(t!("core_move.form.key_browse").to_string())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.browse_key(window, cx);
                                }))
                                .render(),
                        ),
                )
                .child(field(
                    "core-move-passphrase",
                    t!("core_move.form.passphrase"),
                    &self.passphrase,
                    true,
                    cx,
                ))
            })
            .child(field(
                "core-move-password",
                if self.by_key {
                    t!("core_move.form.sudo_password")
                } else {
                    t!("core_move.form.password")
                },
                &self.password,
                true,
                cx,
            ))
            .child(field(
                "core-move-dir",
                t!("core_move.form.dir"),
                &self.dir,
                false,
                cx,
            ))
            .child(field(
                "core-move-port",
                t!("core_move.form.port"),
                &self.port,
                false,
                cx,
            ))
            .child(note(t!("core_move.form.order").to_string(), cx))
            .child(
                h_flex().child(
                    MoonButton::new("core-move-check")
                        .primary()
                        .size(design::CONTROL_TIER)
                        .label(t!("core_move.form.check").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.check(cx)))
                        .render(),
                ),
            )
    }

    /// Where the move stands, with the answers it takes there.
    fn phase_view(
        &self,
        body: Stateful<Div>,
        phase: &MovePhase,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let p = MoonPalette::active(cx);
        let status = |text: String| div().child(text);
        // While an answer is on its way, no other is offered.
        let answering = self
            .backend
            .read(cx)
            .core_moves
            .get(self.uid)
            .is_some_and(|job| job.answering);
        match phase {
            MovePhase::Probing => body.child(status(t!("core_move.phase.probing").to_string())),
            MovePhase::Review {
                target,
                fingerprint,
            } => {
                let (target, fingerprint) = (target.clone(), fingerprint.clone());
                body.child(status(
                    t!(
                        "core_move.review.fingerprint",
                        addr = target.addr(),
                        fingerprint = &fingerprint
                    )
                    .to_string(),
                ))
                .child(note(t!("core_move.review.what").to_string(), cx))
                .child(
                    div()
                        .text_color(rgba_from(p.red_text, 1.0))
                        .child(t!("core_move.review.secrets").to_string()),
                )
                .child(
                    buttons(cx).child(
                        MoonButton::new("core-move-confirm")
                            .primary()
                            .size(design::CONTROL_TIER)
                            .label(t!("core_move.review.confirm").to_string())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm(target.clone(), fingerprint.clone(), window, cx);
                            }))
                            .render(),
                    ),
                )
                .child(buttons(cx).child(self.start_over_button("core-move-review-back", cx)))
            }
            MovePhase::Running => body.child(status(t!("core_move.phase.running").to_string())),
            MovePhase::Refused { open_deals } => body
                .child(status(match open_deals {
                    Some(n) => t!("core_move.phase.refused", n = n).to_string(),
                    None => t!("core_move.phase.refused_unknown").to_string(),
                }))
                .child(note(t!("core_move.phase.refused_hint").to_string(), cx))
                .when(!answering, |body| {
                    body.child(
                        buttons(cx)
                            .child(self.answer_button(
                                "core-move-retry",
                                "core_move.retry",
                                Command::Retry,
                                true,
                                cx,
                            ))
                            .child(self.answer_button(
                                "core-move-wait",
                                "core_move.wait",
                                Command::WaitForStop,
                                false,
                                cx,
                            ))
                            .child(self.answer_button(
                                "core-move-cancel",
                                "core_move.cancel",
                                Command::Cancel,
                                false,
                                cx,
                            )),
                    )
                }),
            MovePhase::Waiting => body
                .child(status(t!("core_move.phase.waiting").to_string()))
                .when(!answering, |body| {
                    body.child(
                        buttons(cx)
                            .child(self.answer_button(
                                "core-move-retry",
                                "core_move.retry",
                                Command::Retry,
                                false,
                                cx,
                            ))
                            .child(self.answer_button(
                                "core-move-cancel",
                                "core_move.cancel",
                                Command::Cancel,
                                false,
                                cx,
                            )),
                    )
                }),
            MovePhase::Done { address } => body
                .child(status(
                    t!("core_move.phase.done", address = address).to_string(),
                ))
                .child(buttons(cx).child(self.start_over_button("core-move-again", cx))),
            MovePhase::Failed { reason } => body
                .child(
                    div()
                        .text_color(rgba_from(p.red_text, 1.0))
                        .child(reason.clone()),
                )
                .child(buttons(cx).child(self.start_over_button("core-move-again", cx))),
            MovePhase::Cancelled => body
                .child(status(t!("core_move.phase.cancelled").to_string()))
                .child(buttons(cx).child(self.start_over_button("core-move-again", cx))),
        }
    }

    /// A button that answers the move waiting on its old core.
    fn answer_button(
        &self,
        id: &'static str,
        label: &'static str,
        command: Command,
        primary: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let button = MoonButton::new(id)
            .size(design::CONTROL_TIER)
            .label(t!(label).to_string())
            .on_click(cx.listener(move |this, _, _, cx| this.answer(command, cx)));
        if primary { button.primary() } else { button }
            .render()
            .into_any_element()
    }

    /// "Start over": drops the finished move and shows the form again.
    fn start_over_button(&self, id: &'static str, cx: &Context<Self>) -> AnyElement {
        MoonButton::new(id)
            .size(design::CONTROL_TIER)
            .label(t!("core_move.start_over").to_string())
            .on_click(cx.listener(|this, _, _, cx| this.start_over(cx)))
            .render()
            .into_any_element()
    }
}

/// A wrapping row of buttons.
fn buttons(cx: &App) -> Div {
    h_flex().flex_wrap().gap(design::ui_px(cx, 8.0))
}

/// Secondary prose in the hint's style.
fn note(text: String, cx: &App) -> impl IntoElement {
    div()
        .text_color(rgba_from(MoonPalette::active(cx).text_muted, 1.0))
        .child(text)
}

/// A labelled input that wraps under its label at narrow widths.
fn field(
    id: &'static str,
    label: impl Into<SharedString>,
    state: &Entity<MoonInputState>,
    masked: bool,
    cx: &App,
) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let input = MoonInput::new(id).state(state).size(design::INPUT_SIZE);
    let input = if masked { input.mask_toggle() } else { input };
    h_flex()
        .flex_wrap()
        .gap(design::ui_px(cx, 10.0))
        .items_center()
        .child(
            div()
                .w(design::font_w_px(cx, LABEL_W))
                .text_color(rgba_from(p.text_soft, 1.0))
                .child(label.into()),
        )
        .child(div().w(design::font_w_px(cx, FIELD_W)).child(input))
}

/// The draggable window header.
fn header(name: &str, p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("core-move-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, HEADER_H, 14.0, 8.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(rgba_from(p.shell_high, 1.0))
        .border_b(px(1.0))
        .border_color(rgba_from(p.border, 1.0))
        .child(
            MoonWindowFrame::tool("core-move-titlebar-title", 0.0)
                .title_cluster(t!("core_move.window_title", name = name).to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("core-move-window-frame-visual", 0.0)
                    .controls(moon_ui::MoonWindowFrameControls::MinimizeMaximizeClose)
                    .header_height(HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}
