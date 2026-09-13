//! Settings -> Telegram section that controls one core's built-in Telegram reader.
//!
//! Intents travel as typed [`TelegramCmd`] values. Inputs are typed, sent and dropped: nothing
//! here is persisted, drafted, or logged.

use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonDisclosureDirection, MoonDropdown, MoonGroupBox, MoonInput, MoonInputState, MoonMenuItem,
    MoonMenuSize, MoonPalette, MoonSize, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::super::SettingsView;
use crate::design;
use crate::panels::common::text_tooltip;
use moon_core::feed::{
    AuthStep, ConnStatus, CoreTelegramProxy, ResendState, TelegramCmd, auth_controls_visible,
    auth_step, resend_state,
};

const PENDING_DEADLINE: Duration = Duration::from_secs(15);
const LOGOUT_DIALOG_ID: &str = "telegram-core-logout";

mod login_steps;

/// Editor state for the core Telegram section.
pub(super) struct CoreTelegramEd {
    pub picked: Option<u64>,
    pub expanded: bool,
    pub proxy_kind: u8,
    pub proxy_host: Entity<MoonInputState>,
    pub proxy_port: Entity<MoonInputState>,
    pub proxy_user: Entity<MoonInputState>,
    pub proxy_password: Entity<MoonInputState>,
    pub mtproto_secret: Entity<MoonInputState>,
    pub pending: Option<PendingSend>,
    pub phone: Entity<MoonInputState>,
    pub code: Entity<MoonInputState>,
    pub password: Entity<MoonInputState>,
    pub email: Entity<MoonInputState>,
    pub email_code: Entity<MoonInputState>,
    pub first_name: Entity<MoonInputState>,
    pub last_name: Entity<MoonInputState>,
    /// Accepted terms: this core and the exact shown text. A different core or text simply
    /// stops matching; nothing clears the field except the checkbox itself.
    pub terms_accepted: Option<(u64, String)>,
    pub resend_pulse_armed: bool,
}

/// In-flight intent waiting for a `telegram_rev` bump.
pub(super) struct PendingSend {
    pub core: u64,
    pub rev_at_send: u64,
    pub at: Instant,
}

/// Build empty proxy inputs and no picked core.
pub(super) fn build(window: &mut Window, cx: &mut Context<SettingsView>) -> CoreTelegramEd {
    CoreTelegramEd {
        picked: None,
        expanded: false,
        proxy_kind: 0,
        proxy_host: cx.new(|cx| MoonInputState::new(window, cx)),
        proxy_port: cx.new(|cx| MoonInputState::new(window, cx)),
        proxy_user: cx.new(|cx| MoonInputState::new(window, cx)),
        proxy_password: masked_input(window, cx),
        mtproto_secret: masked_input(window, cx),
        pending: None,
        phone: cx.new(|cx| MoonInputState::new(window, cx)),
        code: cx.new(|cx| MoonInputState::new(window, cx)),
        password: masked_input(window, cx),
        email: cx.new(|cx| MoonInputState::new(window, cx)),
        email_code: cx.new(|cx| MoonInputState::new(window, cx)),
        first_name: cx.new(|cx| MoonInputState::new(window, cx)),
        last_name: cx.new(|cx| MoonInputState::new(window, cx)),
        terms_accepted: None,
        resend_pulse_armed: false,
    }
}

fn masked_input(window: &mut Window, cx: &mut Context<SettingsView>) -> Entity<MoonInputState> {
    let input = cx.new(|cx| MoonInputState::new(window, cx).masked(true));
    input.update(cx, |st, c| st.set_masked(true, window, c));
    input
}

impl SettingsView {
    /// Render the core-reader group box beneath its segment's introduction.
    pub(super) fn core_telegram_section(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        MoonGroupBox::new("telegram-core-section")
            .title(t!("telegram_core.section").to_string())
            .padding(14.0)
            .gap(10.0)
            .child(self.core_picker_row(cx, p))
            .when(self.telegram.core.expanded, |box_| {
                box_.child(self.core_telegram_body(cx, p, muted))
            })
    }

    /// Refresh the picked core when the Telegram tab is opened. Not called from `render`.
    pub(crate) fn telegram_tab_activated(&mut self, cx: &mut Context<Self>) {
        if let Some(core) = self.telegram.core.picked {
            self.send_telegram(core, TelegramCmd::Refresh, cx);
        }
    }

    /// Render the fitted core selector and a full-button disclosure for its parameters.
    fn core_picker_row(&self, cx: &Context<Self>, p: MoonPalette) -> impl IntoElement {
        let b = self.backend.read(cx);
        let cores: Vec<(u64, String)> = b
            .config
            .servers
            .iter()
            .map(|s| {
                let name = if s.name.trim().is_empty() {
                    format!("#{}", s.id)
                } else {
                    s.name.clone()
                };
                (s.id, name)
            })
            .collect();
        let venues = b.session.core_venues();
        let sections = crate::controls::core_menu_sections(&cores, venues);
        let picked = self.telegram.core.picked;
        let caption = cores
            .iter()
            .find(|(id, _)| Some(*id) == picked)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| t!("telegram_core.core_pick_none").to_string());
        let view = cx.entity();
        let mut items = Vec::new();
        for (venue, members) in sections {
            items.push(MoonMenuItem::label(crate::controls::venue_section_label(
                venue,
            )));
            for (id, name) in members {
                let item_view = view.clone();
                items.push(
                    MoonMenuItem::with_key(format!("tg-core-{id}"), name)
                        .selected(picked == Some(id))
                        .on_click(move |_, window, app| {
                            item_view.update(app, |this, cx| this.pick_core(id, window, cx));
                        }),
                );
            }
        }
        let expanded = self.telegram.core.expanded;
        h_flex()
            .flex_wrap()
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(
                div()
                    .font_family(design::ui_font())
                    .text_color(rgba_from(p.text_soft, 1.0))
                    .child(t!("telegram_core.core_pick").to_string()),
            )
            .child(
                MoonDropdown::new("telegram-core-pick")
                    .label(caption)
                    .trigger_caret(true)
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::Action)
                    .fit_trigger_width(crate::controls::CORE_COMBO_TRIGGER_W, 260.0)
                    // Font-scaled bounds (tokens.font()); MoonUI measures the widest menu row.
                    .fit_menu_width(crate::controls::CORE_COMBO_TRIGGER_W, 560.0)
                    .menu_size(MoonMenuSize::Compact)
                    .items(items)
                    .into_any_element(),
            )
            .child(
                MoonButton::new("telegram-core-disc")
                    .outline()
                    .padding_x(12.0)
                    .mono(false)
                    .label(t!("telegram_core.details").to_string())
                    .leading_icon(MoonButtonIconSlot::caret(
                        MoonDisclosureDirection::DownUp,
                        expanded,
                    ))
                    .tooltip(t!("telegram_core.details").to_string())
                    .on_click(cx.listener(|this, _, _, cx| {
                        let next = !this.telegram.core.expanded;
                        this.telegram.core.expanded = next;
                        if next && let Some(core) = this.telegram.core.picked {
                            this.send_telegram(core, TelegramCmd::Refresh, cx);
                        }
                        cx.notify();
                    }))
                    .render(),
            )
    }

    fn core_telegram_body(
        &self,
        cx: &Context<Self>,
        p: MoonPalette,
        muted: Hsla,
    ) -> impl IntoElement {
        let Some(core) = self.telegram.core.picked else {
            return div().into_any_element();
        };
        let b = self.backend.read(cx);
        let Some(data) = b.session.store().core(core) else {
            return div()
                .font_family(design::ui_font())
                .text_color(muted)
                .child(t!("telegram_core.disconnected").to_string())
                .into_any_element();
        };
        // Ready alone is not current: an in-loop reconnect retains the pre-outage snapshot.
        let live = data.status == ConnStatus::Ready && data.telegram_fresh;
        let state = data.telegram.as_deref();
        let show_pending = self
            .telegram
            .core
            .pending
            .as_ref()
            .is_some_and(|p| p.core == core && data.telegram_rev == p.rev_at_send);

        let mut col = v_flex()
            .gap(design::ui_px(cx, 10.0))
            .opacity(if live { 1.0 } else { 0.55 })
            .child(self.telegram_pulse_hook(cx));
        if show_pending {
            if self.pending_inside_deadline(cx) {
                col = col.child(
                    div()
                        .font_family(design::ui_font())
                        .text_color(muted)
                        .child(t!("telegram_core.sent_waiting").to_string()),
                );
            } else {
                col = col.child(self.no_answer_row(core, muted, cx));
            }
        }
        if !live {
            col = col.child(
                div()
                    .font_family(design::ui_font())
                    .text_color(muted)
                    .child(t!("telegram_core.stale").to_string()),
            );
        }
        if let Some(state) = state {
            col = col.child(status_row(state, muted, cx));
        } else {
            col = col.child(
                div()
                    .font_family(design::ui_font())
                    .text_color(muted)
                    .child(t!("telegram_core.no_snapshot").to_string()),
            );
        }
        let enabled = state.map(|s| s.enabled).unwrap_or(false);
        col = col.child(self.enable_row(enabled, live, cx));
        if let Some(state) = state.filter(|s| !s.state_supported) {
            let line = match state.service_version.as_deref() {
                Some(v) if !v.is_empty() => t!("telegram_core.unsupported_ver", v = v).to_string(),
                _ => t!("telegram_core.unsupported").to_string(),
            };
            col = col.child(
                div()
                    .font_family(design::ui_font())
                    .text_color(muted)
                    .child(line),
            );
        }
        if let Some(state) = state
            && auth_controls_visible(live, state)
            && let Some(service) = state.service.as_ref()
        {
            col = col.child(self.auth_block(core, service, muted, p, cx));
        }
        col = col.child(self.proxy_form(core, state, live, muted, p, cx));
        col.into_any_element()
    }

    fn enable_row(&self, enabled: bool, live: bool, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .id("telegram-core-enable-tip")
            .tooltip(text_tooltip(t!("telegram_core.enable_tip").to_string()))
            .child(
                MoonCheckbox::new("telegram-core-enable")
                    .label(t!("telegram_core.enable").to_string())
                    .checked(enabled)
                    .disabled(!live)
                    .mono(false)
                    .size(MoonSize::Sm)
                    .on_change(cx.listener(move |this, ch: &bool, _, cx| {
                        let v = *ch;
                        if let Some(core) = this.telegram.core.picked {
                            this.send_telegram(core, TelegramCmd::SetEnabled(v), cx);
                        }
                    })),
            )
    }

    fn proxy_form(
        &self,
        core: u64,
        state: Option<&moon_core::feed::CoreTelegramState>,
        can_edit: bool,
        muted: Hsla,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let kind = self.telegram.core.proxy_kind;
        let kind_row = h_flex()
            .flex_wrap()
            .gap(design::ui_px(cx, 8.0))
            .child(kind_btn(
                cx,
                "tg-proxy-none",
                t!("telegram_core.proxy_kind_none").to_string(),
                kind == 0,
                can_edit,
                0,
            ))
            .child(kind_btn(
                cx,
                "tg-proxy-socks",
                t!("telegram_core.proxy_kind_socks5").to_string(),
                kind == 1,
                can_edit,
                1,
            ))
            .child(kind_btn(
                cx,
                "tg-proxy-mtp",
                t!("telegram_core.proxy_kind_mtproto").to_string(),
                kind == 2,
                can_edit,
                2,
            ));
        let mut fields = v_flex().gap(design::ui_px(cx, 8.0));
        if kind == 1 {
            fields = fields
                .child(self.proxy_field(
                    cx,
                    t!("telegram_core.proxy_host").to_string(),
                    "telegram-core-proxy-host",
                    &self.telegram.core.proxy_host,
                    false,
                    can_edit,
                ))
                .child(self.proxy_field(
                    cx,
                    t!("telegram_core.proxy_port").to_string(),
                    "telegram-core-proxy-port",
                    &self.telegram.core.proxy_port,
                    false,
                    can_edit,
                ))
                .child(self.proxy_field(
                    cx,
                    t!("telegram_core.proxy_user").to_string(),
                    "telegram-core-proxy-user",
                    &self.telegram.core.proxy_user,
                    false,
                    can_edit,
                ))
                .child(self.proxy_field(
                    cx,
                    t!("telegram_core.proxy_password").to_string(),
                    "telegram-core-proxy-password",
                    &self.telegram.core.proxy_password,
                    true,
                    can_edit,
                ));
        } else if kind == 2 {
            fields = fields
                .child(self.proxy_field(
                    cx,
                    t!("telegram_core.proxy_host").to_string(),
                    "telegram-core-proxy-host",
                    &self.telegram.core.proxy_host,
                    false,
                    can_edit,
                ))
                .child(self.proxy_field(
                    cx,
                    t!("telegram_core.proxy_port").to_string(),
                    "telegram-core-proxy-port",
                    &self.telegram.core.proxy_port,
                    false,
                    can_edit,
                ))
                .child(self.proxy_field(
                    cx,
                    t!("telegram_core.proxy_secret").to_string(),
                    "telegram-core-proxy-secret",
                    &self.telegram.core.mtproto_secret,
                    true,
                    can_edit,
                ));
        }
        v_flex()
            .gap(design::ui_px(cx, 8.0))
            .child(
                div()
                    .font_family(design::ui_font())
                    .text_color(rgba_from(p.text, 1.0))
                    .child(t!("telegram_core.proxy_section").to_string()),
            )
            .child(kind_row)
            .child(fields)
            .child(
                div()
                    .font_family(design::ui_font())
                    .text_color(muted)
                    .child(t!("telegram_core.proxy_secret_hint").to_string()),
            )
            .child(actual_proxy_line(state, muted))
            .child(
                MoonButton::new("telegram-core-proxy-save")
                    .primary()
                    .padding_x(12.0)
                    .disabled(!can_edit)
                    .mono(false)
                    .label(t!("telegram_core.proxy_save").to_string())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.save_proxy(core, window, cx);
                    }))
                    .render(),
            )
    }

    fn proxy_field(
        &self,
        cx: &Context<Self>,
        label: String,
        id: &'static str,
        state: &Entity<MoonInputState>,
        masked: bool,
        can_edit: bool,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let mut input = MoonInput::new(id)
            .state(state)
            .small()
            .disabled(!can_edit)
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

    fn pick_core(&mut self, core: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.telegram.core.picked = Some(core);
        self.telegram.core.pending = None;
        let kind = self
            .backend
            .read(cx)
            .session
            .store()
            .core(core)
            .and_then(|d| d.telegram.as_ref())
            .and_then(|s| s.proxy_type)
            .unwrap_or(0);
        self.telegram.core.proxy_kind = kind;
        self.clear_secret_inputs(window, cx);
        self.send_telegram(core, TelegramCmd::Refresh, cx);
    }

    fn clear_secret_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for field in [
            self.telegram.core.proxy_host.clone(),
            self.telegram.core.proxy_port.clone(),
            self.telegram.core.proxy_user.clone(),
            self.telegram.core.proxy_password.clone(),
            self.telegram.core.mtproto_secret.clone(),
            self.telegram.core.phone.clone(),
            self.telegram.core.code.clone(),
            self.telegram.core.password.clone(),
            self.telegram.core.email.clone(),
            self.telegram.core.email_code.clone(),
            self.telegram.core.first_name.clone(),
            self.telegram.core.last_name.clone(),
        ] {
            field.update(cx, |st, c| st.set_value(String::new(), window, c));
        }
    }

    fn save_proxy(&mut self, core: u64, window: &mut Window, cx: &mut Context<Self>) {
        let kind = self.telegram.core.proxy_kind;
        let host = self.telegram.core.proxy_host.read(cx).value().to_string();
        let port = self.telegram.core.proxy_port.read(cx).value().to_string();
        let user = self.telegram.core.proxy_user.read(cx).value().to_string();
        let password = self
            .telegram
            .core
            .proxy_password
            .read(cx)
            .value()
            .to_string();
        let secret = self
            .telegram
            .core
            .mtproto_secret
            .read(cx)
            .value()
            .to_string();
        let port_n = port.parse::<u16>().unwrap_or(0);
        let proxy = match kind {
            1 => CoreTelegramProxy::Socks5 {
                host,
                port: port_n,
                user,
                password,
            },
            2 => CoreTelegramProxy::MtProto {
                host,
                port: port_n,
                secret,
            },
            _ => CoreTelegramProxy::None,
        };
        self.send_telegram(core, TelegramCmd::SetProxy(proxy), cx);
        self.clear_proxy_secrets(window, cx);
    }

    fn clear_proxy_secrets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for field in [
            self.telegram.core.proxy_password.clone(),
            self.telegram.core.mtproto_secret.clone(),
        ] {
            field.update(cx, |st, c| st.set_value(String::new(), window, c));
        }
    }

    fn send_telegram(&mut self, core: u64, cmd: TelegramCmd, cx: &mut Context<Self>) {
        let rev = self
            .backend
            .read(cx)
            .session
            .store()
            .core(core)
            .map(|d| d.telegram_rev)
            .unwrap_or(0);
        if self
            .backend
            .read(cx)
            .session
            .telegram_cmd(core, cmd)
            .is_ok()
        {
            self.telegram.core.pending = Some(PendingSend {
                core,
                rev_at_send: rev,
                at: Instant::now(),
            });
            self.arm_telegram_pulse(cx);
        }
        cx.notify();
    }

    fn arm_telegram_pulse(&mut self, cx: &mut Context<Self>) {
        if self.telegram.core.resend_pulse_armed {
            return;
        }
        self.telegram.core.resend_pulse_armed = true;
        crate::pulse::arm_every(Duration::from_secs(1), cx, |this, cx| {
            let keep = this.telegram_pulse_needed(cx);
            cx.notify();
            if !keep {
                this.telegram.core.resend_pulse_armed = false;
            }
            keep
        });
    }

    /// True while a live pending send is inside the deadline, or a phone-code resend Wait is shown.
    fn telegram_pulse_should_run(&self, cx: &Context<Self>) -> bool {
        self.pending_inside_deadline(cx) || self.resend_wait_visible(cx)
    }

    fn resend_wait_visible(&self, cx: &Context<Self>) -> bool {
        let Some(core) = self.telegram.core.picked else {
            return false;
        };
        let Some(data) = self.backend.read(cx).session.store().core(core) else {
            return false;
        };
        let live = data.status == ConnStatus::Ready && data.telegram_fresh;
        let Some(state) = data.telegram.as_deref() else {
            return false;
        };
        if !auth_controls_visible(live, state) {
            return false;
        }
        let Some(service) = state.service.as_ref() else {
            return false;
        };
        matches!(auth_step(&service.auth_state), AuthStep::WaitCode)
            && matches!(
                resend_state(
                    &service.details,
                    moon_core::util::time::now_unix_secs() as i64
                ),
                ResendState::Wait { .. }
            )
    }

    /// Keep the 1s chain while a send is awaiting a rev bump or a resend countdown is visible.
    fn telegram_pulse_needed(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some((core, rev)) = self
            .telegram
            .core
            .pending
            .as_ref()
            .map(|p| (p.core, p.rev_at_send))
        {
            let current = self
                .backend
                .read(cx)
                .session
                .store()
                .core(core)
                .map(|d| d.telegram_rev);
            if current != Some(rev) {
                self.telegram.core.pending = None;
            }
        }
        self.telegram_pulse_should_run(cx)
    }

    /// Arm the 1s pulse from paint when a Wait countdown is on screen without a local send.
    fn telegram_pulse_hook(&self, cx: &Context<Self>) -> impl IntoElement {
        if self.telegram.core.resend_pulse_armed || !self.telegram_pulse_should_run(cx) {
            return div().w(px(0.0)).h(px(0.0)).into_any_element();
        }
        let entity = cx.entity().downgrade();
        canvas(
            |_, _, _| (),
            move |_, _, _, cx| {
                cx.defer(move |app| {
                    if let Some(entity) = entity.upgrade() {
                        entity.update(app, |this, cx| this.arm_telegram_pulse(cx));
                    }
                });
            },
        )
        .w(px(1.0))
        .h(px(1.0))
        .into_any_element()
    }

    /// Pending belongs to the selected core, the store rev has not moved, and the 15s window is open.
    fn pending_inside_deadline(&self, cx: &Context<Self>) -> bool {
        let Some(p) = self.telegram.core.pending.as_ref() else {
            return false;
        };
        let Some(picked) = self.telegram.core.picked else {
            return false;
        };
        if p.core != picked || p.at.elapsed() >= PENDING_DEADLINE {
            return false;
        }
        self.backend
            .read(cx)
            .session
            .store()
            .core(picked)
            .is_some_and(|d| d.telegram_rev == p.rev_at_send)
    }

    fn send_in_flight(&self, cx: &Context<Self>) -> bool {
        self.pending_inside_deadline(cx)
    }

    fn no_answer_row(&self, core: u64, muted: Hsla, cx: &Context<Self>) -> impl IntoElement {
        v_flex()
            .gap(design::ui_px(cx, 6.0))
            .child(
                div()
                    .font_family(design::ui_font())
                    .text_color(muted)
                    .child(t!("telegram_core.no_answer").to_string()),
            )
            .child(self.refresh_btn("telegram-core-refresh-timeout", core, cx))
    }

    fn refresh_btn(&self, id: &'static str, core: u64, cx: &Context<Self>) -> impl IntoElement {
        MoonButton::new(id)
            .ghost()
            .padding_x(12.0)
            .mono(false)
            .label(t!("telegram_core.refresh").to_string())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.send_telegram(core, TelegramCmd::Refresh, cx);
            }))
            .render()
    }
}

fn kind_btn(
    cx: &Context<SettingsView>,
    id: &'static str,
    label: String,
    on: bool,
    enabled: bool,
    kind: u8,
) -> impl IntoElement {
    MoonButton::new(id)
        .ghost()
        .selected(on)
        .padding_x(12.0)
        .disabled(!enabled)
        .mono(false)
        .label(label)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.telegram.core.proxy_kind = kind;
            cx.notify();
        }))
        .render()
}

fn status_row(
    state: &moon_core::feed::CoreTelegramState,
    muted: Hsla,
    cx: &Context<SettingsView>,
) -> impl IntoElement {
    let online = if state.service_online {
        t!("telegram_core.service_online").to_string()
    } else {
        t!("telegram_core.service_offline").to_string()
    };
    let mut col = v_flex().gap(design::ui_px(cx, 4.0)).child(
        div()
            .font_family(design::ui_font())
            .text_color(muted)
            .child(online),
    );
    if let Some(cs) = state.client_state.as_deref().filter(|s| !s.is_empty()) {
        col = col.child(
            div()
                .font_family(design::mono())
                .text_color(muted)
                .child(format!("{}: {cs}", t!("telegram_core.client_state"))),
        );
    }
    if let Some(v) = state.service_version.as_deref().filter(|s| !s.is_empty()) {
        col = col.child(
            div()
                .font_family(design::mono())
                .text_color(muted)
                .child(v.to_string()),
        );
    }
    if let Some(e) = state.setup_error.as_deref().filter(|s| !s.is_empty()) {
        col = col.child(
            div()
                .font_family(design::ui_font())
                .text_color(muted)
                .child(format!("{}: {e}", t!("telegram_core.setup_error"))),
        );
    }
    if let Some(e) = state.client_error.as_deref().filter(|s| !s.is_empty()) {
        col = col.child(
            div()
                .font_family(design::ui_font())
                .text_color(muted)
                .child(format!("{}: {e}", t!("telegram_core.client_error"))),
        );
    }
    col
}

fn actual_proxy_line(
    state: Option<&moon_core::feed::CoreTelegramState>,
    muted: Hsla,
) -> impl IntoElement {
    let text = match state
        .and_then(|s| s.service.as_ref())
        .and_then(|svc| svc.proxy.as_ref())
    {
        None => format!(
            "{}: {}",
            t!("telegram_core.proxy_actual"),
            t!("telegram_core.proxy_actual_unknown")
        ),
        Some(proxy) => {
            let host = proxy.host.as_deref().unwrap_or("");
            let port = proxy.port.map(|p| p.to_string()).unwrap_or_default();
            format!(
                "{}: {} {} {}",
                t!("telegram_core.proxy_actual"),
                proxy.mode,
                host,
                port
            )
        }
    };
    let mut col = v_flex().child(
        div()
            .font_family(design::mono())
            .text_color(muted)
            .child(text),
    );
    if let Some(err) = state
        .and_then(|s| s.service.as_ref())
        .and_then(|svc| svc.proxy_error.as_ref())
        .filter(|e| !e.message.is_empty())
    {
        col = col.child(
            div()
                .font_family(design::ui_font())
                .text_color(muted)
                .child(format!(
                    "{}: {}",
                    t!("telegram_core.proxy_error"),
                    err.message
                )),
        );
    }
    col
}
