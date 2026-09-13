//! Contract tests for the core Telegram mirror's pure rules and secret redaction.

use super::*;

/// `core_telegram.rs:auth_controls_visible` must retain all five availability conjuncts; dropping
/// one shows phone or code inputs on an offline or unsupported core, where a user can submit to a
/// service that cannot handle the login.
#[test]
fn authentication_controls_require_every_availability_condition() {
    let complete = CoreTelegramState {
        enabled: true,
        service_online: true,
        state_supported: true,
        service: Some(CoreTelegramService::default()),
        ..Default::default()
    };
    assert!(auth_controls_visible(true, &complete));

    assert!(
        !auth_controls_visible(false, &complete),
        "a disconnected core must not offer authentication controls"
    );

    let mut disabled = complete.clone();
    disabled.enabled = false;
    assert!(!auth_controls_visible(true, &disabled));

    let mut offline_service = complete.clone();
    offline_service.service_online = false;
    assert!(!auth_controls_visible(true, &offline_service));

    let mut unsupported = complete.clone();
    unsupported.state_supported = false;
    assert!(!auth_controls_visible(true, &unsupported));

    let mut no_snapshot = complete;
    no_snapshot.service = None;
    assert!(!auth_controls_visible(true, &no_snapshot));
}

/// `core_telegram.rs` must keep custom `Debug` implementations for every secret-bearing wrapper;
/// deriving `Debug` exposes QR tokens, credentials, or proxy secrets in the Log panel and collected
/// diagnostics.
#[test]
fn debug_output_never_contains_telegram_secrets() {
    const QR_LINK: &str = "tg://login?token=SECRETVALUE";
    const PASSWORD_HINT: &str = "HINTVALUE";
    const PHONE: &str = "+79990000000";
    const PROXY_USER: &str = "PROXYUSER";
    const PASSWORD: &str = "PWVALUE";

    let details = CoreTelegramAuthDetails {
        qr_link: Some(QR_LINK.into()),
        password_hint: Some(PASSWORD_HINT.into()),
        ..Default::default()
    };
    let state = CoreTelegramState {
        proxy_user: Some(PROXY_USER.into()),
        service: Some(CoreTelegramService {
            details: details.clone(),
            phone: Some(PHONE.into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let outputs = [
        ("state", format!("{state:?}")),
        ("auth details", format!("{details:?}")),
        (
            "set password command",
            format!("{:?}", TelegramCmd::SetPassword(PASSWORD.into())),
        ),
        (
            "set phone command",
            format!("{:?}", TelegramCmd::SetPhone(PHONE.into())),
        ),
        (
            "set code command",
            format!("{:?}", TelegramCmd::SetCode("12345".into())),
        ),
        (
            "SOCKS5 command",
            format!(
                "{:?}",
                TelegramCmd::SetProxy(CoreTelegramProxy::Socks5 {
                    host: "127.0.0.1".into(),
                    port: 1080,
                    user: PROXY_USER.into(),
                    password: PASSWORD.into(),
                })
            ),
        ),
        (
            "MTProto proxy",
            format!(
                "{:?}",
                CoreTelegramProxy::MtProto {
                    host: "127.0.0.1".into(),
                    port: 443,
                    secret: QR_LINK.into(),
                }
            ),
        ),
    ];

    for (subject, output) in outputs {
        for secret in [QR_LINK, PASSWORD_HINT, PHONE, PROXY_USER, PASSWORD, "12345"] {
            assert!(
                !output.contains(secret),
                "{subject} Debug output leaked secret {secret:?}: {output}"
            );
        }
    }
}

/// `core_telegram.rs:auth_step` must preserve each known state and fall back without panicking;
/// folding `closed` into Ready or rejecting a future string makes Settings claim an account is
/// logged in or crashes while rendering it.
#[test]
fn authentication_step_mapping_is_total_and_distinguishes_every_known_step() {
    assert!(matches!(auth_step("wait_phone"), AuthStep::WaitPhone));
    assert!(matches!(
        auth_step("wait_qr_confirmation"),
        AuthStep::WaitQrConfirmation
    ));
    assert!(matches!(auth_step("wait_code"), AuthStep::WaitCode));
    assert!(matches!(auth_step("wait_password"), AuthStep::WaitPassword));
    assert!(matches!(
        auth_step("wait_email_address"),
        AuthStep::WaitEmailAddress
    ));
    assert!(matches!(
        auth_step("wait_email_code"),
        AuthStep::WaitEmailCode
    ));
    assert!(matches!(
        auth_step("wait_registration"),
        AuthStep::WaitRegistration
    ));
    assert!(matches!(auth_step("ready"), AuthStep::Ready));
    assert!(matches!(
        auth_step("wait_premium_purchase"),
        AuthStep::WaitPremiumPurchase
    ));

    for state in [
        "starting",
        "wait_tdlib_parameters",
        "logging_out",
        "closing",
        "closed",
    ] {
        assert!(
            matches!(auth_step(state), AuthStep::Transitional),
            "{state} must remain a transitional state"
        );
    }

    let long_unknown = "x".repeat(300);
    for state in ["", "wait_something_new", "READY", long_unknown.as_str()] {
        assert!(
            matches!(auth_step(state), AuthStep::Unknown),
            "{state:?} must use the safe unknown-state fallback"
        );
    }
}

/// `core_telegram.rs:resend_state` must require both a next delivery method and a resend deadline;
/// dropping either guard spams TDLib with rejected requests or enables a button whose availability
/// the core did not provide.
#[test]
fn resend_state_requires_both_a_next_method_and_a_deadline() {
    const NOW: i64 = 1_000;

    let unavailable_without_method = CoreTelegramAuthDetails {
        resend_at: Some(NOW - 100),
        ..Default::default()
    };
    assert!(matches!(
        resend_state(&unavailable_without_method, NOW),
        ResendState::Unavailable
    ));

    let unavailable_without_deadline = CoreTelegramAuthDetails {
        next_code_type: Some(CoreTelegramCodeType::default()),
        resend_at: None,
        ..Default::default()
    };
    assert!(matches!(
        resend_state(&unavailable_without_deadline, NOW),
        ResendState::Unavailable
    ));

    let waiting = CoreTelegramAuthDetails {
        next_code_type: Some(CoreTelegramCodeType::default()),
        resend_at: Some(NOW + 30),
        ..Default::default()
    };
    assert!(matches!(
        resend_state(&waiting, NOW),
        ResendState::Wait { secs_left: 30 }
    ));

    for resend_at in [Some(NOW), Some(NOW - 1)] {
        let ready = CoreTelegramAuthDetails {
            next_code_type: Some(CoreTelegramCodeType::default()),
            resend_at,
            ..Default::default()
        };
        assert!(matches!(resend_state(&ready, NOW), ResendState::Ready));
    }
}
