use std::collections::BTreeSet;

use super::*;

/// Independent rust-i18n code generation used as the split facade's oracle.
mod combined {
    rust_i18n::i18n!("../../locales", fallback = "en");
}

/// Splitting the generated backend must retain every language and its own translated values.
#[test]
fn every_product_locale_is_available_and_translated() {
    let translations = [
        ("en", "Loading…"),
        ("es", "Cargando…"),
        ("pt", "Carregando…"),
        ("ru", "Загрузка…"),
        ("tr", "Yükleniyor…"),
        ("uk", "Завантаження…"),
        ("vi", "Đang tải…"),
    ];
    let mut expected = translations.map(|(locale, _)| locale);
    expected.sort_unstable();
    let actual = _rust_i18n_available_locales();
    assert_eq!(actual, expected);

    for (locale, expected) in translations {
        assert_eq!(rust_i18n::t!("common.loading", locale = locale), expected);
    }
}

/// A regional locale must traverse to its base language, not directly to English.
#[test]
fn regional_locale_uses_the_base_language() {
    assert_eq!(
        rust_i18n::t!("common.loading", locale = "ru-RU"),
        "Загрузка…"
    );
}

/// A rust-i18n update that changes locale dispatch must fail here, not show raw keys in Telegram.
#[test]
fn split_locale_dispatch_matches_combined_backend() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let mut locales = combined::_rust_i18n_available_locales();
            assert!(!locales.is_empty(), "combined backend returned no locales");
            assert_eq!(_rust_i18n_available_locales(), locales);

            let mut keys = BTreeSet::new();
            for locale in &locales {
                let messages = combined::_rust_i18n_backend()
                    .messages_for_locale(locale)
                    .expect("available locale must have combined messages");
                keys.extend(messages.into_iter().map(|(key, _)| key.into_owned()));
            }
            assert!(!keys.is_empty(), "combined backend returned no keys");
            keys.insert("common.__missing_for_parity__".to_owned());

            locales.extend(
                ["ru-RU", "pt-BR", "en-US", "xx", "xx-YY", ""]
                    .into_iter()
                    .map(Cow::Borrowed),
            );
            for locale in locales {
                assert_eq!(
                    _rust_i18n_lookup_fallback(&locale),
                    combined::_rust_i18n_lookup_fallback(&locale),
                    "locale fallback mismatch for {locale:?}"
                );
                for key in &keys {
                    let actual = _rust_i18n_translate(&locale, key);
                    let expected = combined::_rust_i18n_translate(&locale, key);
                    assert!(
                        actual == expected,
                        "translate mismatch for locale {locale:?}, key {key:?}"
                    );

                    let actual = _rust_i18n_try_translate(&locale, key);
                    let expected = combined::_rust_i18n_try_translate(&locale, key);
                    assert!(
                        actual == expected,
                        "try_translate mismatch for locale {locale:?}, key {key:?}"
                    );
                }
            }
        })
        .expect("locale parity thread")
        .join()
        .expect("locale parity comparison");
}
