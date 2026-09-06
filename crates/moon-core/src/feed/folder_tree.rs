//! Edits to a core's folder tree, applied where the newest tree is known.
//!
//! The protocol takes a folder edit as the COMPLETE desired set — the core deletes every empty
//! folder the list omits — so what an edit is worth depends entirely on the list it was built from.
//! Built from a stale one it silently deletes whatever arrived in between, which is exactly the
//! trap moonproto's own guidance names ("building the next edit from an older confirmed snapshot
//! can undo your own pending folder changes", `docs/strats.md`).
//!
//! So the window states its INTENT — add this folder, move that subtree — and the base list is
//! chosen here, on the feed thread, where both the core's confirmed tree and the one this terminal
//! last sent are known. Nothing upstream assembles a complete tree, because nothing upstream can.

/// Remove one folder and everything under it from a tree.
///
/// Removal IS omission — the wire form is the complete desired set — so this is the whole of it. A
/// folder still holding a strategy is kept by the core regardless of this list, which is the safety
/// the caller relies on rather than a check made here.
///
/// Args:
///     paths: The tree to remove from.
///     path: Canonical path of the folder to remove.
///
/// Returns:
///     The tree without that folder or any of its descendants.
pub fn without(paths: &[String], path: &str) -> Vec<String> {
    let folded = fold(path);
    let under = format!("{folded}/");
    paths
        .iter()
        .filter(|seen| {
            let seen = fold(seen);
            seen != folded && !seen.starts_with(&under)
        })
        .cloned()
        .collect()
}

/// Whether every path in a submission would pass moonproto's own folder validation.
///
/// MIRRORS `validate_strategy_folder_paths` (moonproto `client/active_runtime/handles.rs`), which
/// rejects the WHOLE submission on one bad path. That matters because bundling a folder tree with
/// strategies puts every STRATEGY path through the same check, and this terminal knowingly holds
/// paths it would refuse: MoonBot allows a `/` inside a folder name — `"EMA / ORGANIC"` is one
/// folder there — while the validator splits on every `/` and refuses a segment with surrounding
/// whitespace. Asking first is what keeps a folder tree from turning a working rename into a
/// refusal that moves nothing at all.
///
/// A hand-kept mirror, like the other cross-crate constants in this workspace: moonproto exposes no
/// validator, and being wrong here costs a skipped folder tree rather than a wrong edit.
///
/// Args:
///     paths: Every folder path the submission would carry — the tree AND each strategy's own.
///
/// Returns:
///     Whether moonproto would accept the set.
pub fn sendable<'a>(paths: impl Iterator<Item = &'a str>) -> bool {
    let mut folders = std::collections::HashSet::new();
    for path in paths {
        if path.len() > usize::from(u8::MAX) || path.contains(['\r', '\n', '\0', '"']) {
            return false;
        }
        let mut prefix = String::new();
        for part in path.split('/').filter(|_| !path.is_empty()) {
            if part.is_empty() || part.trim() != part {
                return false;
            }
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            folders.insert(prefix.to_lowercase());
        }
    }
    folders.len() < usize::from(u16::MAX)
}

/// Fold one path into the form two spellings of the same folder share.
///
/// Lowercased because the core compares folder paths case-insensitively, and `\` read as `/`
/// because a strategy's stored path may carry either while the tree the core reports uses `/`. Two
/// spellings that fold alike name one folder, and an edit that missed that would look for a folder
/// the core has under a name it does not.
///
/// Args:
///     path: A folder path in any of those spellings.
///
/// Returns:
///     Its comparison form.
fn fold(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// Add one folder to a tree, or return it unchanged when the tree already holds it.
///
/// Args:
///     paths: The tree to add to.
///     path: Canonical path of the folder to add.
///
/// Returns:
///     The tree with that folder present exactly once.
pub fn with_added(paths: &[String], path: &str) -> Vec<String> {
    let mut out = paths.to_vec();
    let folded = fold(path);
    if !out.iter().any(|seen| fold(seen) == folded) {
        out.push(path.to_string());
    }
    out
}

/// Split one path into the segments the CORE reads it as.
///
/// On every separator, which is the core's own rule for the tree it reports — deliberately NOT the
/// Strategies window's rule, which keeps a `/` with whitespace beside it inside a folder name. The
/// two disagree only for a name the core could never accept an edit to anyway, and on such a core
/// folder editing is switched off whole (`CoreFolders::editable`).
fn segments(path: &str) -> Vec<&str> {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect()
}

/// Rewrite a tree for a subtree that was renamed or moved.
///
/// The half a strategy edit cannot carry: rewriting the rows' paths moves the strategies, but the
/// OLD folder survives in the tree as a folder of its own, now holding nothing. Only a tree that
/// omits it removes it, and the protocol wants both halves in one snapshot.
///
/// A path is rewritten when it IS the moved folder or lies under it. "Under" is a segment
/// boundary, never a character prefix — `Research2` is not inside `Research` — and the comparison
/// is case-insensitive because the core's own is; the surviving spelling is the one the caller
/// asked for, since that is what the operator typed, with the untouched tail kept as the core
/// spelled it.
///
/// Compared segment by segment rather than by slicing the raw path at the length of a folded
/// prefix. That shortcut is wrong twice over: `to_lowercase` does not preserve byte length for
/// every character — `İ` grows — so the offset can land mid-character and panic, on a string this
/// terminal received from a core.
///
/// Args:
///     paths: The tree to rewrite.
///     old_key: Canonical path of the folder that moved.
///     new_key: Canonical path it moved to.
///
/// Returns:
///     The complete desired tree, in the order given, with no path listed twice.
pub fn rebase(paths: &[String], old_key: &str, new_key: &str) -> Vec<String> {
    let old_parts: Vec<String> = segments(old_key)
        .into_iter()
        .map(|part| part.to_lowercase())
        .collect();
    if old_parts.is_empty() || fold(old_key) == fold(new_key) {
        return paths.to_vec();
    }
    let mut out: Vec<String> = Vec::with_capacity(paths.len());
    // A rename onto an existing name merges the two folders, and the tree must say so once: the core
    // reads a repeated path as one folder either way, but a list that repeats itself is one nobody
    // can check against what they asked for. Through a set rather than a scan of what is already
    // out — the tree runs to thousands of paths, and a scan would fold each one per comparison.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for path in paths {
        let parts = segments(path);
        let under = parts.len() >= old_parts.len()
            && parts
                .iter()
                .zip(&old_parts)
                .all(|(part, want)| part.to_lowercase() == *want);
        let rebased = match under {
            false => path.clone(),
            true => {
                let tail = &parts[old_parts.len()..];
                match tail.is_empty() {
                    true => new_key.to_string(),
                    false => format!("{new_key}/{}", tail.join("/")),
                }
            }
        };
        if seen.insert(fold(&rebased)) {
            out.push(rebased);
        }
    }
    out
}

#[cfg(test)]
mod tests;
