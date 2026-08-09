//! Persistence for the one global topology-only Auto workspace dock layout.
//!
//! Classic panel payloads remain in `docks.json`, and detached-window ownership remains in
//! `detached.json`. This module reads and writes only MoonUI's normalized panel-name topology at
//! `auto_dock.json`, so Auto interactions cannot overwrite either Classic authority.

use std::path::Path;

use moon_core::config::{paths, write_file_atomic};
use moon_ui::DockTopologyByName;

/// Classified startup result for the shared Auto topology file.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AutoDockLoad {
    /// No file exists yet, so first Auto entry may seed and persist the requested default.
    Missing,
    /// A valid normalized topology was decoded from disk.
    Loaded(DockTopologyByName),
    /// A present file could not be read or decoded and must not be overwritten automatically.
    InvalidOrUnreadable,
}

/// Runtime persistence state derived from one classified startup load.
pub(crate) struct AutoDockStartupState {
    /// Loaded authority, or `None` while the first Auto Shell supplies its safe default in memory.
    pub(crate) topology: Option<DockTopologyByName>,
    /// Whether automatic seed and repair transitions may write `auto_dock.json`.
    pub(crate) automatic_persistence_allowed: bool,
}

impl AutoDockLoad {
    /// Convert the disk classification into Backend startup state.
    ///
    /// Missing state deliberately allows first-run persistence. Invalid state uses the same
    /// in-memory Shell fallback but keeps automatic persistence locked until a user changes the
    /// topology, preserving the unreadable file for diagnosis or manual recovery.
    ///
    /// Returns:
    ///     Optional loaded topology plus the automatic-persistence policy.
    pub(crate) fn into_startup_state(self) -> AutoDockStartupState {
        match self {
            Self::Missing => AutoDockStartupState {
                topology: None,
                automatic_persistence_allowed: true,
            },
            Self::Loaded(topology) => AutoDockStartupState {
                topology: Some(topology),
                automatic_persistence_allowed: true,
            },
            Self::InvalidOrUnreadable => AutoDockStartupState {
                topology: None,
                automatic_persistence_allowed: false,
            },
        }
    }
}

/// Load the shared Auto dock topology from its centralized config path.
///
/// Returns:
///     Missing, valid loaded topology, or invalid/unreadable without collapsing recovery policy.
pub(crate) fn load() -> AutoDockLoad {
    load_from_path(&paths::auto_dock_path())
}

/// Load and classify one typed Auto topology without overwriting recovery evidence.
///
/// Args:
///     path: Exact JSON file to read.
///
/// Returns:
///     Missing, valid loaded topology, or invalid/unreadable.
fn load_from_path(path: &Path) -> AutoDockLoad {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return AutoDockLoad::Missing,
        Err(error) => {
            log::warn!("could not read auto_dock.json: {error}");
            return AutoDockLoad::InvalidOrUnreadable;
        }
    };
    serde_json::from_str::<DockTopologyByName>(&text).map_or_else(
        |error| {
            log::warn!("invalid auto_dock.json ({error}); using the default Auto topology");
            AutoDockLoad::InvalidOrUnreadable
        },
        |topology| AutoDockLoad::Loaded(topology.normalized()),
    )
}

/// Atomically persist the shared normalized Auto dock topology.
///
/// Args:
///     topology: Topology-only panel-name tree accepted by the Backend authority.
///
/// Returns:
///     `true` only after the atomic write succeeds, allowing the dirty authority to retry failures.
pub(crate) fn save(topology: &DockTopologyByName) -> bool {
    match serialize(topology) {
        Ok(text) => {
            if let Err(error) =
                write_file_atomic(&paths::auto_dock_path(), text.as_bytes(), "auto_dock.json")
            {
                log::warn!("could not write auto_dock.json: {error}");
                false
            } else {
                true
            }
        }
        Err(error) => {
            log::warn!("could not serialize auto_dock.json: {error}");
            false
        }
    }
}

/// Serialize exactly the topology-only MoonUI persistence type.
///
/// Args:
///     topology: Normalized shared Auto layout authority.
///
/// Returns:
///     Pretty JSON for `auto_dock.json`, or the Serde error from the typed projection.
fn serialize(topology: &DockTopologyByName) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&topology.normalized())
}

#[cfg(test)]
mod tests;
