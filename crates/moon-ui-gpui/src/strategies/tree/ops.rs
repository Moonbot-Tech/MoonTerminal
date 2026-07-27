//! Pure strategy-tree operation logic for create, rename, copy, paste, move, and delete workflows.
//! It has no UI or `cx`, only calculations over `StrategyRow` and kind schemas (`SchemaKind`).
//! Results are intents (`NewStrategy` or `(id, new path)` lists) that the dispatch layer converts
//! into `moon-core` commands.
//!
//! A folder exists only as a prefix of strategy paths; the data model has no empty folders.
//! Every operation therefore edits `folder_path` or a row set. That prefix arrives as ONE flat,
//! unescaped string, so where its segment boundaries fall is decided in exactly one place —
//! [`path_segments`]. Nothing else in the window may split a `folder_path` by hand.

use std::collections::HashSet;

use moon_core::feed::{SchemaKind, StrategyRow};
use moon_core::session::CoreId;

/// Field name through which moonproto stores `StrategySnapshot::strategy_name`.
pub const STRATEGY_NAME_FIELD: &str = "StrategyName";

/// Iterates nonempty path segments without allocating. This is the window-wide source of path
/// splitting for trees, counts, expansion, and operations.
///
/// A slash separates two folders only when NEITHER neighbouring character is whitespace. MoonProto
/// delivers a strategy's placement as ONE flat `StrategySnapshot::path` joined with `/` and with no
/// escaping, while MoonBot allows a `/` INSIDE a folder name — `"EMA / ORGANIC WAVE STRUCTURE
/// STRATEGIES LLM"` is a single folder there. Splitting on every slash therefore invents folders
/// that exist nowhere, and the invented ones are recognisable: they carry a leading or trailing
/// space. This rule is what keeps the window's tree agreeing with MoonBot's.
///
/// The edges follow the same rule, the missing neighbour counting as non-whitespace: `"/a"` splits
/// and its empty leading segment is filtered, while `"/ a"` stays one segment named `"/ a"`.
///
/// [`join_path`] inverts this only for canonical paths: nonempty segments joined with `/`.
/// It does not restore doubled, leading, or trailing separators or replace an input `\`.
pub fn path_segments(path: &str) -> impl Iterator<Item = &str> {
    let mut start = 0usize;
    path.match_indices(['/', '\\'])
        .filter(|&(i, _)| {
            !path[..i]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
                && !path[i + 1..]
                    .chars()
                    .next()
                    .is_some_and(char::is_whitespace)
        })
        .map(|(i, _)| i)
        // The tail after the last separator, which has no separator to announce it.
        .chain(std::iter::once(path.len()))
        .map(move |i| {
            let seg = &path[start..i];
            start = i + 1;
            seg
        })
        .filter(|s| !s.is_empty())
}

/// Splits a folder path into owned segments through [`path_segments`].
pub fn split_path(path: &str) -> Vec<String> {
    path_segments(path).map(str::to_string).collect()
}

/// Joins path segments with `/`; for nonempty segments, this inverts [`path_segments`] on
/// canonical paths.
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
    /// `(core, strategy id)` to sit immediately after; `None` appends. Mirrors
    /// `moon_core::feed::NewStrategySpec::insert_after`, which this becomes.
    pub insert_after: Option<(CoreId, u64)>,
}

impl From<NewStrategy> for moon_core::feed::NewStrategySpec {
    /// One converter for both dispatch paths — the create dialog and paste/drop — so a field
    /// added to the intent cannot reach the core through one of them and not the other.
    fn from(n: NewStrategy) -> Self {
        Self {
            kind_ordinal: n.kind_ordinal,
            folder_path: n.folder_path,
            fields: n.fields,
            insert_after: n.insert_after,
        }
    }
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
        // A brand-new strategy has no source to sit beside.
        insert_after: None,
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
    /// `(core, strategy id)` this was copied FROM, so a paste back onto that same core can put
    /// the copy beside its source instead of at the end of a hundred-strategy list.
    ///
    /// Meaningful only on that core: ids are per-core, so the feed command drain drops it when
    /// applying the paste to any other core. `None` for a folder copy — a folder's rows have no
    /// single anchor, and anchoring each one individually would interleave the copies through the
    /// original.
    /// [`clip_to_text`] deliberately does not carry it: a strategy shared as text arrives on a
    /// different machine where that id belongs to something else entirely.
    pub src: Option<(CoreId, u64)>,
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
                src: None,
            }
        })
        .collect()
}

/// Copies selected strategies flat, with an empty `rel_path` for every item.
///
/// Paste therefore places each copy directly in the target folder. Original paths are discarded
/// because a multi-selection can span folders and users expect copies at the chosen destination.
/// Each item retains its source `(core, id)` so the feed can honor same-core placement and reject
/// a foreign anchor at the destination drain.
pub fn copy_rows(rows: &[(CoreId, &StrategyRow)]) -> Vec<ClipItem> {
    rows.iter()
        .map(|(core, r)| ClipItem {
            kind_ordinal: r.kind_ordinal,
            kind: r.kind.clone(),
            name: r.name.clone(),
            rel_path: Vec::new(),
            fields: r.fields.clone(),
            src: Some((*core, r.id)),
        })
        .collect()
}

/// Copies a folder relative to its parent so paste preserves the folder name like a file manager.
pub fn copy_folder(rows: &[StrategyRow], folder_prefix: &[String]) -> Vec<ClipItem> {
    let under = rows_under(rows, folder_prefix);
    let parent_len = folder_prefix.len().saturating_sub(1);
    clip_with_base(&under, &folder_prefix[..parent_len])
}

/// Returns whether the text between brackets marks a copy rather than being part of the name.
///
/// ASCII digits only: a full-width `（2）` or an Arabic-Indic `٢` is somebody's actual name,
/// not an ordinal this code wrote.
fn is_copy_marker(inner: &str) -> bool {
    inner == "copy" || (!inner.is_empty() && inner.bytes().all(|b| b.is_ascii_digit()))
}

/// Strips one LEADING `(N) ` or `(copy) ` marker, returning the rest of the name.
///
/// `None` when the name does not open with a bare marker (`(v2) Grid` is a version tag,
/// `((2)) S` nests a bracket inside the marker) or when stripping would leave nothing.
///
/// The separating space is NOT required. This code only ever writes `"(N) Base"`, but the copy
/// dialog's name box is user-editable: deleting that space must not make the next copy nest into
/// `(2) (2)Grid`. The price is that a name deliberately authored as `(2)Grid` is reduced to
/// `Grid` when copied — a cosmetic loss, chosen over an accumulating one.
fn strip_leading_affix(s: &str) -> Option<&str> {
    let rest = s.strip_prefix('(')?;
    let close = rest.find(')')?;
    let inner = &rest[..close];
    // A bracket inside the marker means the first `)` is not the marker's own.
    if inner.contains('(') || !is_copy_marker(inner) {
        return None;
    }
    let tail = rest[close + 1..].trim_start();
    (!tail.is_empty()).then_some(tail)
}

/// Strips one trailing ` (N)` or ` (copy)` marker.
///
/// Accepting both leading and trailing forms keeps existing names from collecting a marker at
/// each end when copied.
fn strip_trailing_affix(s: &str) -> Option<&str> {
    let open = s.rfind(" (")?;
    let inner = s[open + 2..].strip_suffix(')')?;
    if !is_copy_marker(inner) {
        return None;
    }
    let head = s[..open].trim_end();
    (!head.is_empty()).then_some(head)
}

/// Returns a name with every copy marker removed, from either end.
///
/// `(2) (3) www` and `(2) S (3)` reduce to `www` and `S`; `(v2) Grid` and `Grid (v2 beta)` are
/// left alone, and so is a name made entirely of markers.
fn base_name(name: &str) -> &str {
    let mut s = name.trim();
    loop {
        // No re-trim: `s` starts trimmed, and neither stripper can reintroduce whitespace —
        // the leading one `trim_start`s its tail and keeps the already-trimmed end, the
        // trailing one `trim_end`s its head and keeps the already-trimmed start.
        if let Some(rest) = strip_leading_affix(s) {
            s = rest;
        } else if let Some(head) = strip_trailing_affix(s) {
            s = head;
        } else {
            break;
        }
    }
    s
}

/// Returns `desired` unchanged when it is free.
///
/// On collision, reduces the name through [`base_name`] and returns `(N) Base` with the
/// smallest free `N` starting at two. The ordinal leads because the strategy column is narrow
/// and truncates from the right: a trailing `(4)` is the first thing lost, which is exactly
/// the part that tells two copies apart.
pub fn unique_name(taken: &HashSet<String>, desired: &str) -> String {
    if !taken.contains(desired) {
        return desired.to_string();
    }
    let base = base_name(desired);
    for n in 2.. {
        let cand = format!("({n}) {base}");
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
///
/// An item's remembered source travels through as the anchor: a strategy pasted back onto its
/// OWN core lands directly after the row it was copied from. The cross-core case needs no guard
/// here — the anchor is core-qualified and the feed's command drain drops a foreign one where
/// the destination core is known for certain.
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
            insert_after: item.src,
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
                // Text arrives from another terminal, where a strategy id names something else.
                src: None,
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
