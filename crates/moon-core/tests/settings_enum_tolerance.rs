//! Every enum a settings file can hold must be read through `config::tolerant::or_default`.
//!
//! An enum field that fails to deserialize fails the WHOLE `settings.toml`, which the loader
//! quarantines to `.bak` and replaces with defaults. A variant added by a newer build therefore
//! costs a user who rolls back every setting — unless the field degrades on its own. This contract
//! walks the field types reachable from `SettingsFile` in the crate's source and fails on any
//! enum-typed field that does not carry the adapter, so the next enum added to the settings tree
//! cannot ship without it.
//!
//! A field carrying the adapter is not descended into: whatever it holds degrades as one unit.
//! A struct read `#[serde(from = "X")]` is checked through `X`, the shape serde actually reads.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// Matches `tolerant::or_default` and the `tolerant_*` wrappers over `tolerant::or_else`.
const ADAPTER: &str = "tolerant";

/// Collect every `.rs` file under `dir`.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Return the text between the brace that opens at or after `from` and its matching close.
fn braced_body(text: &str, from: usize) -> Option<&str> {
    let open = from + text[from..].find('{')?;
    let mut depth = 0usize;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[open + 1..open + i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Return the identifier that starts at byte `at`.
fn ident_at(text: &str, at: usize) -> &str {
    let end = text[at..]
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map_or(text.len(), |n| at + n);
    &text[at..end]
}

/// Strip any visibility (`pub`, `pub(crate)`, `pub(super)`, `pub(in path)`) off a declaration.
fn strip_visibility(decl: &str) -> &str {
    if let Some(rest) = decl.strip_prefix("pub(") {
        return rest
            .split_once(')')
            .map_or(decl, |(_, tail)| tail.trim_start());
    }
    decl.strip_prefix("pub ").unwrap_or(decl)
}

/// Return the `from = "X"` target in the attribute lines right above byte `line_start`.
fn serde_from(text: &str, line_start: usize) -> Option<String> {
    let attrs = text[..line_start]
        .lines()
        .rev()
        .map(str::trim)
        .take_while(|l| l.starts_with("#[") || l.starts_with("//"))
        .filter(|l| l.starts_with("#[serde("));
    for attr in attrs {
        if let Some(at) = attr.find("from = \"") {
            let rest = &attr[at + "from = \"".len()..];
            return rest.split('"').next().map(str::to_string);
        }
    }
    None
}

/// Index the crate's braced struct bodies and enum names; a struct read through
/// `#[serde(from = "X")]` is indexed under the name `X`'s body is read by.
fn index_sources(src: &Path) -> (HashMap<String, String>, BTreeSet<String>) {
    let mut files = Vec::new();
    rust_files(src, &mut files);
    let mut structs = HashMap::new();
    let mut read_via = Vec::new();
    let mut enums = BTreeSet::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("read source");
        for line_start in std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)) {
            let line = &text[line_start..];
            let trimmed = line.trim_start();
            let indent = line.len() - trimmed.len();
            let decl = strip_visibility(trimmed);
            let decl_at = line_start + indent + (trimmed.len() - decl.len());
            if let Some(rest) = decl.strip_prefix("struct ") {
                let name_at = decl_at + (decl.len() - rest.len());
                let name = ident_at(&text, name_at);
                let after = name_at + name.len();
                if let Some(target) = serde_from(&text, line_start) {
                    read_via.push((name.to_string(), target));
                } else if text[after..].trim_start().starts_with('{') {
                    if let Some(body) = braced_body(&text, after) {
                        structs.insert(name.to_string(), body.to_string());
                    }
                }
            } else if let Some(rest) = decl.strip_prefix("enum ") {
                let name_at = decl_at + (decl.len() - rest.len());
                enums.insert(ident_at(&text, name_at).to_string());
            }
        }
    }
    for (name, target) in read_via {
        let body = structs
            .get(&target)
            .unwrap_or_else(|| panic!("{name} is read from {target}, which was not found"))
            .clone();
        structs.insert(name, body);
    }
    (structs, enums)
}

/// One field of a braced struct: its attributes, name and type text.
struct Field {
    attrs: String,
    name: String,
    ty: String,
}

/// Split a struct body into fields, skipping comments and keeping each field's attributes.
fn fields(body: &str) -> Vec<Field> {
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    let mut attrs = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() || bytes[i] == b',' {
            i += 1;
        } else if body[i..].starts_with("//") {
            i = body[i..].find('\n').map_or(bytes.len(), |n| i + n);
        } else if body[i..].starts_with("#[") {
            let mut depth = 0usize;
            let start = i;
            while i < bytes.len() {
                match bytes[i] {
                    b'[' => depth += 1,
                    b']' => {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            attrs.push_str(&body[start..i]);
        } else {
            let colon = i + body[i..].find(':').expect("field without a type");
            let name = body[i..colon]
                .split_whitespace()
                .last()
                .unwrap_or("")
                .to_string();
            let mut depth = 0i32;
            let mut end = colon + 1;
            while end < bytes.len() {
                match bytes[end] {
                    b'<' | b'[' | b'(' => depth += 1,
                    b'>' | b']' | b')' => depth -= 1,
                    b',' if depth == 0 => break,
                    _ => {}
                }
                end += 1;
            }
            out.push(Field {
                attrs: std::mem::take(&mut attrs),
                name,
                ty: body[colon + 1..end].trim().to_string(),
            });
            i = end;
        }
    }
    out
}

/// Type names inside a field's type text, minus containers and constants.
fn type_names(ty: &str) -> Vec<&str> {
    ty.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| t.starts_with(|c: char| c.is_ascii_uppercase()))
        .filter(|t| {
            !t.chars()
                .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
        })
        .filter(|t| !matches!(*t, "Option" | "Vec" | "String" | "Box" | "Self"))
        .collect()
}

#[test]
fn every_settings_enum_goes_through_the_tolerant_adapter() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let (structs, enums) = index_sources(&src);

    let mut visited = BTreeSet::new();
    let mut queue = vec!["SettingsFile".to_string()];
    let mut violations = Vec::new();
    while let Some(owner) = queue.pop() {
        if !visited.insert(owner.clone()) {
            continue;
        }
        let body = structs
            .get(&owner)
            .unwrap_or_else(|| panic!("struct {owner} not found in moon-core/src"));
        for field in fields(body) {
            if field.attrs.contains(ADAPTER) || field.attrs.contains("skip_deserializing") {
                continue;
            }
            for name in type_names(&field.ty) {
                if enums.contains(name) {
                    violations.push(format!("{owner}::{}: {}", field.name, field.ty));
                } else if structs.contains_key(name) {
                    queue.push(name.to_string());
                }
            }
        }
    }

    for expected in [
        "SettingsFile",
        "ServerMeta",
        "GroupConfig",
        "GroupExitSettings",
    ] {
        assert!(
            visited.contains(expected),
            "the walk never reached {expected}; the source parser is broken, not the settings"
        );
    }
    assert!(
        violations.is_empty(),
        "settings enum fields without `#[serde(default, deserialize_with = \
         \"crate::config::tolerant::or_default\")]` - an unknown value in any of them would \
         quarantine the whole settings.toml:\n{}",
        violations.join("\n")
    );
}
