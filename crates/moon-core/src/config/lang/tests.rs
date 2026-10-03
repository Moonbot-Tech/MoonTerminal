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
    assert_eq!(Language::from_code("xx"), None);
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

/// Turkish, Brazilian Portuguese, and Vietnamese codes must select those
/// languages. An unmapped `pt-BR` would keep a Brazilian system locale on
/// English, so Settings could not follow it.
#[test]
fn tr_pt_vi_codes_map() {
    assert_eq!(Language::from_code("tr"), Some(Language::Tr));
    assert_eq!(Language::from_code("tr-TR"), Some(Language::Tr));
    assert_eq!(Language::from_code("TR"), Some(Language::Tr));
    assert_eq!(Language::from_code("pt"), Some(Language::Pt));
    assert_eq!(Language::from_code("pt-BR"), Some(Language::Pt));
    assert_eq!(Language::from_code("pt_PT"), Some(Language::Pt));
    assert_eq!(Language::from_code("pt-AO"), Some(Language::Pt));
    assert_eq!(Language::from_code("vi"), Some(Language::Vi));
    assert_eq!(Language::from_code("vi-VN"), Some(Language::Vi));
    assert_eq!(Language::from_code("vi_VN"), Some(Language::Vi));
}

/// settings.toml must store Turkish, Portuguese, and Vietnamese by code and
/// load those codes back. A missing arm would persist the dropdown choice as
/// another language.
#[test]
fn tr_pt_vi_serialize_as_their_codes() {
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct StoredLanguage {
        language: Language,
    }

    for (language, code) in [
        (Language::Tr, "tr"),
        (Language::Pt, "pt"),
        (Language::Vi, "vi"),
    ] {
        let text = toml::to_string(&StoredLanguage { language }).expect("must serialize");
        assert_eq!(text, format!("language = \"{code}\"\n"));
        let back: StoredLanguage = toml::from_str(&text).expect("must parse");
        assert_eq!(back.language, language);
    }
}

/// Dropdown codes and native names must stay distinct. A repeated code would
/// make two Settings rows persist as the same language.
#[test]
fn all_codes_and_labels_are_unique_and_non_empty() {
    let mut codes = std::collections::BTreeSet::new();
    let mut labels = std::collections::BTreeSet::new();
    for language in Language::ALL {
        let code = language.code();
        let label = language.label();
        assert!(!code.is_empty(), "empty code");
        assert!(!label.is_empty(), "empty label");
        assert!(codes.insert(code), "duplicate code {code}");
        assert!(labels.insert(label), "duplicate label {label}");
    }
    assert_eq!(codes.len(), Language::ALL.len());
    assert_eq!(labels.len(), Language::ALL.len());
}

// The translation test (`t!`/rust_i18n) moved to the UI crate (moon-ui-gpui):
// moon-core does not depend on rust-i18n and knows nothing about locales/.
