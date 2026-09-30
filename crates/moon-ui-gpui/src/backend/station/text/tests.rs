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
