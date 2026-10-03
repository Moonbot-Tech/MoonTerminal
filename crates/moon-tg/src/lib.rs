//! The Telegram bot and its Mini App, over whichever process runs them.
//!
//! The transport — polling the Bot API, parsing commands, the Mini App's HTTP server, the tunnel —
//! is `moon_core::telegram` and stays there. This crate is what the transport hands work to: the
//! chat reports, the navigation and labels in the user's language, the Mini App's routes, pairing
//! and the service's lifecycle. Two hosts run it, the desktop terminal and the station, each
//! through [`TgHost`]; nothing here knows which.
//!
//! Translations are the terminal's own `locales/<lang>/*.yml` files, read by this crate's `i18n!`:
//! the same files, one locale per process (`rust_i18n` keeps a single current locale for every crate).

rust_i18n::i18n!("../../locales", fallback = "en");

mod dispatch;
mod host;
mod html;
mod labels;
mod mini_app;
pub(crate) mod notify;
mod report;
mod state;
mod station_status;
mod units;
// The terminal's own helper, compiled into this crate's test binary too: `rust_i18n` keeps one
// locale per process, and each test binary is its own process with its own lock.
#[cfg(test)]
#[path = "../../moon-ui-gpui/src/test_locale.rs"]
mod test_locale;

pub use dispatch::{issue_pairing, reconcile, reset_pairing, tick};
pub use host::{Finish, HostKind, Job, ReportRevision, TgHost};
pub use labels::status_text;
pub use state::TelegramState;
pub use station_status::{
    ReleaseCheck, ReleaseFailure, Section, StatusFacts, UpdateRefusal, station_status_reply,
    station_status_text,
};
pub use units::size_text;

/// Build this crate's dictionary once, on a dedicated 8 MiB thread.
///
/// `i18n!` materialises every key in the first lookup's frame. That frame fits an 8 MiB
/// stack, the size the binaries link with (`/STACK` on MSVC, the default main stack on Linux).
/// This function runs the build on such a thread and joins it, so the caller's stack does not
/// hold the frame. A later call returns immediately.
///
/// Hosts call this at startup. [`t`] calls it before every lookup, so a `t!` reached deep in a
/// request looks up a dictionary that already exists.
///
/// The lookup inside this function is `rust_i18n::t!`. [`t`] calls this function, so the inner
/// lookup stays on `rust_i18n::t!` and does not re-enter it.
pub fn warm_locales() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let _ = rust_i18n::t!("common.loading");
            })
            .expect("locale dictionary thread")
            .join()
            .expect("locale dictionary");
    });
}

/// Look up a key in this crate's dictionary.
///
/// Calls [`warm_locales`] before the lookup, so the dictionary build runs on the dedicated
/// thread. Arguments match [`rust_i18n::t`]: a key, an optional `locale`, and `name = value`
/// replacements.
macro_rules! t {
    ($($all:tt)*) => {{
        $crate::warm_locales();
        rust_i18n::t!($($all)*)
    }};
}
pub(crate) use t;

/// Answer in `language` from now on.
///
/// The locale is one per process for every crate's `i18n!`, so a host with a dictionary of its
/// own (the terminal) sets it there instead; this is for a host that has none (the station).
pub fn set_locale(language: moon_core::config::Language) {
    rust_i18n::set_locale(language.code());
}
