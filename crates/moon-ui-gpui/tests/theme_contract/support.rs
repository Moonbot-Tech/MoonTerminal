//! Shared helpers for the static UI contract: reading a source file, listing the crate's
//! sources, and slicing one function's body out of a file.
//!
//! `moon-ui-gpui` is a BINARY crate with no `[lib]`, so an integration test cannot import
//! anything from it. Every invariant in these modules is therefore checked by reading the
//! sources as text — a workaround for that limitation, not a style choice, and the reason
//! these helpers exist at all.
//!
//! Doubles as the modules' prelude: every one of them reads files, so the two `std` imports that
//! takes are re-exported here rather than repeated seven times.

pub use std::fs;
pub use std::path::{Path, PathBuf};

/// Collect every `.rs` file under `dir`, recursively, for the bans that must hold crate-wide.
pub fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", dir.display());
    });
    for entry in entries {
        let entry = entry.unwrap_or_else(|err| panic!("failed to read dir entry: {err}"));
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// Read one production source file for the static bans below, with line endings normalized to LF.
///
/// The normalization is load-bearing, not tidiness: these checkouts carry CRLF on disk, so any
/// pattern written with a bare `\n` — see [`fn_body`]'s closing-brace delimiter — silently matches
/// nothing and hands back a span far larger than intended. A ban that quietly stops bounding
/// anything still reports PASS.
pub fn read_src(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(rel);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    text.replace("\r\n", "\n")
}

/// Codes of `moon_core::config::Language::ALL`, in dropdown order.
///
/// `languages_match_language_all` reads `crates/moon-core/src/config/lang.rs` as text and
/// checks that this list and the codes of `ALL` contain the same set of languages.
pub const SHIPPED: [&str; 7] = ["ru", "en", "es", "uk", "tr", "pt", "vi"];

/// Repository `locales/` directory, next to the workspace crates.
pub fn locales_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("locales")
}

/// Path of one per-language area file: `locales/<lang>/<area>.<lang>.yml`.
pub fn locale_path(area: &str, lang: &str) -> PathBuf {
    locales_root().join(lang).join(format!("{area}.{lang}.yml"))
}

/// Last path component as UTF-8.
///
/// Args:
///     path: File whose name is needed for a panic or a stem.
///
/// Returns:
///     The file name, or `"?"` when it is missing or not Unicode.
pub fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("?")
}

/// Read one locale file and normalize CRLF to LF.
///
/// Args:
///     path: Area file under `locales/`.
///
/// Returns:
///     File text with `\r\n` replaced by `\n`. Panics when the file cannot be read.
pub fn read_locale_path(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
        .replace("\r\n", "\n")
}

/// Read one area file with line endings normalized to LF.
fn read_locale_file(area: &str, lang: &str) -> String {
    read_locale_path(&locale_path(area, lang))
}

/// Unquote one flat locale scalar.
///
/// Double-quoted values unescape `\"`, `\\`, and `\n`. A quote closes the value only when an
/// even number of backslashes precedes it, so a value ending in `\"` keeps that quote.
/// Single-quoted values turn `''` into `'`.
///
/// Args:
///     key: Localization key, named in the panic when the scalar is not quoted.
///     raw: Text after the `: ` separator, including the surrounding quotes.
///
/// Returns:
///     The decoded scalar. Panics when `raw` has no opening single or double quote.
///     Decoding stops at the closing quote or end of input; closing quotes are not validated.
pub fn unquote_locale_scalar(key: &str, raw: &str) -> String {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('"') {
        return unescape_double(rest);
    }
    if let Some(rest) = raw.strip_prefix('\'') {
        return unescape_single(rest);
    }
    panic!("{key} locale value must be quoted");
}

/// Decode text after an opening double quote until an unescaped quote or end of input.
///
/// Recognizes escaped quotes, backslashes, and newlines; preserves unknown escapes and a
/// trailing backslash. Text after a closing quote is ignored.
fn unescape_double(rest: &str) -> String {
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else if ch == '"' {
            break;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Decode text after an opening single quote, turning doubled quotes into one quote.
///
/// A lone quote ends the value; text after it is ignored. Without one, consumes all input.
fn unescape_single(rest: &str) -> String {
    let mut out = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                out.push('\'');
            } else {
                break;
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Value of one flat key in `locales/<lang>/<area>.<lang>.yml`.
///
/// A line matches only when it starts with `{key}: `, so `report.traded_volume` does not
/// consume `report.traded_volume_tip`. Comment and blank lines are skipped.
///
/// Args:
///     area: Area stem, without a language suffix (`shell`, not `shell.yml`).
///     lang: Shipped language code.
///     key: Fully-qualified localization key.
///
/// Returns:
///     The unquoted scalar. Panics when the file or the key is missing.
pub fn locale_value(area: &str, lang: &str, key: &str) -> String {
    let text = read_locale_file(area, lang);
    let prefix = format!("{key}: ");
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(raw) = line.strip_prefix(&prefix) {
            return unquote_locale_scalar(key, raw);
        }
    }
    panic!("locales/{lang}/{area}.{lang}.yml does not define {key}");
}

/// Every `*.yml` file in `locales/<lang>/`, sorted by path.
///
/// Args:
///     lang: Shipped language code. The directory must exist.
///
/// Returns:
///     File paths. An unreadable directory panics.
pub fn locale_dir_files(lang: &str) -> Vec<PathBuf> {
    let dir = locales_root().join(lang);
    let entries =
        fs::read_dir(&dir).unwrap_or_else(|err| panic!("failed to read {}: {err}", dir.display()));
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.unwrap_or_else(|err| panic!("failed to read dir entry: {err}"));
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("yml") {
            files.push(path);
        }
    }
    files.sort();
    files
}

/// Assert that one localization key is defined in every shipped language.
///
/// Args:
///     area: Area stem, without a language suffix (`shell`, not `shell.yml`).
///     key: Fully-qualified localization key to inspect.
///
/// Returns:
///     Nothing. A missing file or key panics with its language, area, and key.
pub fn assert_locale_key_in_every_language(area: &str, key: &str) {
    for lang in SHIPPED {
        let _value = locale_value(area, lang, key);
    }
}

/// Read one `moon-tg` source file for a static contract on the bot and Mini App, normalizing line
/// endings for the same reason as [`read_src`].
pub fn read_tg_src(rel: &str) -> String {
    read_sibling_src("moon-tg", rel)
}

/// Read one `moon-core` source file for a cross-crate static contract, normalizing line endings.
///
/// `moon-ui-gpui` has no library target, so this integration target owns static contracts that
/// span the UI binary and its sibling core crate. Normalize here for the same reason as
/// [`read_src`]: a CRLF checkout must not make a line-based source assertion silently miss.
pub fn read_core_src(rel: &str) -> String {
    read_sibling_src("moon-core", rel)
}

/// Read one source file of a sibling crate, line endings normalized.
fn read_sibling_src(krate: &str, rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(krate)
        .join("src")
        .join(rel);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    text.replace("\r\n", "\n")
}

/// Read the whole of startup as one text: `startup.rs` plus `startup/boot.rs`.
///
/// Startup was split when the login window arrived — the window can only exist inside `App::run`,
/// so everything that follows `AppConfig::load` had to become callable after a password prompt and
/// moved to `boot`. The invariants these modules pin span both halves, and several of them compare
/// POSITIONS, so the two files are concatenated in call order: `run` first, then `boot`.
pub fn read_startup() -> String {
    format!(
        "{}
{}",
        read_src("startup.rs"),
        read_src("startup/boot.rs")
    )
}

/// The body of a top-level `fn`, from its signature to the closing brace in column 0.
///
/// Deliberately excludes the doc comment above the signature: a rule about what a function CALLS
/// must not be satisfiable by prose that merely mentions the callee.
pub fn fn_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let after = source
        .split_once(signature)
        .unwrap_or_else(|| panic!("expected to find `{signature}` in the source"))
        .1;
    after.split("\n}\n").next().unwrap_or(after)
}

/// Return one brace-delimited function or method body, including its signature.
///
/// Args:
///     source: Rust source containing the target function or method.
///     signature: Unique signature prefix that appears before the target's opening brace.
///
/// Returns:
///     The source slice from the signature through its matching closing brace.
pub fn braced_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("expected to find `{signature}` in the source"));
    let open = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("expected `{signature}` to have a body"));
    let mut depth = 0usize;
    for (offset, byte) in source.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("expected `{signature}` to have a matching closing brace");
}

/// Return the source between `anchor` and the first `stop` after it, excluding both.
///
/// The idiom several of these invariants need: isolate ONE builder chain so a check about the
/// element being built cannot be satisfied — or broken — by a sibling further down the file.
///
/// Args:
///     source: Rust source to slice.
///     anchor: Unique marker the chain starts at.
///     stop: Marker that ends the chain.
///     what: Subject named in the panic messages.
///
/// Returns:
///     The slice between the two markers.
pub fn chain_between<'a>(source: &'a str, anchor: &str, stop: &str, what: &str) -> &'a str {
    let after = source
        .split_once(anchor)
        .unwrap_or_else(|| panic!("{what}: expected to find `{anchor}`"))
        .1;
    after
        .split_once(stop)
        .unwrap_or_else(|| panic!("{what}: expected `{stop}` after `{anchor}`"))
        .0
}

/// Parse the literal value out of a `pub const NAME: f32 = VALUE;` declaration.
///
/// A source-text equivalent of reading the constant directly: `moon-ui-gpui` has no library
/// target (see the module doc), so a numeric design token cannot be imported into an integration
/// test and compared as a real `f32` any other way.
///
/// Args:
///     source: Rust source containing the declaration.
///     name: Constant identifier to find.
///
/// Returns:
///     The parsed value, or `None` if the declaration or a valid float literal is not found.
pub fn parse_f32_const(source: &str, name: &str) -> Option<f32> {
    let needle = format!("const {name}:");
    let after_name = source.split_once(&needle)?.1;
    let after_eq = after_name.split_once('=')?.1;
    let literal = after_eq.split_once(';')?.0;
    literal.trim().parse::<f32>().ok()
}

/// Strip line comments so a substring ban cannot be satisfied by the prose explaining it.
///
/// Every ban written as a substring search has the same gotcha: `braced_body` returns COMMENTS
/// too, and the comment explaining why a call is load-bearing usually names that call — so the
/// assertion passes with the call deleted. Any subject module asserting on code text should run
/// its slice through here first.
///
/// Args:
///     body: Source slice to strip.
///
/// Returns:
///     The same text with everything from each `//` to end of line removed.
pub fn code_only(body: &str) -> String {
    body.lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Read one module FOLDER as a single text.
///
/// A module that outgrows its file becomes a directory, and every rule these tests state is about
/// the module, not about which file inside it happens to hold the code. Reading one file would let
/// a rule stop being enforced simply because someone moved a function next door — and a `contains`
/// assertion that finds nothing because the code left is indistinguishable from one that passes.
///
/// Every `.rs` file DIRECTLY in the folder is included, discovered rather than listed, so a file
/// added beside the others cannot quietly fall outside the checks. It does not recurse: a module
/// deep enough to nest sub-folders needs its own call, and a silent miss there would be the failure
/// this exists to prevent. Order is sorted for determinism — `braced_body` takes the FIRST match, so
/// a needle appearing in two files must resolve the same way on every run.
///
/// The module's own `tests.rs` is excluded. These are rules about what the code DOES, and several
/// are negative ("this module must not call X") — a unit test that names the banned call to explain
/// why it is banned would trip them.
///
/// Args:
///     rel: Folder path under `src`, such as `analytics/profit_monitor`.
///
/// Returns:
///     Every non-test Rust source in the folder, concatenated.
pub fn read_module(rel: &str) -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(rel);
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", dir.display()))
        // A skipped entry would make every assertion over this module a vacuous pass, so an
        // unreadable one is a failure, not a shrug.
        .map(|entry| {
            entry
                .unwrap_or_else(|err| panic!("failed to list {}: {err}", dir.display()))
                .path()
        })
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .filter(|path| path.file_name().is_some_and(|name| name != "tests.rs"))
        .collect();
    files.sort();
    files
        .iter()
        .map(|path| {
            fs::read_to_string(path)
                .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
                .replace("\r\n", "\n")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
