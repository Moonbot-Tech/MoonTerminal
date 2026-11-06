//! The server step every branch of the wizard shares: where the core goes, the provider's login,
//! and the server's host key to confirm before anything secret is sent.

use std::net::Ipv4Addr;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_remote::setup::FirstAccess;
use moon_remote::ssh::Target;
use moon_ui::{
    MoonButton, MoonCheckbox, MoonInput, MoonInputState, MoonPalette, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;
use zeroize::Zeroizing;

use crate::backend::station::job;
use crate::design;
use crate::settings::telegram::server_bot::parse_target;

/// Label column width in unscaled pixels.
const LABEL_W: f32 = 170.0;
/// Field width in unscaled pixels.
const FIELD_W: f32 = 260.0;
/// The core's UDP port offered first.
pub(super) const DEFAULT_PROTO_PORT: &str = "5160";

/// What the server step yields once it is filled in and confirmed.
pub(crate) struct ServerAccess {
    pub(crate) target: Target,
    pub(crate) first: FirstAccess,
    pub(crate) public_ip: Ipv4Addr,
    pub(crate) dir: String,
    pub(crate) proto_port: u16,
}

/// The server step's fields.
pub(super) struct ServerFields {
    host: Entity<MoonInputState>,
    login: Entity<MoonInputState>,
    password: Entity<MoonInputState>,
    key_path: Entity<MoonInputState>,
    passphrase: Entity<MoonInputState>,
    dir: Entity<MoonInputState>,
    port: Entity<MoonInputState>,
    /// The provider's login is a key rather than a password.
    pub(super) by_key: bool,
    /// The user ticked that the host key matches the provider's.
    pub(super) fingerprint_ok: bool,
    /// Whether this form logs in only, hiding and skipping the install destination fields.
    pub(super) login_only: bool,
}

impl ServerFields {
    /// Empty fields, the instance folder prefilled with `dir`.
    pub(super) fn new(dir: &str, window: &mut Window, cx: &mut App) -> Self {
        let input = |masked: bool, value: &str, window: &mut Window, cx: &mut App| {
            let state = cx.new(|cx| MoonInputState::new(window, cx).masked(masked));
            if !value.is_empty() {
                let value = value.to_owned();
                state.update(cx, |st, c| st.set_value(value, window, c));
            }
            state
        };
        Self {
            host: input(false, "", window, cx),
            // Keep the field empty so the provider's alternative login is easy to enter.
            // Leaving it empty uses the same default that the placeholder offers.
            login: {
                let state = input(false, "", window, cx);
                state.update(cx, |st, c| st.set_placeholder("root", window, c));
                state
            },
            password: input(true, "", window, cx),
            key_path: input(false, "", window, cx),
            passphrase: input(true, "", window, cx),
            dir: input(false, dir, window, cx),
            port: input(false, DEFAULT_PROTO_PORT, window, cx),
            by_key: false,
            fingerprint_ok: false,
            login_only: false,
        }
    }

    /// The server to read the host key of, or what is wrong with the address and login.
    pub(super) fn target(&self, cx: &App) -> Result<Target, String> {
        let target = parse_target(&text(&self.host, cx))
            .ok_or_else(|| t!("connect_wizard.server.need_host").to_string())?;
        if target.host.parse::<Ipv4Addr>().is_err() {
            return Err(t!("connect_wizard.server.need_ipv4").to_string());
        }
        effective_login(&text(&self.login, cx))
            .ok_or_else(|| t!("connect_wizard.server.need_login").to_string())?;
        Ok(target)
    }

    /// The provider credentials for a session-only login, with no install destination validation.
    pub(super) fn login_credentials(
        &self,
        target: Target,
        cx: &App,
    ) -> Result<(Target, FirstAccess), String> {
        if target.host.parse::<Ipv4Addr>().is_err() {
            return Err(t!("connect_wizard.server.need_ipv4").to_string());
        }
        let typed_login = text(&self.login, cx);
        let login = effective_login(&typed_login)
            .ok_or_else(|| t!("connect_wizard.server.need_login").to_string())?;
        let key = self
            .by_key
            .then(|| (text(&self.key_path, cx), secret(&self.passphrase, cx)));
        let first = job::first_access(login.to_owned(), secret(&self.password, cx), key)
            .map_err(|e| format!("{e:#}"))?;
        Ok((target, first))
    }

    /// The whole step as an install takes it, against the host key the user confirmed.
    pub(super) fn access(&self, target: Target, cx: &App) -> Result<ServerAccess, String> {
        let public_ip = target
            .host
            .parse::<Ipv4Addr>()
            .map_err(|_| t!("connect_wizard.server.need_ipv4").to_string())?;
        let (target, first) = self.login_credentials(target, cx)?;
        let dir = text(&self.dir, cx);
        if dir.is_empty() || dir.contains(char::is_whitespace) {
            return Err(t!("connect_wizard.server.need_dir").to_string());
        }
        let proto_port = text(&self.port, cx)
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or_else(|| t!("connect_wizard.server.need_port").to_string())?;
        Ok(ServerAccess {
            target,
            first,
            public_ip,
            dir,
            proto_port,
        })
    }

    /// Forget the typed secrets: the install holds the login now.
    pub(super) fn forget_secrets<V: 'static>(&self, window: &mut Window, cx: &mut Context<V>) {
        for state in [&self.password, &self.passphrase] {
            state.update(cx, |st, c| st.set_value(String::new(), window, c));
        }
    }

    /// The form. `on_browse` picks the key file.
    pub(super) fn render<V: 'static>(
        &self,
        on_by_key: impl Fn(&mut V, bool, &mut Context<V>) + 'static,
        on_browse: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
        cx: &mut Context<V>,
    ) -> Div {
        v_flex()
            .gap(design::ui_px(cx, 8.0))
            .child(field(
                "cw-host",
                t!("connect_wizard.server.host"),
                &self.host,
                false,
                cx,
            ))
            .child(field(
                "cw-login",
                t!("connect_wizard.server.login"),
                &self.login,
                false,
                cx,
            ))
            .child(
                MoonCheckbox::new("cw-by-key")
                    .checked(self.by_key)
                    .label(t!("connect_wizard.server.by_key").to_string())
                    .on_change(cx.listener(move |this, v: &bool, _, cx| {
                        on_by_key(this, *v, cx);
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
                            "cw-key",
                            t!("connect_wizard.server.key_path"),
                            &self.key_path,
                            false,
                            cx,
                        ))
                        .child(
                            MoonButton::new("cw-key-browse")
                                .size(design::CONTROL_TIER)
                                .label(t!("connect_wizard.server.key_browse").to_string())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    on_browse(this, window, cx)
                                }))
                                .render(),
                        ),
                )
                .child(field(
                    "cw-passphrase",
                    t!("connect_wizard.server.passphrase"),
                    &self.passphrase,
                    true,
                    cx,
                ))
            })
            .child(field(
                "cw-password",
                if self.by_key {
                    t!("connect_wizard.server.sudo_password")
                } else {
                    t!("connect_wizard.server.password")
                },
                &self.password,
                true,
                cx,
            ))
            .when(!self.login_only, |form| {
                form.child(field(
                    "cw-dir",
                    t!("connect_wizard.server.dir"),
                    &self.dir,
                    false,
                    cx,
                ))
                .child(field(
                    "cw-port",
                    t!("connect_wizard.server.port"),
                    &self.port,
                    false,
                    cx,
                ))
            })
    }

    /// Offer `dir` as the instance folder.
    pub(super) fn set_dir<V: 'static>(&self, dir: &str, window: &mut Window, cx: &mut Context<V>) {
        let dir = dir.to_owned();
        self.dir
            .update(cx, |state, cx| state.set_value(dir, window, cx));
    }

    /// Prefill the server address supplied by a login-only opener.
    pub(super) fn set_host(&self, host: &str, window: &mut Window, cx: &mut App) {
        let host = host.to_owned();
        self.host
            .update(cx, |state, cx| state.set_value(host, window, cx));
    }

    /// Put a picked key file's path into its field.
    pub(super) fn set_key_path<V: 'static>(
        &self,
        path: String,
        window: &mut Window,
        cx: &mut Context<V>,
    ) {
        self.key_path
            .update(cx, |state, cx| state.set_value(path, window, cx));
    }
}

/// Trim a provider login and default an empty value to root; reject whitespace inside it.
fn effective_login(value: &str) -> Option<&str> {
    let login = value.trim();
    if login.contains(char::is_whitespace) {
        None
    } else {
        Some(if login.is_empty() { "root" } else { login })
    }
}

#[cfg(test)]
mod tests;

/// A labelled input that wraps under its label at narrow widths.
pub(super) fn field(
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

/// A field's text, trimmed.
pub(super) fn text(state: &Entity<MoonInputState>, cx: &App) -> String {
    state.read(cx).value().trim().to_owned()
}

/// A secret field's text, `None` when empty.
pub(super) fn secret(state: &Entity<MoonInputState>, cx: &App) -> Option<Zeroizing<String>> {
    let value = state.read(cx).value().to_string();
    (!value.is_empty()).then(|| Zeroizing::new(value))
}
