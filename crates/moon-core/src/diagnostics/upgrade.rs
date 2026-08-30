//! Adding switches introduced by a new release to a file the user already has.
//!
//! Without this, a switch added in a later version would exist only for people who delete their
//! `diagnostics.toml` — everyone else would read about a key in the release notes and not find it.
//!
//! The merge is strictly ADDITIVE and edits nothing that is already there. `toml_edit` is used
//! rather than a serde round-trip or hand-written text surgery for one reason: it keeps every byte
//! it did not change, so the user's own comments, ordering, spacing and edited values survive. A
//! serde round-trip would silently delete every comment in the file, which for this file is the
//! whole content.
//!
//! What it deliberately does NOT do:
//! - remove a key it does not recognise (it may belong to a newer build, or be the user's note);
//! - touch a file that failed to parse — see [`super::load`]. A file we cannot read is one we must
//!   not write, or a single typo would cost the user their configuration;
//! - reorder or reformat anything.

use toml_edit::{DocumentMut, Item, Table, Value};

use super::template::{KEYS, SECTIONS, comment};

/// Insert every documented key the file lacks, each with its comment, at the end of its section.
///
/// Returns the new text and the names of what was added, or `None` when the file is already
/// complete — the common case, which must not rewrite the file and must not disturb its mtime.
pub(super) fn merge_missing(text: &str) -> Option<(String, Vec<String>)> {
    let mut doc = text.parse::<DocumentMut>().ok()?;
    let mut added: Vec<String> = Vec::new();

    for section in SECTIONS {
        let created = !doc.contains_key(section.name);
        if created {
            let mut table = Table::new();
            table
                .decor_mut()
                .set_prefix(format!("\n{}\n", comment(section.doc)));
            doc.insert(section.name, Item::Table(table));
        }
        // A name that exists but is not a table means the user wrote something like `log = 1`, or
        // an inline `log = { … }`. That is their file to fix; replacing it with our table would
        // delete what they typed.
        let Some(table) = doc.get_mut(section.name).and_then(Item::as_table_mut) else {
            continue;
        };
        // Only for a table we just created, which would otherwise render its keys at the document
        // root with no `[section]` header and stop being read at all. Forcing it on a table the
        // user expressed with dotted keys (`log.balances = true`) would rewrite their formatting,
        // which this module promises never to do.
        if created {
            table.set_implicit(false);
        }

        for key in KEYS.iter().filter(|k| k.section == section.name) {
            if table.contains_key(key.key) {
                continue;
            }
            // The default is stored as TOML source so one literal serves both the rendered file and
            // this insert; an unparsable literal is a bug in `template`, not in the user's file, and
            // skipping keeps the rest of the merge working.
            // A real log line, not `debug_assert!`: this workspace sets `debug-assertions = false`
            // even in the dev profile, so an assertion here would be dead in every build. The
            // condition is also covered statically by `config::tests`, making this the backstop.
            let Ok(value) = key.default.parse::<Value>() else {
                log::warn!(
                    "diagnostics: значение по умолчанию «{}» для {}.{} не разбирается как TOML",
                    key.default,
                    key.section,
                    key.key
                );
                continue;
            };
            table.insert(key.key, Item::Value(value));
            if let Some(mut inserted) = table.key_mut(key.key) {
                inserted
                    .leaf_decor_mut()
                    .set_prefix(format!("\n{}\n", comment(key.doc)));
            }
            added.push(format!("{}.{}", section.name, key.key));
        }
    }

    if added.is_empty() {
        return None;
    }
    Some((doc.to_string(), added))
}

#[cfg(test)]
mod tests;
