//! Interface language. Stored in settings.toml as a code ("ru"/"en"/"es"/"uk"/"tr"/"pt"/"vi");
//! applied through `rust_i18n::set_locale(lang.code())`.
//!
//! The default is the system locale (sys-locale), falling back to English when the
//! system language is not supported.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Supported interface languages, persisted by code and displayed by native name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Ru,
    En,
    Es,
    Uk,
    Tr,
    Pt,
    Vi,
}

impl Language {
    /// All supported languages (order = order in the settings dropdown).
    pub const ALL: [Language; 7] = [
        Language::Ru,
        Language::En,
        Language::Es,
        Language::Uk,
        Language::Tr,
        Language::Pt,
        Language::Vi,
    ];

    /// Locale code for rust_i18n / settings.toml.
    pub fn code(self) -> &'static str {
        match self {
            Language::Ru => "ru",
            Language::En => "en",
            Language::Es => "es",
            Language::Uk => "uk",
            Language::Tr => "tr",
            Language::Pt => "pt",
            Language::Vi => "vi",
        }
    }

    /// Language's native name for the dropdown (written in that language).
    pub fn label(self) -> &'static str {
        match self {
            Language::Ru => "Русский",
            Language::En => "English",
            Language::Es => "Español",
            Language::Uk => "Українська",
            Language::Tr => "Türkçe",
            Language::Pt => "Português (Brasil)",
            Language::Vi => "Tiếng Việt",
        }
    }

    /// Parse the leading ASCII language prefix, ignoring case and any region suffix.
    ///
    /// Codes such as `en-US`, `es_ES`, and `pt-BR` select the same language as `en`, `es`,
    /// and `pt`. Returns `None` when the prefix is empty or unsupported.
    pub fn from_code(s: &str) -> Option<Language> {
        let prefix: String = s
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect::<String>()
            .to_ascii_lowercase();
        match prefix.as_str() {
            "ru" => Some(Language::Ru),
            "en" => Some(Language::En),
            "es" => Some(Language::Es),
            "uk" => Some(Language::Uk),
            "tr" => Some(Language::Tr),
            "pt" => Some(Language::Pt),
            "vi" => Some(Language::Vi),
            _ => None,
        }
    }

    /// Language from the system locale; unknown/missing → English.
    pub fn from_system() -> Language {
        sys_locale::get_locale()
            .and_then(|l| Language::from_code(&l))
            .unwrap_or(Language::En)
    }
}

impl Default for Language {
    /// Default = system language (used on first launch and as the serde default
    /// for old settings.toml files without a `language` field).
    fn default() -> Self {
        Language::from_system()
    }
}

impl Serialize for Language {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for Language {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // An unknown code does not fail file parsing; fall back to the system language.
        let s = String::deserialize(d)?;
        Ok(Language::from_code(&s).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests;
