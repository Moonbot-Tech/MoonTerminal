//! Execution-time workspace guards for strategy-tree controls.

use moon_core::feed::ExchangeId;
use moon_core::session::CoreId;
use moon_core::venue::CoreVenue;

use super::{NodeData, drop_dest, id_exchange};

/// Compile-time source used to ensure the checkbox producer retains its action guard.
const SRC: &str = include_str!("../moon.rs");

/// Removing the visibility guard from `tree/moon.rs` checkbox `on_change` would let a stale
/// callback stage a hidden core after switching the owning Auto workspace.
#[test]
fn stale_checkbox_callback_cannot_stage_a_hidden_core() {
    let start = SRC
        .find(".on_change(move |ch: &bool")
        .expect("strategy checkbox handler must exist");
    let handler = &SRC[start..];
    let staged_write = handler
        .find("let before = this.staged")
        .expect("strategy checkbox must still stage an edit");
    let guard = handler
        .find("strategy_core_is_visible(this.workspace_cores.as_deref(), key.0)")
        .expect("strategy checkbox must validate the current workspace at dispatch");

    assert!(
        guard < staged_write,
        "the workspace guard must execute before any retained staging mutation"
    );
}

/// Build a venue with independently controlled identity and display metadata.
fn venue(code: u8, id_dex: u32, dex: &str, reported: &str) -> CoreVenue {
    CoreVenue {
        id: ExchangeId { code, dex: id_dex },
        dex: dex.to_string(),
        reported: reported.to_string(),
    }
}

/// `tree/moon.rs:id_exchange`: deriving the node ID from a reported caption would reset expansion
/// when only wire spelling changes and could merge two HIP-3 exchanges that share a caption.
#[test]
fn exchange_node_ids_follow_identity_and_have_one_unknown_value() {
    let first = venue(9, 17, "alpha", "First caption");
    let renamed = venue(9, 17, "beta", "Second caption");
    let other_code = venue(10, 17, "alpha", "First caption");
    let other_dex = venue(9, 18, "alpha", "First caption");

    assert_eq!(id_exchange(Some(&first)), id_exchange(Some(&renamed)));
    assert_ne!(id_exchange(Some(&first)), id_exchange(Some(&other_code)));
    assert_ne!(id_exchange(Some(&first)), id_exchange(Some(&other_dex)));
    assert_eq!(id_exchange(None), id_exchange(None));
    assert_eq!(id_exchange(None).as_ref(), "x:unknown");
}

/// Core row used as a live drop destination in `drop_dest` assertions.
fn core_node(core: CoreId) -> NodeData {
    NodeData::Core {
        core,
        label: "core".into(),
        active: 0,
        total: 0,
        open_orders: 0,
        selected: false,
        checked: false,
    }
}

/// Folder row with the given path segments; must remain a live drop destination.
fn folder_node(core: CoreId, path: &[&str]) -> NodeData {
    NodeData::Folder {
        core,
        path: path.iter().map(|p| (*p).to_string()).collect(),
        label: "folder".into(),
        active: 0,
        total: 0,
        selected: false,
        checked: false,
    }
}

/// Strategy row that `drop_dest` must reject so a drop never targets another strategy.
fn strategy_node(core: CoreId) -> NodeData {
    NodeData::Strategy {
        core,
        id: 9,
        name: "row".into(),
        kind: "Demo".into(),
        open_orders: 0,
        server_checked: false,
        staged: None,
        highlighted: false,
        is_short: false,
        drag_ids: None,
    }
}

/// Confinement must not strip folder or core drop targets. Changing the Folder arm to `None`
/// would make same-core moves and cross-core copies onto a folder silently fail.
#[test]
fn drop_dest_keeps_folder_and_core_targets() {
    assert_eq!(drop_dest(&core_node(7)), Some((7, Vec::new())));
    assert_eq!(
        drop_dest(&folder_node(7, &["desk", "live"])),
        Some((7, vec!["desk".into(), "live".into()]))
    );
    assert_eq!(
        drop_dest(&NodeData::Exchange {
            label: "ex".into(),
            logo: None,
        }),
        None
    );
    assert_eq!(drop_dest(&strategy_node(7)), None);
    assert_eq!(
        drop_dest(&NodeData::DeletedFolder { core: 7, count: 1 }),
        None
    );
    assert_eq!(
        drop_dest(&NodeData::DeletedStrategy {
            core: 7,
            id: 1,
            name: "gone".into(),
            kind: "Demo".into(),
            is_short: false,
            highlighted: false,
        }),
        None
    );
}

/// Preview closures must pass origin window and live tree bounds into both chip constructors.
/// Both payloads are confined while FolderDrag payload construction stays `core` + `path` only.
#[test]
fn preview_closures_wire_drag_chip_confinement() {
    let strat = SRC
        .find("// ── DnD: strategies")
        .expect("StratDrag wiring must exist");
    let folder = SRC
        .find(".draggable::<FolderDrag, DragChip")
        .expect("FolderDrag draggable must exist");
    let strat_preview = &SRC[strat..folder];
    let folder_preview = &SRC[folder..];
    assert!(
        strat_preview.contains("origin_window,"),
        "StratDrag payload must carry the originating window"
    );
    assert!(
        strat_preview.contains("window.window_handle().window_id()"),
        "StratDrag must capture the originating window from the row decorator"
    );
    assert!(
        strat_preview.contains("stop_when_outside: true"),
        "StratDrag must cancel when the chip would paint outside the tree"
    );
    assert!(
        !strat_preview.contains(".draggable::<StratDrag"),
        "StratDrag must not go through Tree::draggable, which cannot capture Window"
    );
    assert!(
        folder_preview.contains("window.window_handle().window_id()"),
        "FolderDrag preview must compile the shared DragChip origin field"
    );
    assert!(
        folder_preview.contains("stop_when_outside: true"),
        "FolderDrag must cancel when its global preview leaves the origin tree"
    );
    assert!(
        folder_preview.contains("path: path.clone()"),
        "FolderDrag payload must remain core + path"
    );
}
