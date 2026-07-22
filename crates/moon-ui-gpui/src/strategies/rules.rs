//! Strategy-field dependency rules that determine whether a field is editable or a section is
//! active from the values of OTHER fields. Rules come from `assets/param_deps.toml`
//! (`"Field" = "A=VAL;B<>VAL"`). Startup first tries the external development path, then uses the
//! bundled fallback. External-file hot reload requires the explicit
//! `MOON_STRATEGY_RULES_HOT_RELOAD` environment variable; production does not poll the filesystem
//! every second. Field names and conditions are parsed on initial load and each external reload.
//! This is a verbatim port of egui's `src/strategies/rules.rs`.
//!
//! Section activity has no separate configuration: `section_active` evaluates these dependencies
//! and keeps a section active when more than one of its fields is dependency-active.

use std::collections::HashMap;
use std::time::SystemTime;

/// External path relative to the cwd for development hot reload (`cargo run` uses the workspace root).
const EXTERNAL: &str = "assets/param_deps.toml";
/// Fallback bundled into the binary for release runs without adjacent assets.
const BUNDLED: &str = include_str!("../../../../assets/param_deps.toml");

/// Effective dependency values keyed by lowercase field name.
///
/// Stored fields are overlaid with staged edits, while schema defaults fill omitted fields.
pub type Values = HashMap<String, String>;

/// Dependency-condition operator.
#[derive(Clone, Copy)]
enum Op {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
}

/// One dependency condition: `field` (op) `value`.
#[derive(Clone)]
struct Cond {
    field: String,
    op: Op,
    value: String,
}

pub struct Rules {
    /// Lowercase field name mapped to conditions joined by `;` as logical AND.
    deps: HashMap<String, Vec<Cond>>,
    /// Modification time of the external file, used for hot reload.
    mtime: Option<SystemTime>,
}

impl Rules {
    /// Load rules from the external file when present, otherwise from the bundled fallback.
    pub fn load() -> Self {
        let mut r = Rules {
            deps: HashMap::new(),
            mtime: None,
        };
        match std::fs::read_to_string(EXTERNAL) {
            Ok(content) => {
                r.mtime = file_mtime();
                r.parse_into(&content);
            }
            Err(_) => r.parse_into(BUNDLED),
        }
        r
    }

    /// Reload the external file when it changes, returning true when a new frame is needed.
    pub fn reload_if_changed(&mut self) -> bool {
        let m = file_mtime();
        if m.is_some() && m != self.mtime {
            if let Ok(content) = std::fs::read_to_string(EXTERNAL) {
                self.mtime = m;
                self.deps.clear();
                self.parse_into(&content);
                return true;
            }
        }
        false
    }

    /// Parse manually edited `"Field" = "condition"` entries line by line.
    ///
    /// This accepts duplicate keys (the last wins), full-line `#` comments, the `[deps]` header,
    /// and quotes.
    /// Unlike strict TOML, one malformed key does not invalidate the entire file.
    fn parse_into(&mut self, content: &str) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            // Split on the FIRST `=`; any `=` inside the value follows the closing key quote.
            let Some(eq) = line.find('=') else { continue };
            let key = line[..eq].trim().trim_matches('"').trim().to_lowercase();
            let expr = line[eq + 1..].trim().trim_matches('"').trim();
            if key.is_empty() {
                continue;
            }
            self.deps.insert(key, parse_conds(expr));
        }
        log::info!(
            "strategy param rules: {} полей с зависимостями",
            self.deps.len()
        );
    }

    /// Return whether a field is active and editable under the current values.
    ///
    /// Every condition must hold; a field without a rule is active. A condition referring to a
    /// field absent from `values` is inapplicable because that field does not exist for this
    /// strategy kind, so it does not block. `selected_values` inserts every schema field with its
    /// default or an empty value, making absence mean "not part of this kind" while an unsaved
    /// field is still compared using its default.
    pub fn field_active(&self, name: &str, values: &Values) -> bool {
        match self.deps.get(&name.to_lowercase()) {
            None => true,
            Some(conds) => conds.iter().all(|c| match values.get(&c.field) {
                None => true,
                Some(v) => cond_true(c, v),
            }),
        }
    }
}

/// Evaluate condition `c` against value `v`.
///
/// `=` and `<>` compare booleans or strings; `>`, `<`, `>=`, and `<=` compare numbers. A
/// nonnumeric operand makes a numeric condition false.
fn cond_true(c: &Cond, v: &str) -> bool {
    match c.op {
        Op::Eq => value_eq(v, &c.value),
        Op::Ne => !value_eq(v, &c.value),
        _ => match (v.trim().parse::<f64>(), c.value.trim().parse::<f64>()) {
            (Ok(a), Ok(e)) => match c.op {
                Op::Gt => a > e,
                Op::Lt => a < e,
                Op::Ge => a >= e,
                Op::Le => a <= e,
                _ => true,
            },
            _ => false,
        },
    }
}

/// Interpret the core's boolean forms: `1/0`, `Yes/No`, and `true/false`.
///
/// None means the value is a number or string rather than a boolean.
fn as_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "1" | "on" => Some(true),
        "no" | "false" | "0" | "off" | "" => Some(false),
        _ => None,
    }
}

/// Compare condition values as booleans when BOTH sides are boolean forms, including `0/1`.
///
/// This makes `IgnoreVolume=NO` match the raw value `"0"`; all other values compare as strings.
fn value_eq(actual: &str, expected: &str) -> bool {
    match (as_bool(actual), as_bool(expected)) {
        (Some(a), Some(e)) => a == e,
        _ => actual.eq_ignore_ascii_case(expected),
    }
}

/// Return the external rules file's modification time, or None when the file is absent.
fn file_mtime() -> Option<SystemTime> {
    std::fs::metadata(EXTERNAL)
        .ok()
        .and_then(|m| m.modified().ok())
}

/// Parse `A=VAL;B<>VAL;C>1` into lowercase field/value conditions.
///
/// Operators are checked longest first: `<>`, `>=`, and `<=` precede `>`, `<`, and `=`.
fn parse_conds(expr: &str) -> Vec<Cond> {
    const OPS: [(&str, Op); 6] = [
        ("<>", Op::Ne),
        (">=", Op::Ge),
        ("<=", Op::Le),
        (">", Op::Gt),
        ("<", Op::Lt),
        ("=", Op::Eq),
    ];
    expr.split(';')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            OPS.iter().find_map(|&(s, op)| {
                part.find(s).map(|i| Cond {
                    field: part[..i].trim().to_lowercase(),
                    op,
                    value: part[i + s.len()..].trim().to_lowercase(),
                })
            })
        })
        .collect()
}
