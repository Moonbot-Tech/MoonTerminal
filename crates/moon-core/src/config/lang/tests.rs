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

/// Ukrainian codes must select Ukrainian. Leaving them unmapped would keep
/// `uk`, `uk-UA`, `uk_UA`, and `UK` on the system language, so Settings and
/// settings.toml could not stay on Ukrainian.
#[test]
fn uk_codes_map_to_ukrainian() {
    assert_eq!(Language::from_code("uk"), Some(Language::Uk));
    assert_eq!(Language::from_code("uk-UA"), Some(Language::Uk));
    assert_eq!(Language::from_code("uk_UA"), Some(Language::Uk));
    assert_eq!(Language::from_code("UK"), Some(Language::Uk));
}

/// settings.toml must store Ukrainian as `uk` and load that code back.
/// An unknown code must still load as the system language instead of rejecting the file.
#[test]
fn uk_serializes_as_uk() {
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct StoredLanguage {
        language: Language,
    }

    let text = toml::to_string(&StoredLanguage {
        language: Language::Uk,
    })
    .expect("uk must serialize");
    assert_eq!(text, "language = \"uk\"\n");
    let back: StoredLanguage = toml::from_str(&text).expect("uk must parse");
    assert_eq!(back.language, Language::Uk);

    let unknown: StoredLanguage =
        toml::from_str("language = \"zz\"\n").expect("unknown code must not reject the file");
    assert_eq!(unknown.language, Language::from_system());
}

// The translation test (`t!`/rust_i18n) moved to the UI crate (moon-ui-gpui):
// moon-core does not depend on rust-i18n and knows nothing about locales/.
