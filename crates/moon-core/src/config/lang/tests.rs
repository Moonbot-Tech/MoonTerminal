use super::Language;

#[test]
fn code_roundtrip() {
    for l in Language::ALL {
        assert_eq!(Language::from_code(l.code()), Some(l));
    }
    // For regional codes and separators, use only the language prefix.
    assert_eq!(Language::from_code("en-US"), Some(Language::En));
    assert_eq!(Language::from_code("es_ES"), Some(Language::Es));
    assert_eq!(Language::from_code("ru-RU.UTF-8"), Some(Language::Ru));
    assert_eq!(Language::from_code("zh"), None);
}

// The translation test (`t!`/rust_i18n) moved to the UI crate (moon-ui-gpui):
// moon-core does not depend on rust-i18n and knows nothing about locales/.
