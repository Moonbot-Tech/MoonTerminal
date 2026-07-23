//! Pure strategy-tree operation logic for create, rename, copy, paste, move, and delete workflows.
//! It has no UI or `cx`, only calculations over `StrategyRow` and kind schemas (`SchemaKind`).
//! Results are intents (`NewStrategy` or `(id, new path)` lists) that the dispatch layer converts
//! into `moon-core` commands.
//!
//! A folder exists only as a prefix of strategy paths; the data model has no empty folders.
//! Every operation therefore edits `folder_path` or a row set.

use std::collections::HashSet;

use moon_core::feed::{SchemaKind, StrategyRow};

/// Field name through which moonproto stores `StrategySnapshot::strategy_name`.
pub const STRATEGY_NAME_FIELD: &str = "StrategyName";

/// Iterates nonempty path segments using `/` and `\` as separators without allocating.
/// This is the window-wide source of path splitting for trees, counts, expansion, and operations.
pub fn path_segments(path: &str) -> impl Iterator<Item = &str> {
    path.split(['/', '\\']).filter(|s| !s.is_empty())
}

/// Splits a folder path into owned segments through [`path_segments`].
pub fn split_path(path: &str) -> Vec<String> {
    path_segments(path).map(str::to_string).collect()
}

/// Joins path segments using canonical `/` separators.
pub fn join_path(parts: &[String]) -> String {
    parts.join("/")
}

/// Returns whether `path` starts with `prefix` segment by segment, preserving data case.
fn starts_with(path: &[String], prefix: &[String]) -> bool {
    path.len() >= prefix.len() && prefix.iter().zip(path).all(|(a, b)| a == b)
}

/// Returns every row at or below a path prefix.
pub fn rows_under<'a>(rows: &'a [StrategyRow], prefix: &[String]) -> Vec<&'a StrategyRow> {
    rows.iter()
        .filter(|r| starts_with(&split_path(&r.folder_path), prefix))
        .collect()
}

/// Returns whether every affected strategy is disabled (`!checked`), as deletion requires.
pub fn all_off(rows: &[&StrategyRow]) -> bool {
    rows.iter().all(|r| !r.checked)
}

// --- Creation -------------------------------------------------------------

/// New strategy intent containing its kind, folder, and fields, with its name in `StrategyName`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewStrategy {
    pub kind_ordinal: u8,
    pub folder_path: String,
    pub fields: Vec<(String, String)>,
}

/// Returns schema defaults for every field in a kind, using an empty string when absent.
pub fn default_fields(kind: &SchemaKind) -> Vec<(String, String)> {
    kind.sections
        .iter()
        .flat_map(|s| &s.fields)
        .map(|f| (f.name.clone(), f.default.clone().unwrap_or_default()))
        .collect()
}

/// Replaces a named field value or appends it when absent.
pub fn set_field(fields: &mut Vec<(String, String)>, name: &str, value: &str) {
    if let Some(slot) = fields.iter_mut().find(|(n, _)| n == name) {
        slot.1 = value.to_string();
    } else {
        fields.push((name.to_string(), value.to_string()));
    }
}

/// Builds a named strategy of the requested kind from its schema defaults.
pub fn new_strategy(kind: &SchemaKind, name: &str, folder_path: &str) -> NewStrategy {
    let mut fields = default_fields(kind);
    set_field(&mut fields, STRATEGY_NAME_FIELD, name);
    // Moonbot represents the strategy kind in `SignalType`; during sync the server reconstructs
    // the snapshot kind byte from it. See `feed/live/commands.rs`. Without this explicit value, a
    // newly created Volumes strategy returned with the schema's default SignalType, Drops, ignoring
    // the kind selected in the dialog.
    set_field(&mut fields, "SignalType", &kind.name);
    NewStrategy {
        kind_ordinal: kind.ordinal,
        folder_path: folder_path.to_string(),
        fields,
    }
}

// --- Copy and paste -------------------------------------------------------

/// Clipboard item containing source strategy data rather than a core reference, plus a path
/// relative to the copy base so it can be pasted into any core or folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipItem {
    pub kind_ordinal: u8,
    /// Kind name serialized as clipboard metadata for text round-tripping.
    pub kind: String,
    pub name: String,
    /// Path below the copy base; empty means the clipboard root.
    pub rel_path: Vec<String>,
    pub fields: Vec<(String, String)>,
}

fn clip_with_base(rows: &[&StrategyRow], base: &[String]) -> Vec<ClipItem> {
    rows.iter()
        .map(|r| {
            let path = split_path(&r.folder_path);
            let rel = path.get(base.len()..).unwrap_or(&[]).to_vec();
            ClipItem {
                kind_ordinal: r.kind_ordinal,
                kind: r.kind.clone(),
                name: r.name.clone(),
                rel_path: rel,
                fields: r.fields.clone(),
            }
        })
        .collect()
}

/// Copies selected strategies flat, with an empty `rel_path` for every item.
///
/// Paste therefore places each copy directly in the target folder. Original paths are discarded
/// because a multi-selection can span folders and users expect copies at the chosen destination.
pub fn copy_rows(rows: &[&StrategyRow]) -> Vec<ClipItem> {
    rows.iter()
        .map(|r| ClipItem {
            kind_ordinal: r.kind_ordinal,
            kind: r.kind.clone(),
            name: r.name.clone(),
            rel_path: Vec::new(),
            fields: r.fields.clone(),
        })
        .collect()
}

/// Copies a folder relative to its parent so paste preserves the folder name like a file manager.
pub fn copy_folder(rows: &[StrategyRow], folder_prefix: &[String]) -> Vec<ClipItem> {
    let under = rows_under(rows, folder_prefix);
    let parent_len = folder_prefix.len().saturating_sub(1);
    clip_with_base(&under, &folder_prefix[..parent_len])
}

/// Returns a name without any trailing ` (copy)` or ` (N)` copy suffixes.
///
/// For example, both `S (copy) (copy)` and `S (copy) (2)` reduce to `S`. A name made entirely of
/// suffixes is left unchanged.
fn base_name(name: &str) -> &str {
    let mut s = name.trim_end();
    loop {
        let Some(open) = s.rfind(" (") else { break };
        let Some(inner) = s[open + 2..].strip_suffix(')') else {
            break;
        };
        let is_copy_suffix =
            inner == "copy" || (!inner.is_empty() && inner.bytes().all(|b| b.is_ascii_digit()));
        if !is_copy_suffix {
            break;
        }
        let head = s[..open].trim_end();
        if head.is_empty() {
            break;
        }
        s = head;
    }
    s
}

/// Returns `desired` unchanged when it is free.
/// On collision, removes existing suffixes through [`base_name`] and returns `Base (N)` with the
/// smallest free `N` starting at two, preventing nested copy suffixes.
pub fn unique_name(taken: &HashSet<String>, desired: &str) -> String {
    if !taken.contains(desired) {
        return desired.to_string();
    }
    let base = base_name(desired);
    for n in 2.. {
        let cand = format!("{base} ({n})");
        if !taken.contains(&cand) {
            return cand;
        }
    }
    unreachable!()
}

/// Plans clipboard paste into a target folder, creating a uniquely named strategy per item.
///
/// Collisions within the batch are included. `taken_names` contains names already used anywhere
/// in the target core because Moonbot strategy names are global across its folders.
pub fn paste_plan(
    clip: &[ClipItem],
    target: &[String],
    taken_names: &HashSet<String>,
) -> Vec<NewStrategy> {
    let mut taken = taken_names.clone();
    let mut out = Vec::with_capacity(clip.len());
    for item in clip {
        let name = unique_name(&taken, &item.name);
        taken.insert(name.clone());
        let mut full = target.to_vec();
        full.extend(item.rel_path.iter().cloned());
        let mut fields = item.fields.clone();
        set_field(&mut fields, STRATEGY_NAME_FIELD, &name);
        out.push(NewStrategy {
            kind_ordinal: item.kind_ordinal,
            folder_path: join_path(&full),
            fields,
        });
    }
    out
}

// --- Text clipboard for editors and user-to-user sharing ------------------

/// Escapes a line-format field value by mapping `\` to `\\` and newlines to `\n`.
fn escape_value(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for ch in v.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            other => out.push(other),
        }
    }
    out
}

fn unescape_value(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(ch) = it.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Serializes the clipboard as text alongside the internal representation.
///
/// Each strategy becomes a `[Strategy]` block with one `Key=Value` per line. The format is
/// self-contained for [`clip_from_text`] in any core or terminal instance, allowing an entire
/// strategy folder to pass through a text editor or message.
pub fn clip_to_text(clip: &[ClipItem]) -> String {
    let mut out = String::new();
    for item in clip {
        out.push_str("[Strategy]\n");
        out.push_str(&format!("Kind={}\n", escape_value(&item.kind)));
        out.push_str(&format!("KindOrdinal={}\n", item.kind_ordinal));
        if !item.rel_path.is_empty() {
            out.push_str(&format!(
                "Path={}\n",
                escape_value(&join_path(&item.rel_path))
            ));
        }
        out.push_str(&format!("Name={}\n", escape_value(&item.name)));
        for (n, v) in &item.fields {
            out.push_str(&format!("{n}={}\n", escape_value(v)));
        }
        out.push('\n');
    }
    out
}

/// Parses [`clip_to_text`] output, returning `None` for another format or malformed blocks.
pub fn clip_from_text(text: &str) -> Option<Vec<ClipItem>> {
    let mut out: Vec<ClipItem> = Vec::new();
    let mut cur: Option<ClipItem> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim() == "[Strategy]" {
            if let Some(item) = cur.take() {
                out.push(item);
            }
            cur = Some(ClipItem {
                kind_ordinal: 0,
                kind: String::new(),
                name: String::new(),
                rel_path: Vec::new(),
                fields: Vec::new(),
            });
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let item = cur.as_mut()?; // Content before the first block is not our format.
        let (key, value) = line.split_once('=')?;
        let value = unescape_value(value);
        match key {
            "Kind" => item.kind = value,
            "KindOrdinal" => item.kind_ordinal = value.parse().ok()?,
            "Path" => item.rel_path = split_path(&value),
            "Name" => item.name = value,
            _ => item.fields.push((key.to_string(), value)),
        }
    }
    if let Some(item) = cur.take() {
        out.push(item);
    }
    (!out.is_empty() && out.iter().all(|i| !i.name.is_empty())).then_some(out)
}

// --- Rename and move existing `folder_path` values ------------------------

/// Plans a folder rename as `(id, new folder_path)` for rows under `old_prefix`.
/// Replaces the last prefix segment with `new_name` and leaves other rows untouched.
pub fn rename_folder(
    rows: &[StrategyRow],
    old_prefix: &[String],
    new_name: &str,
) -> Vec<(u64, String)> {
    if old_prefix.is_empty() {
        return Vec::new();
    }
    let idx = old_prefix.len() - 1;
    rows.iter()
        .filter_map(|r| {
            let path = split_path(&r.folder_path);
            if !starts_with(&path, old_prefix) {
                return None;
            }
            let mut np = path.clone();
            np[idx] = new_name.to_string();
            Some((r.id, join_path(&np)))
        })
        .collect()
}

/// Plans dragging a folder beneath a new parent as `(id, new folder_path)` entries.
///
/// Preserves the folder name and rebases its subtree under `target_parent + name + suffix`.
/// Returns no edits when the target is the folder itself or its descendant, preventing cycles.
pub fn move_folder(
    rows: &[StrategyRow],
    folder_path: &[String],
    target_parent: &[String],
) -> Vec<(u64, String)> {
    if folder_path.is_empty() || starts_with(target_parent, folder_path) {
        return Vec::new();
    }
    let name = folder_path[folder_path.len() - 1].clone();
    rows_under(rows, folder_path)
        .iter()
        .map(|r| {
            let path = split_path(&r.folder_path);
            let rel = path.get(folder_path.len()..).unwrap_or(&[]).to_vec();
            let mut np = target_parent.to_vec();
            np.push(name.clone());
            np.extend(rel);
            (r.id, join_path(&np))
        })
        .collect()
}

/// Plans a flat move of selected strategies directly into `target`, discarding their original
/// paths because the multi-selection may span folders.
pub fn move_to(rows: &[&StrategyRow], target: &[String]) -> Vec<(u64, String)> {
    let path = join_path(target);
    rows.iter().map(|r| (r.id, path.clone())).collect()
}

#[cfg(test)]
mod tests;
