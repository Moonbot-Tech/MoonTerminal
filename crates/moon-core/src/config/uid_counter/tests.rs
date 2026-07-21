//! Structural and edge-case guards for [`UidCounter`].

use super::UidCounter;
use std::path::{Path, PathBuf};

/// Every `.rs` file under this crate's `src`, as `(path, text)`.
///
/// The whole-crate sweep lets guards inspect trait implementations that need not be colocated with
/// the type. Individual tests remain responsible for documenting the syntax they recognize.
fn crate_sources() -> Vec<(PathBuf, String)> {
    /// Recursively append readable Rust sources below `dir`.
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
                out.push((path, text));
            }
        }
    }
    let mut out = Vec::new();
    walk(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut out,
    );
    assert!(!out.is_empty(), "the crate source sweep found no files");
    out
}

/// Breakage guarded: any hand-written trait implementation for `UidCounter` anywhere in
/// `moon-core`, including the constructor-bypassing traits identified in the module docs.
///
/// This conservative guard rejects every single-line `impl … for UidCounter`, even if a specific
/// trait would be harmless. Implementations split across lines, emitted by macros, or written
/// through a type alias are outside its textual matcher.
#[test]
fn no_trait_is_implemented_for_the_counter_anywhere_in_the_crate() {
    for (path, src) in crate_sources() {
        for line in src.lines() {
            let line = line.trim();
            assert!(
                !(line.starts_with("impl") && line.contains("for UidCounter")),
                "{}: `{line}` — a trait impl can rebuild the counter without a floor; \
                 see the uid_counter module docs",
                path.display()
            );
        }
    }
}

/// Return every trait named by a `#[derive(...)]` attribute in `src`.
///
/// Scans the raw text rather than line by line, because a derive attribute may span several
/// lines — which is exactly how rustfmt writes a long list, and how a stacked `#[derive(Default)]`
/// would most plausibly arrive.
fn derived_traits(src: &str) -> Vec<&str> {
    let mut traits = Vec::new();
    let mut rest = src;
    while let Some(at) = rest.find("#[derive(") {
        let after = &rest[at + "#[derive(".len()..];
        let end = after
            .find(")]")
            .expect("uid_counter.rs has an unterminated derive attribute");
        traits.extend(
            after[..end]
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty()),
        );
        rest = &after[end + 2..];
    }
    traits
}

/// Breakage guarded: a banned trait joining the derives on `UidCounter` — the same reopening as
/// the sweep above, reached through a derive rather than a hand-written impl. `Clone` and `Debug`
/// are required because `AppConfig` derives them; nothing else may join.
///
/// Reads EVERY derive in the file, because Rust lets attributes stack and a separate
/// `#[derive(Default)]` above the real one is the likeliest way one arrives. That is only sound
/// while the file holds a single type, which the first assertion pins.
#[test]
fn the_counter_derives_only_clone_and_debug() {
    let src = include_str!("../uid_counter.rs");

    let items = src
        .lines()
        .map(str::trim)
        .filter(|line| {
            let decl = line.strip_prefix("pub ").unwrap_or(line);
            decl.starts_with("struct ") || decl.starts_with("enum ") || decl.starts_with("union ")
        })
        .count();
    assert_eq!(
        items, 1,
        "this guard reads every derive in uid_counter.rs, which only tells you about the counter \
         while the file holds one type — teach it to scope by item before adding another"
    );

    let mut traits = derived_traits(src);
    traits.sort_unstable();
    assert_eq!(
        traits,
        ["Clone", "Debug"],
        "the uid counter must derive nothing that can rebuild it without a floor"
    );
}

/// Breakage guarded: the field gaining ANY visibility — `pub next`, but equally `pub(super) next`
/// or `pub(crate) next`, which would let a sibling module build or rewrite a counter by struct
/// literal and skip `new` altogether. Same reissued-uid consequence, reached without touching a
/// single trait.
#[test]
fn the_counter_field_stays_private() {
    // Matched by the field's TYPE, not by a leading `next:` — a `pub` prefix would push the name
    // off the start of the line, and the test would then fail with "no such field" instead of
    // naming the visibility that actually broke it.
    let field = include_str!("../uid_counter.rs")
        .lines()
        .map(str::trim)
        .find(|line| line.contains("next: u64"))
        .expect("uid_counter.rs must declare the `next` field");
    assert!(
        !field.contains("pub"),
        "`{field}` — the counter's field must stay private, or `new` stops being the only way in"
    );
}

/// Breakage guarded: computing `persisted.max(seen).wrapping_add(1)` would turn an
/// unrepresentable `u64::MAX` floor into 0 instead of retaining the persisted value 7.
///
/// The `+ 1` half of the same expression, and `raise_to`'s monotonicity, are already pinned by
/// `config::reconcile::uid_tests`.
#[test]
fn a_floor_without_a_successor_leaves_the_counter_alone() {
    assert_eq!(UidCounter::new(7, Some(u64::MAX)).get(), 7);
}
