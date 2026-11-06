//! Session-login form and completion regressions; no test opens a network connection.

use gpui::{Context, ParentElement, Render, TestAppContext, Window, div, px};
use moon_core::config::UiThemeMode;
use moon_remote::setup::FirstAccess;
use moon_remote::ssh::{OpenError, Target};
use zeroize::Zeroizing;

use super::{LoginState, ProviderLogin, finish_login};
use crate::connect_wizard::server::ServerFields;

/// Render only the shared server fields, without constructing the application backend.
struct ServerProbe {
    fields: ServerFields,
}

impl Render for ServerProbe {
    /// Draw the production field layout for bounds assertions.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        div().child(self.fields.render(|_, _, _| {}, |_, _, _| {}, cx))
    }
}

/// A login request must retain the requested IP and remove install-only inputs and validation.
#[gpui::test]
fn login_request_prefills_host_and_hides_install_fields(cx: &mut TestAppContext) {
    let _locale = crate::test_locale::force("en");
    cx.update(|app| {
        moon_ui::MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            app,
        );
    });
    let view = cx.open_window(gpui::size(px(700.0), px(500.0)), |window, cx| {
        let mut fields = ServerFields::new("mbcore", window, cx);
        fields.login_only = true;
        fields.set_host("203.0.113.7", window, cx);
        assert_eq!(fields.host_value(cx), "203.0.113.7");
        assert!(fields.login_only);
        let target = Target {
            host: "203.0.113.7".into(),
            port: 22,
        };
        let error = match fields.login_credentials(target, cx) {
            Ok(_) => panic!("empty provider password must be rejected"),
            Err(error) => error,
        };
        assert_eq!(error, "the password is empty");
        ServerProbe { fields }
    });
    let _ = view;
    let install_fields = include_str!("../server.rs")
        .split_once(".when(!self.login_only, |form| {")
        .and_then(|(_, tail)| tail.split_once("})"))
        .expect("folder and port must be conditional on login_only")
        .0;
    assert!(install_fields.contains("cw-dir"));
    assert!(install_fields.contains("cw-port"));
}

/// A successful provider login must hand the same host and confirmed pin to session storage.
#[test]
fn successful_login_stores_host_and_confirmed_pin() {
    let call: ProviderLogin = |_, _, _| Ok(());
    let target = Target {
        host: "203.0.113.7".into(),
        port: 22,
    };
    let first = password_login();
    let result = call(&target, &first, "HOST_KEY");
    let mut stored = None;
    finish_login(target, first, "HOST_KEY".into(), result, |ip, login| {
        stored = Some((ip, login))
    })
    .expect("stubbed login succeeds");
    let (ip, login) = stored.expect("successful login must be stored");
    assert_eq!(ip, "203.0.113.7");
    assert_eq!(login.target, "203.0.113.7:22");
    assert_eq!(login.pin, "HOST_KEY");
    assert!(matches!(login.first, FirstAccess::Password { user, .. } if user == "root"));
}

/// A refused password must leave storage empty and show the wizard's existing refusal text.
#[test]
fn refused_login_stores_nothing_and_shows_refusal() {
    let _locale = crate::test_locale::force("en");
    let call: ProviderLogin = |_, _, _| {
        Err(OpenError::Refused {
            password_offered: true,
        }
        .into())
    };
    let target = Target {
        host: "203.0.113.7".into(),
        port: 22,
    };
    let first = password_login();
    let result = call(&target, &first, "HOST_KEY");
    let mut stored = false;
    let error = finish_login(target, first, "HOST_KEY".into(), result, |_, _| {
        stored = true
    })
    .expect_err("stubbed login is refused");
    assert!(!stored);
    assert_eq!(
        error,
        rust_i18n::t!("connect_wizard.server.login_refused", user = "root")
    );
}

/// Build the smallest password login accepted by the provider seam.
fn password_login() -> FirstAccess {
    FirstAccess::Password {
        user: "root".into(),
        password: Zeroizing::new("NEUTRAL_PASSWORD".into()),
    }
}

/// A completion from the previous host must not clear or close the wizard's newer login flow.
#[test]
fn reused_login_flow_rejects_the_previous_completion() {
    let mut state = LoginState::default();
    state.running = true;
    let previous = state.generation;
    assert!(state.completion_current(previous));

    state.restart_flow();

    assert!(!state.completion_current(previous));
    assert!(!state.running);
}
