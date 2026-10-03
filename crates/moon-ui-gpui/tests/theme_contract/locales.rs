//! Static contract for the per-language locale dictionaries.
//!
//! rust-i18n reads `locales/<lang>/<area>.<lang>.yml`: the locale code is the filename's last
//! dot-segment, `_version` is 1, and each key is one flat `key: value` line. These tests fail
//! when a language drifts from English, a placeholder is renamed in only one file, a stem does
//! not match its folder, a `locales/` folder has no `Language::ALL` variant, or an `ALL`
//! variant has no folder.
//!
//! `SHIPPED` is hardcoded because this integration test cannot import `moon-core`. The last test
//! reads `crates/moon-core/src/config/lang.rs` so the two lists cannot diverge quietly.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use super::support::{self, SHIPPED, fs, locales_root, unquote_locale_scalar};

struct LocaleEntry {
    file: String,
    placeholders: Vec<String>,
}

/// Parse one v1 area file into key -> entry.
///
/// Blank lines, `#` comments, and `_version:` are skipped first, including when indented.
/// Every remaining line must be a top-level `key: value`: no leading whitespace, and no
/// whitespace inside the key. A repeated key panics with both origins.
///
/// Args:
///     text: File text with LF line endings.
///     file_name: Area file name used in panics.
///
/// Returns:
///     One entry per key. Panics on a nested line, a key that contains whitespace, an
///     unquoted value, an unterminated `%{`, or a repeated key.
fn parse_area(text: &str, file_name: &str) -> BTreeMap<String, LocaleEntry> {
    let mut entries = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("_version:") {
            continue;
        }
        let Some((key, raw)) = line.split_once(':') else {
            panic!("{file_name}:{} is not a flat key line", index + 1);
        };
        if line.chars().next().is_some_and(char::is_whitespace)
            || key.chars().any(char::is_whitespace)
        {
            panic!("{file_name}:{} is not a top-level key", index + 1);
        }
        let value = unquote_locale_scalar(key, raw.trim());
        let entry = LocaleEntry {
            file: file_name.to_string(),
            placeholders: placeholders_of(file_name, key, &value),
        };
        if let Some(previous) = entries.insert(key.to_string(), entry) {
            panic!(
                "{file_name} repeats {key}; first defined in {}",
                previous.file
            );
        }
    }
    entries
}

/// Sorted multiset of `%{name}` placeholders in one scalar.
///
/// Args:
///     file_name: Area file name included in the panic.
///     key: Localization key included in the panic.
///     value: Unquoted scalar.
///
/// Returns:
///     Placeholder names in sorted order, duplicates kept. Panics when `%{` has no closing `}`.
fn placeholders_of(file_name: &str, key: &str, value: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("%{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            panic!("{file_name} {key} has an unterminated %{{");
        };
        names.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    names.sort();
    names
}

/// Read one shipped language from disk.
///
/// Args:
///     lang: Shipped language code. The directory must exist.
///
/// Returns:
///     Key map for that language. Panics when a file cannot be read or a key repeats.
fn read_language(lang: &str) -> BTreeMap<String, LocaleEntry> {
    let mut entries = BTreeMap::new();
    for path in support::locale_dir_files(lang) {
        let file_name = support::file_name(&path).to_string();
        let text = support::read_locale_path(&path);
        for (key, entry) in parse_area(&text, &file_name) {
            if let Some(previous) = entries.insert(key.clone(), entry) {
                panic!(
                    "{lang} key {key} is in both {} and {file_name}",
                    previous.file
                );
            }
        }
    }
    entries
}

/// Catalogues for every [`SHIPPED`] language, loaded once per process.
///
/// Returns:
///     Language code to key map. The first call reads the files; later calls share that map.
fn shipped_catalogues() -> &'static BTreeMap<String, BTreeMap<String, LocaleEntry>> {
    static CATALOGUES: OnceLock<BTreeMap<String, BTreeMap<String, LocaleEntry>>> = OnceLock::new();
    CATALOGUES.get_or_init(|| {
        let mut catalogues = BTreeMap::new();
        for lang in SHIPPED {
            catalogues.insert((*lang).to_string(), read_language(lang));
        }
        catalogues
    })
}

/// Catalogue for one shipped language, shared with every test in this process.
///
/// Args:
///     lang: Shipped language code (`en`, `ru`, `es`, `uk`).
///
/// Returns:
///     Shared key map. Panics when `lang` is not in [`SHIPPED`].
fn load_language(lang: &str) -> &'static BTreeMap<String, LocaleEntry> {
    shipped_catalogues()
        .get(lang)
        .unwrap_or_else(|| panic!("{lang} is not in SHIPPED"))
}

fn area_stems(lang: &str) -> BTreeSet<String> {
    let suffix = format!(".{lang}.yml");
    support::locale_dir_files(lang)
        .into_iter()
        .map(|path| {
            let name = support::file_name(&path);
            name.strip_suffix(&suffix).unwrap_or(name).to_string()
        })
        .collect()
}

/// Every non-English folder must carry exactly English's keys. The failure names each missing
/// or extra key and the file it lives in.
#[test]
fn every_language_has_exactly_en_keys() {
    let english = load_language("en");
    assert!(!english.is_empty(), "en locale catalogue is empty");
    for lang in SHIPPED {
        if lang == "en" {
            continue;
        }
        let other = load_language(lang);
        let mut missing: Vec<String> = english
            .keys()
            .filter(|key| !other.contains_key(*key))
            .map(|key| format!("{key} ({})", english[key].file))
            .collect();
        let mut extra: Vec<String> = other
            .iter()
            .filter(|(key, _)| !english.contains_key(*key))
            .map(|(key, entry)| format!("{key} ({})", entry.file))
            .collect();
        missing.sort();
        extra.sort();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "{lang} keys drifted from en; missing: {missing:?}; extra: {extra:?}"
        );
    }
}

/// Each key must interpolate the same `%{name}` set in every language.
#[test]
fn placeholders_match_en() {
    let english = load_language("en");
    for lang in SHIPPED {
        if lang == "en" {
            continue;
        }
        let other = load_language(lang);
        for (key, en_entry) in english {
            let Some(entry) = other.get(key) else {
                continue;
            };
            assert_eq!(
                entry.placeholders, en_entry.placeholders,
                "{lang} {key} ({}) placeholders differ from en ({})",
                entry.file, en_entry.file
            );
        }
    }
}

/// The same area stems must exist in every language folder.
#[test]
fn each_area_exists_in_every_language() {
    let english = area_stems("en");
    assert!(!english.is_empty(), "en has no locale areas");
    for lang in SHIPPED {
        assert_eq!(area_stems(lang), english, "{lang} area stems must match en");
    }
}

/// Every area file is `<area>.<lang>.yml` inside `locales/<lang>/`, nothing `*.yml` sits at the
/// top of `locales/`, and the first real line of each file is `_version: 1`.
#[test]
fn stem_suffix_matches_folder() {
    let root = locales_root();
    let top = fs::read_dir(&root).unwrap_or_else(|err| panic!("read {}: {err}", root.display()));
    for entry in top {
        let entry = entry.unwrap_or_else(|err| panic!("read locales entry: {err}"));
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("yml") {
            panic!(
                "top-level locale file {} is not a per-language dictionary",
                path.display()
            );
        }
    }
    for lang in SHIPPED {
        let suffix = format!(".{lang}.yml");
        for path in support::locale_dir_files(lang) {
            let name = support::file_name(&path);
            assert!(
                name.ends_with(&suffix),
                "{} must end in {suffix} so rust-i18n reads locale {lang}",
                path.display()
            );
            let text = support::read_locale_path(&path);
            let first = text
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty() && !line.starts_with('#'));
            assert_eq!(
                first,
                Some("_version: 1"),
                "{} must open with _version: 1",
                path.display()
            );
        }
    }
}

/// `locales/` folders, the codes of `Language::ALL`, and [`SHIPPED`] are the same set.
///
/// A folder with no `ALL` variant fails, and an `ALL` variant with no folder fails. The enum
/// lives in `crates/moon-core/src/config/lang.rs`. This target cannot link that crate, so the
/// check reads the `ALL` list and the `code()` arms as text.
#[test]
fn languages_match_language_all() {
    let root = locales_root();
    let mut dirs = BTreeSet::new();
    for entry in fs::read_dir(&root).unwrap_or_else(|err| panic!("read {}: {err}", root.display()))
    {
        let entry = entry.unwrap_or_else(|err| panic!("read locales entry: {err}"));
        if entry.path().is_dir() {
            dirs.insert(entry.file_name().to_string_lossy().into_owned());
        }
    }
    let shipped: BTreeSet<String> = SHIPPED.iter().map(|code| (*code).to_string()).collect();
    let codes = language_all_codes(&support::read_core_src("config/lang.rs"));
    assert_eq!(
        dirs, shipped,
        "locales/ folders must match SHIPPED and Language::ALL"
    );
    assert_eq!(
        codes, shipped,
        "Language::ALL codes must match SHIPPED and the locales/ folders"
    );
}

/// A rustfmt wrap of `Language::ALL` must still resolve every shipped code.
///
/// Once the array exceeds the width, rustfmt puts `= [` on the declaration line
/// and each `Language::X` on its own line. A reader that keeps only the
/// `pub const ALL` line then sees an empty list, so this contract fails on a
/// formatting edit and hides a real language drift.
#[test]
fn language_all_codes_reads_a_wrapped_array() {
    let wrapped = r#"
impl Language {
    pub const ALL: [Language; 4] = [
        Language::Ru,
        Language::En,
        Language::Es,
        Language::Uk,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Language::Ru => "ru",
            Language::En => "en",
            Language::Es => "es",
            Language::Uk => "uk",
        }
    }
}
"#;
    let codes = language_all_codes(wrapped);
    assert_eq!(
        codes,
        BTreeSet::from([
            "en".to_string(),
            "es".to_string(),
            "ru".to_string(),
            "uk".to_string(),
        ])
    );
}

/// Codes of the variants listed in `Language::ALL`, resolved through `code()`.
///
/// The initializer may be one line or a rustfmt wrap. The span runs from
/// `pub const ALL` through the statement's closing `];`, so a value bracket
/// that starts on the next line is still part of the list. The bracketed list
/// after `=` is the value. The `[Language; N]` type before `=` is ignored.
/// Each `Language::X` is mapped with the `code()` match arm, not with `label()`
/// or `from_code`.
///
/// Args:
///     lang_rs: Full text of `crates/moon-core/src/config/lang.rs`.
///
/// Returns:
///     One code per `ALL` variant. Panics when `ALL` or a `code()` arm is missing.
fn language_all_codes(lang_rs: &str) -> BTreeSet<String> {
    let arms = code_arms(lang_rs);
    let start = lang_rs
        .find("pub const ALL")
        .expect("Language::ALL must stay in crates/moon-core/src/config/lang.rs");
    let tail = &lang_rs[start..];
    let end = tail
        .find("];")
        .expect("Language::ALL must assign a bracketed list");
    // `end` points at `]`; include `];` so the value's closing bracket stays in the span.
    let span = &tail[..end + 2];
    let after_eq = span
        .split_once('=')
        .expect("Language::ALL must assign a list")
        .1;
    let list = after_eq
        .split_once('[')
        .and_then(|(_, rest)| rest.split_once(']'))
        .expect("Language::ALL must assign a bracketed list")
        .0;
    let mut codes = BTreeSet::new();
    for variant in list
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let code = arms.get(variant).unwrap_or_else(|| {
            panic!("{variant} is in Language::ALL but Language::code has no arm for it")
        });
        assert!(
            codes.insert(code.clone()),
            "Language::ALL maps more than one variant to {code}"
        );
    }
    assert!(
        !codes.is_empty(),
        "Language::ALL must list at least one variant"
    );
    codes
}

/// `Language::X => "code"` arms inside `Language::code`.
///
/// Args:
///     lang_rs: Full text of `crates/moon-core/src/config/lang.rs`.
///
/// Returns:
///     Variant path to locale code. Panics when `code()` is missing or has no arms.
fn code_arms(lang_rs: &str) -> BTreeMap<String, String> {
    let mut lines = lang_rs.lines();
    if lines
        .by_ref()
        .find(|line| line.contains("fn code("))
        .is_none()
    {
        panic!("Language::code must stay in crates/moon-core/src/config/lang.rs");
    }
    let mut arms = BTreeMap::new();
    for line in lines {
        if line.contains("fn ") {
            break;
        }
        let Some((left, right)) = line.trim().split_once("=>") else {
            continue;
        };
        let variant = left.trim();
        if !variant.starts_with("Language::") {
            continue;
        }
        let quoted = right.trim().trim_end_matches(',');
        let Some(code) = quoted
            .strip_prefix('"')
            .and_then(|rest| rest.split('"').next())
        else {
            panic!("Language::code arm {variant} must map to a quoted code");
        };
        if arms.insert(variant.to_string(), code.to_string()).is_some() {
            panic!("Language::code repeats {variant}");
        }
    }
    assert!(
        !arms.is_empty(),
        "Language::code must map each variant to a code"
    );
    arms
}
