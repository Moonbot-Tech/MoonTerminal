//! Strategy-field dependency rules — whether a field is in effect given the values of OTHER
//! fields. The rules come from `assets/param_deps.toml` (`"Field" = "A=VAL;B<>VAL"`): a field is in
//! effect only when every condition of its rule holds, as the Strategies window greys it out
//! otherwise.
//!
//! The parsing and the evaluation live here so that a model can ask "is this field switched on"
//! without a UI (`db::tuner::ticks::unmodelled`, whose caller reads the file on every load of the
//! tuner's axis — so an edit reaches the tuner on its next load, and an open Strategies window only
//! under its development hot reload). The window keeps its own loading and that hot reload on top
//! of [`FieldDeps`]. Moved verbatim from the window's `strategies/rules.rs`, itself a port of
//! egui's.

use std::collections::HashMap;

/// External path relative to the cwd for development hot reload (`cargo run` uses the workspace
/// root).
pub const EXTERNAL: &str = "assets/param_deps.toml";
/// Fallback bundled into the binary for release runs without adjacent assets.
const BUNDLED: &str = include_str!("../../../../assets/param_deps.toml");

/// Effective dependency values keyed by lowercase field name.
///
/// Stored fields are overlaid with staged edits, while schema defaults fill omitted fields.
pub type Values = HashMap<String, String>;

/// Dependency-condition operator.
#[derive(Clone, Copy, Debug)]
enum Op {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
}

/// One dependency condition: `field` (op) `value`.
#[derive(Clone, Debug)]
struct Cond {
    field: String,
    op: Op,
    value: String,
}

/// The parsed rules: each lowercase field name mapped to its conditions, joined by `;` as a
/// logical AND.
#[derive(Clone, Debug, Default)]
pub struct FieldDeps {
    deps: HashMap<String, Vec<Cond>>,
}

impl FieldDeps {
    /// The rules from the external file when present, otherwise from the bundled fallback.
    pub fn load() -> Self {
        match std::fs::read_to_string(EXTERNAL) {
            Ok(content) => Self::parse(&content),
            Err(_) => Self::bundled(),
        }
    }

    /// The rules bundled into the binary.
    pub fn bundled() -> Self {
        Self::parse(BUNDLED)
    }

    /// Parse manually edited `"Field" = "condition"` entries line by line.
    ///
    /// This accepts duplicate keys (the last wins), full-line `#` comments, the `[deps]` header,
    /// and quotes.
    /// Unlike strict TOML, one malformed key does not invalidate the entire file.
    pub fn parse(content: &str) -> Self {
        let mut deps = HashMap::new();
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
            deps.insert(key, parse_conds(expr));
        }
        Self { deps }
    }

    /// How many fields carry a rule.
    pub fn len(&self) -> usize {
        self.deps.len()
    }

    /// Whether no field carries a rule.
    pub fn is_empty(&self) -> bool {
        self.deps.is_empty()
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

    /// The fields `name`'s rule reads, lowercase — what a caller must have the values of for
    /// [`Self::field_active`] to answer on them rather than on their absence.
    pub fn conditions_of(&self, name: &str) -> impl Iterator<Item = &str> {
        self.deps
            .get(&name.to_lowercase())
            .into_iter()
            .flatten()
            .map(|c| c.field.as_str())
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
pub fn as_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "1" | "on" => Some(true),
        "no" | "false" | "0" | "off" | "" => Some(false),
        _ => None,
    }
}

/// Compare condition values as booleans when BOTH sides are boolean forms, including `0/1`.
///
/// This makes `IgnoreVolume=NO` match the raw value `"0"`; all other values compare as strings.
pub fn value_eq(actual: &str, expected: &str) -> bool {
    match (as_bool(actual), as_bool(expected)) {
        (Some(a), Some(e)) => a == e,
        _ => actual.eq_ignore_ascii_case(expected),
    }
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

#[cfg(test)]
mod tests;
