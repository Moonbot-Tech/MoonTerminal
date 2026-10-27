//! Compare real Rust module/item gates with direct dependency availability, without a Mac build.
//! This is deliberately a lexical contract, not macro expansion or Rust name resolution.

use super::support::{Path, PathBuf, code_only, fs};
use std::collections::{BTreeMap, BTreeSet};

mod tests;

/// Platforms distinguished by the contract; the last entry prevents unknown OS gates narrowing.
const SYSTEMS: [&str; 4] = ["windows", "macos", "linux", "other"];
/// A conservative set of targets on which source can compile.
type Targets = [bool; 4];
/// Local bindings retain the targets on which their declarations are present.
type Names = BTreeMap<String, Targets>;

/// Count independent declarations so excluding one import preserves every other binding.
type NameCounts = BTreeMap<String, [usize; 4]>;

/// Predeclared bindings and each import's contribution, collected once per lexical scope.
struct ScopeNames {
    counts: NameCounts,
    imports: BTreeMap<usize, Names>,
}

impl ScopeNames {
    /// Add one declaration without losing overlapping declarations on the same target.
    fn declare(&mut self, name: String, targets: Targets) {
        let counts = self.counts.entry(name).or_insert([0; 4]);
        for n in 0..4 {
            counts[n] += usize::from(targets[n]);
        }
    }

    /// Resolve bindings while optionally subtracting exactly one import's contribution.
    fn targets(&self, name: &str, excluded: Targets, enclosing: &Names) -> Targets {
        let counts = self.counts.get(name).copied().unwrap_or([0; 4]);
        let outer = enclosing.get(name).copied().unwrap_or([false; 4]);
        std::array::from_fn(|n| outer[n] || counts[n] > usize::from(excluded[n]))
    }
}

/// Tokenize source, preserving quoted attribute values but discarding comments and raw strings.
fn tokens(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        if bytes[i..].starts_with(b"//") {
            let end = source[i..].find('\n').map_or(bytes.len(), |n| i + n);
            i += source[i..end].len() - code_only(&source[i..end]).len();
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if bytes[i] == b'r' && {
            let mut n = i + 1;
            while n < bytes.len() && bytes[n] == b'#' {
                n += 1;
            }
            n < bytes.len() && bytes[n] == b'"'
        } {
            i += 1;
            while bytes[i] == b'#' {
                i += 1;
            }
            let end = format!("\"{}", "#".repeat(i - start - 1));
            i += 1;
            i = source[i..]
                .find(&end)
                .map_or(bytes.len(), |n| i + n + end.len());
            out.push("\"\"".into());
        } else if bytes[i] == b'"'
            || (bytes[i] == b'\''
                && (bytes.get(i + 2) == Some(&b'\'') || bytes.get(i + 1) == Some(&b'\\')))
        {
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                } else if bytes[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            out.push(source[start..i].into());
        } else if bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            out.push(source[start..i].into());
        } else {
            i += 1;
            if !bytes[start].is_ascii_whitespace() {
                // Punctuation is ASCII; skip Unicode rather than slicing inside a code point.
                if bytes[start].is_ascii() {
                    out.push(source[start..i].into());
                }
            }
        }
    }
    out
}

/// Find the balanced delimiter closing a token group, failing loudly on malformed source.
fn close(ts: &[String], start: usize) -> usize {
    let closing = match ts[start].as_str() {
        "(" => ")",
        "[" => "]",
        "{" => "}",
        _ => panic!("expected group"),
    };
    let mut i = start + 1;
    while i < ts.len() {
        if ts[i] == closing {
            return i;
        }
        if matches!(ts[i].as_str(), "(" | "[" | "{") {
            i = close(ts, i);
        }
        i += 1;
    }
    panic!("unclosed {}", ts[start]);
}

/// Evaluate known OS predicates; unknown predicates remain unknown even under negation.
fn predicate(ts: &[String], os: &str) -> Option<bool> {
    match ts {
        [name] if name == "windows" => Some(os == "windows"),
        [name] if name == "unix" => Some(os == "macos" || os == "linux" || os == "other"),
        [name, eq, value]
            if name == "target_os"
                && eq == "="
                && matches!(value.as_str(), "\"windows\"" | "\"macos\"" | "\"linux\"") =>
        {
            Some(value == &format!("\"{os}\""))
        }
        [name, rest @ ..]
            if matches!(name.as_str(), "any" | "all" | "not")
                && rest.first().is_some_and(|s| s == "(") =>
        {
            let args = &rest[1..rest.len() - 1];
            let mut values = Vec::new();
            let mut start = 0;
            let mut i = 0;
            while i < args.len() {
                if matches!(args[i].as_str(), "(" | "[") {
                    i = close(args, i);
                } else if args[i] == "," {
                    values.push(predicate(&args[start..i], os));
                    start = i + 1;
                }
                i += 1;
            }
            if start < args.len() {
                values.push(predicate(&args[start..], os));
            }
            match name.as_str() {
                "not" => values.first().copied().flatten().map(|v| !v),
                "all" if values.contains(&Some(false)) => Some(false),
                "any" if values.contains(&Some(true)) => Some(true),
                _ if values.contains(&None) => None,
                "all" => Some(true),
                _ => Some(false),
            }
        }
        _ => None,
    }
}

/// Classify a cfg expression conservatively, so unrecognized conditions never hide a use.
fn targets(ts: &[String]) -> Targets {
    SYSTEMS.map(|os| predicate(ts, os).unwrap_or(true))
}

/// Parse direct dependency tables; Cargo's dependency KEY is the identifier even with package renames.
fn dependencies(manifest: &str) -> BTreeMap<String, Targets> {
    let mut result = BTreeMap::new();
    let mut available = None;
    let mut depth = 0i32;
    for line in manifest.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') && depth == 0 {
            available = if line == "[dependencies]" {
                Some([true; 4])
            } else if line.starts_with("[target.") && line.ends_with(".dependencies]") {
                let cfg = line
                    .strip_prefix("[target.")
                    .unwrap()
                    .strip_suffix(".dependencies]")
                    .unwrap()
                    .trim_matches(['\'', '"']);
                let ts = tokens(cfg);
                let known = ts.first().is_some_and(|s| s == "cfg");
                Some(if known {
                    SYSTEMS.map(|os| predicate(&ts[2..ts.len() - 1], os) == Some(true))
                } else {
                    [false; 4]
                })
            } else {
                None
            };
            continue;
        }
        if let Some(available) = available {
            if depth == 0
                && let Some((key, _)) = line.split_once('=')
            {
                let id = key.trim().trim_matches(['\'', '"']).replace('-', "_");
                let entry = result.entry(id).or_insert([false; 4]);
                for i in 0..4 {
                    entry[i] |= available[i];
                }
            }
            for token in tokens(line) {
                match token.as_str() {
                    "[" | "{" => depth += 1,
                    "]" | "}" => depth -= 1,
                    _ => {}
                }
            }
        }
    }
    result
}

/// Collect explicit use-tree bindings, including grouped imports, aliases, and `self` leaves.
fn import_names(ts: &[String], prefix: Option<&str>, names: &mut BTreeSet<String>) {
    let mut i = 0;
    let mut last = None;
    while i < ts.len() {
        match ts[i].as_str() {
            "{" => {
                let end = close(ts, i);
                import_names(&ts[i + 1..end], last, names);
                i = end;
                last = None;
            }
            "as" => {
                last = ts.get(i + 1).map(String::as_str);
                i += 1;
            }
            "," => {
                if let Some(name) = last.take().filter(|name| *name != "_") {
                    names.insert(name.to_owned());
                }
                last = None;
            }
            ":" => {}
            "*" => last = None,
            "self" => last = prefix,
            name => last = Some(name),
        }
        i += 1;
    }
    if let Some(name) = last.filter(|name| *name != "_") {
        names.insert(name.to_owned());
    }
}

/// Union declarations so a gated shadow does not erase an enclosing binding on other targets.
fn merge_names(names: &mut Names, declarations: Names) {
    for (name, targets) in declarations {
        let available = names.entry(name).or_insert([false; 4]);
        for n in 0..4 {
            available[n] |= targets[n];
        }
    }
}

/// Predeclare cfg-aware scope items once, retaining each import's removable contribution.
fn local_names(ts: &[String]) -> ScopeNames {
    let mut names = ScopeNames {
        counts: NameCounts::new(),
        imports: BTreeMap::new(),
    };
    let mut inherited = [true; 4];
    let mut active = inherited;
    let mut i = 0;
    while i < ts.len() {
        if ts[i] == "#" && ts.get(i + 1).is_some_and(|s| s == "[" || s == "!") {
            let inner = ts[i + 1] == "!";
            let open = i + if inner { 2 } else { 1 };
            let end = close(ts, open);
            let attr = &ts[open + 1..end];
            if attr.first().is_some_and(|s| s == "cfg") {
                for (n, os) in SYSTEMS.iter().enumerate() {
                    // Unknown gates cannot prove that a shadowing declaration exists.
                    active[n] &= predicate(&attr[2..attr.len() - 1], os) == Some(true);
                }
                if inner {
                    inherited = active;
                }
            }
            i = end + 1;
            continue;
        }
        if ts[i] == "use" && ts.get(i + 1).is_some_and(|token| token != "<") {
            let end = (i + 1..ts.len()).find(|&n| ts[n] == ";").unwrap();
            let mut bindings = BTreeSet::new();
            import_names(&ts[i + 1..end], None, &mut bindings);
            for name in &bindings {
                names.declare(name.clone(), active);
            }
            names
                .imports
                .insert(i, bindings.into_iter().map(|name| (name, active)).collect());
            i = end;
            active = inherited;
        } else if matches!(
            ts[i].as_str(),
            "mod" | "struct" | "enum" | "union" | "trait" | "type"
        ) && let Some(name) = ts.get(i + 1)
        {
            names.declare(name.clone(), active);
        } else if matches!(ts[i].as_str(), "{" | "(" | "[") {
            let end = close(ts, i);
            if ts[i] == "{" {
                active = inherited;
            }
            i = end;
        } else if ts[i] == ";" {
            active = inherited;
        }
        i += 1;
    }
    names
}

/// A module traversal keeps declaration gates and local names attached to descendants.
struct Contract {
    dependencies: BTreeMap<String, Targets>,
    failures: Vec<String>,
    files: usize,
}

impl Contract {
    /// Read one reachable module file; derive Rust's normal child directory from its file stem.
    fn file(&mut self, path: &Path, inherited: Targets, module_dir: Option<PathBuf>) {
        self.file_in_scope(path, inherited, module_dir, &Names::new());
    }

    /// Carry enclosing declarations across file boundaries, including relocated modules.
    fn file_in_scope(
        &mut self,
        path: &Path,
        inherited: Targets,
        module_dir: Option<PathBuf>,
        names: &Names,
    ) {
        let source = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        self.files += 1;
        let dir = module_dir.unwrap_or_else(|| {
            if matches!(
                path.file_name().and_then(|s| s.to_str()),
                Some("mod.rs" | "main.rs" | "lib.rs")
            ) {
                path.parent().unwrap().to_path_buf()
            } else {
                path.with_extension("")
            }
        });
        self.scope(
            &tokens(&source),
            path,
            &dir,
            path.parent().unwrap(),
            inherited,
            names,
        );
    }

    /// Scan scopes with item/block cfg attributes; imports and fully qualified paths share the check.
    fn walk(&mut self, ts: &[String], path: &Path, dir: &Path, inherited: Targets) {
        self.scope(
            ts,
            path,
            dir,
            path.parent().unwrap(),
            inherited,
            &Names::new(),
        );
    }

    /// Resolve path attributes against the containing inline module, separately from file children.
    fn scope(
        &mut self,
        ts: &[String],
        path: &Path,
        dir: &Path,
        attribute_dir: &Path,
        inherited: Targets,
        names: &Names,
    ) {
        let enclosing_names = names;
        let mut names = names.clone();
        let declarations = local_names(ts);
        merge_names(
            &mut names,
            declarations
                .counts
                .keys()
                .map(|name| {
                    (
                        name.clone(),
                        declarations.targets(name, [false; 4], enclosing_names),
                    )
                })
                .collect(),
        );
        let mut inherited = inherited;
        let mut i = 0;
        let mut active = inherited;
        let mut relocated = None;
        let mut import_bindings = None;
        while i < ts.len() {
            if ts[i] == "#" && ts.get(i + 1).is_some_and(|s| s == "[" || s == "!") {
                let inner = ts[i + 1] == "!";
                let open = i + if inner { 2 } else { 1 };
                let end = close(ts, open);
                let attr = &ts[open + 1..end];
                if attr.first().is_some_and(|s| s == "cfg") {
                    let gate = targets(&attr[2..attr.len() - 1]);
                    for n in 0..4 {
                        active[n] &= gate[n];
                    }
                    if inner {
                        inherited = active;
                    }
                } else if let [name, eq, value] = attr
                    && name == "path"
                    && eq == "="
                {
                    relocated = Some(value.trim_matches('"').to_owned());
                }
                i = end + 1;
                continue;
            }
            if ts[i] == "mod" && ts.get(i + 2).is_some_and(|s| s == ";" || s == "{") {
                let name = &ts[i + 1];
                if ts[i + 2] == ";" {
                    let has_path = relocated.is_some();
                    let child = if let Some(rel) = relocated.take() {
                        attribute_dir.join(rel)
                    } else if dir.join(format!("{name}.rs")).exists() {
                        dir.join(format!("{name}.rs"))
                    } else {
                        dir.join(name).join("mod.rs")
                    };
                    // Optional private overlays are not repository source and cannot be checked here.
                    if child.exists() {
                        let child_dir = has_path.then(|| child.parent().unwrap().to_path_buf());
                        self.file_in_scope(&child, active, child_dir, &names);
                    } else if !child.components().any(|part| part.as_os_str() == "private") {
                        panic!("{}: missing module {}", path.display(), child.display());
                    }
                    i += 3;
                } else {
                    let end = close(ts, i + 2);
                    let inline_dir = relocated
                        .take()
                        .map_or_else(|| dir.join(name), |rel| attribute_dir.join(rel));
                    self.scope(
                        &ts[i + 3..end],
                        path,
                        &inline_dir,
                        &inline_dir,
                        active,
                        &names,
                    );
                    i = end + 1;
                }
                active = inherited;
                relocated = None;
                continue;
            }
            // A grouped local import such as super::{windows::foo} is not an external crate.
            if ts[i] == "use"
                && ts.get(i + 1).is_some_and(|s| {
                    matches!(
                        s.as_str(),
                        "crate" | "self" | "super" | "std" | "core" | "alloc"
                    )
                })
            {
                while i < ts.len() && ts[i] != ";" {
                    i += 1;
                }
                continue;
            }
            if ts[i] == "use" && ts.get(i + 1).is_some_and(|token| token != "<") {
                // Only this import's bindings change; never rescan or clone the entire scope.
                let mut restore = Names::new();
                for (name, targets) in &declarations.imports[&i] {
                    restore.insert(name.clone(), names[name]);
                    names.insert(
                        name.clone(),
                        declarations.targets(name, *targets, enclosing_names),
                    );
                }
                import_bindings = Some(restore);
            }
            let nested_path = i >= 3
                && ts[i - 1] == ":"
                && ts[i - 2] == ":"
                && ts[i - 3]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_');
            let direct_import = (i > 0 && ts[i - 1] == "use")
                || (i >= 2 && ts[i - 2] == "extern" && ts[i - 1] == "crate");
            let absolute = i >= 2 && ts[i - 2] == ":" && ts[i - 1] == ":";
            // An import's own bindings cannot make its external source appear local.
            let local = if direct_import {
                (i > 0 && ts[i - 1] == "use" && import_bindings.is_some()).then_some(&names)
            } else {
                Some(&names)
            }
            .and_then(|scope| scope.get(&ts[i]))
            .copied()
            .unwrap_or([false; 4]);
            if !nested_path
                && (direct_import
                    || (ts.get(i + 1).is_some_and(|s| s == ":")
                        && ts.get(i + 2).is_some_and(|s| s == ":")))
                && let Some(available) = self.dependencies.get(&ts[i])
            {
                let missing: Vec<_> = (0..4)
                    .filter(|&n| active[n] && !available[n] && (absolute || !local[n]))
                    .collect();
                if !missing.is_empty() {
                    let table = if active == [true; 4] {
                        "[dependencies]".into()
                    } else {
                        format!(
                            "[target.'cfg(target_os = \"{}\")'.dependencies]",
                            SYSTEMS[missing[0]]
                        )
                    };
                    self.failures.push(format!(
                        "{}: {} is unavailable; add it to {table}",
                        path.display(),
                        ts[i]
                    ));
                }
            }
            if matches!(ts[i].as_str(), "{" | "(" | "[") {
                let end = close(ts, i);
                self.scope(&ts[i + 1..end], path, dir, attribute_dir, active, &names);
                if ts[i] == "{" {
                    active = inherited;
                    relocated = None;
                }
                i = end + 1;
            } else {
                if ts[i] == ";" {
                    active = inherited;
                    relocated = None;
                    if let Some(restore) = import_bindings.take() {
                        names.extend(restore);
                    }
                }
                i += 1;
            }
        }
    }
}

/// Removing a macOS dependency while leaving its macOS use must fail Windows CI before release.
#[test]
fn platform_dependencies_cover_reachable_source() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut contract = Contract {
        dependencies: dependencies(&fs::read_to_string(root.join("Cargo.toml")).unwrap()),
        failures: Vec::new(),
        files: 0,
    };
    // The full-tree walk IS the coverage; optimizing scope bindings must never prune modules.
    contract.file(&root.join("src/main.rs"), [true; 4], None);
    contract.failures.sort();
    contract.failures.dedup();
    assert!(contract.files > 1, "module traversal must not be vacuous");
    assert!(
        contract.failures.is_empty(),
        "{}",
        contract.failures.join("\n")
    );
}
