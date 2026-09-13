//! Static contracts preventing core Telegram credentials from reaching diagnostics.

use super::support::{Path, code_only, read_core_src, read_src};

/// Read the complete core-Telegram implementation surface whose secrets must never be logged.
///
/// This list is explicit rather than best-effort: omitting a split login-step module would make
/// the no-secret-logging check silently stop protecting passwords and codes.
fn telegram_sources() -> Vec<(&'static str, String)> {
    let ui_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let core_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("moon-core")
        .join("src");
    let mut sources = Vec::new();

    for rel in [
        "settings/telegram/core_section.rs",
        "settings/telegram/core_section/login_steps.rs",
        "settings/telegram/qr.rs",
    ] {
        assert!(
            ui_root.join(rel).is_file(),
            "missing protected core Telegram UI source {rel}"
        );
        sources.push((rel, read_src(rel)));
    }
    for rel in ["feed/live/telegram.rs", "feed/types/core_telegram.rs"] {
        assert!(
            core_root.join(rel).is_file(),
            "missing protected core Telegram source {rel}"
        );
        sources.push((rel, read_core_src(rel)));
    }

    sources
}

/// `telegram.rs` source files must never combine credential names with a logging macro; adding a
/// QR or password debug line leaks an account takeover credential into the Log panel and collected
/// diagnostics.
#[test]
fn core_telegram_sources_never_log_credential_values() {
    const LOG_MACROS: [&str; 4] = ["log::", "tracing::", "eprintln!", "println!"];
    const SECRET_NAMES: [&str; 5] = ["qr_link", "password", "secret", "phone", "code"];

    for (rel, source) in telegram_sources() {
        for (line_number, line) in code_only(&source).lines().enumerate() {
            let logs = LOG_MACROS
                .iter()
                .any(|macro_name| line.contains(macro_name));
            let names_secret = SECRET_NAMES
                .iter()
                .any(|secret_name| line.contains(secret_name));
            assert!(
                !(logs && names_secret),
                "{rel}:{} logs a Telegram credential-bearing value: {}",
                line_number + 1,
                line.trim()
            );
        }
    }
}

/// `telegram.rs` source files must not format `details` or `qr_link` with `Debug`; doing so makes
/// the token printable even if it was not passed directly to a logging macro.
#[test]
fn core_telegram_sources_never_debug_format_auth_details_or_qr_links() {
    for (rel, source) in telegram_sources() {
        let compact = code_only(&source)
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        for secret_field in ["details", "qr_link"] {
            let formatted = compact.match_indices("{:?}").any(|(offset, _)| {
                compact[offset..]
                    .split_once(')')
                    .is_some_and(|(arguments, _)| arguments.contains(secret_field))
            });
            assert!(
                !formatted,
                "{rel} Debug-formats Telegram {secret_field}, which can expose a login token"
            );
        }
    }
}
