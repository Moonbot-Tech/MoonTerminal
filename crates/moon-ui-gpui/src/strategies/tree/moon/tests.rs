//! Execution-time workspace guards for strategy-tree controls.

use moon_core::feed::ExchangeId;
use moon_core::session::CoreId;
use moon_core::venue::CoreVenue;

use super::{
    FolderFill, NodeData, RowCounts, ToggleTarget, disclosure_caret_shift, disclosure_caret_x,
    drop_dest, heading_chrome, id_exchange, tree_row_indent,
};

/// Minimal row fixture for sibling ordering, with a folder path independent of its id.
fn ordering_row(id: u64, path: &str) -> moon_core::feed::StrategyRow {
    moon_core::feed::StrategyRow {
        id,
        name: id.to_string(),
        kind: "Long".into(),
        kind_ordinal: 1,
        folder_path: path.into(),
        checked: false,
        is_short: false,
        fields: Vec::new(),
    }
}

/// Rendering folders before loose strategies would hide a successful folder move on screen.
/// Ranking a folder from only its visible rows would also reorder it when its first row is hidden.
#[test]
fn folder_and_loose_strategy_siblings_follow_the_complete_order() {
    use crate::strategies::filter::StrategyFilter;
    use crate::strategies::logic::{FolderCounts, build_node, ensure_folder};
    use std::collections::HashMap;

    let rows = [
        ordering_row(1, ""),
        ordering_row(2, "F/Deep"),
        ordering_row(3, ""),
        ordering_row(4, "F"),
        ordering_row(5, "G"),
    ];
    let mut node = build_node(rows.iter().filter(|row| row.id != 2));
    ensure_folder(&mut node, &["Empty".into()]);
    let filter = StrategyFilter::default().prepare();
    let mut counts = FolderCounts::default();
    let mut ranks = HashMap::new();
    for (at, row) in rows.iter().enumerate() {
        counts.add(row, &filter, at);
        ranks.insert(row.id, at);
    }
    let labels: Vec<_> = super::ordered_siblings(&node, "", &counts, &ranks)
        .into_iter()
        .map(|sibling| match sibling {
            super::OrderedSibling::Folder { path, .. } => path,
            super::OrderedSibling::Strategy(row) => row.id.to_string(),
        })
        .collect();
    assert_eq!(labels, ["1", "F", "3", "G", "Empty"]);
}

/// Removing the folder icon or trusting stale expansion on an empty folder hides its identity
/// or advertises nonexistent children; both locally-created and core-confirmed folders count.
#[test]
fn empty_folders_keep_an_icon_without_a_disclosure() {
    let target = ToggleTarget::Folder(7, vec!["desk".into()]);
    for fill in [FolderFill::EmptyLocal, FolderFill::EmptyOnCore] {
        for expanded in [false, true] {
            assert_eq!(
                heading_chrome(&target, fill, expanded),
                (Some("icons/folder-closed.svg"), None)
            );
        }
    }
}

/// Freezing the icon pose would misidentify an open folder; marking every heading as a folder
/// would also change core and Deleted chrome outside the folder-row requirement.
#[test]
fn populated_folder_icons_follow_their_disclosure_pose() {
    let folder = ToggleTarget::Folder(7, vec!["desk".into()]);
    for (expanded, path) in [
        (false, "icons/folder-closed.svg"),
        (true, "icons/folder-open.svg"),
    ] {
        assert_eq!(
            heading_chrome(&folder, FolderFill::Populated, expanded),
            (Some(path), Some(expanded))
        );
        for target in [ToggleTarget::Core(7), ToggleTarget::Deleted(7)] {
            assert_eq!(
                heading_chrome(&target, FolderFill::Populated, expanded),
                (None, Some(expanded))
            );
        }
    }
}

/// Compile-time source used to ensure the checkbox producer retains its action guard.
const SRC: &str = concat!(
    include_str!("../moon.rs"),
    include_str!("../moon/geometry.rs"),
    include_str!("../moon/node.rs"),
    include_str!("../moon/build.rs"),
    include_str!("../moon/shape.rs"),
    include_str!("../moon/callbacks.rs"),
    include_str!("../moon/rows.rs"),
    include_str!("../moon/counts.rs"),
    include_str!("../moon/headings.rs"),
    include_str!("../moon/strategy_row.rs"),
);

/// Issue #689: pulling a folder caret left by the disclosure box stacks it on its
/// core, because that box and the indent step are both 12 design units. A larger
/// text step used to slide the folder caret even further left. Restoring that
/// pull makes every expanded folder read as the same column as its core.
#[test]
fn folder_caret_sits_one_indent_step_right_of_its_parent() {
    // Oracle is the row inset the tree already shipped (6 + 12 per level), not
    // the caret helper's own constants. UI zoom multiplies this design-unit x
    // once in `render_row` / `core_folder_row`, so a uniform scale cannot close
    // the gap.
    const INSET: f32 = 6.0;
    const STEP: f32 = 12.0;
    let steps = [0.0_f32, 1.0, 4.0];
    for depth in 0..6u32 {
        for step in steps {
            for icon in [false, true] {
                let caret = disclosure_caret_x(depth as f32, icon, step);
                assert_eq!(caret, INSET + STEP * depth as f32);
                assert_eq!(caret, disclosure_caret_x(depth as f32, !icon, 0.0));
            }
            let parent = disclosure_caret_x(depth as f32, false, step);
            let folder = disclosure_caret_x(depth as f32 + 1.0, true, step);
            assert_eq!(folder - parent, STEP);
            // The reported column: indent step minus the disclosure box and the
            // text step. That difference is zero at the default text size.
            let stacked = STEP - (crate::design::DISCLOSURE_BOX + step);
            assert_ne!(folder - parent, stacked);
        }
    }
    assert_eq!(tree_row_indent(0.0), INSET);
    assert_eq!(tree_row_indent(3.0), INSET + STEP * 3.0);
    assert_eq!(disclosure_caret_shift(true, 4.0), 0.0);
    assert_eq!(disclosure_caret_shift(false, 0.0), 0.0);
}

/// `core_folder_row` must place the glyph with `disclosure_caret_shift`. Putting
/// the caret back at the left of the previous indent stacks folder carets on
/// the core again, and the geometry test above would stay green.
#[test]
fn core_folder_row_places_the_caret_with_the_geometry_helper() {
    let start = SRC
        .find("fn core_folder_row(")
        .expect("core_folder_row must exist");
    let body = &SRC[start..];
    assert!(
        body.contains("disclosure_caret_shift(folder_icon.is_some(), step)"),
        "the folder caret must use the geometry helper"
    );
    assert!(
        body.contains(".when_some(folder_icon,"),
        "the folder mark must sit outside the caret slot"
    );
    assert!(
        !body.contains("-edge"),
        "subtracting the disclosure box from the caret cancels the indent step"
    );
}

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
        engine: None,
    }
}

/// Folder row with the given path segments; must remain a live drop destination.
fn folder_node(core: CoreId, path: &[&str]) -> NodeData {
    NodeData::Folder {
        fill: super::FolderFill::Populated,
        core,
        path: path.iter().map(|p| (*p).to_string()).collect(),
        label: "folder".into(),
        active: 0,
        total: 0,
        selected: false,
        checked: false,
        engine: None,
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
        engine: None,
        highlighted: false,
        is_short: false,
        cut: false,
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

/// `tree/moon/counts.rs::RowCounts::subtree`: dropping the open-orders tooltip clause would leave the
/// displayed `(N)` count unexplained, so users could no longer tell what the second counter means.
#[test]
fn subtree_tooltip_names_counts_and_open_orders_when_present() {
    let _locale = crate::test_locale::force("en");
    let with_orders = RowCounts::subtree(1, 2, 3, None);
    let counts_tip = rust_i18n::t!("strat.tree_counts_tip").to_string();
    let orders_tip = rust_i18n::t!("strat.tree_open_orders_tip").to_string();
    assert!(with_orders.tip.to_string().contains(&counts_tip));
    assert!(with_orders.tip.to_string().contains(&orders_tip));

    let without_orders = RowCounts::subtree(1, 2, 0, None);
    assert_eq!(without_orders.tip.to_string(), counts_tip);
    assert!(without_orders.orders.is_empty());
}
