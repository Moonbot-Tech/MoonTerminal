//! What a reader can still find untranslated, past what `locales.rs` checks.
//!
//! `locales.rs` proves every language has every key. It cannot see the three ways text escapes that:
//! a `t!` naming a key no file has, which draws the key itself; English copied into a translation,
//! which passes as a translated key; and a string written straight into a label or a tooltip, which
//! never reaches a dictionary at all.
//!
//! The last two have legitimate cases (`locales/README.md`, "Deliberately NOT translated"), so each
//! has a list beside the dictionaries. A list entry is a decision or a debt, in the open; a new case
//! fails here until it is translated or listed, and a translated one fails until it leaves the list,
//! so the lists only shrink by being worked through.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::support::{
    self, SHIPPED, code_only, fs, locales_root, read_locale_path, rust_sources,
    unquote_locale_scalar,
};

/// Keys whose value equals English in some language, with those languages.
const SAME_AS_ENGLISH: &str = "same-as-english.txt";

/// Text written straight into a label or a tooltip, as `file | text`.
const LITERAL_TEXT: &str = "literal-ui-text.txt";

/// The calls that put a string on screen, in the forms the UI crate writes them.
const TEXT_CALLS: &[&str] = &[
    ".label(",
    ".tooltip(",
    ".placeholder(",
    ".title(",
    ".hint(",
    ".caption(",
    ".child(",
    ".text_segment(",
    "text_tooltip(",
    "MoonTooltipView::new(",
    "MoonButtonSegment::new(",
];

/// Folders of the UI crate no user of a release reads: test harnesses and the debug window.
const NOT_FOR_USERS: &[&str] = &["firetest", "diagnostics"];

/// Every `key -> value` of one language.
fn values_of(lang: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for path in support::locale_dir_files(lang) {
        for line in read_locale_path(&path).lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("_version:") {
                continue;
            }
            if let Some((key, raw)) = line.split_once(':') {
                values.insert(key.to_string(), unquote_locale_scalar(key, raw.trim()));
            }
        }
    }
    values
}

/// Whether a dictionary value has a word in it to translate, its `%{placeholders}` aside.
fn has_a_word(value: &str) -> bool {
    let mut rest = value;
    let mut body = String::new();
    while let Some(start) = rest.find("%{") {
        body.push_str(&rest[..start]);
        match rest[start..].find('}') {
            Some(end) => rest = &rest[start + end + 1..],
            None => return false,
        }
    }
    body.push_str(rest);
    has_a_run_of_letters(&body, 2)
}

/// Whether a text has `count` letters in a row.
fn has_a_run_of_letters(text: &str, count: usize) -> bool {
    let (mut run, mut best) = (0, 0);
    for ch in text.chars() {
        run = if ch.is_alphabetic() { run + 1 } else { 0 };
        best = best.max(run);
    }
    best >= count
}

/// The entries of a list file: its lines, without comments and blanks.
fn listed(name: &str) -> BTreeSet<String> {
    let path = locales_root().join(name);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
        .replace("\r\n", "\n");
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Fail with what differs between what the tree holds and what its list says.
fn assert_list_matches(name: &str, found: &BTreeSet<String>, advice: &str) {
    let list = listed(name);
    let new: Vec<&String> = found.difference(&list).collect();
    let gone: Vec<&String> = list.difference(found).collect();
    assert!(
        new.is_empty() && gone.is_empty(),
        "locales/{name} is out of date.\n{advice}\nNot listed:\n{}\nListed, no longer so (delete \
         the line):\n{}",
        new.iter()
            .map(|line| line.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        gone.iter()
            .map(|line| line.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// Every `.rs` file a shipped crate compiles, its unit tests aside.
fn production_sources() -> Vec<PathBuf> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits in crates/")
        .to_path_buf();
    let mut files = Vec::new();
    for entry in fs::read_dir(&crates).expect("crates/ is readable") {
        let src = entry.expect("a readable entry").path().join("src");
        if src.is_dir() {
            rust_sources(&src, &mut files);
        }
    }
    files.retain(|path| path.file_name().is_none_or(|name| name != "tests.rs"));
    files
}

/// The string literals a text of source opens right after `needle`, whitespace allowed.
fn literals_after<'a>(source: &'a str, needle: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find(needle) {
        rest = &rest[at + needle.len()..];
        let Some(open) = rest.trim_start().strip_prefix('"') else {
            continue;
        };
        let mut escaped = false;
        for (index, ch) in open.char_indices() {
            match ch {
                '"' if !escaped => {
                    found.push(&open[..index]);
                    break;
                }
                '\\' if !escaped => escaped = true,
                _ => escaped = false,
            }
        }
    }
    found
}

/// Whether a literal reads as words rather than as an element id.
fn is_prose(literal: &str) -> bool {
    has_a_run_of_letters(literal, 3)
        && (literal.contains(' ')
            || !literal
                .chars()
                .any(|ch| matches!(ch, '-' | '.' | '_' | ':' | '{')))
}

/// `t!("no.such.key")` draws the key itself and nothing fails: `common.cancel` stood on a station
/// dialog's button that way. Mutation: name a key no language has anywhere in `crates/*/src`.
#[test]
fn every_key_the_code_names_exists() {
    let english = values_of("en");
    let mut missing = BTreeSet::new();
    for path in production_sources() {
        let source = code_only(&fs::read_to_string(&path).expect("a readable source"));
        for at in source.match_indices("t!(").map(|(at, _)| at) {
            // `format!(` and `print!(` end the same way, and only a bare `t!` is the dictionary.
            let word = source[..at]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '_');
            if word {
                continue;
            }
            let after = source[at + 3..].trim_start();
            let Some(open) = after.strip_prefix('"') else {
                continue;
            };
            let Some(end) = open.find('"') else {
                continue;
            };
            let key = &open[..end];
            if !english.contains_key(key) {
                missing.insert(format!("{key} ({})", support::file_name(&path)));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "t! names keys no dictionary has, and draws them raw: {missing:?}"
    );
}

/// English pasted into a translation satisfies `every_language_has_exactly_en_keys` and reads as
/// English to everyone. Mutation: copy an English value over its `uk` line.
#[test]
fn english_left_in_a_translation_is_a_listed_decision() {
    let english = values_of("en");
    let others: Vec<(&str, BTreeMap<String, String>)> = SHIPPED
        .iter()
        .filter(|lang| **lang != "en")
        .map(|lang| (*lang, values_of(lang)))
        .collect();
    let mut found = BTreeSet::new();
    for (key, value) in &english {
        if !has_a_word(value) {
            continue;
        }
        let same: Vec<&str> = others
            .iter()
            .filter(|(_, values)| values.get(key) == Some(value))
            .map(|(lang, _)| *lang)
            .collect();
        if !same.is_empty() {
            found.insert(format!("{key}: {}", same.join(" ")));
        }
    }
    assert_list_matches(
        SAME_AS_ENGLISH,
        &found,
        "Translate the value, or add `key: languages` if it stays English on purpose.",
    );
}

/// A label or tooltip written as a string never reaches a dictionary, so `every_language` cannot
/// see it. Mutation: write `.tooltip("Close")` in a panel.
#[test]
fn text_written_into_a_control_is_a_listed_decision() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&root, &mut files);
    let mut found = BTreeSet::new();
    for path in files {
        let relative = path
            .strip_prefix(&root)
            .expect("under src")
            .to_string_lossy()
            .replace('\\', "/");
        if relative.ends_with("tests.rs")
            || NOT_FOR_USERS
                .iter()
                .any(|folder| relative.starts_with(&format!("{folder}/")))
        {
            continue;
        }
        let source = code_only(&fs::read_to_string(&path).expect("a readable source"));
        for call in TEXT_CALLS {
            for literal in literals_after(&source, call) {
                if is_prose(literal) {
                    found.insert(format!("{relative} | {literal}").trim_end().to_string());
                }
            }
        }
    }
    assert_list_matches(
        LITERAL_TEXT,
        &found,
        "Put it through t! in every shipped language, or add `file | text` if it stays as it is.",
    );
}
