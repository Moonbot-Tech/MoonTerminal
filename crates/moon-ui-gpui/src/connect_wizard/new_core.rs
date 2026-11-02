//! The "new core on a Linux server" branch: the server, the install, the exchange and its keys,
//! and the result — one screen per phase of the backend's install.

use gpui::*;
use moon_remote::core_move::ServerCheck;
use moon_remote::core_new::{ApiKeys, CoreState, CoreStatus};
use moon_ui::{
    MoonButton, MoonCheckbox, MoonLink, MoonPalette, MoonSelect, MoonSelectItem, MoonSpinner,
    MoonTextArea, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;
use zeroize::Zeroizing;

use super::ConnectWizardView;
use super::render::{Screen, note};
use super::server::{field, secret};
use crate::backend::core_new::{NewCommand, NewPhase, state_text};
use crate::design;

/// Memory one core takes on its server, the measure "fits N cores" counts with (170–230 MB were
/// measured on running cores).
const CORE_MB: u64 = 230;

impl ConnectWizardView {
    pub(super) fn new_core_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Screen {
        self.sync_exchanges(cx);
        let job = self.backend.read(cx).core_new.as_ref().map(|job| {
            (
                job.phase.clone(),
                job.status.clone(),
                job.public_ip.clone(),
                job.proto_port,
                job.uid,
                job.address(),
            )
        });
        let Some((phase, status, public_ip, port, uid, address)) = job else {
            return self.server_form_screen(cx);
        };
        let _ = window;
        match phase {
            NewPhase::Probing => waiting(
                t!("connect_wizard.server.title"),
                t!("connect_wizard.server.probing"),
                cx,
            ),
            NewPhase::Review {
                target,
                fingerprint,
            } => self.review_screen(target, fingerprint, cx),
            NewPhase::Checking => waiting(
                t!("connect_wizard.server.title"),
                t!("connect_wizard.server.checking"),
                cx,
            )
            .with_log(),
            NewPhase::Checked { check } => self.checked_screen(&check, port, cx),
            NewPhase::Installing => waiting(
                t!("connect_wizard.install.title"),
                t!("connect_wizard.install.running"),
                cx,
            )
            .sub(t!("connect_wizard.install.sub"))
            .with_log(),
            NewPhase::Keys => self.keys_screen(&public_ip, cx),
            NewPhase::LoggingIn => {
                let mut screen = waiting(
                    t!("connect_wizard.login.title"),
                    t!("connect_wizard.login.running"),
                    cx,
                );
                if let Some(status) = &status {
                    screen = screen.child(note(status_line(status), cx));
                }
                screen.with_log()
            }
            NewPhase::Stuck { status, error } => {
                self.stuck_screen(status.as_ref(), error, &public_ip, uid.is_some(), cx)
            }
            NewPhase::Connecting { .. } => waiting(
                t!("connect_wizard.connect.title"),
                t!("connect_wizard.connect.running"),
                cx,
            )
            .with_log(),
            NewPhase::Unreachable => Screen::new(t!("connect_wizard.unreachable.title"))
                .sub(t!("connect_wizard.unreachable.sub", port = port))
                .left(self.remove_button(cx))
                .right(
                    MoonButton::new("cw-reconnect")
                        .primary()
                        .size(design::CONTROL_TIER)
                        .label(t!("connect_wizard.check_again").to_string())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.backend.update(cx, |b, bcx| b.core_new_reconnect(bcx));
                        }))
                        .render(),
                )
                .with_log(),
            NewPhase::Done => {
                let body = uid.map(|uid| {
                    self.done_body(uid, Some(address), status.and_then(|s| s.emulator), cx)
                        .into_any_element()
                });
                let mut screen = Screen::new(t!("connect_wizard.done.title"));
                if let Some(body) = body {
                    screen = screen.child(body);
                }
                screen
                    .left(self.again_button(cx))
                    .right(self.finish_button(cx))
            }
            NewPhase::Removing => waiting(
                t!("connect_wizard.remove.title"),
                t!("connect_wizard.remove.running"),
                cx,
            )
            .with_log(),
            NewPhase::Removed => Screen::new(t!("connect_wizard.remove.title"))
                .sub(t!("connect_wizard.remove.done"))
                .right(self.again_button(cx)),
            NewPhase::Failed { reason } => Screen::new(t!("connect_wizard.failed.title"))
                .child(
                    div()
                        .text_color(rgba_from(MoonPalette::active(cx).red_text, 1.0))
                        .child(reason),
                )
                .left(self.back_button(cx))
                .with_log(),
        }
    }

    /// The server form, before anything is asked of the server.
    fn server_form_screen(&mut self, cx: &mut Context<Self>) -> Screen {
        let form = self.server.render(
            |this: &mut Self, on, _| this.server.by_key = on,
            |this: &mut Self, window, cx| this.browse_key(window, cx),
            cx,
        );
        Screen::new(t!("connect_wizard.server.title"))
            .sub(t!("connect_wizard.server.sub"))
            .child(form)
            .child(note(t!("connect_wizard.server.probe_note"), cx))
            .left(self.back_button(cx))
            .right(
                MoonButton::new("cw-probe")
                    .primary()
                    .size(design::CONTROL_TIER)
                    .label(t!("connect_wizard.server.check").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.probe(cx)))
                    .render(),
            )
    }

    /// The server's host key, confirmed before the login goes to it.
    fn review_screen(
        &mut self,
        target: moon_remote::ssh::Target,
        fingerprint: String,
        cx: &mut Context<Self>,
    ) -> Screen {
        let ok = self.server.fingerprint_ok;
        let (t2, f2) = (target.clone(), fingerprint.clone());
        Screen::new(t!("connect_wizard.server.title"))
            .sub(t!("connect_wizard.review.sub", addr = target.addr()))
            .child(div().font_family(design::mono()).child(fingerprint.clone()))
            .child(
                MoonCheckbox::new("cw-fingerprint")
                    .checked(ok)
                    .label(t!("connect_wizard.review.matches").to_string())
                    .on_change(cx.listener(|this, v: &bool, _, cx| {
                        this.server.fingerprint_ok = *v;
                        cx.notify();
                    })),
            )
            .child(note(t!("connect_wizard.review.secrets"), cx))
            .left(self.back_button(cx))
            .right(
                MoonButton::new("cw-login")
                    .primary()
                    .size(design::CONTROL_TIER)
                    .disabled(!ok)
                    .label(t!("connect_wizard.review.login").to_string())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.server.fingerprint_ok {
                            this.check_server(t2.clone(), f2.clone(), window, cx);
                        }
                    }))
                    .render(),
            )
    }

    /// What the server check found, the agreement, and "Install".
    fn checked_screen(&mut self, check: &ServerCheck, port: u16, cx: &mut Context<Self>) -> Screen {
        let fits = check.mem_available_mb / CORE_MB;
        let mut facts = v_flex()
            .gap(design::ui_px(cx, 4.0))
            .child(fact(
                true,
                t!(
                    "connect_wizard.check.os",
                    os = &check.os,
                    glibc = &check.glibc
                )
                .to_string(),
                cx,
            ))
            .child(fact(
                fits > 0,
                if fits > 0 {
                    t!(
                        "connect_wizard.check.memory",
                        total = check.mem_total_mb,
                        free = check.mem_available_mb,
                        n = fits
                    )
                } else {
                    t!(
                        "connect_wizard.check.memory_low",
                        total = check.mem_total_mb,
                        free = check.mem_available_mb
                    )
                }
                .to_string(),
                cx,
            ))
            .child(fact(
                true,
                t!("connect_wizard.check.port", port = port).to_string(),
                cx,
            ))
            .child(fact(
                true,
                if check.ufw_active {
                    t!("connect_wizard.check.ufw_on", port = port)
                } else {
                    t!("connect_wizard.check.ufw_off")
                }
                .to_string(),
                cx,
            ))
            .child(note(t!("connect_wizard.check.outside"), cx));
        facts = facts.pb(design::ui_px(cx, 6.0));
        let eula = self.eula_block(cx);
        let can_install = self.eula.is_some() || self.eula_ticked;
        Screen::new(t!("connect_wizard.check.title"))
            .child(facts)
            .child(eula)
            .left(self.back_button(cx))
            .right(
                MoonButton::new("cw-install")
                    .primary()
                    .size(design::CONTROL_TIER)
                    .disabled(!can_install)
                    .label(t!("connect_wizard.check.install").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.install(cx)))
                    .render(),
            )
            .with_log()
    }

    /// The agreement: a checkbox until it is accepted, then the date it was.
    fn eula_block(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(accepted) = &self.eula {
            let date = chrono::DateTime::from_timestamp_millis(accepted.accepted_at_ms)
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            return note(t!("connect_wizard.eula.accepted", date = date), cx).into_any_element();
        }
        h_flex()
            .flex_wrap()
            .items_center()
            .gap(design::ui_px(cx, 8.0))
            .child(
                MoonCheckbox::new("cw-eula")
                    .checked(self.eula_ticked)
                    .label(t!("connect_wizard.eula.accept").to_string())
                    .on_change(cx.listener(|this, v: &bool, _, cx| {
                        this.eula_ticked = *v;
                        cx.notify();
                    })),
            )
            .child(
                MoonLink::new("cw-eula-read", t!("connect_wizard.eula.read").to_string())
                    .on_click(|_, _, app| app.open_url(moon_core::config::eula::READ_URL)),
            )
            .into_any_element()
    }

    /// The exchange and its keys.
    fn keys_screen(&mut self, public_ip: &str, cx: &mut Context<Self>) -> Screen {
        Screen::new(t!("connect_wizard.keys.title"))
            .sub(t!("connect_wizard.keys.sub"))
            .child(self.keys_form(public_ip, cx))
            .left(self.remove_button(cx))
            .right(self.send_keys_button(cx))
            .with_log()
    }

    /// The exchange select, its key fields and the hints that apply to it.
    fn keys_form(&mut self, public_ip: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let picked = self.picked_exchange(cx);
        let rsa_supported = picked.as_ref().is_some_and(|e| e.rsa_supported);
        if !rsa_supported {
            self.rsa = false;
        }
        let mut form = v_flex().gap(design::ui_px(cx, 8.0)).child(
            h_flex()
                .flex_wrap()
                .items_center()
                .gap(design::ui_px(cx, 10.0))
                .child(
                    div()
                        .w(design::font_w_px(cx, 170.0))
                        .child(t!("connect_wizard.keys.exchange").to_string()),
                )
                .child(
                    div().w(design::font_w_px(cx, 260.0)).child(
                        MoonSelect::new(&self.exchange)
                            .placeholder(t!("connect_wizard.keys.pick").to_string())
                            .menu_width(design::font_w(cx, 260.0)),
                    ),
                ),
        );
        if rsa_supported {
            form = form.child(
                MoonCheckbox::new("cw-rsa")
                    .checked(self.rsa)
                    .label(t!("connect_wizard.keys.rsa").to_string())
                    .on_change(cx.listener(|this, v: &bool, _, cx| {
                        this.rsa = *v;
                        cx.notify();
                    })),
            );
        }
        form = if self.rsa {
            form.child(note(t!("connect_wizard.keys.rsa_hint"), cx))
                .child(
                    MoonTextArea::new("cw-rsa-bundle")
                        .state(&self.rsa_bundle)
                        .rows(4)
                        .mono(true),
                )
        } else {
            form.child(field(
                "cw-api-key",
                t!("connect_wizard.keys.api_key"),
                &self.api_key,
                false,
                cx,
            ))
            .child(field(
                "cw-api-secret",
                t!("connect_wizard.keys.api_secret"),
                &self.api_secret,
                true,
                cx,
            ))
        };
        let mut hints = v_flex()
            .gap(design::ui_px(cx, 4.0))
            .child(note(
                t!("connect_wizard.keys.whitelist", ip = public_ip),
                cx,
            ))
            .child(note(t!("connect_wizard.keys.rights"), cx));
        if let Some(ex) = &picked {
            if !ex.required_passphrase.is_empty() {
                hints = hints.child(note(
                    t!(
                        "connect_wizard.keys.passphrase",
                        value = &ex.required_passphrase
                    ),
                    cx,
                ));
            }
            if ex.id.starts_with("hyperliquid") {
                hints = hints.child(note(t!("connect_wizard.keys.hyperliquid"), cx));
            }
        }
        form.child(hints)
            .child(note(t!("connect_wizard.keys.emulator"), cx))
    }

    /// The exchange picked in the select, among those the core offers.
    fn picked_exchange(&self, cx: &App) -> Option<moon_remote::core_new::Exchange> {
        let id = self.exchange.read(cx).selected_value().cloned()?;
        self.backend
            .read(cx)
            .core_new
            .as_ref()?
            .exchanges
            .iter()
            .find(|e| e.id == id)
            .cloned()
    }

    /// Give the select the exchanges the core offers, once it offered them.
    fn sync_exchanges(&mut self, cx: &mut Context<Self>) {
        let offered: Vec<(String, String)> = self
            .backend
            .read(cx)
            .core_new
            .as_ref()
            .map(|job| {
                job.exchanges
                    .iter()
                    .map(|e| (e.id.clone(), e.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let shown = self.exchange.read(cx).items().len();
        if offered.len() != shown {
            self.exchange.update(cx, |state, cx| {
                state.set_items(
                    offered
                        .into_iter()
                        .map(|(id, name)| MoonSelectItem::new(id, name)),
                    cx,
                )
            });
        }
    }

    /// "Connect the exchange": the picked exchange and the typed keys to the core.
    fn send_keys_button(&self, cx: &mut Context<Self>) -> AnyElement {
        MoonButton::new("cw-send-keys")
            .primary()
            .size(design::CONTROL_TIER)
            .label(t!("connect_wizard.keys.send").to_string())
            .on_click(cx.listener(|this, _, window, cx| this.send_keys(window, cx)))
            .render()
            .into_any_element()
    }

    /// Check the keys form and pass it to the install; the fields forget the secret.
    fn send_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(exchange) = self.exchange.read(cx).selected_value().cloned() else {
            self.error = Some(t!("connect_wizard.keys.need_exchange").to_string());
            cx.notify();
            return;
        };
        let keys = if self.rsa {
            let bundle = self.rsa_bundle.read(cx).value().trim().to_string();
            if bundle.is_empty() {
                self.error = Some(t!("connect_wizard.keys.need_keys").to_string());
                cx.notify();
                return;
            }
            ApiKeys::Rsa {
                bundle: Zeroizing::new(bundle),
            }
        } else {
            match (secret(&self.api_key, cx), secret(&self.api_secret, cx)) {
                (Some(api_key), Some(api_secret)) => ApiKeys::Pair {
                    api_key,
                    api_secret,
                },
                _ => {
                    self.error = Some(t!("connect_wizard.keys.need_keys").to_string());
                    cx.notify();
                    return;
                }
            }
        };
        self.error = None;
        self.fixing_keys = false;
        self.api_secret
            .update(cx, |st, c| st.set_value(String::new(), window, c));
        self.rsa_bundle
            .update(cx, |st, c| st.set_value(String::new(), window, c));
        self.command(NewCommand::SetKeys { exchange, keys }, cx);
    }

    /// The core stopped on its login, or a step failed with the core on the server.
    fn stuck_screen(
        &mut self,
        status: Option<&CoreStatus>,
        error: Option<String>,
        public_ip: &str,
        has_row: bool,
        cx: &mut Context<Self>,
    ) -> Screen {
        let state = status.map(|s| &s.state);
        let title = match state {
            Some(CoreState::LoginError) => t!("connect_wizard.stuck.login_title"),
            Some(CoreState::RegistrationError) => t!("connect_wizard.stuck.registration_title"),
            Some(CoreState::ActionRequired) => t!("connect_wizard.stuck.action_title"),
            _ => t!("connect_wizard.stuck.title"),
        };
        let p = MoonPalette::active(cx);
        let mut screen = Screen::new(title);
        if has_row {
            screen = screen.sub(t!("connect_wizard.stuck.sub"));
        }
        if let Some(status) = status {
            if !status.login_error.is_empty() {
                screen = screen.child(
                    v_flex()
                        .gap(design::ui_px(cx, 4.0))
                        .child(note(
                            t!(
                                "connect_wizard.stuck.answer",
                                exchange = &status.exchange_name
                            ),
                            cx,
                        ))
                        .child(
                            div()
                                .font_family(design::mono())
                                .child(status.login_error.clone()),
                        ),
                );
            } else if !status.connectable() {
                screen = screen.child(note(status_line(status), cx));
            }
        }
        if let Some(error) = error {
            screen = screen.child(div().text_color(rgba_from(p.red_text, 1.0)).child(error));
        }
        if matches!(state, Some(CoreState::LoginError)) {
            screen = screen
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t!("connect_wizard.stuck.usually").to_string()),
                )
                .child(note(
                    t!("connect_wizard.stuck.reason_ip", ip = public_ip),
                    cx,
                ))
                .child(note(t!("connect_wizard.stuck.reason_rights"), cx))
                .child(note(t!("connect_wizard.stuck.reason_copy"), cx));
        }
        if self.fixing_keys {
            screen = screen.child(self.keys_form(public_ip, cx));
        }
        screen = screen.left(self.remove_button(cx));
        if has_row {
            if self.fixing_keys {
                screen = screen.right(self.send_keys_button(cx));
            } else {
                screen = screen
                    .right(
                        MoonButton::new("cw-retry")
                            .size(design::CONTROL_TIER)
                            .label(t!("connect_wizard.check_again").to_string())
                            .on_click(
                                cx.listener(|this, _, _, cx| this.command(NewCommand::Retry, cx)),
                            )
                            .render(),
                    )
                    .right(
                        MoonButton::new("cw-fix-keys")
                            .primary()
                            .size(design::CONTROL_TIER)
                            .label(t!("connect_wizard.stuck.fix_keys").to_string())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.fixing_keys = true;
                                cx.notify();
                            }))
                            .render(),
                    );
            }
        }
        screen.with_log()
    }

    /// "Remove the install": the core, its service and firewall rule leave the server.
    fn remove_button(&self, cx: &mut Context<Self>) -> AnyElement {
        MoonButton::new("cw-remove")
            .size(design::CONTROL_TIER)
            .label(t!("connect_wizard.remove.button").to_string())
            .on_click(cx.listener(|this, _, _, cx| {
                this.fixing_keys = false;
                this.command(NewCommand::Remove, cx);
            }))
            .render()
            .into_any_element()
    }

    /// "Back": to the start screen, dropping an install that is over.
    pub(super) fn back_button(&self, cx: &mut Context<Self>) -> AnyElement {
        MoonButton::new("cw-back")
            .size(design::CONTROL_TIER)
            .label(t!("connect_wizard.back").to_string())
            .on_click(cx.listener(|this, _, _, cx| this.back_to_start(cx)))
            .render()
            .into_any_element()
    }

    /// "Connect another one": back to the start with this core done.
    pub(super) fn again_button(&self, cx: &mut Context<Self>) -> AnyElement {
        MoonButton::new("cw-again")
            .size(design::CONTROL_TIER)
            .label(t!("connect_wizard.again").to_string())
            .on_click(cx.listener(|this, _, _, cx| {
                this.by_key_uid = None;
                this.by_key_since = None;
                this.back_to_start(cx);
            }))
            .render()
            .into_any_element()
    }

    /// "Done": closes the wizard.
    pub(super) fn finish_button(&self, cx: &mut Context<Self>) -> AnyElement {
        MoonButton::new("cw-finish")
            .primary()
            .size(design::CONTROL_TIER)
            .label(t!("connect_wizard.finish").to_string())
            .on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))
            .render()
            .into_any_element()
    }
}

/// A step that runs: a spinner and what it does.
fn waiting(title: impl Into<String>, doing: impl Into<SharedString>, cx: &App) -> Screen {
    Screen::new(title).child(
        h_flex()
            .items_center()
            .gap(design::ui_px(cx, 8.0))
            .child(MoonSpinner::new())
            .child(doing.into()),
    )
}

/// One fact of the server check, marked fine or not.
fn fact(fine: bool, text: String, cx: &App) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    h_flex()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .child(
            div()
                .text_color(rgba_from(if fine { p.green_text } else { p.amber }, 1.0))
                .child(if fine { "✓" } else { "!" }),
        )
        .child(text)
}

/// The core's status in one line, for while it settles.
fn status_line(status: &CoreStatus) -> String {
    let state = state_text(&status.state);
    t!(
        "connect_wizard.login.status",
        state = &state,
        exchange = &status.exchange_name,
        listening = if status.moonproto_listening {
            "✓"
        } else {
            "—"
        }
    )
    .to_string()
}
