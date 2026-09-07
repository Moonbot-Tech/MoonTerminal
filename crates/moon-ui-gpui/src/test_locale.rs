//! Serializing the tests that depend on the process-wide interface locale.
//!
//! `rust_i18n` keeps ONE locale for the whole process, and `cargo test` runs this binary's tests in
//! parallel threads. So a test that switches the locale switches it for every test running at that
//! instant, and a test that asserts a localized string reads whatever the last switcher left — the
//! failure looks like `left: "Exchange not identified", right: "Биржа не определена"`, on a test
//! that touches neither language.
//!
//! The race predates this module and is rare enough to hide: eight consecutive runs of the whole
//! binary can pass. It is not rare enough to ignore, because the same suite is a blocking CI job,
//! and a red run there costs a person the time it takes to prove it was noise.
//!
//! Every test that switches the locale OR asserts localized text takes [`force`]. One mutex means
//! they cannot overlap each other, and the guard puts the previous locale back — including when the
//! test panics, so one failure does not cascade into the next test's language.

use std::sync::{Mutex, MutexGuard, OnceLock};

/// The lock itself. Poisoning is deliberately ignored: a panicking test has already failed, and
/// refusing the lock afterwards would turn one red test into a red suite.
fn lock() -> MutexGuard<'static, ()> {
    static LOCALE: OnceLock<Mutex<()>> = OnceLock::new();
    LOCALE
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Hold the interface locale at `code` for as long as the returned guard lives.
///
/// Args:
///     code: Locale to switch to for the duration of the test.
///
/// Returns:
///     A guard that restores the previous locale and releases the lock when dropped.
#[must_use = "the locale is only held while the guard lives"]
pub(crate) fn force(code: &str) -> LocaleGuard {
    let guard = lock();
    let previous = rust_i18n::locale().to_string();
    rust_i18n::set_locale(code);
    LocaleGuard {
        _lock: guard,
        previous,
    }
}

/// Restores the locale that was active before [`force`].
pub(crate) struct LocaleGuard {
    _lock: MutexGuard<'static, ()>,
    previous: String,
}

impl Drop for LocaleGuard {
    fn drop(&mut self) {
        rust_i18n::set_locale(&self.previous);
    }
}
