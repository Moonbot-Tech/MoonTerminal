//! Execution-time workspace guards for strategy-tree controls.

use moon_core::feed::ExchangeId;
use moon_core::session::CoreId;
use moon_core::venue::CoreVenue;

use super::{
    EnginePill, FolderFill, NodeData, ORDERS_NONE, RowCounts, ToggleTarget, disclosure_caret_shift,
    disclosure_caret_x, drop_dest, heading_chrome, id_exchange, tree_row_indent,
};

/// Hiding empty folders during search would lose both a matching empty folder and an empty child
/// of a matching populated folder; omitting `ensure_folder` would lose their ancestor headings.
#[test]
fn search_keeps_matching_empty_and_nested_folder_subtrees() {
    use crate::strategies::filter::StrategyFilter;
    use crate::strategies::logic::{build_node, ensure_folder};
    let rows = [
        ordering_row(1, "Root/Needle/Child"),
        ordering_row(2, "Other"),
    ];
    let filter = StrategyFilter {
        search: "needle".into(),
        ..Default::default()
    }
    .prepare();
    let paths = [
        "Empty Needle",
        "Root/Needle/Empty",
        "Other Empty",
        "Root/Needle",
    ];
    let empty = super::build::retained_empty_folder_paths(
        paths
            .into_iter()
            .map(moon_core::feed::strategy_path::split_path),
        &rows,
        &filter,
    );
    assert_eq!(
        empty,
        vec![
            vec!["Empty Needle".to_string()],
            vec!["Root".into(), "Needle".into(), "Empty".into()]
        ]
    );
    let mut node = build_node(rows.iter().filter(|r| filter.matches(r)));
    for path in &empty {
        ensure_folder(&mut node, path);
    }
    let root_names: Vec<_> = node.children().map(|(name, _)| name).collect();
    assert_eq!(root_names, ["Root", "Empty Needle"]);
    let root = node.children().find(|(name, _)| *name == "Root").unwrap().1;
    let needle = root
        .children()
        .find(|(name, _)| *name == "Needle")
        .unwrap()
        .1;
    assert_eq!(
        needle.children().map(|(name, _)| name).collect::<Vec<_>>(),
        ["Child", "Empty"]
    );
    let child = needle
        .children()
        .find(|(name, _)| *name == "Child")
        .unwrap()
        .1;
    assert_eq!(
        child.strategies.iter().map(|r| r.id).collect::<Vec<_>>(),
        [1]
    );
}

/// A folder-only core must remain searchable without a strategy providing its parent chain.
#[test]
fn an_empty_core_keeps_folder_query_matches() {
    use crate::strategies::filter::StrategyFilter;
    use moon_core::feed::strategy_path::split_path;
    for (query, expected) in [
        ("fast wave", true),
        ("slow, WAVE", true),
        ("wave !fast", false),
        ("wave !slow", true),
        ("fast missing", false),
    ] {
        let filter = StrategyFilter {
            search: query.into(),
            ..Default::default()
        }
        .prepare();
        let empty = super::build::retained_empty_folder_paths(
            [split_path("Root/Fast Wave/Empty")].into_iter(),
            &[],
            &filter,
        );
        assert_eq!(!empty.is_empty(), expected, "query={query}");
    }
}

/// Expanding an exclusion-only folder query would reveal empty folders and revive excluded rows.
#[test]
fn an_exclusion_only_query_keeps_empty_folders_hidden() {
    use crate::strategies::filter::StrategyFilter;
    use moon_core::feed::strategy_path::split_path;
    let filter = StrategyFilter {
        search: "!test".into(),
        ..Default::default()
    }
    .prepare();
    let empty = super::build::retained_empty_folder_paths(
        [split_path("Production"), split_path("Test")].into_iter(),
        &[],
        &filter,
    );
    assert!(empty.is_empty());
    let mut excluded = ordering_row(1, "Production");
    excluded.name = "Test Strategy".into();
    let mut retained = ordering_row(2, "Test");
    retained.name = "Live Strategy".into();
    assert!(!filter.matches(&excluded));
    assert!(filter.matches(&retained));
}

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
/// displayed "N ord." count unexplained, so users could no longer tell what the second counter means.
#[test]
fn subtree_tooltip_names_counts_and_open_orders_when_present() {
    let _locale = crate::test_locale::force("en");
    let with_orders = RowCounts::subtree(1, 2, Some(3), None);
    let counts_tip = rust_i18n::t!("strat.tree_counts_tip").to_string();
    let orders_tip = rust_i18n::t!("strat.tree_open_orders_tip").to_string();
    assert!(with_orders.tip.to_string().contains(&counts_tip));
    assert!(with_orders.tip.to_string().contains(&orders_tip));

    let without_orders = RowCounts::subtree(1, 2, Some(0), None);
    assert_eq!(without_orders.tip.to_string(), counts_tip);
    assert_eq!(without_orders.orders, ORDERS_NONE);
}

/// `RowCounts::subtree`: the counters name what they count. A refactor back to `57/57 (50)`
/// would hand the operator a three-part figure again; a folder drawing a dash would read as a
/// folder with orders that failed to arrive.
#[test]
fn subtree_counts_are_spelled_with_words_and_folders_leave_orders_blank() {
    let _locale = crate::test_locale::force("en");
    let core = RowCounts::subtree(54, 57, Some(50), Some(true));
    assert_eq!(core.primary, "54 of 57 on");
    assert_eq!(core.orders, "50 ord.");
    let folder = RowCounts::subtree(3, 4, None, Some(true));
    assert_eq!(folder.primary, "3 of 4 on");
    assert!(folder.orders.is_empty());
    // A stopped engine runs nothing, whatever is ticked — the same zero as before the rewording.
    let stopped = RowCounts::subtree(38, 38, Some(0), Some(false));
    assert_eq!(stopped.primary, "0 of 38 on");
}

/// `EnginePill::from_engine`: the pill must follow the CONFIRMED engine state only; an
/// unconfirmed core claiming "running" would tell the operator strategies trade when they may not.
#[test]
fn engine_pill_claims_nothing_for_an_unconfirmed_core() {
    assert_eq!(EnginePill::from_engine(Some(true)), EnginePill::Running);
    assert_eq!(EnginePill::from_engine(Some(false)), EnginePill::Stopped);
    assert_eq!(EnginePill::from_engine(None), EnginePill::Unknown);
    assert!(EnginePill::Unknown.label().is_empty());
}

/// Only a core that REPORTED its engine stopped moves into the Stopped section; running and
/// unconfirmed cores stay with the rest, and both halves keep canonical order.
#[test]
fn the_stopped_partition_moves_only_reported_stops() {
    let (stopped, others) = super::build::partition_stopped(
        [Some(true), Some(false), None, Some(false), Some(true)].into_iter(),
    );
    assert_eq!(stopped, vec![1, 3]);
    assert_eq!(others, vec![0, 2, 4]);
    let (stopped, others) = super::build::partition_stopped(std::iter::empty());
    assert!(stopped.is_empty() && others.is_empty());
}

/// The core row's own source, scanned so the layout harness below cannot drift from it.
const HEADINGS_SRC: &str = include_str!("headings.rs");

/// The harness rebuilds a core heading from the SAME pieces `core_folder_row` draws, in its order;
/// the full row needs a live window entity for its checkbox and run cell, so it cannot be mounted
/// in a test. This pins those pieces to the production row.
#[test]
fn the_core_row_harness_uses_the_production_pieces() {
    for piece in [
        "EnginePill::from_engine(engine),",
        "compact_headings(view.read(app).panels.tree_w)",
        "COUNTS_SLOT_W_COMPACT,",
        "COUNTS_SLOT_W,",
        "ORDERS_SLOT_W),",
        ".gap(design::ui_px(app, HEADING_GAP))",
        ".gap(design::ui_px(app, COUNTS_GAP))",
        "div().flex_1().min_w_0().truncate().child(",
        "trading: true,",
    ] {
        assert!(
            HEADINGS_SRC.contains(piece),
            "core_folder_row no longer has {piece}"
        );
    }
}

/// Layout of a core heading in every locale.
mod core_row_layout {
    use gpui::{
        Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Render, Styled,
        VisualTestContext, Window, div, px,
    };
    use moon_core::config::{Language, UiThemeMode};
    use moon_ui::{MoonPalette, MoonText, MoonTheme, h_flex};

    use super::super::{
        COUNTS_GAP, COUNTS_SLOT_W, COUNTS_SLOT_W_COMPACT, EnginePill, HEADING_GAP, ORDERS_SLOT_W,
        ORDERS_SLOT_W_COMPACT, RowCounts, compact_headings, counts_slot, engine_pill, row_h,
    };
    use crate::controls::core_run::{RunSlots, reserved_cell};
    use crate::design;

    /// The narrowest tree pane the split allows (`split.rs`).
    const MIN_TREE_W: f32 = 240.0;
    /// The window's default tree width (`PanelsLayout::default`).
    const DEFAULT_TREE_W: f32 = 418.0;
    /// The narrowest tree that still draws the worded row: the hardest case for it.
    const COMPACT_EDGE_W: f32 = super::super::COMPACT_TREE_W;

    struct Host {
        pill: EnginePill,
        width: f32,
    }

    impl Render for Host {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let step = 0.0;
            // Widest realistic figures: three-digit strategy counts, two-digit orders.
            let counts = RowCounts::subtree(999, 999, Some(99), Some(true));
            let slots = RunSlots {
                status: false,
                trading: true,
                auto: false,
            };
            let compact = compact_headings(self.width);
            let (primary, primary_w, orders, orders_w) = match compact {
                true => (
                    counts.primary_short.clone(),
                    COUNTS_SLOT_W_COMPACT,
                    counts.orders_short.clone(),
                    ORDERS_SLOT_W_COMPACT,
                ),
                false => (
                    counts.primary.clone(),
                    COUNTS_SLOT_W,
                    counts.orders.clone(),
                    ORDERS_SLOT_W,
                ),
            };
            let p = MoonPalette::active(cx);
            let square = design::ui_px(cx, design::DISCLOSURE_BOX);
            div().w(px(self.width)).child(
                h_flex()
                    .debug_selector(|| "row".into())
                    .w_full()
                    .h(row_h(cx, step))
                    .pr(design::ui_px(cx, 6.0))
                    .items_center()
                    .gap(design::ui_px(cx, HEADING_GAP))
                    // Caret box and checkbox column, as fixed squares.
                    .child(div().flex_none().size(square))
                    .child(div().flex_none().size(square))
                    .child(
                        div()
                            .debug_selector(|| "pill".into())
                            .flex_none()
                            .child(engine_pill(self.pill, compact, step, cx)),
                    )
                    .child(
                        div()
                            .debug_selector(|| "name".into())
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(
                                MoonText::new("CORE_A with a long synthetic name")
                                    .mono(true)
                                    .uppercase(false)
                                    .render(),
                            ),
                    )
                    .child(
                        h_flex()
                            .debug_selector(|| "counts".into())
                            .flex_none()
                            .items_center()
                            .gap(design::ui_px(cx, COUNTS_GAP))
                            .child(counts_slot(primary, primary_w, p.text_muted, step, cx))
                            .child(counts_slot(orders, orders_w, p.text_soft, step, cx)),
                    )
                    .child(
                        div()
                            .debug_selector(|| "run".into())
                            .flex_none()
                            .children(reserved_cell(slots, cx)),
                    ),
            )
        }
    }

    fn inside(child: Bounds<Pixels>, parent: Bounds<Pixels>) -> bool {
        child.origin.x + px(0.5) >= parent.origin.x && child.right() <= parent.right() + px(0.5)
    }

    fn before(left: Bounds<Pixels>, right: Bounds<Pixels>) -> bool {
        left.right() <= right.origin.x + px(0.5)
    }

    /// Every locale and pill: pill, name, counters and the run button sit left to right without
    /// overlapping and inside the row, and only the name gives way — at the default tree width,
    /// and at the narrowest the split allows.
    #[gpui::test]
    fn the_core_row_fits_in_every_locale(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0),
                cx,
            );
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for pill in [
                EnginePill::Running,
                EnginePill::Stopped,
                EnginePill::Unknown,
            ] {
                for width in [DEFAULT_TREE_W, COMPACT_EDGE_W, MIN_TREE_W] {
                    let window = cx.open_window(gpui::size(px(width), px(200.0)), |_, _| Host {
                        pill,
                        width,
                    });
                    let mut visual = VisualTestContext::from_window(window.into(), cx);
                    for _ in 0..3 {
                        visual.update(|window, cx| {
                            let _ = window.draw(cx);
                        });
                    }
                    let at = format!("{} {pill:?} {width}px", language.code());
                    let mut get = |id: &'static str| {
                        visual
                            .debug_bounds(id)
                            .unwrap_or_else(|| panic!("{id} not laid out ({at})"))
                    };
                    let row = get("row");
                    let pill_b = get("pill");
                    let name = get("name");
                    let counts = get("counts");
                    let run = get("run");
                    for (id, b) in [("pill", pill_b), ("counts", counts), ("run", run)] {
                        assert!(inside(b, row), "{id} left the row ({at})");
                    }
                    assert!(before(pill_b, name), "pill overlaps the name ({at})");
                    assert!(before(name, counts), "name overlaps the counters ({at})");
                    assert!(before(counts, run), "counters overlap the button ({at})");
                    if !compact_headings(width) {
                        assert!(
                            f32::from(name.size.width) >= 40.0,
                            "name squeezed to {:.1}px in the worded row ({at})",
                            f32::from(name.size.width)
                        );
                    }
                }
            }
        }
    }
}
