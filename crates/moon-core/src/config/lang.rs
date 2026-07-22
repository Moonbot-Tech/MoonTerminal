//! Interface language. Stored in settings.toml as a code ("ru"/"en"/"es");
//! applied through `rust_i18n::set_locale(lang.code())`.
//!
//! The default is the system locale (sys-locale), falling back to English when the
//! system language is not supported.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Ru,
    En,
    Es,
}

impl Language {
    /// All supported languages (order = order in the settings dropdown).
    pub const ALL: [Language; 3] = [Language::Ru, Language::En, Language::Es];

    /// Locale code for rust_i18n / settings.toml.
    pub fn code(self) -> &'static str {
        match self {
            Language::Ru => "ru",
            Language::En => "en",
            Language::Es => "es",
        }
    }

    /// Language's native name for the dropdown (written in that language).
    pub fn label(self) -> &'static str {
        match self {
            Language::Ru => "Русский",
            Language::En => "English",
            Language::Es => "Español",
        }
    }

    /// Parses a code ("ru", "en-US", "es_ES", …), considering only the language prefix.
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
