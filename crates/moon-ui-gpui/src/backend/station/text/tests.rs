//! Regression checks for station presentation at the UI boundary.

use super::{error, progress};
use moon_remote::error::StationError;
use moon_remote::progress::{Progress, Step};
use moon_remote::ssh::OpenError;
use moon_remote::station::bot::StationMayStillPoll;

/// Flattening anyhow context into English hides the action in Russian and Spanish Settings.
#[test]
fn typed_context_keeps_the_localized_action_first() {
    for (locale, expected) in [
        ("ru", "Введите пароль сервера"),
        ("en", "Enter the server password"),
        ("es", "Introduce la contraseña del servidor"),
    ] {
        let _locale = crate::test_locale::force(locale);
        let failure = anyhow::anyhow!(StationError::EmptyPassword).context("first server login");
        let text = error(&failure);
        assert!(text.lines().next().unwrap().starts_with(expected), "{text}");
        assert!(text.lines().nth(1).unwrap().contains("first server login"));
    }
}

/// Dropping SSH error kinds would replace the host-key recovery action with generic English.
#[test]
fn ssh_headlines_preserve_actions_and_deadlines() {
    let _locale = crate::test_locale::force("en");
    let changed = anyhow::Error::new(OpenError::HostKeyChanged {
        pinned: "synthetic-old".into(),
        presented: "synthetic-new".into(),
    })
    .context("connect");
    assert!(
        error(&changed)
            .lines()
            .next()
            .unwrap()
            .contains("Forget the server")
    );
    let timeout = anyhow::Error::new(OpenError::Other(anyhow::anyhow!(StationError::Timeout)))
        .context("connect scope");
    assert!(error(&timeout).starts_with("The server did not answer in time."));
    assert!(error(&timeout).contains("connect scope"));
    let refused = anyhow::Error::new(OpenError::Refused {
        password_offered: true,
    });
    assert!(error(&refused).starts_with("The server refused the login."));
    let held = anyhow::anyhow!(StationError::BotNotReady).context(StationMayStillPoll);
    let headline = error(&held).lines().next().unwrap().to_owned();
    assert!(headline.contains("Take the bot off the server in the Telegram tab"));
}

/// Forwarding helper diagnostics to Event::Line leaks admin= and rmem_max= into progress.
#[test]
fn helper_tokens_do_not_reach_progress() {
    let _locale = crate::test_locale::force("ru");
    assert!(
        progress(Progress::Diagnostic(
            "admin=exists\nrmem_max=8388608".into()
        ))
        .is_none()
    );
    let step = progress(Progress::step(
        Step::TokenWritten,
        "bot token: credential written",
    ))
    .unwrap();
    assert_eq!(step, "Токен бота сохранён в шифрованных данных.");
    assert!(!step.contains("credential written"));
}

/// Flattening G's new typed failures drops the localized recovery action and context separation.
#[test]
fn access_failures_keep_localized_headlines_and_secondary_details() {
    use moon_remote::hosts::HostEditError;
    use moon_remote::station::access::RemovalError;
    for (locale, changed, unsupported, detail) in [
        (
            "ru",
            "Запись сервера изменилась",
            "Установленная служебная программа не поддерживает",
            "Подробности:",
        ),
        (
            "en",
            "The server record changed",
            "The installed helper does not support",
            "Details:",
        ),
        (
            "es",
            "La entrada del servidor cambió",
            "El script instalado no permite",
            "Detalles:",
        ),
    ] {
        let _locale = crate::test_locale::force(locale);
        for (failure, expected) in [
            (anyhow::Error::new(HostEditError::Changed), changed),
            (anyhow::Error::new(RemovalError::Unsupported), unsupported),
        ] {
            let shown = error(&failure.context("synthetic access context"));
            assert!(
                shown.lines().next().unwrap().starts_with(expected),
                "{shown}"
            );
            let diagnostics = shown.lines().nth(1).unwrap();
            assert!(diagnostics.starts_with(detail), "{shown}");
            assert!(diagnostics.contains("synthetic access context"));
        }
    }
}

/// A generic host-key headline hides Change address; forwarding diagnostics hides the user's locale.
#[test]
fn address_and_removal_actions_use_localized_progress_and_recovery() {
    for (locale, station, address, forget, probing) in [
        (
            "ru",
            "Станция",
            "Изменить адрес",
            "Забыть сервер",
            "Проверяем",
        ),
        (
            "en",
            "Station",
            "Change address",
            "Forget the server",
            "Reading",
        ),
        (
            "es",
            "Estación",
            "Cambiar dirección",
            "Olvidar el servidor",
            "Comprobando",
        ),
    ] {
        let _locale = crate::test_locale::force(locale);
        let changed = anyhow::Error::new(OpenError::HostKeyChanged {
            pinned: "synthetic-old".into(),
            presented: "synthetic-new".into(),
        });
        let shown = error(&changed);
        let headline = shown.lines().next().unwrap();
        for action in [station, address, forget] {
            assert!(headline.contains(action), "{shown}");
        }
        assert!(shown.lines().nth(1).unwrap().contains("synthetic-old"));
        assert!(shown.lines().nth(1).unwrap().contains("synthetic-new"));
        let shown = progress(Progress::step(Step::AddressProbe, "raw diagnostic")).unwrap();
        assert!(shown.starts_with(probing), "{shown}");
        for step in [Step::AddressVerify, Step::StationRemove] {
            let shown = progress(Progress::step(step, "raw diagnostic")).unwrap();
            assert!(!shown.contains("raw diagnostic"));
            assert!(!shown.starts_with("station.progress."));
        }
    }
}

/// Dropping any typed edit/removal arm replaces its specific recovery action with a generic failure.
#[test]
fn every_access_error_preserves_its_specific_action_through_context() {
    use moon_remote::hosts::HostEditError;
    use moon_remote::station::access::RemovalError;
    for locale in ["ru", "en", "es"] {
        let _locale = crate::test_locale::force(locale);
        let cases = [
            (
                anyhow::Error::new(HostEditError::Changed),
                "telegram.server.record_changed",
            ),
            (
                anyhow::Error::new(HostEditError::NotSetUp),
                "telegram.server.no_known_server",
            ),
            (
                anyhow::Error::new(HostEditError::SameAddress),
                "telegram.server.address_unchanged",
            ),
            (
                anyhow::Error::new(HostEditError::AddressKnown),
                "telegram.server.address_in_use",
            ),
            (
                anyhow::Error::new(HostEditError::NoFingerprint),
                "telegram.server.no_fingerprint",
            ),
            (
                anyhow::Error::new(RemovalError::Unsupported),
                "telegram.server.remove_unsupported",
            ),
            (
                anyhow::Error::new(RemovalError::Unconfirmed),
                "telegram.server.remove_unconfirmed",
            ),
            (
                anyhow::Error::new(RemovalError::LocalForgetFailed),
                "telegram.server.remove_local_failed",
            ),
            (
                anyhow::Error::new(RemovalError::NotConfigured),
                "telegram.server.core_sync_refused",
            ),
        ];
        for (failure, key) in cases {
            let shown = error(&failure.context("synthetic operation"));
            // The locale dictionary is independent of the error-to-action dispatch being tested.
            assert_eq!(shown.lines().next().unwrap(), rust_i18n::t!(key));
            assert!(
                shown
                    .lines()
                    .nth(1)
                    .unwrap()
                    .contains("synthetic operation")
            );
        }
    }
}
