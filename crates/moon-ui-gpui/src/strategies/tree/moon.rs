//! Strategy tree built on MoonUI's forked `MoonTree` and its headless `Tree::custom` mode.
//! It replaces the former `tree.rs` manual flattening, virtualization, and hitbox-based DnD:
//! MoonTree flattens `expanded_ids`, virtualizes rows, handles the keyboard, and supplies row
//! hitboxes to DnD decorators. Selection, staging, and expansion remain in `StrategiesView`; this
//! module only adapts `CoreStore` to `MoonTreeItem`, builds the `id -> NodeData` side map for row
//! and drag data, and renders rows and decorators. Callbacks outside `Context<Self>` mutate through
//! `Entity::update`.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonCheckbox, MoonCheckboxSize, MoonPalette,
    MoonText, MoonTone, MoonTree, MoonTreeEntry, MoonTreeItem, MoonTreeRowMeta, h_flex,
};

use super::super::logic::{build_node, ensure_folder, folder_counts, toggle};
use super::super::{Key, StrategiesView, moon_alpha};
use super::ui::{ContextMenu, DragChip, FolderDrag, MenuTarget, StratDrag};
use crate::design;
use moon_core::feed::StrategyRow;
use moon_core::session::{CoreId, CoreStore};

// ── Node ID encoding: stable string IDs for MoonTree ─────────────────────────
fn id_core(core: CoreId) -> SharedString {
    SharedString::from(format!("c:{core}"))
}
fn id_folder(core: CoreId, path: &str) -> SharedString {
    SharedString::from(format!("f:{core}:{path}"))
}
pub(crate) fn id_strat(core: CoreId, id: u64) -> SharedString {
    SharedString::from(format!("s:{core}:{id}"))
}
fn id_del_folder(core: CoreId) -> SharedString {
    SharedString::from(format!("d:{core}"))
}
fn id_del_strat(core: CoreId, id: u64) -> SharedString {
    SharedString::from(format!("ds:{core}:{id}"))
}

/// Data for one tree row, looked up by node ID from `render_row` and decorators.
pub(crate) enum NodeData {
    Core {
        core: CoreId,
        label: String,
        active: usize,
        total: usize,
        open_orders: usize,
    },
    Folder {
        core: CoreId,
        path: Vec<String>,
        label: String,
        active: usize,
        total: usize,
        /// Whether a click selected the folder for highlighting and Ctrl+C folder copying.
        selected: bool,
    },
    Strategy {
        core: CoreId,
        id: u64,
        name: String,
        kind: String,
        open_orders: usize,
        server_checked: bool,
        staged: Option<bool>,
        highlighted: bool,
        is_short: bool,
        drag_ids: Vec<u64>,
    },
    /// Core's Deleted folder: strategies absent from the server and retained only in the local DB.
    /// The DB retains their folder paths for restoration, while the UI lists them flat here.
    DeletedFolder { core: CoreId, count: usize },
    DeletedStrategy {
        core: CoreId,
        id: u64,
        name: String,
        kind: String,
        is_short: bool,
        highlighted: bool,
    },
}

/// Adapter result containing tree items, the side map, expanded IDs, and visible flat order.
pub(crate) struct MoonTreeBuild {
    pub(crate) items: Vec<MoonTreeItem>,
    pub(crate) node_data: HashMap<SharedString, NodeData>,
    pub(crate) expanded_ids: Vec<SharedString>,
    pub(crate) flat: Vec<Key>,
    pub(crate) searching: bool,
}

/// Builds an owned MoonTree representation from the store without exposing store borrows.
pub(crate) fn build(
    view: &StrategiesView,
    store: &CoreStore,
    cores: &crate::core_order::OrderedCores,
) -> MoonTreeBuild {
    let filter = &view.filter;
    let searching = filter.searching();
    let mut items = Vec::new();
    let mut data: HashMap<SharedString, NodeData> = HashMap::new();
    let mut expanded: Vec<SharedString> = Vec::new();
    let mut flat: Vec<Key> = Vec::new();

    for (core_id, core_name) in cores.iter() {
        let core = *core_id;
        let Some(cd) = store.core(core) else { continue };
        if cd.strategies.is_empty() || !cd.strategies.iter().any(|r| filter.matches(r)) {
            continue;
        }
        let core_open = searching || view.expanded_cores.contains(&core);
        let total = cd.strategies.iter().filter(|r| filter.counts(r)).count();
        let active = cd
            .strategies
            .iter()
            .filter(|r| filter.counts(r) && r.checked)
            .count();
        let open_orders_total = cd.orders.iter().filter(|o| !o.job_is_done).count();
        // Count the core's open orders by strategy ID.
        let order_counts: HashMap<u64, usize> = {
            let mut m = HashMap::new();
            for o in cd.orders.iter().filter(|o| !o.job_is_done) {
                *m.entry(o.strat_id).or_insert(0) += 1;
            }
            m
        };

        let cid = id_core(core);
        if core_open {
            expanded.push(cid.clone());
        }

        // Build the folder tree from visible strategies plus empty UI-only folders.
        let mut root = build_node(cd.strategies.iter().filter(|r| filter.matches(r)));
        for parts in view.ui_folder_paths(core) {
            ensure_folder(&mut root, &parts);
        }

        let mut children = Vec::new();
        let mut prefix: Vec<String> = Vec::new();
        convert_node(
            &root,
            core,
            &cd.strategies,
            &order_counts,
            &mut prefix,
            view,
            searching,
            core_open,
            &mut children,
            &mut data,
            &mut flat,
            &mut expanded,
        );

        // Append the Deleted folder after live strategies and filter its rows by name during search.
        let search_lc = filter.search.trim().to_lowercase();
        let del: Vec<&moon_core::strat_db::stats::HeadRow> = view
            .deleted
            .get(&core)
            .map(|v| {
                v.iter()
                    .filter(|h| search_lc.is_empty() || h.name.to_lowercase().contains(&search_lc))
                    .collect()
            })
            .unwrap_or_default();
        if !del.is_empty() {
            let did = id_del_folder(core);
            if searching || view.expanded_deleted.contains(&core) {
                expanded.push(did.clone());
            }
            let mut dchildren = Vec::new();
            for h in &del {
                let sid_u = h.strategy_id as u64;
                let key: Key = (core, sid_u);
                let dsid = id_del_strat(core, sid_u);
                data.insert(
                    dsid.clone(),
                    NodeData::DeletedStrategy {
                        core,
                        id: sid_u,
                        name: h.name.clone(),
                        kind: h.kind.clone(),
                        is_short: h.is_short,
                        highlighted: if view.sel.is_empty() {
                            view.selected == Some(key)
                        } else {
                            view.sel.contains(&key)
                        },
                    },
                );
                dchildren.push(MoonTreeItem::new(dsid, h.name.clone()));
            }
            data.insert(
                did.clone(),
                NodeData::DeletedFolder {
                    core,
                    count: del.len(),
                },
            );
            children.push(
                MoonTreeItem::new(did, rust_i18n::t!("strat.deleted_folder").to_string())
                    .folder(true)
                    .children(dchildren),
            );
        }

        data.insert(
            cid.clone(),
            NodeData::Core {
                core,
                label: core_name.clone(),
                active,
                total,
                open_orders: open_orders_total,
            },
        );
        items.push(
            MoonTreeItem::new(cid, core_name.clone())
                .folder(true)
                .children(children),
        );
    }

    MoonTreeBuild {
        items,
        node_data: data,
        expanded_ids: expanded,
        flat,
        searching,
    }
}

#[allow(clippy::too_many_arguments)]
fn convert_node(
    node: &super::super::logic::FolderNode,
    core: CoreId,
    all_strats: &[StrategyRow],
    order_counts: &HashMap<u64, usize>,
    prefix: &mut Vec<String>,
    view: &StrategiesView,
    searching: bool,
    ancestors_visible: bool,
    out: &mut Vec<MoonTreeItem>,
    data: &mut HashMap<SharedString, NodeData>,
    flat: &mut Vec<Key>,
    expanded: &mut Vec<SharedString>,
) {
    for (name, child) in &node.children {
        prefix.push(name.clone());
        let path = prefix.join("/");
        let fid = id_folder(core, &path);
        let fopen = searching || view.expanded_folders.contains(&(core, path.clone()));
        if fopen {
            expanded.push(fid.clone());
        }
        let (active, total) = folder_counts(all_strats, &view.filter, prefix);
        let mut fchildren = Vec::new();
        convert_node(
            child,
            core,
            all_strats,
            order_counts,
            prefix,
            view,
            searching,
            ancestors_visible && fopen,
            &mut fchildren,
            data,
            flat,
            expanded,
        );
        data.insert(
            fid.clone(),
            NodeData::Folder {
                core,
                path: prefix.clone(),
                label: name.clone(),
                active,
                total,
                selected: view.selected_folder.as_ref() == Some(&(core, prefix.join("/"))),
            },
        );
        out.push(
            MoonTreeItem::new(fid, name.clone())
                .folder(true)
                .children(fchildren),
        );
        prefix.pop();
    }

    for r in &node.strategies {
        let key: Key = (core, r.id);
        let sid = id_strat(core, r.id);
        let staged = view.staged.get(&key).copied();
        let highlighted = if view.sel.is_empty() {
            view.selected == Some(key)
        } else {
            view.sel.contains(&key)
        };
        if ancestors_visible {
            flat.push(key);
        }
        data.insert(
            sid.clone(),
            NodeData::Strategy {
                core,
                id: r.id,
                name: r.name.clone(),
                kind: r.kind.clone(),
                open_orders: order_counts.get(&r.id).copied().unwrap_or(0),
                server_checked: r.checked,
                staged,
                highlighted,
                is_short: r.is_short,
                drag_ids: view.drag_ids_for(core, r.id),
            },
        );
        out.push(MoonTreeItem::new(sid, r.name.clone()));
    }
}

impl StrategiesView {
    /// Builds the headless `MoonTree` element using the current frame's `data` side map.
    pub(super) fn moon_tree_el(
        &self,
        data: Rc<HashMap<SharedString, NodeData>>,
        cx: &Context<Self>,
    ) -> AnyElement {
        // WEAK only, never a strong `cx.entity()`. On every frame, `MoonTree` moves BOTH the row
        // renderer AND decorators into long-lived `MoonTreeState` (MoonUI `tree.rs`,
        // `impl RenderOnce for Tree`), while that state lives in THIS view. A strong handle in
        // either closure closes `StrategiesView -> tree_state -> closure -> StrategiesView`, so
        // the view and its subscriptions never drop. Both back-references must be weak; one is not
        // enough.
        let view = cx.entity().downgrade();

        // ── Row rendering ──
        let row_data = data.clone();
        let row_view = view.clone();
        let tree = MoonTree::custom(&self.tree_state, move |entry, meta, _window, app| {
            // A dead handle means the view is gone; render an empty row.
            let Some(row_view) = row_view.upgrade() else {
                return div().into_any_element();
            };
            render_row(&row_data, &row_view, entry, meta, app)
        })
        // ── DnD: strategies ──
        .draggable::<StratDrag, DragChip, _, _>(
            {
                let data = data.clone();
                move |entry, _meta| match data.get(entry.item().id()) {
                    Some(NodeData::Strategy { core, drag_ids, .. }) => Some(StratDrag {
                        core: *core,
                        ids: drag_ids.clone(),
                    }),
                    _ => None,
                }
            },
            |drag: &StratDrag, _pos, _window, app| {
                let n = drag.ids.len();
                app.new(|_| DragChip {
                    label: SharedString::from(if n > 1 {
                        format!("{n}×")
                    } else {
                        "≡".to_string()
                    }),
                })
            },
        )
        // ── DnD: folders ──
        .draggable::<FolderDrag, DragChip, _, _>(
            {
                let data = data.clone();
                move |entry, _meta| match data.get(entry.item().id()) {
                    Some(NodeData::Folder {
                        core, path, label, ..
                    }) => {
                        let _ = label;
                        Some(FolderDrag {
                            core: *core,
                            path: path.clone(),
                        })
                    }
                    _ => None,
                }
            },
            |_drag: &FolderDrag, _pos, _window, app| {
                app.new(|_| DragChip {
                    label: SharedString::from("▣"),
                })
            },
        )
        // ── Drop target: core or folder. Use one `can_drop` for both payload types because GPUI
        // stores only one slot; two drop targets would overwrite each other and disable dropping.
        // The decorator also supplies drag-over highlighting and payload-specific `on_drop`. ──
        .row_decorator({
            let data = data.clone();
            let view = view.clone();
            move |row, entry, _meta, _w, app| {
                let Some((core, target)) = drop_dest(&data, entry) else {
                    return row;
                };
                let p = MoonPalette::active(app);
                let hl = moon_alpha(p.blue, 0.22);
                let (vs, ts) = (view.clone(), target.clone());
                let (vf, tf) = (view.clone(), target.clone());
                row.can_drop(|drag, _w, _a| drag.is::<StratDrag>() || drag.is::<FolderDrag>())
                    .drag_over::<StratDrag>(move |s, _d, _w, _a| s.bg(hl))
                    .drag_over::<FolderDrag>(move |s, _d, _w, _a| s.bg(hl))
                    .on_drop::<StratDrag>(move |drag: &StratDrag, _w, app| {
                        let d = drag.clone();
                        // The decorator outlives the frame; a dead handle means the view is gone
                        // and there is nowhere to apply the drop.
                        let Some(vs) = vs.upgrade() else {
                            return;
                        };
                        vs.update(app, |this, cx| {
                            this.drop_strategies(core, ts.clone(), &d, cx)
                        });
                    })
                    .on_drop::<FolderDrag>(move |drag: &FolderDrag, _w, app| {
                        let d = drag.clone();
                        let Some(vf) = vf.upgrade() else {
                            return;
                        };
                        vf.update(app, |this, cx| this.drop_folder(core, tf.clone(), &d, cx));
                    })
            }
        });

        tree.into_any_element()
    }
}

/// Resolves a core-root or folder drop target as `(target core, path)`.
fn drop_dest(
    data: &HashMap<SharedString, NodeData>,
    entry: &MoonTreeEntry,
) -> Option<(CoreId, Vec<String>)> {
    match data.get(entry.item().id())? {
        NodeData::Core { core, .. } => Some((*core, Vec::new())),
        NodeData::Folder { core, path, .. } => Some((*core, path.clone())),
        NodeData::Strategy { .. }
        | NodeData::DeletedFolder { .. }
        | NodeData::DeletedStrategy { .. } => None,
    }
}

/// Renders one row from `NodeData`.
fn render_row(
    data: &HashMap<SharedString, NodeData>,
    view: &Entity<StrategiesView>,
    entry: &MoonTreeEntry,
    meta: MoonTreeRowMeta,
    app: &mut App,
) -> AnyElement {
    let _ = meta;
    let p = MoonPalette::active(app);
    let depth = entry.depth();
    let indent = design::ui_px(app, 6.0 + 12.0 * depth as f32);
    let Some(node) = data.get(entry.item().id()) else {
        return div().into_any_element();
    };

    match node {
        NodeData::Core {
            core,
            label,
            active,
            total,
            open_orders,
        } => {
            let core = *core;
            let txt = if *open_orders > 0 {
                format!("{label}  {active}/{total}  ({open_orders})")
            } else {
                format!("{label}  {active}/{total}")
            };
            core_folder_row(
                view,
                entry.is_expanded(),
                false,
                indent,
                txt,
                p.blue,
                600.0,
                ToggleTarget::Core(core),
                app,
            )
        }
        NodeData::Folder {
            core,
            path,
            label,
            active,
            total,
            selected,
        } => {
            let core = *core;
            let path = path.clone();
            let txt = format!("{label}  {active}/{total}");
            core_folder_row(
                view,
                entry.is_expanded(),
                *selected,
                indent,
                txt,
                p.text_soft,
                400.0,
                ToggleTarget::Folder(core, path),
                app,
            )
        }
        NodeData::Strategy {
            core,
            id,
            name,
            kind,
            open_orders,
            server_checked,
            staged,
            highlighted,
            is_short,
            ..
        } => strategy_row(
            view,
            *core,
            *id,
            name,
            kind,
            *open_orders,
            *server_checked,
            *staged,
            *highlighted,
            *is_short,
            indent,
            app,
        ),
        NodeData::DeletedFolder { core, count } => {
            let core = *core;
            core_folder_row(
                view,
                entry.is_expanded(),
                false,
                indent,
                format!("{}  {count}", rust_i18n::t!("strat.deleted_folder")),
                p.text_muted,
                400.0,
                ToggleTarget::Deleted(core),
                app,
            )
        }
        NodeData::DeletedStrategy {
            core,
            id,
            name,
            kind,
            is_short,
            highlighted,
        } => deleted_strategy_row(
            view,
            *core,
            *id,
            name,
            kind,
            *is_short,
            *highlighted,
            indent,
            app,
        ),
    }
}

enum ToggleTarget {
    Core(CoreId),
    Folder(CoreId, Vec<String>),
    /// The core's Deleted folder.
    Deleted(CoreId),
}

#[allow(clippy::too_many_arguments)]
fn core_folder_row(
    view: &Entity<StrategiesView>,
    expanded: bool,
    selected: bool,
    indent: Pixels,
    text: String,
    color: u32,
    weight: f32,
    target: ToggleTarget,
    app: &App,
) -> AnyElement {
    let p = MoonPalette::active(app);
    let marker = if expanded { "▼" } else { "▶" };
    // In this core/folder row type, only actual folders receive the egui-style context menu for
    // rename, copy, paste, create, and delete; core and Deleted headings do not.
    let menu = match &target {
        ToggleTarget::Folder(c, path) => Some((*c, path.clone())),
        ToggleTarget::Core(_) | ToggleTarget::Deleted(_) => None,
    };
    let view_click = view.clone();
    let view_menu = view.clone();
    h_flex()
        .id(SharedString::from(format!("strat-tree-cf-{text}")))
        .w_full()
        .h(design::fit_h_px(app, 23.0, 14.0, 4.5))
        .pl(indent)
        .pr(design::ui_px(app, 6.0))
        .items_center()
        .gap(design::ui_px(app, 5.0))
        .cursor_pointer()
        .rounded(design::ui_px(app, 3.0))
        .when(selected, |s| s.bg(moon_alpha(p.amber, 0.14)))
        .hover(move |s| s.bg(moon_alpha(p.panel, 0.74)))
        .child(
            MoonText::new(marker)
                .mono(true)
                .uppercase(false)
                .color(p.text_muted)
                .render(),
        )
        .child(
            MoonText::new(text)
                .mono(true)
                .uppercase(false)
                .color(color)
                .weight(weight)
                .render(),
        )
        .on_click(move |_e, _window, app| {
            view_click.update(app, |this, cx| {
                match &target {
                    ToggleTarget::Core(c) => {
                        toggle(&mut this.expanded_cores, *c);
                        this.selected_folder = None;
                    }
                    ToggleTarget::Folder(c, path) => {
                        toggle(&mut this.expanded_folders, (*c, path.join("/")));
                        // Match Moonbot by selecting the clicked folder for highlighting and Ctrl+C.
                        this.selected_folder = Some((*c, path.join("/")));
                    }
                    ToggleTarget::Deleted(c) => {
                        toggle(&mut this.expanded_deleted, *c);
                        this.selected_folder = None;
                    }
                }
                cx.notify();
            });
        })
        .when_some(menu, |row, (core, path)| {
            row.on_mouse_down(
                MouseButton::Right,
                move |e: &MouseDownEvent, window, app| {
                    app.stop_propagation();
                    let pos = e.position;
                    let path = path.clone();
                    view_menu.update(app, |this, cx| {
                        this.open_menu(
                            ContextMenu {
                                core,
                                target: MenuTarget::Folder(path),
                                pos,
                            },
                            window,
                            cx,
                        );
                    });
                },
            )
        })
        .into_any_element()
}

/// Renders a muted deleted-strategy row without a checkbox or DnD.
/// Clicking selects it and jumps to its latest version; right-click opens its Restore context menu.
#[allow(clippy::too_many_arguments)]
fn deleted_strategy_row(
    view: &Entity<StrategiesView>,
    core: CoreId,
    id: u64,
    name: &str,
    kind: &str,
    is_short: bool,
    highlighted: bool,
    indent: Pixels,
    app: &App,
) -> AnyElement {
    let p = MoonPalette::active(app);
    let key: Key = (core, id);
    let view_click = view.clone();
    let view_menu = view.clone();
    let mut name_row = h_flex()
        .id(SharedString::from(format!("dstrat-{core}-{id}")))
        .flex_1()
        .min_w_0()
        .h(design::fit_h_px(app, 23.0, 14.0, 4.5))
        .items_center()
        .justify_between()
        .gap(design::ui_px(app, 6.0))
        .px(design::ui_px(app, 6.0))
        .rounded(design::ui_px(app, 3.0))
        .border_1()
        .border_color(moon_alpha(p.border, 0.0))
        .cursor_pointer()
        .child(
            div().flex_1().min_w_0().truncate().child(
                MoonText::new(name.to_string())
                    .mono(true)
                    .uppercase(false)
                    .color(p.text_muted)
                    .render(),
            ),
        )
        .child(
            MoonBadge::new(kind.to_string())
                .tone(if is_short {
                    MoonTone::Negative
                } else {
                    MoonTone::Muted
                })
                .variant(MoonBadgeVariant::Soft)
                .size(MoonBadgeSize::Tiny)
                .render_with_palette(p),
        )
        .on_click(move |_e, window, app| {
            view_click.update(app, |this, cx| {
                window.focus(&this.focus, cx);
                this.select_deleted_strategy(key, cx);
            });
        })
        .on_mouse_down(
            MouseButton::Right,
            move |e: &MouseDownEvent, window, app| {
                app.stop_propagation();
                let pos = e.position;
                view_menu.update(app, |this, cx| {
                    this.select_deleted_strategy(key, cx);
                    this.open_menu(
                        ContextMenu {
                            core,
                            target: MenuTarget::DeletedStrategy(id),
                            pos,
                        },
                        window,
                        cx,
                    );
                });
            },
        );
    if highlighted {
        name_row = name_row
            .bg(moon_alpha(p.amber, 0.16))
            .border_color(moon_alpha(p.amber, 0.55));
    } else {
        name_row = name_row.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)));
    }
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(app, 6.0))
        .pl(indent)
        .pr(design::ui_px(app, 2.0))
        .py(design::ui_px(app, 1.0))
        .child(
            MoonText::new("✕")
                .mono(true)
                .uppercase(false)
                .color(p.text_muted)
                .render(),
        )
        .child(name_row)
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn strategy_row(
    view: &Entity<StrategiesView>,
    core: CoreId,
    id: u64,
    name: &str,
    kind: &str,
    open_orders: usize,
    server_checked: bool,
    staged: Option<bool>,
    highlighted: bool,
    is_short: bool,
    indent: Pixels,
    app: &App,
) -> AnyElement {
    let p = MoonPalette::active(app);
    let key: Key = (core, id);
    let val = staged.unwrap_or(server_checked);
    let dot = if server_checked {
        p.green
    } else {
        p.text_muted
    };
    let kind_txt = if open_orders > 0 {
        format!("{kind}({open_orders})")
    } else {
        kind.to_string()
    };

    // Make the name and kind/open-order count the clickable selection area.
    let view_click = view.clone();
    let view_menu = view.clone();
    let mut name_row = h_flex()
        .id(SharedString::from(format!("strat-{core}-{id}")))
        .flex_1()
        .min_w_0()
        .h(design::fit_h_px(app, 23.0, 14.0, 4.5))
        .items_center()
        .justify_between()
        .gap(design::ui_px(app, 6.0))
        .px(design::ui_px(app, 6.0))
        .rounded(design::ui_px(app, 3.0))
        .border_1()
        .border_color(moon_alpha(p.border, 0.0))
        .cursor_pointer()
        .child(
            div().flex_1().min_w_0().truncate().child(
                MoonText::new(name.to_string())
                    .mono(true)
                    .uppercase(false)
                    .color(p.text)
                    .render(),
            ),
        )
        .child(
            // Distinguish direction through the kind badge: SHORT is orange (`Negative`) and LONG
            // is greenish (`Positive`).
            MoonBadge::new(kind_txt)
                .tone(if is_short {
                    MoonTone::Negative
                } else {
                    MoonTone::Positive
                })
                .variant(MoonBadgeVariant::Soft)
                .size(MoonBadgeSize::Tiny)
                .render_with_palette(p),
        )
        .on_click(move |e: &ClickEvent, window, app| {
            let m = e.modifiers();
            let shift = m.shift;
            let cmd = m.secondary();
            view_click.update(app, |this, cx| {
                window.focus(&this.focus, cx);
                let order = this.flat_order.clone();
                if this.apply_click(key, &order, shift, cmd) {
                    this.clamp_selected_section(cx);
                    cx.notify();
                }
            });
        })
        .on_mouse_down(
            MouseButton::Right,
            move |e: &MouseDownEvent, window, app| {
                app.stop_propagation();
                let pos = e.position;
                view_menu.update(app, |this, cx| {
                    if !this.sel.contains(&key) {
                        this.focus_strategy(key);
                        this.clamp_selected_section(cx);
                    }
                    this.open_menu(
                        ContextMenu {
                            core,
                            target: MenuTarget::Strategy(id),
                            pos,
                        },
                        window,
                        cx,
                    );
                });
            },
        );
    if highlighted {
        name_row = name_row
            .bg(moon_alpha(p.amber, 0.16))
            .border_color(moon_alpha(p.amber, 0.55));
    } else {
        name_row = name_row.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)));
    }

    let view_chk = view.clone();
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(app, 6.0))
        .pl(indent)
        .pr(design::ui_px(app, 2.0))
        .py(design::ui_px(app, 1.0))
        .child(
            // Use a green tone for enabled/active. The default Info tone produced a pale blue box
            // that was indistinguishable from empty on the light theme; Positive also makes the
            // checkmark glyph green.
            MoonCheckbox::new(SharedString::from(format!("chk-{core}-{id}")))
                .checked(val)
                .tone(MoonTone::Positive)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |ch: &bool, _window, app| {
                    let v = *ch;
                    view_chk.update(app, |this, cx| {
                        let before = this.staged.get(&key).copied();
                        if v == server_checked {
                            this.staged.remove(&key);
                        } else {
                            this.staged.insert(key, v);
                        }
                        if before != this.staged.get(&key).copied() {
                            cx.notify();
                        }
                    });
                }),
        )
        .child(
            MoonText::new("●")
                .mono(true)
                .uppercase(false)
                .color(dot)
                .render(),
        )
        .child(name_row)
        .into_any_element()
}
