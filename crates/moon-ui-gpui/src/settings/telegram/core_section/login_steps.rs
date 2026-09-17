//! Login-step wizard for Settings -> Telegram on a picked core.
//!
//! Intents travel as typed [`TelegramCmd`] values. Inputs are typed, sent and dropped: nothing
//! here is persisted, drafted, or logged.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonCheckbox, MoonInput, MoonInputState, MoonPalette,
    MoonWindowExt as _, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::super::super::SettingsView;
use super::super::qr;
use crate::design;
use crate::display_text::fmt_duration_short;
use moon_core::feed::{
    AuthStep, CoreTelegramAuthDetails, CoreTelegramCodeType, CoreTelegramLoginMode,
    CoreTelegramService, ResendState, TelegramCmd, auth_step, resend_state,
};

use super::LOGOUT_DIALOG_ID;

impl SettingsView {
    pub(super) fn auth_block(
        &self,
        core: u64,
        service: &CoreTelegramService,
        muted: Hsla,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let mut col = v_flex().gap(design::ui_px(cx, 8.0)).child(
            div()
                .font_family(design::mono())
                .text_color(muted)
                .child(format!(
                    "{}: {}",
                    t!("telegram_core.connection"),
                    connection_label(&service.connection)
                )),
        );
        col = col.child(self.step_panel(core, service, muted, p, cx));
        if let Some(err) = service.error.as_ref() {
            col = col.child(
                div()
                    .font_family(design::ui_font())
                    .text_color(muted)
                    .child(
                        t!(
                            "telegram_core.service_error",
                            code = err.code,
                            message = err.message.as_str()
                        )
                        .to_string(),
                    ),
            );
        }
        col
    }

    fn step_panel(
        &self,
        core: u64,
        service: &CoreTelegramService,
        muted: Hsla,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        match auth_step(&service.auth_state) {
            AuthStep::WaitPhone => self.wait_phone(core, muted, p, cx).into_any_element(),
            AuthStep::WaitQrConfirmation => self
                .wait_qr(core, &service.details, muted, cx)
                .into_any_element(),
            AuthStep::WaitCode => self
                .wait_code(core, &service.details, muted, p, cx)
                .into_any_element(),
            AuthStep::WaitPassword => self
                .wait_password(core, &service.details, muted, p, cx)
                .into_any_element(),
            AuthStep::WaitEmailAddress => self.wait_email(core, muted, p, cx).into_any_element(),
            AuthStep::WaitEmailCode => self
                .wait_email_code(core, &service.details, muted, p, cx)
                .into_any_element(),
            AuthStep::WaitRegistration => self
                .wait_registration(core, &service.details, muted, p, cx)
                .into_any_element(),
            AuthStep::Ready => self
                .ready_row(core, service.phone.as_deref(), muted, cx)
                .into_any_element(),
            AuthStep::WaitPremiumPurchase => self
                .wait_premium(core, &service.details, muted, cx)
                .into_any_element(),
            AuthStep::Transitional => div()
                .font_family(design::ui_font())
                .text_color(muted)
                .child(t!("telegram_core.transitional").to_string())
                .into_any_element(),
            AuthStep::Unknown => v_flex()
                .gap(design::ui_px(cx, 6.0))
                .child(
                    div()
                        .font_family(design::ui_font())
                        .text_color(muted)
                        .child(t!("telegram_core.step_unknown").to_string()),
                )
                .child(self.refresh_btn("telegram-core-refresh-unknown", core, cx))
                .into_any_element(),
        }
    }

    fn wait_phone(
        &self,
        core: u64,
        muted: Hsla,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .gap(design::ui_px(cx, 8.0))
            .child(
                div()
                    .font_family(design::ui_font())
                    .text_color(muted)
                    .child(t!("telegram_core.phone_hint").to_string()),
            )
            .child(self.auth_field(
                cx,
                t!("telegram_core.phone").to_string(),
                "telegram-core-phone",
                &self.telegram.core.phone,
                false,
                p,
            ))
            .child(self.auth_send_btn(
                cx,
                "telegram-core-phone-send",
                t!("telegram_core.phone_send").to_string(),
                true,
                move |this, window, cx| {
                    this.send_auth_text(
                        core,
                        this.telegram.core.phone.clone(),
                        TelegramCmd::SetPhone,
                        window,
                        cx,
                    );
                },
            ))
            .child(
                MoonButton::new("telegram-core-login-qr")
                    .ghost()
                    .padding_x(12.0)
                    .mono(false)
                    .label(t!("telegram_core.login_qr").to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send_telegram(
                            core,
                            TelegramCmd::SetLoginMode(CoreTelegramLoginMode::Qr),
                            cx,
                        );
                    }))
                    .render(),
            )
    }

    fn wait_qr(
        &self,
        core: u64,
        details: &CoreTelegramAuthDetails,
        muted: Hsla,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let mut col = v_flex().gap(design::ui_px(cx, 8.0));
        col = match details.qr_link.as_deref().filter(|s| !s.is_empty()) {
            Some(link) => match qr::encode(link) {
                Some(matrix) => col.child(qr::qr_element(&matrix, cx)),
                None => col
                    .child(
                        div()
                            .text_color(muted)
                            .font_family(design::ui_font())
                            .child(t!("telegram_core.qr_failed").to_string()),
                    )
                    .child(self.refresh_btn("telegram-core-refresh-qr", core, cx)),
            },
            None => col.child(
                div()
                    .text_color(muted)
                    .font_family(design::ui_font())
                    .child(t!("telegram_core.qr_wait").to_string()),
            ),
        };
        col.child(
            MoonButton::new("telegram-core-login-phone")
                .ghost()
                .padding_x(12.0)
                .mono(false)
                .label(t!("telegram_core.login_phone").to_string())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send_telegram(
                        core,
                        TelegramCmd::SetLoginMode(CoreTelegramLoginMode::Phone),
                        cx,
                    );
                }))
                .render(),
        )
    }

    fn wait_code(
        &self,
        core: u64,
        details: &CoreTelegramAuthDetails,
        muted: Hsla,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let mut col = v_flex().gap(design::ui_px(cx, 8.0));
        if let Some(phone) = details.phone.as_deref().filter(|s| !s.is_empty()) {
            col = col.child(
                div()
                    .font_family(design::mono())
                    .text_color(muted)
                    .child(phone.to_string()),
            );
        }
        if let Some(ct) = details.code_type.as_ref() {
            col = col.child(
                div()
                    .font_family(design::ui_font())
                    .text_color(muted)
                    .child(code_kind_line(ct)),
            );
        }
        if let Some(ct) = details.next_code_type.as_ref() {
            col = col.child(
                div()
                    .font_family(design::ui_font())
                    .text_color(muted)
                    .child(format!(
                        "{}: {}",
                        t!("telegram_core.next_code"),
                        code_kind_line(ct)
                    )),
            );
        }
        col.child(self.auth_field(
            cx,
            t!("telegram_core.code").to_string(),
            "telegram-core-code",
            &self.telegram.core.code,
            false,
            p,
        ))
        .child(self.auth_send_btn(
            cx,
            "telegram-core-code-send",
            t!("telegram_core.code_send").to_string(),
            true,
            move |this, window, cx| {
                this.send_auth_text(
                    core,
                    this.telegram.core.code.clone(),
                    TelegramCmd::SetCode,
                    window,
                    cx,
                );
            },
        ))
        .child(self.resend_row(core, details, true, cx))
        .child(self.cancel_to_phone_btn(core, cx))
    }

    fn wait_password(
        &self,
        core: u64,
        details: &CoreTelegramAuthDetails,
        muted: Hsla,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let mut col = v_flex().gap(design::ui_px(cx, 8.0));
        if let Some(hint) = details.password_hint.as_deref().filter(|s| !s.is_empty()) {
            col = col.child(
                div()
                    .font_family(design::ui_font())
                    .text_color(muted)
                    .child(format!(
                        "{}: {hint}",
                        t!("telegram_core.password_hint_label")
                    )),
            );
        }
        if let Some(pat) = details
            .recovery_email_pattern
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            col = col.child(
                div()
                    .font_family(design::mono())
                    .text_color(muted)
                    .child(format!("{}: {pat}", t!("telegram_core.recovery_email"))),
            );
        }
        col.child(self.auth_field(
            cx,
            t!("telegram_core.password").to_string(),
            "telegram-core-password",
            &self.telegram.core.password,
            true,
            p,
        ))
        .child(self.auth_send_btn(
            cx,
            "telegram-core-password-send",
            t!("telegram_core.password_send").to_string(),
            true,
            move |this, window, cx| {
                this.send_auth_text(
                    core,
                    this.telegram.core.password.clone(),
                    TelegramCmd::SetPassword,
                    window,
                    cx,
                );
            },
        ))
        .child(self.cancel_to_phone_btn(core, cx))
    }

    fn wait_email(
        &self,
        core: u64,
        _muted: Hsla,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .gap(design::ui_px(cx, 8.0))
            .child(self.auth_field(
                cx,
                t!("telegram_core.email").to_string(),
                "telegram-core-email",
                &self.telegram.core.email,
                false,
                p,
            ))
            .child(self.auth_send_btn(
                cx,
                "telegram-core-email-send",
                t!("telegram_core.email_send").to_string(),
                true,
                move |this, window, cx| {
                    this.send_auth_text(
                        core,
                        this.telegram.core.email.clone(),
                        TelegramCmd::SetEmail,
                        window,
                        cx,
                    );
                },
            ))
            .child(self.cancel_to_phone_btn(core, cx))
    }

    fn wait_email_code(
        &self,
        core: u64,
        details: &CoreTelegramAuthDetails,
        muted: Hsla,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let mut col = v_flex().gap(design::ui_px(cx, 8.0));
        if let Some(pat) = details.email_pattern.as_deref().filter(|s| !s.is_empty()) {
            col = col.child(
                div()
                    .font_family(design::mono())
                    .text_color(muted)
                    .child(t!("telegram_core.email_pattern", pattern = pat).to_string()),
            );
        }
        if let Some(n) = details.code_length {
            col = col.child(
                div()
                    .font_family(design::mono())
                    .text_color(muted)
                    .child(t!("telegram_core.code_length", n = n).to_string()),
            );
        }
        col.child(self.auth_field(
            cx,
            t!("telegram_core.email_code").to_string(),
            "telegram-core-email-code",
            &self.telegram.core.email_code,
            false,
            p,
        ))
        .child(self.auth_send_btn(
            cx,
            "telegram-core-email-code-send",
            t!("telegram_core.email_code_send").to_string(),
            true,
            move |this, window, cx| {
                this.send_auth_text(
                    core,
                    this.telegram.core.email_code.clone(),
                    TelegramCmd::SetEmailCode,
                    window,
                    cx,
                );
            },
        ))
        .child(self.resend_row(core, details, false, cx))
        .child(self.cancel_to_phone_btn(core, cx))
    }

    fn wait_registration(
        &self,
        core: u64,
        details: &CoreTelegramAuthDetails,
        muted: Hsla,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let terms = details.terms.clone().unwrap_or_default();
        let accepted = self
            .telegram
            .core
            .terms_accepted
            .as_ref()
            .is_some_and(|(c, text)| *c == core && *text == terms);
        let mut col = v_flex().gap(design::ui_px(cx, 8.0));
        if !terms.is_empty() {
            col = col.child(
                div()
                    .font_family(design::ui_font())
                    .text_color(muted)
                    .child(terms.clone()),
            );
        }
        if let Some(age) = details.min_user_age {
            col = col.child(
                div()
                    .font_family(design::ui_font())
                    .text_color(muted)
                    .child(t!("telegram_core.min_age", n = age).to_string()),
            );
        }
        let terms_for_click = terms.clone();
        col.child(self.auth_field(
            cx,
            t!("telegram_core.first_name").to_string(),
            "telegram-core-first-name",
            &self.telegram.core.first_name,
            false,
            p,
        ))
        .child(self.auth_field(
            cx,
            t!("telegram_core.last_name").to_string(),
            "telegram-core-last-name",
            &self.telegram.core.last_name,
            false,
            p,
        ))
        .child(
            MoonCheckbox::new("telegram-core-terms")
                .label(t!("telegram_core.terms_accept").to_string())
                .checked(accepted)
                .mono(false)
                .on_change(cx.listener(move |this, ch: &bool, _, cx| {
                    this.telegram.core.terms_accepted = if *ch {
                        Some((core, terms_for_click.clone()))
                    } else {
                        None
                    };
                    cx.notify();
                })),
        )
        .child(
            MoonButton::new("telegram-core-register")
                .primary()
                .padding_x(12.0)
                .disabled(!accepted)
                .mono(false)
                .label(t!("telegram_core.register").to_string())
                .on_click(cx.listener(move |this, _, window, cx| {
                    let first_name = this.telegram.core.first_name.read(cx).value().to_string();
                    let last_name = this.telegram.core.last_name.read(cx).value().to_string();
                    this.telegram.core.first_name.update(cx, |st, c| {
                        st.set_value(String::new(), window, c);
                    });
                    this.telegram.core.last_name.update(cx, |st, c| {
                        st.set_value(String::new(), window, c);
                    });
                    this.send_telegram(
                        core,
                        TelegramCmd::Register {
                            first_name,
                            last_name,
                        },
                        cx,
                    );
                }))
                .render(),
        )
    }

    fn ready_row(
        &self,
        core: u64,
        phone: Option<&str>,
        muted: Hsla,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let account = match phone.filter(|s| !s.is_empty()) {
            Some(phone) => t!("telegram_core.logged_in", phone = phone).to_string(),
            None => t!("telegram_core.logged_in_unknown").to_string(),
        };
        v_flex()
            .gap(design::ui_px(cx, 8.0))
            .child(
                div()
                    .font_family(design::mono())
                    .text_color(muted)
                    .child(account),
            )
            .child(
                MoonButton::new("telegram-core-logout")
                    .danger()
                    .padding_x(12.0)
                    .mono(false)
                    .label(t!("telegram_core.logout").to_string())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_logout_dialog(core, window, cx);
                    }))
                    .render(),
            )
    }

    fn wait_premium(
        &self,
        core: u64,
        details: &CoreTelegramAuthDetails,
        muted: Hsla,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let mut col = v_flex().gap(design::ui_px(cx, 8.0)).child(
            div()
                .font_family(design::ui_font())
                .text_color(muted)
                .child(t!("telegram_core.premium").to_string()),
        );
        if let Some(email) = details.support_email.as_deref().filter(|s| !s.is_empty()) {
            col = col.child(
                div()
                    .font_family(design::mono())
                    .text_color(muted)
                    .child(format!("{}: {email}", t!("telegram_core.support_email"))),
            );
        }
        if let Some(subj) = details.support_subject.as_deref().filter(|s| !s.is_empty()) {
            col = col.child(
                div()
                    .font_family(design::mono())
                    .text_color(muted)
                    .child(format!("{}: {subj}", t!("telegram_core.support_subject"))),
            );
        }
        col.child(
            MoonButton::new("telegram-core-cancel-login")
                .ghost()
                .padding_x(12.0)
                .mono(false)
                .label(t!("telegram_core.cancel_login").to_string())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send_telegram(
                        core,
                        TelegramCmd::SetLoginMode(CoreTelegramLoginMode::Phone),
                        cx,
                    );
                }))
                .render(),
        )
    }

    fn resend_row(
        &self,
        core: u64,
        details: &CoreTelegramAuthDetails,
        phone_gate: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let in_flight = self.send_in_flight(cx);
        if phone_gate {
            let now = moon_core::util::time::now_unix_secs() as i64;
            match resend_state(details, now) {
                ResendState::Unavailable => return div().into_any_element(),
                ResendState::Wait { secs_left } => {
                    return MoonButton::new("telegram-core-resend")
                        .ghost()
                        .padding_x(12.0)
                        .disabled(true)
                        .mono(true)
                        .label(
                            t!(
                                "telegram_core.resend_in",
                                wait = fmt_duration_short(secs_left as f64)
                            )
                            .to_string(),
                        )
                        .render()
                        .into_any_element();
                }
                ResendState::Ready => {}
            }
        }
        MoonButton::new("telegram-core-resend")
            .ghost()
            .padding_x(12.0)
            .disabled(in_flight)
            .mono(false)
            .label(t!("telegram_core.resend").to_string())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.send_telegram(core, TelegramCmd::ResendCode, cx);
            }))
            .render()
            .into_any_element()
    }

    fn cancel_to_phone_btn(&self, core: u64, cx: &Context<Self>) -> impl IntoElement {
        MoonButton::new("telegram-core-cancel-to-phone")
            .ghost()
            .padding_x(12.0)
            .mono(false)
            .label(t!("telegram_core.cancel_to_phone").to_string())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.send_telegram(
                    core,
                    TelegramCmd::SetLoginMode(CoreTelegramLoginMode::Phone),
                    cx,
                );
            }))
            .render()
    }

    fn auth_field(
        &self,
        cx: &Context<Self>,
        label: String,
        id: &'static str,
        state: &Entity<MoonInputState>,
        masked: bool,
        p: MoonPalette,
    ) -> impl IntoElement {
        let mut input = MoonInput::new(id)
            .state(state)
            .size(design::input_tier(cx))
            .mono(true);
        if masked {
            input = input.mask_toggle();
        }
        h_flex()
            .flex_wrap()
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(
                div()
                    .font_family(design::ui_font())
                    .text_color(rgba_from(p.text_soft, 1.0))
                    .child(label),
            )
            .child(div().w(design::font_w_px(cx, 220.0)).child(input))
    }

    fn auth_send_btn(
        &self,
        cx: &Context<Self>,
        id: &'static str,
        label: String,
        primary: bool,
        on_send: impl Fn(&mut SettingsView, &mut Window, &mut Context<SettingsView>) + 'static,
    ) -> impl IntoElement {
        let mut btn = MoonButton::new(id)
            .padding_x(12.0)
            .mono(false)
            .disabled(self.send_in_flight(cx))
            .label(label);
        if primary {
            btn = btn.primary();
        } else {
            btn = btn.ghost();
        }
        btn.on_click(cx.listener(move |this, _, window, cx| on_send(this, window, cx)))
            .render()
    }

    fn send_auth_text(
        &mut self,
        core: u64,
        field: Entity<MoonInputState>,
        make: impl FnOnce(String) -> TelegramCmd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let val = field.read(cx).value().to_string();
        field.update(cx, |st, c| st.set_value(String::new(), window, c));
        self.send_telegram(core, make(val), cx);
    }

    fn open_logout_dialog(&mut self, core: u64, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        window.open_unique_moon_dialog(LOGOUT_DIALOG_ID, cx, move |dialog, _window, cx| {
            let p = MoonPalette::active(cx);
            let view = view.clone();
            dialog
                .w(px(380.0))
                .close_button(true)
                .overlay(true)
                .overlay_closable(true)
                .bg(rgb(p.shell_high))
                .border_color(rgb(p.border))
                .rounded(design::r_container(cx))
                .text_color(rgb(p.text))
                .header(
                    div()
                        .w_full()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .font_family(design::ui_font())
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t!("telegram_core.logout_title").to_string()),
                )
                .content(move |content, _window, cx| {
                    let p = MoonPalette::active(cx);
                    content.child(
                        v_flex().w_full().gap_2().child(
                            div()
                                .font_family(design::ui_font())
                                .text_size(design::t_body(cx))
                                .text_color(rgb(p.text))
                                .child(t!("telegram_core.logout_q").to_string()),
                        ),
                    )
                })
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(
                            MoonButton::new("telegram-core-logout-no")
                                .outline()
                                .mono(false)
                                .label(format!("  {}  ", t!("dialogs.no")))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                })
                                .render(),
                        )
                        .child(
                            MoonButton::new("telegram-core-logout-yes")
                                .variant(MoonButtonVariant::Danger)
                                .mono(false)
                                .label(format!("  {}  ", t!("dialogs.yes")))
                                .on_click(move |_, window, cx| {
                                    if let Some(view) = view.upgrade() {
                                        view.update(cx, |this, cx| {
                                            this.send_telegram(core, TelegramCmd::Logout, cx);
                                        });
                                    }
                                    window.close_dialog(cx);
                                })
                                .render(),
                        ),
                )
        });
    }
}

fn connection_label(conn: &str) -> String {
    match conn {
        "unknown" => t!("telegram_core.conn.unknown").to_string(),
        "waiting_for_network" => t!("telegram_core.conn.waiting_for_network").to_string(),
        "connecting_to_proxy" => t!("telegram_core.conn.connecting_to_proxy").to_string(),
        "connecting" => t!("telegram_core.conn.connecting").to_string(),
        "updating" => t!("telegram_core.conn.updating").to_string(),
        "ready" => t!("telegram_core.conn.ready").to_string(),
        other => other.to_string(),
    }
}

fn code_kind_line(ct: &CoreTelegramCodeType) -> String {
    let first_letter = ct.first_letter.clone().unwrap_or_default();
    let first_word = ct.first_word.clone().unwrap_or_default();
    let pattern = ct.pattern.clone().unwrap_or_default();
    let prefix = ct.prefix.clone().unwrap_or_default();
    let url = ct.url.clone().unwrap_or_default();
    let kind = match ct.kind.as_str() {
        "telegram" => t!("telegram_core.code_kind.telegram").to_string(),
        "sms" => t!("telegram_core.code_kind.sms").to_string(),
        "sms_word" => t!(
            "telegram_core.code_kind.sms_word",
            first_letter = first_letter,
            first_word = first_word
        )
        .to_string(),
        "sms_phrase" => t!(
            "telegram_core.code_kind.sms_phrase",
            first_word = first_word,
            pattern = pattern
        )
        .to_string(),
        "call" => t!("telegram_core.code_kind.call").to_string(),
        "flash_call" => t!("telegram_core.code_kind.flash_call", prefix = prefix).to_string(),
        "missed_call" => t!("telegram_core.code_kind.missed_call", prefix = prefix).to_string(),
        "fragment" => t!("telegram_core.code_kind.fragment", url = url).to_string(),
        "unsupported" => t!("telegram_core.code_kind.unsupported").to_string(),
        other => t!("telegram_core.code_kind.other", kind = other).to_string(),
    };
    match (ct.kind.as_str(), ct.length) {
        ("telegram" | "sms" | "call", Some(n)) => {
            format!("{} {}", kind, t!("telegram_core.code_length", n = n))
        }
        _ => kind,
    }
}
