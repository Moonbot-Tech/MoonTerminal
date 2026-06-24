//! Язык интерфейса. Хранится в settings.toml как код ("ru"/"en"/"es");
//! применяется через `rust_i18n::set_locale(lang.code())`.
//!
//! Дефолт — системная локаль (sys-locale), с откатом на английский, если язык
//! системы не входит в поддерживаемые.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Ru,
    En,
    Es,
}

impl Language {
    /// Все поддерживаемые языки (порядок = порядок в выпадающем списке настроек).
    pub const ALL: [Language; 3] = [Language::Ru, Language::En, Language::Es];

    /// Код локали для rust_i18n / settings.toml.
    pub fn code(self) -> &'static str {
        match self {
            Language::Ru => "ru",
            Language::En => "en",
            Language::Es => "es",
        }
    }

    /// Самоназвание языка для выпадающего списка (на самом этом языке).
    pub fn label(self) -> &'static str {
        match self {
            Language::Ru => "Русский",
            Language::En => "English",
            Language::Es => "Español",
        }
    }

    /// Разбор кода ("ru", "en-US", "es_ES", …) — смотрим только префикс языка.
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

    /// Язык по системной локали; неизвестный/отсутствующий → английский.
    pub fn from_system() -> Language {
        sys_locale::get_locale()
            .and_then(|l| Language::from_code(&l))
            .unwrap_or(Language::En)
    }
}

impl Default for Language {
    /// Дефолт = язык системы (используется при первом запуске и для serde-дефолта
    /// в старых settings.toml без поля `language`).
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
        // Неизвестный код не роняем разбор файла — откатываемся на системный язык.
        let s = String::deserialize(d)?;
        Ok(Language::from_code(&s).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
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
}
