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

mod control;
mod deal_chart;
mod dispatch;
mod host;
mod html;
mod labels;
mod menu;
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
pub use labels::{station_owner_navigation, status_text};
pub use mini_app::{check_notify_rows, notify_rows, save_notify_rows};

/// How long a closed trade waits for its dollar value when a chat's threshold needs one, in
/// minutes; the terminal's notification editor says so.
pub const TRADE_HOLD_MINUTES: i64 = notify::trades::HOLD_SECS / 60;
pub use state::TelegramState;
pub use station_status::{
    ReleaseCheck, ReleaseFailure, Section, StatusFacts, UpdateRefusal, station_status_reply,
    station_status_text,
};
pub use units::size_text;

/// Initialize this crate's dictionary once on a joined thread with an 8 MiB stack.
///
/// The seven-locale dictionary outgrows a default spawned thread's stack, so the initializing
/// lookup runs on this larger stack rather than the caller's. Concurrent calls wait for
/// initialization; after it succeeds, later calls reuse the completed `Once`.
///
/// Hosts call this at startup, and the crate-local [`t`] macro calls it before every lookup.
/// A successful return means the dictionary is ready, including when the first wrapped lookup
/// is reached deep inside a request.
///
/// The initializing lookup uses `rust_i18n::t!` directly to avoid recursively entering [`t`].
///
/// # Panics
///
/// Panics if the thread cannot be spawned, its lookup panics, or an earlier initialization
/// attempt poisoned the `Once`.
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
/// Calls [`warm_locales`] before forwarding all arguments unchanged to `rust_i18n::t!`, so
/// a lookup on a small caller stack uses an initialized dictionary. Available only inside
/// this crate; initialization panics are described by [`warm_locales`].
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
