//! Session-only provider login: the shared server form and host-key review, without installing
//! or moving a core.

use gpui::*;
use moon_remote::setup::FirstAccess;
use moon_remote::ssh::Target;
use moon_ui::{MoonButton, MoonSpinner, h_flex};
use rust_i18n::t;

use super::render::{Screen, note};
use super::{Branch, ConnectWizardView};
use crate::backend::core_cmds::SessionLogin;
use crate::design;

#[cfg(test)]
mod tests;

/// Injectable provider-login call, returning no connection or credentials to the UI executor.
pub(super) type ProviderLogin = fn(&Target, &FirstAccess, &str) -> anyhow::Result<()>;

/// State of the login-only branch.
pub(super) struct LoginState {
    /// Identity of the current wizard flow; completions may mutate only this generation.
    pub(super) generation: u64,
    /// Whether a confirmed login is running off the UI thread.
    pub(super) running: bool,
    /// Network seam; tests replace it with a success or refusal stub.
    pub(super) call: ProviderLogin,
}

impl Default for LoginState {
    /// Build the production login state using the real provider-login call.
    fn default() -> Self {
        Self {
            generation: 0,
            running: false,
            call: provider_login,
        }
    }
}

impl LoginState {
    /// Invalidate any running login before another branch or host reuses the wizard.
    pub(super) fn restart_flow(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.running = false;
    }

    /// Whether an asynchronous completion still belongs to the active login flow.
    pub(super) fn completion_current(&self, generation: u64) -> bool {
        self.running && self.generation == generation
    }
}

/// Log in through moon-remote and drop the resulting connection before returning to the UI.
fn provider_login(target: &Target, first: &FirstAccess, pin: &str) -> anyhow::Result<()> {
    moon_remote::setup::provider_login(target, first, pin).map(drop)
}

/// Apply an off-thread result only to its originating flow, returning its storage or error outcome.
fn complete_provider_login(
    state: &mut LoginState,
    generation: u64,
    target: Target,
    first: FirstAccess,
    pin: String,
    result: anyhow::Result<()>,
    store: impl FnOnce(String, SessionLogin),
) -> Option<Result<(), String>> {
    if !state.completion_current(generation) {
        return None;
    }
    state.running = false;
    Some(finish_login(target, first, pin, result, store))
}

impl ConnectWizardView {
    /// Show the server form, the existing host-key probe/review, or the running login state.
    pub(super) fn login_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Screen {
        if self.login.running {
            return Screen::new(t!("connect_wizard.server.title")).child(
                h_flex()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(MoonSpinner::new())
                    .child(t!("connect_wizard.server.checking").to_string()),
            );
        }
        match self.job_phase(cx) {
            Some(crate::backend::core_new::NewPhase::Probing)
            | Some(crate::backend::core_new::NewPhase::Review { .. })
            | Some(crate::backend::core_new::NewPhase::Failed { .. }) => {
                self.new_core_screen(window, cx)
            }
            _ => self.login_form(cx),
        }
    }

    /// Build the reduced server form whose only remote operation is the shared host-key probe.
    fn login_form(&mut self, cx: &mut Context<Self>) -> Screen {
        let form = self.server.render(
            |this: &mut Self, on, _| this.server.by_key = on,
            |this: &mut Self, window, cx| this.browse_key(window, cx),
            cx,
        );
        Screen::new(t!("connect_wizard.server.title"))
            .child(form)
            .child(note(t!("connect_wizard.server.probe_note"), cx))
            .left(self.back_button(cx))
            .right(
                MoonButton::new("cw-login-probe")
                    .primary()
                    .size(design::CONTROL_TIER)
                    .label(t!("connect_wizard.server.check").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.probe(cx)))
                    .render(),
            )
    }

    /// Run a confirmed provider login off-thread, then store it or show the existing refusal text.
    pub(super) fn login_server(
        &mut self,
        target: Target,
        pin: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.login.running {
            return;
        }
        let (target, first) = match self.server.login_credentials(target, cx) {
            Ok(access) => access,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.error = None;
        self.login.running = true;
        cx.notify();
        let generation = self.login.generation;
        let call = self.login.call;
        cx.spawn(async move |view, cx| {
            let (target, first, pin, result) = cx
                .background_spawn(async move {
                    let result = call(&target, &first, &pin);
                    (target, first, pin, result)
                })
                .await;
            let _ = view.update_in(cx, |this, window, cx| {
                let backend = this.backend.clone();
                match complete_provider_login(
                    &mut this.login,
                    generation,
                    target,
                    first,
                    pin,
                    result,
                    |ip, login| {
                        backend.update(cx, |backend, cx| {
                            backend.core_cmds_add_login(ip, login);
                            cx.notify();
                        });
                    },
                ) {
                    None => {}
                    Some(Ok(())) => {
                        this.server.forget_secrets(window, cx);
                        this.close(window, cx);
                    }
                    Some(Err(error)) => {
                        this.error = Some(error);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Leave the login-only branch before another wizard flow uses the shared server fields.
    pub(super) fn leave_login(&mut self) {
        if self.branch == Some(Branch::Login) {
            return;
        }
        self.server.login_only = false;
        self.login.restart_flow();
    }
}

/// Store a successful login under its authenticated host and return any localized refusal.
fn finish_login(
    target: Target,
    first: FirstAccess,
    pin: String,
    result: anyhow::Result<()>,
    store: impl FnOnce(String, SessionLogin),
) -> Result<(), String> {
    if let Err(error) = result {
        return Err(login_failure(&error, &first));
    }
    store(
        target.host.clone(),
        SessionLogin {
            target: target.addr(),
            first,
            pin,
        },
    );
    Ok(())
}

/// Reuse the install wizard's credential-refusal explanation without exposing the underlying error.
fn login_failure(error: &anyhow::Error, first: &FirstAccess) -> String {
    if matches!(
        error.downcast_ref::<moon_remote::ssh::OpenError>(),
        Some(moon_remote::ssh::OpenError::Refused { .. })
    ) && let FirstAccess::Password { user, .. } = first
    {
        t!("connect_wizard.server.login_refused", user = user).to_string()
    } else {
        format!("{error:#}")
    }
}
