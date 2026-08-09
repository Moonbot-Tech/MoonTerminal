//! Crash-recoverable joint persistence for Classic dock topology and detached-panel ownership.

use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};

use moon_core::config::{paths, write_file_atomic};

use crate::persistence::dock_persist::{self, DockMap};
use crate::window::detached::{self, DetachedSpec};

#[cfg(test)]
mod tests;

/// Complete Classic window ownership snapshot staged before either public file changes.
#[derive(Deserialize, Serialize)]
struct WindowStateSnapshot {
    docks: DockMap,
    detached: Vec<DetachedSpec>,
}

/// Load a pending joint snapshot when present, otherwise load the two normal authorities.
///
/// A valid pending file means a prior process may have stopped between the two atomic replacements.
/// It is therefore authoritative for both maps and is replayed idempotently before window creation.
///
/// Returns:
///     Complete Classic dock and detached state from recovery or the normal files.
pub(crate) fn load_all() -> (DockMap, Vec<DetachedSpec>) {
    let pending = paths::window_state_pending_path();
    let Some(snapshot) = recover_pending(&pending, &paths::docks_path(), &paths::detached_path())
    else {
        return (dock_persist::load_all(), detached::load_all());
    };
    (snapshot.docks, detached::deduplicated(snapshot.detached))
}

/// Read and replay one pending snapshot from exact paths.
///
/// Args:
///     pending_path: Journal file that makes an interrupted two-file commit recoverable.
///     docks_path: Public Classic dock-state destination.
///     detached_path: Public Classic detached-spec destination.
///
/// Returns:
///     Parsed authoritative snapshot when a valid journal exists, otherwise `None`.
fn recover_pending(
    pending_path: &Path,
    docks_path: &Path,
    detached_path: &Path,
) -> Option<WindowStateSnapshot> {
    let text = std::fs::read_to_string(pending_path).ok()?;
    let snapshot: WindowStateSnapshot = match serde_json::from_str(&text) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            log::warn!("window-state pending snapshot is invalid: {error}");
            return None;
        }
    };
    if !finish_snapshot(&snapshot, pending_path, docks_path, detached_path) {
        log::warn!("window-state recovery remains pending for the next flush or restart");
    }
    Some(snapshot)
}

/// Persist the two Classic ownership maps as one recoverable logical commit.
///
/// Args:
///     docks: Complete Classic dock topology and panel payload map.
///     detached_specs: Complete Classic detached-panel ownership list.
///
/// Returns:
///     `true` only when the journal, both public files, and journal removal all succeed.
pub(crate) fn save_all(docks: &DockMap, detached_specs: &[DetachedSpec]) -> bool {
    save_all_to_paths(
        docks,
        detached_specs,
        &paths::window_state_pending_path(),
        &paths::docks_path(),
        &paths::detached_path(),
    )
}

/// Write one pending snapshot, then finish both public file replacements.
///
/// Args:
///     docks: Complete Classic dock topology and panel payload map.
///     detached_specs: Complete Classic detached-panel ownership list.
///     pending_path: Journal destination written before either public authority.
///     docks_path: Public Classic dock-state destination.
///     detached_path: Public Classic detached-spec destination.
///
/// Returns:
///     `true` only when the journaled logical commit finishes completely.
fn save_all_to_paths(
    docks: &DockMap,
    detached_specs: &[DetachedSpec],
    pending_path: &Path,
    docks_path: &Path,
    detached_path: &Path,
) -> bool {
    let snapshot = WindowStateSnapshot {
        docks: docks.clone(),
        detached: detached::deduplicated(detached_specs.to_vec()),
    };
    let text = match serde_json::to_string_pretty(&snapshot) {
        Ok(text) => text,
        Err(error) => {
            log::warn!("could not serialize window-state snapshot: {error}");
            return false;
        }
    };
    if let Err(error) = write_file_atomic(pending_path, text.as_bytes(), "window-state pending") {
        log::warn!("could not stage window-state snapshot: {error}");
        return false;
    }
    finish_snapshot(&snapshot, pending_path, docks_path, detached_path)
}

/// Replay one staged snapshot and remove its marker only after both public files succeed.
///
/// Args:
///     snapshot: Complete journaled Classic window-state authority.
///     pending_path: Journal marker removed after both public writes succeed.
///     docks_path: Public Classic dock-state destination.
///     detached_path: Public Classic detached-spec destination.
///
/// Returns:
///     `true` only when both public authorities match the snapshot and no marker remains.
fn finish_snapshot(
    snapshot: &WindowStateSnapshot,
    pending_path: &Path,
    docks_path: &Path,
    detached_path: &Path,
) -> bool {
    if !dock_persist::save_all_to_path(&snapshot.docks, docks_path)
        || !detached::save_all_to_path(&snapshot.detached, detached_path)
    {
        return false;
    }
    match std::fs::remove_file(pending_path) {
        Ok(()) => true,
        Err(error) if error.kind() == ErrorKind::NotFound => true,
        Err(error) => {
            log::warn!("could not clear completed window-state snapshot: {error}");
            false
        }
    }
}
