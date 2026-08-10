//! Missing and invalid Auto-topology persistence regressions.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use moon_ui::DockTopologyByName;

use super::{AutoDockLoad, load_from_path, serialize};

/// Sequence making each test-owned temporary path unique inside one process.
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

/// Return a unique test-owned file path without touching application config.
///
/// Args:
///     case: Short ASCII case label for diagnostics.
///
/// Returns:
///     A path under the operating-system temporary directory.
fn temp_file(case: &str) -> PathBuf {
    let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("test clock must follow the Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "moonterminal-auto-dock-{case}-{}-{timestamp}-{sequence}.json",
        std::process::id()
    ))
}

/// Protects first-run startup when `auto_dock.json` has never been created.
///
/// Plausible breakage: replacing the absent-file fallback with `expect` prevents legacy users from
/// opening any group window after upgrading to the shared Auto topology.
#[test]
fn absent_auto_dock_file_falls_back_to_default_authority() {
    let path = temp_file("absent");
    assert!(!path.exists());
    let load = load_from_path(&path);
    assert_eq!(load, AutoDockLoad::Missing);
    let startup = load.into_startup_state();
    assert!(startup.topology.is_none());
    assert!(startup.automatic_persistence_allowed);
}

/// Protects startup from a partially written or hand-edited `auto_dock.json`.
///
/// Plausible breakage: propagating the JSON error aborts application startup instead of letting
/// every Auto Shell use the deterministic default topology while Classic remains available.
#[test]
fn invalid_auto_dock_file_falls_back_to_default_authority() {
    let path = temp_file("invalid");
    std::fs::write(&path, b"{not valid json").expect("temporary invalid topology must be written");
    let load = load_from_path(&path);
    std::fs::remove_file(&path).expect("temporary invalid topology must be removed");
    assert_eq!(load, AutoDockLoad::InvalidOrUnreadable);
    let startup = load.into_startup_state();
    assert!(startup.topology.is_none());
    assert!(
        !startup.automatic_persistence_allowed,
        "invalid user data must survive automatic fallback and repair"
    );
}

/// Protects a valid file from being mistaken for first-run or recovery state.
///
/// Plausible breakage: returning `Missing` after successful decoding would replace the user's
/// persisted topology with the default on the next Auto entry.
#[test]
fn valid_auto_dock_file_restores_and_allows_later_repairs() {
    let path = temp_file("valid");
    let expected = DockTopologyByName::tab_preset(["ChartTabs", "Report"]);
    let text = serialize(&expected).expect("typed topology must serialize");
    std::fs::write(&path, text).expect("temporary valid topology must be written");

    let load = load_from_path(&path);
    std::fs::remove_file(&path).expect("temporary valid topology must be removed");
    assert_eq!(load, AutoDockLoad::Loaded(expected.clone()));
    let startup = load.into_startup_state();
    assert_eq!(startup.topology, Some(expected));
    assert!(startup.automatic_persistence_allowed);
}

/// Protects `auto_dock.json` from acquiring Classic panel payloads or active-tab state.
///
/// Plausible breakage: serializing `DockAreaState` instead of the name topology couples one
/// group's panel filters and active tab to every Auto window and can later overwrite Classic.
#[test]
fn saved_auto_dock_json_contains_only_name_topology() {
    let topology = DockTopologyByName::tab_preset(["ChartTabs", "Report"]);
    let text = serialize(&topology).expect("typed Auto topology must serialize");
    let json: serde_json::Value = serde_json::from_str(&text).expect("saved JSON must be readable");

    assert_eq!(json["center"]["kind"], "tabs");
    assert_eq!(json["center"]["names"][0], "ChartTabs");
    assert_eq!(json["center"]["names"][1], "Report");
    assert!(json.get("version").is_none());
    assert!(!text.contains("\"active_index\""));
    assert!(!text.contains("\"panel_info\""));
}

/// Protects a damaged Auto file from programmatic topology events during fallback installation.
///
/// Plausible breakage: bypassing
/// `shell/workspace.rs:auto_workspace_topology_is_persistable` during `apply_topology_by_name`
/// would unlock persistence and replace the damaged file before the user changes dock geometry.
#[test]
fn programmatic_auto_reconciliation_keeps_invalid_persistence_locked() {
    let persistence = include_str!("../auto_dock_persist.rs");
    let backend = include_str!("../../backend/mod.rs");
    let shell_init = include_str!("../../shell/init.rs");
    let shell_workspace = include_str!("../../shell/workspace.rs");

    let reconcile = backend
        .split("pub(crate) fn reconcile_auto_dock_topology(")
        .nth(1)
        .and_then(|tail| tail.split("pub(crate) fn auto_workspace_rail_width").next())
        .expect("Backend must keep a separate programmatic Auto reconcile path");
    assert!(
        persistence.contains("Self::InvalidOrUnreadable => AutoDockStartupState {")
            && persistence.contains("automatic_persistence_allowed: false"),
        "invalid startup data must begin with automatic persistence locked"
    );
    assert!(
        reconcile.contains("if self.auto_dock_automatic_persistence_allowed {")
            && reconcile.contains("self.auto_dock_dirty = true;"),
        "programmatic repair may dirty only startup states that allow automatic persistence"
    );
    assert!(
        shell_init.contains("auto_workspace_topology_is_persistable(")
            && shell_init.contains("this.applying_auto_topology")
            && shell_workspace
                .matches("backend.reconcile_auto_dock_topology(")
                .count()
                == 2,
        "programmatic installs must be suppressed at DockEvent and use the recovery-aware path"
    );
}
