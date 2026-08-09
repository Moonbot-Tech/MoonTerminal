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
