//! Per-language dictionaries behind the `rust-i18n` backend and macro API.
//!
//! A single `i18n!` invocation puts every map insertion in one optimizer function. Keeping the
//! generated backends separate bounds that function's size without changing lookup semantics.

use std::borrow::Cow;

use rust_i18n::{Backend, NamespacedBackend};

mod en {
    rust_i18n::i18n!("../../locales/en", fallback = "en");
}
mod es {
    rust_i18n::i18n!("../../locales/es", fallback = "en");
}
mod pt {
    rust_i18n::i18n!("../../locales/pt", fallback = "en");
}
mod ru {
    rust_i18n::i18n!("../../locales/ru", fallback = "en");
}
mod tr {
    rust_i18n::i18n!("../../locales/tr", fallback = "en");
}
mod uk {
    rust_i18n::i18n!("../../locales/uk", fallback = "en");
}
mod vi {
    rust_i18n::i18n!("../../locales/vi", fallback = "en");
}

pub(crate) use en::_rust_i18n_t;

/// Locale-dispatching facade over the independently initialized dictionaries.
struct SplitBackend;

static BACKEND: SplitBackend = SplitBackend;
static EXTENSION: std::sync::OnceLock<NamespacedBackend> = std::sync::OnceLock::new();

impl SplitBackend {
    /// Return the generated backend for one exact locale code.
    fn for_locale(locale: &str) -> Option<&'static dyn Backend> {
        match locale {
            "en" => Some(en::_rust_i18n_backend()),
            "es" => Some(es::_rust_i18n_backend()),
            "pt" => Some(pt::_rust_i18n_backend()),
            "ru" => Some(ru::_rust_i18n_backend()),
            "tr" => Some(tr::_rust_i18n_backend()),
            "uk" => Some(uk::_rust_i18n_backend()),
            "vi" => Some(vi::_rust_i18n_backend()),
            _ => None,
        }
    }
}

impl Backend for SplitBackend {
    /// List the same locales as the combined generated backend, in sorted order.
    fn available_locales(&self) -> Vec<Cow<'_, str>> {
        ["en", "es", "pt", "ru", "tr", "uk", "vi"]
            .into_iter()
            .map(Cow::Borrowed)
            .collect()
    }

    /// Translate a key from its exact locale's generated backend.
    fn translate(&self, locale: &str, key: &str) -> Option<Cow<'_, str>> {
        Self::for_locale(locale)?.translate(locale, key)
    }

    /// Return all messages from one exact locale's generated backend.
    fn messages_for_locale(&self, locale: &str) -> Option<Vec<(Cow<'_, str>, Cow<'_, str>)>> {
        Self::for_locale(locale)?.messages_for_locale(locale)
    }
}

/// Return the combined backend behind the crate's translation macros.
#[doc(hidden)]
#[allow(missing_docs)]
pub fn _rust_i18n_backend() -> &'static dyn Backend {
    &BACKEND
}

/// Install the one optional namespaced extension supported by `rust-i18n`.
#[doc(hidden)]
#[allow(missing_docs)]
pub fn _rust_i18n_extend(backend: &'static dyn Backend, namespace: &'static str) {
    let extension = NamespacedBackend::new(backend, namespace);
    assert!(
        EXTENSION.set(extension).is_ok(),
        "rust-i18n backend for {} has already been extended",
        namespace
    );
}

/// Translate through the extension first, then the per-language backends.
fn backend_translate<'r>(locale: &str, key: &str) -> Option<Cow<'r, str>> {
    EXTENSION
        .get()
        .and_then(|backend| backend.translate(locale, key))
        .or_else(|| BACKEND.translate(locale, key))
}

/// Strip the final region or private-use component of a locale code.
#[inline]
#[doc(hidden)]
#[allow(missing_docs)]
pub fn _rust_i18n_lookup_fallback(locale: &str) -> Option<&str> {
    locale
        .rfind('-')
        .map(|n| locale[..n].trim_end_matches("-x"))
}

/// Translate a key, or reproduce `rust-i18n`'s missing-key display.
#[inline]
#[doc(hidden)]
#[allow(missing_docs)]
pub fn _rust_i18n_translate<'r>(locale: &str, key: &'r str) -> Cow<'r, str> {
    _rust_i18n_try_translate(locale, key).unwrap_or_else(|| {
        if locale.is_empty() {
            key.into()
        } else {
            format!("{}.{}", locale, key).into()
        }
    })
}

/// Translate through exact, regional, and finally English fallback lookups.
#[inline]
#[doc(hidden)]
#[allow(missing_docs)]
pub fn _rust_i18n_try_translate<'r>(locale: &str, key: impl AsRef<str>) -> Option<Cow<'r, str>> {
    backend_translate(locale, key.as_ref()).or_else(|| {
        let mut current_locale = locale;
        while let Some(fallback_locale) = _rust_i18n_lookup_fallback(current_locale) {
            if let Some(value) = backend_translate(fallback_locale, key.as_ref()) {
                return Some(value);
            }
            current_locale = fallback_locale;
        }
        backend_translate("en", key.as_ref())
    })
}

/// List locales from the dictionaries and optional extension without duplicates.
#[inline]
#[doc(hidden)]
#[allow(missing_docs)]
pub fn _rust_i18n_available_locales() -> Vec<Cow<'static, str>> {
    let mut locales = BACKEND.available_locales();
    if let Some(extension) = EXTENSION.get() {
        for locale in extension.available_locales() {
            if !locales.contains(&locale) {
                locales.push(locale.into_owned().into());
            }
        }
    }
    locales.sort();
    locales
}

#[cfg(test)]
mod tests;
