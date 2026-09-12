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

mod moonbot_text;

/// Retain local copy/cut identity only while its serialized text remains on the system clipboard.
/// A temporarily unavailable clipboard keeps the local operation usable.
pub fn clipboard_matches_internal(internal: Option<&[ClipItem]>, text: Option<&str>) -> bool {
    internal.is_some_and(|items| {
        !items.is_empty()
            && text.is_none_or(|text| text.replace("\r\n", "\n") == clip_to_text(items))
    })
}

/// Resolve current clipboard text, retaining placement anchors only for an unchanged local copy.
/// MoonBot exports need the destination schema to resolve their textual SignalType.
pub fn resolve_clipboard(
    internal: Option<&[ClipItem]>,
    text: Option<&str>,
    kinds: &[SchemaKind],
) -> Option<Vec<ClipItem>> {
    let Some(text) = text else {
        return internal
            .filter(|items| !items.is_empty())
            .map(<[_]>::to_vec);
    };
    if clipboard_matches_internal(internal, Some(text)) {
        return internal.map(<[_]>::to_vec);
    }
    clip_from_text(text).or_else(|| moonbot_text::parse(text, kinds))
}

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

/// Return distinct folder names the core will split, in first-seen order.
/// Paths are planned destinations; only separators retained INSIDE a `path_segments` segment
/// warn, so ordinary nested paths stay silent. No path is rewritten.
pub fn split_folder_names<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .flat_map(path_segments)
        .filter(|name| name.contains(['/', '\\']) && seen.insert(*name))
        .map(str::to_string)
        .collect()
}

/// Returns whether `path` starts with `prefix` segment by segment, preserving data case.
fn starts_with(path: &[String], prefix: &[String]) -> bool {
    path.len() >= prefix.len() && prefix.iter().zip(path).all(|(a, b)| a == b)
}

/// Returns whether a strategy's flat `folder_path` lies at or below a segment prefix.
///
/// The allocation-free form of [`starts_with`], for callers holding the raw wire path: it walks
/// [`path_segments`] instead of materializing a `Vec<String>` per row, which is what a prefix
/// tested against every row of a core costs otherwise.
pub fn path_starts_with(folder_path: &str, prefix: &[String]) -> bool {
    let mut segments = path_segments(folder_path);
    prefix
        .iter()
        .all(|want| segments.next() == Some(want.as_str()))
}

/// Returns every row at or below a path prefix.
pub fn rows_under<'a>(rows: &'a [StrategyRow], prefix: &[String]) -> Vec<&'a StrategyRow> {
    rows.iter()
        .filter(|r| path_starts_with(&r.folder_path, prefix))
        .collect()
}

/// Returns whether any strategy row is at or below a path prefix without allocating a row list.
pub fn has_row_under(rows: &[StrategyRow], prefix: &[String]) -> bool {
    rows.iter()
        .any(|row| path_starts_with(&row.folder_path, prefix))
}

/// Why a delete cannot go ahead: how many of the targeted rows the core still runs.
///
/// A COUNT rather than a bare refusal, because the refusal has to be shown to the operator and
/// "some are enabled" is not an answer they can act on — the menu's right label and the Delete
/// key's notice both name the figure, which is the whole point of the guard being visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteBlock {
    /// Targeted rows the core reports as `checked`.
    pub enabled: usize,
    /// Every targeted row, enabled or not.
    pub total: usize,
}

/// Returns why a delete is blocked, or `None` when every affected strategy is disabled.
pub fn delete_block(rows: &[&StrategyRow]) -> Option<DeleteBlock> {
    let enabled = rows.iter().filter(|r| r.checked).count();
    (enabled > 0).then_some(DeleteBlock {
        enabled,
        total: rows.len(),
    })
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

// --- Reordering inside a folder -------------------------------------------

/// Which way one reorder step moves the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveStep {
    /// One place towards the start of the folder.
    Up,
    /// One place towards its end.
    Down,
}

/// Move every selected strategy one place inside its own folder, and return the core's new order.
///
/// A core's strategy list is an ORDER the operator arranged in MoonBot, and moonproto synchronizes
/// it as the row sequence of a Full snapshot (`docs/strats.md`, "Strategy Order"). So a reorder is
/// not a local view preference: the whole list goes back to the core in its new sequence.
///
/// Two rules keep it behaving like every other list reorder:
///
///   * A row moves only inside its own folder. The protocol asks that one folder's strategies stay
///     a contiguous group, and a row that could walk out the top of its folder would silently
///     change what folder it is in — that is what dragging is for.
///   * Only rows the tree currently DRAWS take part. With a filter on, "up" therefore means above
///     the row visibly above it, while a hidden strategy keeps the slot it holds in the core's own
///     list. Moving against invisible neighbours instead would spend a press on nothing.
///
/// A block of adjacent selected rows moves together and stops at its folder's edge, one row at a
/// time, which is the usual behaviour of such a control.
///
/// Args:
///     rows: The core's complete strategy list, in the order the tree currently shows it.
///     selected: Strategy ids the operator is moving.
///     visible: Whether one row is drawn under the active filter.
///     step: Direction to move the selection.
///
/// Returns:
///     The core's complete new id sequence, or `None` when nothing in the selection can move —
///     an empty selection, or a block already sitting against the edge of its folder.
pub fn reorder_step(
    rows: &[&StrategyRow],
    selected: &HashSet<u64>,
    visible: impl Fn(&StrategyRow) -> bool,
    step: MoveStep,
) -> Option<Vec<u64>> {
    // Per folder, the INDEXES of the rows that folder draws. Indexes rather than ids because the
    // permutation is written back into exactly these slots, which leaves every hidden row and every
    // other folder's row untouched wherever it sits.
    let mut slots_by_folder: Vec<Vec<usize>> = Vec::new();
    let mut folder_at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (at, row) in rows.iter().enumerate() {
        if !visible(row) {
            continue;
        }
        // The tree's own folder identity, so a row groups with the folder it is drawn under even
        // when the wire spelled the path with a backslash or a doubled separator.
        let folder = join_path(&split_path(&row.folder_path));
        let group = match folder_at.get(&folder) {
            Some(&group) => group,
            None => {
                folder_at.insert(folder, slots_by_folder.len());
                slots_by_folder.push(Vec::new());
                slots_by_folder.len() - 1
            }
        };
        slots_by_folder[group].push(at);
    }

    let mut order: Vec<u64> = rows.iter().map(|row| row.id).collect();
    let mut moved = false;
    for slots in &slots_by_folder {
        let mut ids: Vec<u64> = slots.iter().map(|&at| order[at]).collect();
        match step {
            // Front to back going up, back to front going down: each selected row is swapped with
            // the neighbour beyond it only when that neighbour is NOT itself selected, so a block
            // shifts by one and cannot pass through its own members.
            MoveStep::Up => {
                for at in 1..ids.len() {
                    if selected.contains(&ids[at]) && !selected.contains(&ids[at - 1]) {
                        ids.swap(at - 1, at);
                        moved = true;
                    }
                }
            }
            MoveStep::Down => {
                for at in (0..ids.len().saturating_sub(1)).rev() {
                    if selected.contains(&ids[at]) && !selected.contains(&ids[at + 1]) {
                        ids.swap(at, at + 1);
                        moved = true;
                    }
                }
            }
        }
        for (&at, id) in slots.iter().zip(ids) {
            order[at] = id;
        }
    }
    moved.then_some(order)
}

// --- Keyboard navigation over the drawn rows ------------------------------

/// One row the tree currently DRAWS, in draw order.
///
/// Distinct from `flat_order`, which the window keeps for Shift ranges and which holds strategies
/// alone: keyboard navigation moves over cores and folders too, so it needs the node-level
/// sequence. Both come from the same build walk, so they cannot describe different trees.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NavNode {
    Core(CoreId),
    Folder(CoreId, String),
    Strategy(CoreId, u64),
    /// A core's Deleted heading. Present so the order matches what is drawn, and skipped by
    /// [`nav_step`] — it is neither a folder nor a strategy, and the window has no third
    /// selection concept to land on it with.
    DeletedFolder(CoreId),
    DeletedStrategy(CoreId, u64),
}

impl NavNode {
    /// The `(core, folder path)` this node addresses, or `None` for the rows that address none.
    fn folder_key(&self) -> Option<(CoreId, String)> {
        match self {
            Self::Core(core) => Some((*core, String::new())),
            Self::Folder(core, path) => Some((*core, path.clone())),
            Self::Strategy(..) | Self::DeletedFolder(_) | Self::DeletedStrategy(..) => None,
        }
    }

    /// Whether the keyboard cursor may rest here.
    fn is_landable(&self) -> bool {
        !matches!(self, Self::DeletedFolder(_))
    }
}

/// Which way one navigation step moves the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavStep {
    Up,
    Down,
}

/// The next landable row in draw order, or `None` at the edge.
///
/// Deliberately does NOT wrap. A list that jumps from its last row back to its first turns a held
/// arrow into an endless loop over the whole tree, and the operator loses their place; stopping is
/// what every file manager does.
///
/// Args:
///     order: Rows in the order the tree draws them.
///     cursor: Row the selection rests on, or `None` when nothing is selected.
///     step: Direction to move.
///
/// Returns:
///     The row to select next; `None` at the edge, on an empty order, or when the cursor is not
///     drawn at all (a selection the current filter hides).
pub fn nav_step(order: &[NavNode], cursor: Option<&NavNode>, step: NavStep) -> Option<NavNode> {
    let landable = |node: &NavNode| node.is_landable();
    let Some(cursor) = cursor else {
        // Nothing selected: enter the list from the end the arrow points away from.
        return match step {
            NavStep::Down => order.iter().find(|n| landable(n)).cloned(),
            NavStep::Up => order.iter().rev().find(|n| landable(n)).cloned(),
        };
    };
    let at = order.iter().position(|n| n == cursor)?;
    match step {
        NavStep::Down => order.get(at + 1..)?.iter().find(|n| landable(n)).cloned(),
        NavStep::Up => order.get(..at)?.iter().rev().find(|n| landable(n)).cloned(),
    }
}

/// The row Left moves to when the cursor is a leaf or already collapsed.
///
/// Args:
///     cursor: Row the selection rests on.
///     strategy_folder: The cursor strategy's own `folder_path`, when it is a strategy.
///
/// Returns:
///     The containing folder, or the core when the row sits at its root; `None` for a row with no
///     navigable parent.
pub fn nav_parent(cursor: &NavNode, strategy_folder: Option<&str>) -> Option<NavNode> {
    match cursor {
        NavNode::Core(_) | NavNode::DeletedFolder(_) | NavNode::DeletedStrategy(..) => None,
        NavNode::Folder(core, path) => {
            let mut parts = split_path(path);
            parts.pop();
            Some(match parts.is_empty() {
                true => NavNode::Core(*core),
                false => NavNode::Folder(*core, join_path(&parts)),
            })
        }
        NavNode::Strategy(core, _) => {
            let parts = split_path(strategy_folder.unwrap_or_default());
            Some(match parts.is_empty() {
                true => NavNode::Core(*core),
                false => NavNode::Folder(*core, join_path(&parts)),
            })
        }
    }
}

/// Folder and core keys between two folder nodes inclusive, in draw order.
///
/// The folder-set counterpart of the Shift range `apply_click` runs over `flat_order`: strategies
/// between the two ends are stepped over rather than selected, because a folder selection and a
/// strategy selection are different sets and a Shift drag across a folder must not silently start
/// filling the other one.
///
/// Returns an empty vec when either end is not currently drawn.
pub fn folder_range(
    order: &[NavNode],
    anchor: &(CoreId, String),
    target: &(CoreId, String),
) -> Vec<(CoreId, String)> {
    let at = |key: &(CoreId, String)| {
        order
            .iter()
            .position(|n| n.folder_key().as_ref() == Some(key))
    };
    let (Some(ia), Some(ib)) = (at(anchor), at(target)) else {
        return Vec::new();
    };
    let (lo, hi) = if ia <= ib { (ia, ib) } else { (ib, ia) };
    order[lo..=hi]
        .iter()
        .filter_map(NavNode::folder_key)
        .collect()
}

// --- Cut, and the moves a cut paste becomes -------------------------------

/// What a pending Cut is holding, as the rows and folders the operator marked.
///
/// Folders travel WHOLE rather than as their expanded row lists, because a cut folder pasted into
/// the same core is one `move_folder` plus a rebase — its rows keep their ids — while the same
/// folder expanded into individual rows would arrive flat and lose the subtree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct CutOrigin {
    /// Strategies cut one by one.
    pub rows: Vec<(CoreId, u64)>,
    /// Folders cut whole. An empty path never appears: a core root is not something to cut.
    pub folders: Vec<(CoreId, Vec<String>)>,
}

impl CutOrigin {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.folders.is_empty()
    }

    /// How many marks the operator made, for the notice that names the pending cut.
    pub fn count(&self) -> usize {
        self.rows.len() + self.folders.len()
    }

    /// Whether a drawn strategy row renders dimmed: cut directly, or sitting under a cut folder.
    pub fn dims(&self, key: (CoreId, u64), folder_path: &str) -> bool {
        self.rows.contains(&key)
            || self
                .folders
                .iter()
                .any(|(core, path)| *core == key.0 && path_starts_with(folder_path, path))
    }

    /// Whether a drawn folder row renders dimmed: cut itself, or nested under a cut folder.
    ///
    /// Through [`path_starts_with`] rather than [`split_path`] + [`starts_with`], for the reason
    /// that function documents: this is drawn once per folder row per frame, and the split form
    /// allocates a `Vec<String>` every time to answer a question that needs no allocation.
    pub fn dims_folder(&self, core: CoreId, path: &str) -> bool {
        self.folders
            .iter()
            .any(|(c, cut)| *c == core && path_starts_with(path, cut))
    }

    /// The cut folders belonging to one core, with a selected PARENT subsuming its selected
    /// descendants.
    ///
    /// A Shift range over the tree selects every node between its ends, so a parent and a child
    /// of it routinely arrive together. Planning both would move the same rows twice — two
    /// overlapping rebases on the same core, or, across cores, the descendant's rows created a
    /// second time before anything is retired. The parent already carries its whole subtree, so
    /// the descendant is dropped here, once, rather than at each of the two call sites.
    fn folders_of(&self, core: CoreId) -> Vec<Vec<String>> {
        let mine: Vec<Vec<String>> = self
            .folders
            .iter()
            .filter(|(c, _)| *c == core)
            .map(|(_, path)| path.clone())
            .collect();
        mine.iter()
            .filter(|path| {
                !mine
                    .iter()
                    .any(|other| other != *path && starts_with(path, other))
            })
            .cloned()
            .collect()
    }

    /// The loosely cut rows of one core that no cut FOLDER of that core already carries.
    ///
    /// Same subsumption, one level down: a strategy inside a cut folder travels with that folder,
    /// so planning it individually as well would move it twice.
    fn loose_rows_of(&self, core: CoreId, rows: &[StrategyRow]) -> Vec<u64> {
        let folders = self.folders_of(core);
        self.rows
            .iter()
            .filter(|(c, _)| *c == core)
            .map(|(_, id)| *id)
            .filter(|id| {
                let Some(row) = rows.iter().find(|r| r.id == *id) else {
                    return true;
                };
                !folders
                    .iter()
                    .any(|folder| path_starts_with(&row.folder_path, folder))
            })
            .collect()
    }

    /// Every core this cut touches, in first-marked order.
    pub fn cores(&self) -> Vec<CoreId> {
        let mut out: Vec<CoreId> = Vec::new();
        for core in self
            .rows
            .iter()
            .map(|(c, _)| *c)
            .chain(self.folders.iter().map(|(c, _)| *c))
        {
            if !out.contains(&core) {
                out.push(core);
            }
        }
        out
    }
}

/// One `session::move_strategies` call: the row moves, plus the folder rebase that travels with a
/// relocated subtree so the core does not keep the emptied original path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveIntent {
    pub moves: Vec<(u64, String)>,
    pub rebase: Option<(String, String)>,
}

/// Plans the SAME-CORE half of pasting a cut: the intents that move the marked rows and folders
/// into `target`, keeping their original ids.
///
/// Keeping the ids is the whole difference between a cut and a copy — versions and order profit
/// are joined to a strategy through its id, so a "move" that created new rows would silently
/// detach both. Nothing here ever produces a [`NewStrategy`].
///
/// Args:
///     rows: The destination core's complete live strategy list.
///     cut: The pending cut.
///     core: The core being pasted INTO; only its own cut marks are handled here.
///     target: Destination folder segments; empty means the core root.
///
/// Returns:
///     One intent per folder plus at most one for the loose rows; empty when the paste would be a
///     no-op (nothing cut on this core, rows already in `target`, or a folder pasted into itself
///     or its own descendant).
pub fn cut_move_plan(
    rows: &[StrategyRow],
    cut: &CutOrigin,
    core: CoreId,
    target: &[String],
) -> Vec<MoveIntent> {
    let mut out = Vec::new();
    let target_path = join_path(target);

    for folder in cut.folders_of(core) {
        // `move_folder` already refuses a cycle; the equality check catches the other no-op, a
        // paste back into the folder's current parent.
        let mut moved_to = target.to_vec();
        moved_to.extend(folder.last().cloned());
        if moved_to == folder {
            continue;
        }
        let moves = move_folder(rows, &folder, target);
        if moves.is_empty() && starts_with(target, &folder) {
            continue;
        }
        out.push(MoveIntent {
            moves,
            rebase: Some((join_path(&folder), join_path(&moved_to))),
        });
    }

    let ids: HashSet<u64> = cut.loose_rows_of(core, rows).into_iter().collect();
    if !ids.is_empty() {
        // A row already sitting in the target is dropped rather than sent: the core would accept
        // the no-op move, but the whole list travels with it and a paste that changes nothing
        // should cost the core nothing.
        //
        // Compared through the tree's OWN path identity, never against the raw wire string: the
        // same folder can arrive spelled `a\b` and be drawn as `a/b`, so a raw `!=` would call a
        // row "elsewhere" and send a move that relocates it to where it already is.
        // `reorder_step` normalizes for the same reason.
        let loose: Vec<&StrategyRow> = rows
            .iter()
            .filter(|r| {
                ids.contains(&r.id) && join_path(&split_path(&r.folder_path)) != target_path
            })
            .collect();
        if !loose.is_empty() {
            out.push(MoveIntent {
                moves: move_to(&loose, target),
                rebase: None,
            });
        }
    }
    out
}

/// One row exactly as it stood when it was COPIED to the destination.
///
/// The identity travels, not just the id, because retirement happens a round trip later and the
/// operator can edit or move the source row in between. Deleting by id alone would then destroy an
/// edit the destination never received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarriedRow {
    pub id: u64,
    pub name: String,
    pub folder_path: String,
}

/// What one SOURCE core contributes to a cross-core cut paste, and what must be retired there
/// once the destination confirms it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutCarry {
    pub src: CoreId,
    /// The clipboard to create at the destination.
    pub clip: Vec<ClipItem>,
    /// EVERY row this carry copied, folder contents included, as it stood at copy time.
    pub rows: Vec<CarriedRow>,
    /// Folders marked whole on this core, for emptied-folder cleanup after the rows go.
    pub folders: Vec<Vec<String>>,
}

/// Plans the CROSS-CORE half of pasting a cut: what to create at `dst`, per source core.
///
/// Ids are per-core, so a strategy cannot be moved between cores at all — it is created at the
/// destination and retired at the source, and the retirement waits for the destination's echo
/// ([`cut_retire_plan`]). This function only prepares the create half; nothing here deletes.
///
/// Args:
///     by_core: Each core's complete live strategy list.
///     cut: The pending cut.
///     dst: The core being pasted INTO; its own marks belong to [`cut_move_plan`] instead.
///
/// Returns:
///     One carry per source core that contributed anything.
pub fn cut_carry_plan(
    by_core: &[(CoreId, &[StrategyRow])],
    cut: &CutOrigin,
    dst: CoreId,
) -> Vec<CutCarry> {
    let mut out = Vec::new();
    for src in cut.cores() {
        if src == dst {
            continue;
        }
        let Some((_, rows)) = by_core.iter().find(|(core, _)| *core == src) else {
            continue;
        };
        let ids = cut.loose_rows_of(src, rows);
        let folders = cut.folders_of(src);
        let id_set: HashSet<u64> = ids.iter().copied().collect();
        let loose: Vec<(CoreId, &StrategyRow)> = rows
            .iter()
            .filter(|r| id_set.contains(&r.id))
            .map(|r| (src, r))
            .collect();
        let mut clip = copy_rows(&loose);
        // Identity of everything that travels, so retirement can tell "the row I copied" from "a
        // row that happens to be there now".
        let mut carried: Vec<CarriedRow> = loose
            .iter()
            .map(|(_, r)| CarriedRow {
                id: r.id,
                name: r.name.clone(),
                folder_path: r.folder_path.clone(),
            })
            .collect();
        for folder in &folders {
            clip.extend(copy_folder(rows, folder));
            // A folder travels as its CONTENTS, recorded one by one. Retirement then deletes
            // exactly these rows rather than whatever the folder holds by the time the echo lands.
            for row in rows_under(rows, folder) {
                if carried.iter().any(|c| c.id == row.id) {
                    continue;
                }
                carried.push(CarriedRow {
                    id: row.id,
                    name: row.name.clone(),
                    folder_path: row.folder_path.clone(),
                });
            }
        }
        if clip.is_empty() {
            continue;
        }
        out.push(CutCarry {
            src,
            clip,
            rows: carried,
            folders,
        });
    }
    out
}

/// What may be retired at the SOURCE once the destination echoed the pasted names back, and what
/// has to stay.
///
/// An ENABLED strategy is never deleted — the same rule the delete guard enforces everywhere else
/// — so a cut whose source rows are running ends as a copy, and the count of what stayed is what
/// the notice reports. A folder goes whole only when every row beneath it may go; otherwise its
/// disabled rows leave individually and the folder remains with the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutRetire {
    /// Rows that may go: still present, still disabled, and still exactly as they were copied.
    pub delete_rows: Vec<u64>,
    /// Folders that hold nothing but the rows above, so they can be retired once those go.
    pub empty_folders: Vec<Vec<String>>,
    /// Rows left behind because the core still runs them.
    pub kept_enabled: usize,
    /// Rows left behind because they were EDITED after the copy, so the destination has a stale
    /// version and deleting the source would destroy the newer one.
    pub kept_changed: usize,
}

/// Plans the source-side retirement of a confirmed cross-core cut.
///
/// Args:
///     rows: The SOURCE core's complete live strategy list.
///     carry_rows: Loose strategy ids that were carried across.
///     carry_folders: Folders that were carried across whole.
///
/// Returns:
///     The rows and folders to delete, and how many enabled rows were kept.
pub fn cut_retire_plan(
    rows: &[StrategyRow],
    carried: &[CarriedRow],
    carry_folders: &[Vec<String>],
) -> CutRetire {
    // Indexed once: `reconcile_cut_followups` calls this every frame while a cross-core cut is
    // waiting on its echo, and a per-id scan of the core's whole list would pay for that wait
    // over and over.
    let by_id: std::collections::HashMap<u64, &StrategyRow> =
        rows.iter().map(|row| (row.id, row)).collect();

    let mut delete_rows: Vec<u64> = Vec::new();
    let mut kept_enabled = 0usize;
    let mut kept_changed = 0usize;

    for want in carried {
        // Already gone from the source — nothing to retire, and nothing to report.
        let Some(row) = by_id.get(&want.id) else {
            continue;
        };
        if row.checked {
            kept_enabled += 1;
            continue;
        }
        // The row must still be the one that was COPIED. An operator has a whole round trip in
        // which to rename it or move it elsewhere, and the destination holds the version from
        // before that edit — so deleting it here would destroy the newer one and leave the stale
        // copy standing. Compared on the tree's own path identity, not the raw wire spelling.
        let same_place =
            join_path(&split_path(&row.folder_path)) == join_path(&split_path(&want.folder_path));
        if row.name != want.name || !same_place {
            kept_changed += 1;
            continue;
        }
        delete_rows.push(want.id);
    }

    delete_rows.sort_unstable();
    delete_rows.dedup();

    // A carried folder is retired only once NOTHING is left in it that is not already going. A row
    // added to it after the copy was never carried, so deleting the folder wholesale would destroy
    // a strategy the destination never received.
    let empty_folders: Vec<Vec<String>> = carry_folders
        .iter()
        .filter(|folder| {
            rows_under(rows, folder)
                .iter()
                .all(|row| delete_rows.binary_search(&row.id).is_ok())
        })
        .cloned()
        .collect();

    CutRetire {
        delete_rows,
        empty_folders,
        kept_enabled,
        kept_changed,
    }
}

/// Whether every name a paste sent has come back in the destination core's rows.
///
/// The echo is what makes a cross-core cut safe: until the destination reports the names, the
/// source must not lose anything.
pub fn names_echoed(rows: &[StrategyRow], names: &[String]) -> bool {
    // Indexed rather than re-scanned per name: this is polled every frame for as long as the echo
    // is outstanding, against a core that may hold hundreds of rows.
    let present: HashSet<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    names.iter().all(|name| present.contains(name.as_str()))
}

// --- Copying a whole selection of folders ---------------------------------

/// Concatenates [`copy_folder`] over a selection, each folder relative to its OWN parent.
///
/// So pasting two selected folders recreates BOTH by name, which is what a file manager does with
/// a multi-folder copy. A folder nested inside another selected folder contributes nothing of its
/// own: its rows already travel inside the parent, and copying it twice would duplicate them at
/// the destination.
///
/// Args:
///     rows_by_core: Each core's complete live strategy list.
///     folders: The selected `(core, segments)` nodes; an EMPTY path means that core's root, which
///         contributes every one of its rows with its full path.
///
/// Returns:
///     One clipboard item per copied strategy. EMPTY when every selected folder is empty - the
///     caller must not treat that as "nothing happened" and leave a previous clipboard armed.
pub fn copy_folders(
    rows_by_core: &[(CoreId, &[StrategyRow])],
    folders: &[(CoreId, Vec<String>)],
) -> Vec<ClipItem> {
    let mut out = Vec::new();
    for (core, path) in folders {
        // A root selection subsumes every other folder of that core, and any folder nested in
        // another selected folder is already carried by it.
        let nested = folders.iter().any(|(other_core, other)| {
            other_core == core && other != path && starts_with(path, other)
        });
        if nested {
            continue;
        }
        let Some((_, rows)) = rows_by_core.iter().find(|(c, _)| c == core) else {
            continue;
        };
        match path.is_empty() {
            // The core root: every row, keeping its full path so the whole tree is recreated.
            true => {
                let all: Vec<&StrategyRow> = rows.iter().collect();
                out.extend(clip_with_base(&all, &[]));
            }
            false => out.extend(copy_folder(rows, path)),
        }
    }
    out
}

// --- Move to a chosen folder ----------------------------------------------

/// What a "Move to folder…" request is moving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveSources {
    Strategies(Vec<u64>),
    Folders(Vec<Vec<String>>),
}

/// The destinations a "Move to folder…" dialog offers, in the order it lists them.
///
/// Three sources, because a folder can exist in three ways here: as the prefix of a live row's
/// path, as a folder the core reports in its own tree, and as a UI-only folder this window is
/// holding until its first strategy arrives. Folded case-insensitively, the way the core itself
/// compares folder names.
///
/// Args:
///     rows: The core's complete live strategy list.
///     reported: Folders the core reports in its own tree, already split.
///     local: UI-only folder paths this window holds for that core.
///     exclude: Folders being moved; neither they nor their descendants may be a destination.
///
/// Returns:
///     The root (an empty path) first, then every folder ordered case-insensitively.
pub fn move_destinations(
    rows: &[StrategyRow],
    reported: &[Vec<String>],
    local: &[Vec<String>],
    exclude: &[Vec<String>],
) -> Vec<Vec<String>> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<Vec<String>> = Vec::new();

    let mut consider = |parts: Vec<String>, out: &mut Vec<Vec<String>>| {
        if parts.is_empty() {
            return;
        }
        // A folder cannot move into itself or into its own subtree.
        if exclude.iter().any(|bad| starts_with(&parts, bad)) {
            return;
        }
        if seen.insert(join_path(&parts).to_lowercase()) {
            out.push(parts);
        }
    };

    for row in rows {
        // Every cumulative prefix, so an intermediate folder holding no row of its own is still
        // offered — it is drawn in the tree, so it must be reachable here.
        let parts = split_path(&row.folder_path);
        for take in 1..=parts.len() {
            consider(parts[..take].to_vec(), &mut out);
        }
    }
    for parts in reported.iter().chain(local.iter()) {
        for take in 1..=parts.len() {
            consider(parts[..take].to_vec(), &mut out);
        }
    }

    out.sort_by_cached_key(|parts| {
        let joined = join_path(parts);
        (joined.to_lowercase(), joined)
    });
    // The root leads: it is the one destination that always exists, and it is where "out of this
    // folder" means.
    let mut all = vec![Vec::new()];
    all.extend(out);
    all
}

/// Plans a "Move to folder…" confirmation as `move_strategies` intents.
///
/// Args:
///     rows: The core's complete live strategy list.
///     sources: The strategies or folders being moved.
///     target: Destination folder segments; empty means the core root.
///
/// Returns:
///     One intent for the loose strategies, or one per folder; empty when nothing would move.
pub fn move_to_folder_plan(
    rows: &[StrategyRow],
    sources: &MoveSources,
    target: &[String],
) -> Vec<MoveIntent> {
    match sources {
        MoveSources::Strategies(ids) => {
            let target_path = join_path(target);
            let wanted: HashSet<u64> = ids.iter().copied().collect();
            // Normalized before comparing, for the reason `cut_move_plan` documents: the raw wire
            // path may spell the target folder differently from the tree's own identity for it.
            let moving: Vec<&StrategyRow> = rows
                .iter()
                .filter(|r| {
                    wanted.contains(&r.id) && join_path(&split_path(&r.folder_path)) != target_path
                })
                .collect();
            match moving.is_empty() {
                true => Vec::new(),
                false => vec![MoveIntent {
                    moves: move_to(&moving, target),
                    rebase: None,
                }],
            }
        }
        MoveSources::Folders(folders) => folders
            .iter()
            .filter_map(|folder| {
                let mut moved_to = target.to_vec();
                moved_to.extend(folder.last().cloned());
                if moved_to == *folder || starts_with(target, folder) {
                    return None;
                }
                Some(MoveIntent {
                    moves: move_folder(rows, folder, target),
                    rebase: Some((join_path(folder), join_path(&moved_to))),
                })
            })
            .collect(),
    }
}

// --- Renaming a strategy --------------------------------------------------

/// Whether a strategy name is already used on this core, ignoring the strategy being renamed.
///
/// Names are global per core (see [`paste_plan`]), so the check spans every folder. Exact match,
/// the same rule [`unique_name`] encodes: the core distinguishes names that differ only in case,
/// and folding them here would refuse a rename the core would have accepted.
pub fn name_taken(rows: &[StrategyRow], own_id: u64, name: &str) -> bool {
    rows.iter().any(|r| r.id != own_id && r.name == name)
}

#[cfg(test)]
mod tests;
