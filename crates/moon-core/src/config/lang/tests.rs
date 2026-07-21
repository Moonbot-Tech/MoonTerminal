use super::Language;

#[test]
fn code_roundtrip() {
    for l in Language::ALL {
        assert_eq!(Language::from_code(l.code()), Some(l));
    }
    // Региональные коды и разделители — берём только префикс языка.
    assert_eq!(Language::from_code("en-US"), Some(Language::En));
    assert_eq!(Language::from_code("es_ES"), Some(Language::Es));
    assert_eq!(Language::from_code("ru-RU.UTF-8"), Some(Language::Ru));
    assert_eq!(Language::from_code("zh"), None);
}

// Тест переводов (`t!`/rust_i18n) переехал в UI-крейт (moon-terminal):
// moon-core не зависит от rust-i18n и не знает про locales/.
