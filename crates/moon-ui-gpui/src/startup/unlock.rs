//! Deciding what the user has to prove before the terminal opens, and doing it in one window.
//!
//! Three questions can be due at startup, and this module is what sequences them so the user never
//! meets two dialogs in a row:
//!
//! 1. `servers.enc` does not open with any key on this machine, but it carries a password slot →
//!    ask for the encryption password. A correct answer also records THIS machine, so the question
//!    is asked once per machine rather than once per launch.
//! 2. A launch password is configured → ask for it. This is a local latch and is checked after the
//!    file is open, because the verifier lives in the file's header.
//! 3. Neither is possible → say so. Before this existed, that case exited the process before any
//!    window appeared, which reads as "the application does not start".
//!
//! The configuration is loaded here rather than in `run` because loading is exactly what can fail
//! with "ask the user", and asking needs a window.

use gpui::*;
use moon_core::config::crypto::AccessError;
use moon_core::config::{AppConfig, UiThemeMode};

use super::boot::{self, BootInput};
use crate::window::login::{self, LoginOutcome, LoginStep};

/// Load the configuration, run whatever prompts it requires, and boot the terminal.
pub(super) fn start(uid_floor: Option<u64>, input: BootInput, cx: &mut App) {
    // The login window renders before any configuration is available, so its theme comes from
    // `settings.toml` — plaintext, and readable with no key at all.
    install_login_theme(cx);

    let backups_allowed = input.firetest.is_none();
    match AppConfig::load(uid_floor, backups_allowed) {
        Ok(cfg) => launch_gate(cfg, input, cx),
        Err(error) => match error.downcast_ref::<AccessError>() {
            // A diagnostic run has nobody to type a password. Fail loudly instead of parking a
            // headless process on a prompt that will never be answered.
            _ if input.firetest.is_some() => {
                log::error!("FireTest: servers.enc требует ввода — прогон невозможен: {error:#}");
                std::process::exit(2);
            }
            Some(AccessError::NeedsPassword) => ask_for_file_password(uid_floor, input, cx),
            // Nothing on this machine opens the file, whether because it belongs to another
            // machine or because it is damaged — the two are indistinguishable for a legacy file
            // and lead to the same choice. Name it rather than exiting silently.
            Some(AccessError::NoKey | AccessError::Damaged(_)) => {
                unopenable_file(uid_floor, input, cx)
            }
            _ => fail_to_start(error, cx),
        },
    }
}

/// Offer the only two things left when nothing can open `servers.enc`.
///
/// Starting over is not a delete: the window renames the file aside first, and only then is the
/// vault told to forget it, because that seal is what protects a still-recoverable file from being
/// replaced by an empty one.
fn unopenable_file(uid_floor: Option<u64>, input: BootInput, cx: &mut App) {
    let mut carried = Some(input);
    login::open(LoginStep::Locked, cx, move |outcome, cx| {
        let Some(input) = carried.take() else {
            return;
        };
        match outcome {
            LoginOutcome::Unlocked => {
                moon_core::config::crypto::forget_sealed_file();
                let backups_allowed = input.firetest.is_none();
                match AppConfig::load(uid_floor, backups_allowed) {
                    Ok(cfg) => launch_gate(cfg, input, cx),
                    Err(error) => fail_to_start(error, cx),
                }
            }
            LoginOutcome::Abandoned => cx.quit(),
        }
    });
}

/// Ask for the encryption password, then continue the same startup.
///
/// The configuration is loaded AGAIN after a successful unlock rather than being carried out of
/// the window: unlocking rewrites the file's key slots, and `AppConfig::load` is the one path that
/// performs uid assignment, schema migration and the write-back those depend on. Reproducing any
/// of that here would be a second, quieter copy of the startup rules.
fn ask_for_file_password(uid_floor: Option<u64>, input: BootInput, cx: &mut App) {
    let mut carried = Some(input);
    login::open(LoginStep::FilePassword, cx, move |outcome, cx| {
        let Some(input) = carried.take() else {
            return;
        };
        match outcome {
            LoginOutcome::Unlocked => {
                let backups_allowed = input.firetest.is_none();
                match AppConfig::load(uid_floor, backups_allowed) {
                    Ok(cfg) => launch_gate(cfg, input, cx),
                    Err(error) => fail_to_start(error, cx),
                }
            }
            // Abandoning the prompt is the user's way out of a machine they cannot unlock.
            LoginOutcome::Abandoned => cx.quit(),
        }
    });
}

/// Run the launch-password gate, if one is configured, and boot afterwards.
fn launch_gate(cfg: AppConfig, input: BootInput, cx: &mut App) {
    // FireTest drives production surfaces with no human present. A launch password would stop a
    // diagnostic run at a prompt nothing can answer, so the latch — which protects a screen, not
    // the data — is skipped for it and said out loud.
    if input.firetest.is_some() {
        if moon_core::config::crypto::launch_password_is_set() {
            log::warn!("FireTest: пароль запуска пропущен (диагностический прогон)");
        }
        boot::boot(cfg, input, cx);
        return;
    }
    if !moon_core::config::crypto::launch_password_is_set() {
        boot::boot(cfg, input, cx);
        return;
    }
    // The saved theme is known now, so the second prompt is not painted in default colours after
    // the first one used the user's.
    super::install_moon_theme_for_config(&cfg, cx);
    let mut carried = Some((cfg, input));
    login::open(LoginStep::LaunchPassword, cx, move |outcome, cx| {
        let Some((cfg, input)) = carried.take() else {
            return;
        };
        match outcome {
            LoginOutcome::Unlocked => boot::boot(cfg, input, cx),
            LoginOutcome::Abandoned => cx.quit(),
        }
    });
}

/// Report a configuration failure that no password can fix, then stop.
///
/// The message reaches the log and `panic.log`'s neighbours rather than a dialog: this is the
/// "settings.toml is unreadable" class of failure, which the terminal has always treated as fatal.
fn fail_to_start(error: anyhow::Error, cx: &mut App) {
    log::error!("конфигурация не загружена: {error:#}");
    cx.quit();
}

/// Install the interface theme for a window shown before the configuration is available.
///
/// Reads the same plaintext `settings.toml` values the shell uses, so the login window matches the
/// user's dark/light choice and font size instead of flashing defaults and then re-themeing.
fn install_login_theme(cx: &mut App) {
    let prefs = moon_core::config::presentation_prefs();
    let mut theme = match prefs.ui_theme_mode {
        UiThemeMode::Dark => moon_ui::MoonThemeConfig::moon_terminal(),
        UiThemeMode::Light => moon_ui::MoonThemeConfig::moon_light(),
    };
    theme.mode = match prefs.ui_theme_mode {
        UiThemeMode::Dark => moon_ui::ThemeMode::Dark,
        UiThemeMode::Light => moon_ui::ThemeMode::Light,
    };
    rust_i18n::set_locale(prefs.language.code());
    moon_ui::MoonTheme::install_config(
        theme
            .with_font_delta(prefs.ui_font_delta)
            .with_ui_scale(prefs.ui_scale),
        cx,
    );
}
