use super::{LOGIN_STEPS, login_labels, login_step};
use moon_core::feed::{AuthStep, auth_step};

/// Every protocol auth state lands on one of the four steps; the states the steps alone do not
/// name carry their own words, and only `ready` marks the login done.
#[test]
fn every_auth_state_maps_onto_the_four_steps() {
    // One fixture per AuthStep variant, as the protocol spells it.
    let cases = [
        ("wait_phone", AuthStep::WaitPhone, 0, None),
        (
            "wait_qr_confirmation",
            AuthStep::WaitQrConfirmation,
            1,
            None,
        ),
        ("wait_code", AuthStep::WaitCode, 1, None),
        ("wait_password", AuthStep::WaitPassword, 2, None),
        (
            "wait_email_address",
            AuthStep::WaitEmailAddress,
            1,
            Some("tgcore.steps.email"),
        ),
        (
            "wait_email_code",
            AuthStep::WaitEmailCode,
            1,
            Some("tgcore.steps.email"),
        ),
        (
            "wait_registration",
            AuthStep::WaitRegistration,
            1,
            Some("tgcore.steps.registration"),
        ),
        ("ready", AuthStep::Ready, LOGIN_STEPS, None),
        (
            "wait_premium_purchase",
            AuthStep::WaitPremiumPurchase,
            1,
            Some("tgcore.steps.premium_required"),
        ),
        (
            "logging_out",
            AuthStep::Transitional,
            0,
            Some("tgcore.steps.waiting"),
        ),
        (
            "some_future_state",
            AuthStep::Unknown,
            0,
            Some("tgcore.steps.unknown"),
        ),
    ];
    for (wire, step, index, note) in cases {
        assert_eq!(auth_step(wire), step, "{wire}");
        assert_eq!(login_step(step), (index, note), "{wire}");
    }
}

/// The steps read Phone, Code, 2FA password, Done in every locale's own words.
#[test]
fn the_steps_are_four_named_ones() {
    let _locale = crate::test_locale::force("ru");
    assert_eq!(
        login_labels(),
        [
            "Номер телефона",
            "Код из Telegram",
            "Облачный пароль",
            "Подключено"
        ]
    );
    assert_eq!(login_labels().len(), LOGIN_STEPS);
}

/// The step indicator with a note under it stays inside the body and keeps the note whole, in
/// every language at the default and the minimum window.
#[gpui::test]
fn the_login_steps_fit_the_body(cx: &mut gpui::TestAppContext) {
    use crate::settings::layout_probe::{ProbeSpec, sweep};
    use crate::settings::telegram::tests::Host;
    use gpui::{InteractiveElement, IntoElement, ParentElement, Styled};
    sweep(
        cx,
        |_, _, (width, _)| Host {
            width,
            scroll: gpui::ScrollHandle::new(),
            build: std::rc::Rc::new(|_, _| {
                gpui::div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(gpui::px(10.0))
                    .child(gpui::div().debug_selector(|| "probe-steps".into()).child(
                        crate::settings::common::steps("probe-login-steps", login_labels(), 1),
                    ))
                    .child(
                        gpui::div()
                            .debug_selector(|| "probe-note".into())
                            .child(rust_i18n::t!("tgcore.steps.premium_required").to_string()),
                    )
                    .into_any_element()
            }),
        },
        |_| ProbeSpec {
            container: "probe-body",
            inside: vec!["probe-steps".into(), "probe-note".into()],
            no_overlap: vec![vec!["probe-steps".into(), "probe-note".into()]],
            last: Some("probe-note".into()),
            ..Default::default()
        },
    );
}
