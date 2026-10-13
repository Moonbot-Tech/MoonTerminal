//! Settings -> Telegram section that controls each core's built-in Telegram reader: the cores on
//! the left with their reader state, the picked core's reader on the right.
//!
//! Intents travel as typed [`TelegramCmd`] values. Inputs are typed, sent and dropped: nothing
//! here is persisted, drafted, or logged.

use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonInput, MoonInputState, MoonPalette, MoonSegmentItem, MoonSegmentedControl,
    MoonToggle, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::super::SettingsView;
use crate::design;
use crate::panels::common::text_tooltip;
use master::{TgTag, tg_tag};
use moon_core::feed::{
    AuthStep, ConnStatus, CoreTelegramProxy, CoreTelegramState, ResendState, TelegramCmd,
    auth_controls_visible, auth_step, resend_state,
};
use moon_core::session::core_order::CoreOrder;

const PENDING_DEADLINE: Duration = Duration::from_secs(15);
const LOGOUT_DIALOG_ID: &str = "telegram-core-logout";

mod login_steps;
pub(super) mod master;

/// Editor state for the core Telegram section.
pub(super) struct CoreTelegramEd {
    pub picked: Option<u64>,
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

/// A core's name as the list shows it: `#id` when it has none.
fn core_name(s: &moon_core::config::ServerConfig) -> String {
    if s.name.trim().is_empty() {
        format!("#{}", s.id)
    } else {
        s.name.clone()
    }
}

impl SettingsView {
    /// Render the cores list beside the picked core's reader card in a page column `column_w`
    /// wide.
    pub(super) fn core_telegram_section(&self, column_w: f32, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let b = self.backend.read(cx);
        let mut cores: Vec<(u64, String)> = b
            .config
            .servers
            .iter()
            .map(|s| (s.id, core_name(s)))
            .collect();
        CoreOrder::new(&b.config).sort_by(&mut cores, |(id, _)| *id);
        let store = b.session.store();
        let picked = self.telegram.core.picked;
        let rows = cores
            .iter()
            .map(|(id, name)| {
                let id = *id;
                let data = store.core(id);
                let status = data.map(|d| &d.status);
                let tag = tg_tag(
                    status,
                    data.is_some_and(|d| d.telegram_fresh),
                    data.and_then(|d| d.telegram.as_deref()),
                );
                master::core_row(
                    id,
                    name.clone(),
                    master::dot_color(status, p),
                    tag,
                    picked == Some(id),
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.telegram.core.picked != Some(id) {
                        this.pick_core(id, window, cx);
                    }
                }))
                .into_any_element()
            })
            .collect();
        let card = match picked.and_then(|id| cores.iter().find(|(c, _)| *c == id)) {
            Some((id, name)) => self.core_card(*id, name.clone(), cx),
            None => master::card(
                vec![master::note_box(
                    "tgcore-pick",
                    t!("telegram_core.core_pick_none").to_string(),
                    cx,
                )],
                cx,
            ),
        };
        master::split(master::core_list(rows, cx), card, column_w, cx)
    }

    /// Refresh the picked core when the Telegram tab is opened, picking the first core in
    /// canonical order when none is picked yet. Not called from `render`.
    pub(crate) fn telegram_tab_activated(&mut self, cx: &mut Context<Self>) {
        if self.telegram.core.picked.is_none() {
            let first = {
                let b = self.backend.read(cx);
                let mut ids: Vec<u64> = b.config.servers.iter().map(|s| s.id).collect();
                CoreOrder::new(&b.config).sort_by(&mut ids, |id| *id);
                ids.first().copied()
            };
            if let Some(core) = first {
                // The inputs are still empty: nothing typed belongs to another core yet.
                self.telegram.core.picked = Some(core);
                self.telegram.core.proxy_kind = self.saved_proxy_kind(core, cx);
            }
        }
        if let Some(core) = self.telegram.core.picked {
            self.send_telegram(core, TelegramCmd::Refresh, cx);
        }
        // The station's bot, when one runs on a server: its status and pairing code.
        self.backend.update(cx, |b, bcx| b.station_refresh_bot(bcx));
        // A catch-up already running starts its minute re-read now, not at the next answer.
        self.server_tape_sync(cx);
    }

    /// The picked core's reader card: header, error banners, then the reader's rows and actions.
    fn core_card(&self, core: u64, name: String, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        let b = self.backend.read(cx);
        let Some(data) = b
            .session
            .store()
            .core(core)
            .filter(|d| d.status == ConnStatus::Ready)
        else {
            return master::card(
                vec![
                    master::card_header(name, TgTag::NoCore, None, cx),
                    master::disconnected(self.reconnect_btn(core, cx), cx),
                ],
                cx,
            );
        };
        // Ready alone is not current: an in-loop reconnect retains the pre-outage snapshot.
        let live = data.telegram_fresh;
        let state = data.telegram.as_deref();
        let tag = tg_tag(Some(&data.status), live, state);
        let toggle = state
            .filter(|s| s.state_supported)
            .map(|s| self.enable_toggle(s.enabled, live, cx).into_any_element());
        let mut children = vec![master::card_header(name, tag, toggle, cx)];
        children.extend(error_banners(state, cx));
        children.push(self.telegram_pulse_hook(cx).into_any_element());
        let show_pending = self
            .telegram
            .core
            .pending
            .as_ref()
            .is_some_and(|p| p.core == core && data.telegram_rev == p.rev_at_send);
        if show_pending {
            if self.pending_inside_deadline(cx) {
                children.push(muted_line(
                    t!("telegram_core.sent_waiting").to_string(),
                    muted,
                ));
            } else {
                // The action row below carries the one Refresh button.
                children.push(muted_line(t!("telegram_core.no_answer").to_string(), muted));
            }
        }
        if !live && state.is_some() {
            children.push(muted_line(t!("telegram_core.stale").to_string(), muted));
        }
        let Some(state) = state else {
            children.push(master::note_box(
                "tgcore-no-snapshot",
                t!("telegram_core.no_snapshot").to_string(),
                cx,
            ));
            children.push(self.actions_row(core, false, None, cx));
            return master::card(children, cx);
        };
        if !state.state_supported {
            let line = match state.service_version.as_deref() {
                Some(v) if !v.is_empty() => t!("telegram_core.unsupported_ver", v = v).to_string(),
                _ => t!("telegram_core.unsupported").to_string(),
            };
            children.push(master::note_box("tgcore-unsupported", line, cx));
            return master::card(children, cx);
        }
        let mut body = v_flex()
            .debug_selector(|| "tgcore-body".into())
            .w_full()
            .when(!state.enabled || !live, |el| el.opacity(0.5))
            .child(master::card_row(
                "state",
                t!("telegram_core.client_state").to_string(),
                status_value(state, cx),
                cx,
            ));
        // Logged in, the same block holds the logout action and its confirmation.
        if auth_controls_visible(live, state)
            && let Some(service) = state.service.as_ref()
        {
            let label = if auth_step(&service.auth_state) == AuthStep::Ready {
                t!("telegram_core.row_account")
            } else {
                t!("telegram_core.row_login")
            };
            body = body.child(master::card_row(
                "login",
                label.to_string(),
                self.auth_block(core, service, muted, p, cx)
                    .into_any_element(),
                cx,
            ));
        }
        body = body
            .child(master::card_row(
                "proxy",
                t!("tgcore.proxy.title").to_string(),
                self.proxy_form(live, muted, cx),
                cx,
            ))
            .child(master::card_row(
                "actual",
                t!("telegram_core.proxy_actual").to_string(),
                actual_proxy_value(state, muted),
                cx,
            ));
        children.push(body.into_any_element());
        let note = (!state.enabled).then(|| t!("telegram_core.enable_tip").to_string());
        children.push(self.actions_row(core, live, note, cx));
        master::card(children, cx)
    }

    /// "Включить ридер" in the card header.
    fn enable_toggle(&self, enabled: bool, live: bool, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .id("telegram-core-enable-tip")
            .tooltip(text_tooltip(t!("telegram_core.enable_tip").to_string()))
            .child(
                MoonToggle::new("telegram-core-enable")
                    .label(t!("telegram_core.enable").to_string())
                    .checked(enabled)
                    .disabled(!live)
                    .mono(false)
                    .on_change(cx.listener(move |this, ch: &bool, _, cx| {
                        let v = *ch;
                        if let Some(core) = this.telegram.core.picked {
                            this.send_telegram(core, TelegramCmd::SetEnabled(v), cx);
                        }
                    })),
            )
    }

    /// The card's bottom row: send the proxy, refresh, and a muted note.
    fn actions_row(
        &self,
        core: u64,
        can_send: bool,
        note: Option<String>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        h_flex()
            .debug_selector(|| "tgcore-actions".into())
            .w_full()
            .flex_wrap()
            .items_center()
            .gap(design::ui_px(cx, 8.0))
            .pt(design::ui_px(cx, 10.0))
            .border_t_1()
            .border_color(rgba_from(p.border, 0.6))
            .child(
                MoonButton::new("telegram-core-proxy-save")
                    .primary()
                    .padding_x(12.0)
                    .disabled(!can_send)
                    .mono(false)
                    .label(t!("tgcore.proxy.send").to_string())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.save_proxy(core, window, cx);
                    }))
                    .render(),
            )
            .child(self.refresh_btn("telegram-core-refresh", core, cx))
            .children(note.map(|n| {
                div()
                    .debug_selector(|| "tgcore-actions-note".into())
                    .flex_1()
                    .min_w(design::font_w_px(cx, 160.0))
                    .font_family(design::ui_font())
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(n)
            }))
            .into_any_element()
    }

    /// The proxy kind control and its fields.
    fn proxy_form(&self, can_edit: bool, muted: Hsla, cx: &Context<Self>) -> AnyElement {
        let kind = self.telegram.core.proxy_kind;
        let view = cx.entity().downgrade();
        let kinds = [
            t!("telegram_core.proxy_kind_none").to_string(),
            t!("telegram_core.proxy_kind_socks5").to_string(),
            t!("telegram_core.proxy_kind_mtproto").to_string(),
        ];
        let seg = MoonSegmentedControl::new("tg-proxy-kind")
            .items(kinds.into_iter().enumerate().map(|(i, label)| {
                MoonSegmentItem::new("", label)
                    .fit_width(cx, 40.0, 200.0)
                    .selected(usize::from(kind) == i)
                    .disabled(!can_edit)
            }))
            .on_click(move |ix, _, _, app| {
                let Ok(kind) = u8::try_from(ix) else {
                    return;
                };
                let _ = view.update(app, |this, cx| {
                    this.telegram.core.proxy_kind = kind;
                    cx.notify();
                });
            });
        let core = &self.telegram.core;
        let fields: Vec<(String, &'static str, &Entity<MoonInputState>, bool)> = match kind {
            1 => vec![
                (
                    t!("telegram_core.proxy_host").to_string(),
                    "telegram-core-proxy-host",
                    &core.proxy_host,
                    false,
                ),
                (
                    t!("telegram_core.proxy_port").to_string(),
                    "telegram-core-proxy-port",
                    &core.proxy_port,
                    false,
                ),
                (
                    t!("telegram_core.proxy_user").to_string(),
                    "telegram-core-proxy-user",
                    &core.proxy_user,
                    false,
                ),
                (
                    t!("telegram_core.proxy_password").to_string(),
                    "telegram-core-proxy-password",
                    &core.proxy_password,
                    true,
                ),
            ],
            2 => vec![
                (
                    t!("telegram_core.proxy_host").to_string(),
                    "telegram-core-proxy-host",
                    &core.proxy_host,
                    false,
                ),
                (
                    t!("telegram_core.proxy_port").to_string(),
                    "telegram-core-proxy-port",
                    &core.proxy_port,
                    false,
                ),
                (
                    t!("telegram_core.proxy_secret").to_string(),
                    "telegram-core-proxy-secret",
                    &core.mtproto_secret,
                    true,
                ),
            ],
            _ => Vec::new(),
        };
        let has_fields = !fields.is_empty();
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 8.0))
            .child(
                div()
                    .debug_selector(|| "tgcore-proxy-kind".into())
                    .child(seg),
            )
            .when(has_fields, |col| {
                col.child(h_flex().flex_wrap().gap(design::ui_px(cx, 8.0)).children(
                    fields.into_iter().map(|(label, id, state, masked)| {
                        proxy_field(label, id, state, masked, can_edit, cx)
                    }),
                ))
                .child(
                    div()
                        .font_family(design::ui_font())
                        .text_color(muted)
                        .child(t!("telegram_core.proxy_secret_hint").to_string()),
                )
            })
            .into_any_element()
    }

    /// The saved proxy kind the store holds for `core`, 0 when unknown.
    fn saved_proxy_kind(&self, core: u64, cx: &App) -> u8 {
        self.backend
            .read(cx)
            .session
            .store()
            .core(core)
            .and_then(|d| d.telegram.as_ref())
            .and_then(|s| s.proxy_type)
            .unwrap_or(0)
    }

    fn pick_core(&mut self, core: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.telegram.core.picked = Some(core);
        self.telegram.core.pending = None;
        self.telegram.core.proxy_kind = self.saved_proxy_kind(core, cx);
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

    /// "Переподключить" in a disconnected core's empty state: the same request the Cores page
    /// sends.
    fn reconnect_btn(&self, core: u64, cx: &Context<Self>) -> AnyElement {
        MoonButton::new("telegram-core-reconnect")
            .ghost()
            .padding_x(12.0)
            .mono(false)
            .label(t!("telegram_core.reconnect").to_string())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.backend.update(cx, |b, bcx| {
                    b.reconnect_request.push(core);
                    bcx.notify();
                });
            }))
            .render()
            .into_any_element()
    }
}

fn muted_line(text: String, muted: Hsla) -> AnyElement {
    div()
        .font_family(design::ui_font())
        .text_color(muted)
        .child(text)
        .into_any_element()
}

/// One proxy field: caption above its input.
fn proxy_field(
    label: String,
    id: &'static str,
    state: &Entity<MoonInputState>,
    masked: bool,
    can_edit: bool,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let mut input = MoonInput::new(id)
        .state(state)
        .size(design::INPUT_SIZE)
        .disabled(!can_edit)
        .mono(true);
    if masked {
        input = input.mask_toggle();
    }
    v_flex()
        .w(design::font_w_px(cx, 180.0))
        .gap(design::ui_px(cx, 3.0))
        .child(
            div()
                .font_family(design::ui_font())
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(label),
        )
        .child(input)
        .into_any_element()
}

/// The red banners at the top of the card: proxy, setup and client errors.
fn error_banners(state: Option<&CoreTelegramState>, cx: &App) -> Vec<AnyElement> {
    let Some(state) = state else {
        return Vec::new();
    };
    let proxy = state
        .service
        .as_ref()
        .and_then(|svc| svc.proxy_error.as_ref())
        .map(|e| e.message.as_str());
    [
        ("tgcore-err-proxy", "telegram_core.proxy_error", proxy),
        (
            "tgcore-err-setup",
            "telegram_core.setup_error",
            state.setup_error.as_deref(),
        ),
        (
            "tgcore-err-client",
            "telegram_core.client_error",
            state.client_error.as_deref(),
        ),
    ]
    .into_iter()
    .filter_map(|(sel, lead, msg)| {
        let msg = msg.filter(|m| !m.is_empty())?;
        Some(master::error_banner(
            sel.to_string(),
            t!(lead).to_string(),
            msg.to_string(),
            cx,
        ))
    })
    .collect()
}

/// The Состояние row's value: service online/offline with its dot, the account line, then the
/// client state and the service version in mono.
fn status_value(state: &CoreTelegramState, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let muted = rgba_from(p.text_muted, 1.0);
    let (dot, online) = if state.service_online {
        (p.green, t!("telegram_core.service_online").to_string())
    } else {
        (p.red, t!("telegram_core.service_offline").to_string())
    };
    let account = if !state.enabled {
        t!("telegram_core.account_reader_off").to_string()
    } else {
        match state.service.as_ref() {
            Some(svc) if auth_step(&svc.auth_state) == AuthStep::Ready => {
                match svc.phone.as_deref().filter(|ph| !ph.is_empty()) {
                    Some(phone) => t!("telegram_core.account_phone", phone = phone).to_string(),
                    None => t!("telegram_core.tag.logged_in").to_string(),
                }
            }
            _ => t!("telegram_core.account_none").to_string(),
        }
    };
    let detail = [
        state.client_state.as_deref(),
        state.service_version.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");
    v_flex()
        .gap(design::ui_px(cx, 4.0))
        .child(
            h_flex()
                .flex_wrap()
                .items_center()
                .gap_x(design::ui_px(cx, 14.0))
                .gap_y(design::ui_px(cx, 4.0))
                .child(
                    h_flex()
                        .items_center()
                        .gap(design::ui_px(cx, 6.0))
                        .child(
                            div()
                                .flex_none()
                                .w(design::ui_px(cx, 7.0))
                                .h(design::ui_px(cx, 7.0))
                                .rounded_full()
                                .bg(rgb(dot)),
                        )
                        .child(div().font_family(design::ui_font()).child(online)),
                )
                .child(
                    div()
                        .debug_selector(|| "tgcore-account".into())
                        .font_family(design::ui_font())
                        .text_color(muted)
                        .child(account),
                ),
        )
        .when(!detail.is_empty(), |col| {
            col.child(
                div()
                    .font_family(design::mono())
                    .text_color(muted)
                    .child(detail),
            )
        })
        .into_any_element()
}

/// The Фактический прокси row's value: the mode, host and port the service actually runs.
fn actual_proxy_value(state: &CoreTelegramState, muted: Hsla) -> AnyElement {
    let text = match state.service.as_ref().and_then(|svc| svc.proxy.as_ref()) {
        None => t!("telegram_core.proxy_actual_unknown").to_string(),
        Some(proxy) => {
            let host = proxy.host.as_deref().unwrap_or("");
            let port = proxy.port.map(|p| p.to_string()).unwrap_or_default();
            format!("{} {} {}", proxy.mode, host, port)
                .trim()
                .to_string()
        }
    };
    div()
        .font_family(design::mono())
        .text_color(muted)
        .child(text)
        .into_any_element()
}
