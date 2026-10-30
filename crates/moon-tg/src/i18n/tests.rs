use super::*;

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
