//! Per-strategy edits for whitespace-separated trigger-key lists, owned by the staging actions.

use std::collections::HashMap;

use moon_core::feed::{SchemaField, SchemaFieldUi};

use super::{FieldEditKey, Key};

/// Operation on token keys, independent of any optional `=seconds` suffix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::strategies) enum ListEdit {
    Append,
    Remove,
}

/// Accept only known trigger-key lists rendered as string edits.
///
/// The streamed schema has types and widget kinds, but no list marker. Keep this allowlist
/// deliberately narrow: singular trigger outputs and arbitrary strings are not token lists.
/// MoonBot's extension-pack manual documents `TriggerByKey`, `TriggerKeysBL`, and
/// `ClearTriggerKeys` as space-separated lists; `TriggerKeys` is the issue's schema variant.
pub(in crate::strategies) fn is_list_field(field: &SchemaField) -> bool {
    field.type_name == "String"
        && field.ui == SchemaFieldUi::Edit
        && matches!(
            field.name.as_str(),
            "TriggerKeys" | "TriggerKeysBL" | "ClearTriggerKeys" | "TriggerByKey"
        )
}

/// Edit `current` by token key, preserving first-key order and letting the last entered token win.
///
/// Empty input is an exact no-op. Otherwise output is trimmed with single spaces; append updates
/// existing keys in place and adds new keys at the end, while remove ignores time suffixes.
pub(super) fn edit_tokens(current: &str, entered: &str, operation: ListEdit) -> String {
    if entered.trim().is_empty() {
        return current.to_string();
    }
    let key = |token: &str| token.split('=').next().unwrap_or_default().to_string();
    let mut tokens: Vec<&str> = Vec::new();
    let mut positions = HashMap::new();
    for token in current.split_whitespace() {
        let name = key(token);
        if let Some(&at) = positions.get(&name) {
            tokens[at] = token;
        } else {
            positions.insert(name, tokens.len());
            tokens.push(token);
        }
    }
    match operation {
        ListEdit::Append => {
            for token in entered.split_whitespace() {
                let name = key(token);
                if let Some(&at) = positions.get(&name) {
                    tokens[at] = token;
                } else {
                    positions.insert(name, tokens.len());
                    tokens.push(token);
                }
            }
        }
        ListEdit::Remove => {
            let removed: std::collections::HashSet<String> =
                entered.split_whitespace().map(key).collect();
            tokens.retain(|token| !removed.contains(&key(token)));
        }
    }
    tokens.join(" ")
}

/// Stage each target's own result, leaving unrelated drafts and unchanged values untouched.
///
/// `current` contains already-resolved draft/pending/confirmed values for the complete target
/// set. The caller validates authority and schema before passing it here. Returns changed count.
pub(super) fn stage_list_values(
    drafts: &mut HashMap<FieldEditKey, String>,
    field: &str,
    current: &[(Key, String)],
    entered: &str,
    operation: ListEdit,
) -> usize {
    let mut changed = 0;
    for ((core, id), value) in current {
        let next = edit_tokens(value, entered, operation);
        if next != *value {
            drafts.insert((*core, *id, field.to_string()), next);
            changed += 1;
        }
    }
    changed
}

#[cfg(test)]
mod tests;
