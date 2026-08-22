//! Strategy tree built on MoonUI's headless `Tree::custom` mode.
//! MoonTree flattens `expanded_ids`, virtualizes rows, handles keyboard input, and supplies row
//! hitboxes to DnD decorators. Selection, staging, and expansion remain in `StrategiesView`; this
//! module adapts `CoreStore` to `MoonTreeItem`, builds the `id -> NodeData` side map for row and drag
//! data, and renders rows and decorators. Callbacks outside `Context<Self>` mutate through
//! `Entity::update`.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonCheckbox, MoonCheckboxSize, MoonDisclosure,
    MoonPalette, MoonText, MoonTone, MoonTree, MoonTreeEntry, MoonTreeItem, MoonTreeRowMeta,
    h_flex,
};

use super::super::filter::PreparedFilter;
use super::super::logic::{
    FolderCounts, build_node, ensure_folder, strategy_core_is_visible, toggle,
};
use super::super::{Key, StrategiesView, moon_alpha};
use super::ui::{ContextMenu, DragChip, FolderDrag, MenuTarget, StratDrag};
use crate::design;
use moon_core::feed::StrategyRow;
use moon_core::session::{CoreId, CoreStore};
use moon_core::venue::CoreVenue;

#[cfg(test)]
mod tests;

// ── Node ID encoding: stable string IDs for MoonTree ─────────────────────────
/// Build the stable tree ID for an exchange identity or the single unidentified section.
///
/// Args:
///     venue: Nameable venue behind a section, or `None` for every unidentified core.
///
/// Returns:
///     Identity-derived tree ID that is independent of localized or wire-reported captions.
fn id_exchange(venue: Option<&CoreVenue>) -> SharedString {
    match venue {
        Some(venue) => SharedString::from(format!("x:{}:{}", venue.id.code, venue.id.dex)),
        None => SharedString::from("x:unknown"),
    }
}

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
    /// Always-expanded, non-interactive heading for one canonical exchange section.
    Exchange {
        label: String,
        logo: Option<Arc<RenderImage>>,
    },
    Core {
        core: CoreId,
        label: String,
        active: usize,
        total: usize,
        open_orders: usize,
        selected: bool,
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
        /// The whole selection of this core when the row belongs to it, else `None` — a row
        /// outside the selection drags only its own `id`.
        ///
        /// Sharing one list across selected rows keeps large multi-selections linear, while the
        /// `None` case keeps ordinary unselected rows allocation-free.
        drag_ids: Option<Rc<[u64]>>,
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

/// Builds the visible MoonTree forest and row data without exposing store borrows.
///
/// Collapsed cores contribute only their root row and totals; open cores are each scanned once for
/// visible rows and folder counts. The exchange filter excludes whole sections or cores before
/// row-level filtering; the display preference then wraps the retained nonempty cores in canonical
/// exchange sections or emits those same core roots directly.
///
/// Args:
///     view: Strategies state providing filters, expansion, and selection.
///     store: Current per-core strategy and order snapshot.
///     cores: Visible cores in canonical member order.
///     venues: Session-owned venue identities used by the shared section partition.
///
/// Returns:
///     Visible grouped or flat forest, row data, expansion IDs, and flat strategy order.
pub(crate) fn build(
    view: &StrategiesView,
    store: &CoreStore,
    cores: &crate::core_order::OrderedCores,
    venues: &HashMap<CoreId, CoreVenue>,
) -> MoonTreeBuild {
    let filter = view.filter.prepare();
    let searching = filter.searching();
    let mut items = Vec::new();
    let mut data: HashMap<SharedString, NodeData> = HashMap::new();
    let mut expanded: Vec<SharedString> = Vec::new();
    let mut flat: Vec<Key> = Vec::new();

    if view.prefs.group_by_venue {
        let sections = crate::core_order::exchange_sections(
            cores
                .iter()
                .enumerate()
                .map(|(index, (core, _))| (index, venues.get(core))),
        );
        for (venue, members) in sections {
            // Skipped whole, heading included: a section the exchange filter excludes has nothing
            // left to caption. Asked through the filter's own predicate, which resolves the section
            // exactly as `exchange_sections` did when it put these members here.
            if !view.filter.core_matches(venue) {
                continue;
            }
            let mut section_children = Vec::new();
            for member in members {
                let (core, core_name) = &cores[member];
                if let Some(root) = build_core_root(
                    view,
                    store,
                    *core,
                    core_name,
                    &filter,
                    searching,
                    (&mut data, &mut flat, &mut expanded),
                ) {
                    section_children.push(root);
                }
            }

            if section_children.is_empty() {
                continue;
            }
            let exchange_id = id_exchange(venue);
            let label = crate::controls::venue_section_label(venue);
            let logo = view
                .exchange_logos_ready
                .then_some(venue)
                .flatten()
                .and_then(|venue| venue.brand())
                .and_then(crate::media::exchange_logos::exchange_logo);
            expanded.push(exchange_id.clone());
            data.insert(
                exchange_id.clone(),
                NodeData::Exchange {
                    label: label.clone(),
                    logo,
                },
            );
            items.push(
                MoonTreeItem::new(exchange_id, label)
                    .folder(false)
                    .disabled(true)
                    .children(section_children),
            );
        }
    } else {
        for (core, core_name) in cores.iter() {
            // The same exclusion one core at a time: ungrouped mode draws no headings, so the
            // filter has to be applied per core rather than per section. Both branches ask the
            // SAME predicate, so grouping cannot change which cores a selection keeps.
            if !view.filter.core_matches(venues.get(core)) {
                continue;
            }
            if let Some(root) = build_core_root(
                view,
                store,
                *core,
                core_name,
                &filter,
                searching,
                (&mut data, &mut flat, &mut expanded),
            ) {
                items.push(root);
            }
        }
    }

    MoonTreeBuild {
        items,
        node_data: data,
        expanded_ids: expanded,
        flat,
        searching,
    }
}

/// Build one visible core root with identical contents in grouped and flat tree modes.
///
/// Args:
///     view: Strategies state providing expansion, selection, and retained folders.
///     store: Current per-core strategy and order snapshot.
///     core: Core whose root is being built.
///     core_name: Canonical display name for the root row.
///     filter: Prepared row predicate shared by the complete build.
///     searching: Whether search forces the core and its folders open.
///     outputs: Side map, visible strategy order, and expanded ids receiving this root's output.
///
/// Returns:
///     The core root, or `None` when no live strategy matches the visibility predicate.
fn build_core_root(
    view: &StrategiesView,
    store: &CoreStore,
    core: CoreId,
    core_name: &str,
    filter: &PreparedFilter,
    searching: bool,
    outputs: (
        &mut HashMap<SharedString, NodeData>,
        &mut Vec<Key>,
        &mut Vec<SharedString>,
    ),
) -> Option<MoonTreeItem> {
    let (data, flat, expanded) = outputs;
    let cd = store.core(core)?;
    // Nothing below a collapsed core can render, so it needs only the totals in its own caption.
    // Search and reveal paths force their required core/folder chain open before this build runs.
    let core_open = searching || view.expanded_cores.contains(&core);

    // One pass feeds both the visible set and every folder count.
    let mut counts = if core_open {
        FolderCounts::default()
    } else {
        FolderCounts::totals_only()
    };
    let mut matched: Vec<&StrategyRow> = Vec::new();
    let mut any_matched = false;
    for row in &cd.strategies {
        counts.add(row, filter);
        if filter.matches(row) {
            any_matched = true;
            if core_open {
                matched.push(row);
            }
        }
    }
    if !any_matched {
        return None;
    }
    let (active, total) = counts.root();
    let open_orders_total = cd.orders.iter().filter(|order| !order.job_is_done).count();

    let cid = id_core(core);
    let mut children = Vec::new();
    if core_open {
        expanded.push(cid.clone());
        build_core_subtree(
            view,
            cd,
            core,
            filter,
            searching,
            &counts,
            &matched,
            &mut children,
            data,
            flat,
            expanded,
        );
    }

    data.insert(
        cid.clone(),
        NodeData::Core {
            core,
            label: core_name.to_string(),
            active,
            total,
            open_orders: open_orders_total,
            selected: view
                .selected_folder
                .as_ref()
                .is_some_and(|(selected_core, path)| *selected_core == core && path.is_empty()),
        },
    );
    Some(
        MoonTreeItem::new(cid, core_name.to_string())
            .folder(true)
            .children(children),
    )
}

/// Builds the folder and strategy rows of one open core, followed by its Deleted folder.
#[allow(clippy::too_many_arguments)]
fn build_core_subtree(
    view: &StrategiesView,
    cd: &moon_core::session::store::CoreData,
    core: CoreId,
    filter: &PreparedFilter,
    searching: bool,
    counts: &FolderCounts,
    matched: &[&StrategyRow],
    children: &mut Vec<MoonTreeItem>,
    data: &mut HashMap<SharedString, NodeData>,
    flat: &mut Vec<Key>,
    expanded: &mut Vec<SharedString>,
) {
    let mut order_counts: HashMap<u64, usize> = HashMap::new();
    for o in cd.orders.iter().filter(|o| !o.job_is_done) {
        *order_counts.entry(o.strat_id).or_insert(0) += 1;
    }
    // Every selected row in this core shares the same drag payload.
    let selected_ids: Rc<[u64]> = view.drag_ids_for_core(core);
    // Borrowed once per core so the per-folder probe below needs no owned key.
    let open_folders: std::collections::HashSet<&str> = view
        .expanded_folders
        .iter()
        .filter(|(c, _)| *c == core)
        .map(|(_, p)| p.as_str())
        .collect();

    // Build the folder tree from visible strategies plus empty UI-only folders.
    let mut root = build_node(matched.iter().copied());
    for parts in view.ui_folder_paths(core) {
        ensure_folder(&mut root, &parts);
    }

    let mut prefix: Vec<String> = Vec::new();
    convert_node(
        &root,
        core,
        counts,
        &order_counts,
        &selected_ids,
        &open_folders,
        &mut prefix,
        view,
        searching,
        children,
        data,
        flat,
        expanded,
    );

    // Append the Deleted folder after live strategies and filter its rows by name during search.
    let del: Vec<&moon_core::strat_db::stats::HeadRow> = view
        .deleted
        .get(&core)
        .map(|v| {
            v.iter()
                .filter(|h| {
                    filter
                        .query()
                        .is_none_or(|q| h.name.to_lowercase().contains(q))
                })
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
}

/// Hashes the tree shape that [`MoonTreeState`] renders from: node ids, labels, folder flags,
/// nesting, the expanded set, and the search-forced expansion.
///
/// Equal signatures allow the caller to skip an otherwise redundant forest push. Row contents are
/// intentionally excluded because they live in `NodeData`, which is rebuilt every frame and is not
/// stored by MoonTree.
pub(crate) fn shape_sig(items: &[MoonTreeItem], expanded: &[SharedString], searching: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    searching.hash(&mut h);
    /// Adds one item subtree to the structural signature.
    fn walk(items: &[MoonTreeItem], h: &mut impl Hasher) {
        items.len().hash(h);
        for it in items {
            it.id().hash(h);
            it.label.hash(h);
            it.is_folder().hash(h);
            walk(&it.children, h);
        }
    }
    walk(items, &mut h);
    expanded.hash(&mut h);
    h.finish()
}

/// Converts one folder node and its subtree.
///
/// Every node reached here is visible, so recursion stops at a closed folder because
/// `MoonTreeState` cannot render its descendants.
#[allow(clippy::too_many_arguments)]
fn convert_node(
    node: &super::super::logic::FolderNode,
    core: CoreId,
    counts: &FolderCounts,
    order_counts: &HashMap<u64, usize>,
    selected_ids: &Rc<[u64]>,
    open_folders: &std::collections::HashSet<&str>,
    prefix: &mut Vec<String>,
    view: &StrategiesView,
    searching: bool,
    out: &mut Vec<MoonTreeItem>,
    data: &mut HashMap<SharedString, NodeData>,
    flat: &mut Vec<Key>,
    expanded: &mut Vec<SharedString>,
) {
    for (name, child) in &node.children {
        prefix.push(name.clone());
        // One joined path keeps the node id, expansion probe, selection comparison, and count
        // lookup on the same folder identity without repeated allocation.
        let path = prefix.join("/");
        let fid = id_folder(core, &path);
        let fopen = searching || open_folders.contains(path.as_str());
        let (active, total) = counts.for_path(&path);
        let mut fchildren = Vec::new();
        if fopen {
            expanded.push(fid.clone());
            convert_node(
                child,
                core,
                counts,
                order_counts,
                selected_ids,
                open_folders,
                prefix,
                view,
                searching,
                &mut fchildren,
                data,
                flat,
                expanded,
            );
        }
        data.insert(
            fid.clone(),
            NodeData::Folder {
                core,
                path: prefix.clone(),
                label: name.clone(),
                active,
                total,
                selected: view.selected_folder.as_ref() == Some(&(core, path)),
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
        let in_sel = view.sel.contains(&key);
        let highlighted = if view.sel.is_empty() {
            view.selected == Some(key)
        } else {
            in_sel
        };
        flat.push(key);
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
                drag_ids: in_sel.then(|| selected_ids.clone()),
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
                    Some(NodeData::Strategy {
                        core, id, drag_ids, ..
                    }) => Some(StratDrag {
                        core: *core,
                        ids: drag_ids
                            .as_ref()
                            .map_or_else(|| vec![*id], |ids| ids.to_vec()),
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
        NodeData::Exchange { .. } => None,
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
    crate::diag::bump(&crate::diag::STRAT_ROW_RENDER);
    let p = MoonPalette::active(app);
    let depth = entry.depth();
    let indent = design::ui_px(app, 6.0 + 12.0 * depth as f32);
    let node_id = entry.item().id().clone();
    let Some(node) = data.get(entry.item().id()) else {
        return div().into_any_element();
    };

    match node {
        NodeData::Exchange { label, logo } => {
            exchange_row(node_id, indent, label, logo.clone(), app)
        }
        NodeData::Core {
            core,
            label,
            active,
            total,
            open_orders,
            selected,
        } => {
            let core = *core;
            let txt = if *open_orders > 0 {
                format!("{label}  {active}/{total}  ({open_orders})")
            } else {
                format!("{label}  {active}/{total}")
            };
            core_folder_row(
                view,
                node_id,
                entry.is_expanded(),
                *selected,
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
                node_id,
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
                node_id,
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

/// Render a passive exchange section heading above its core children.
///
/// Args:
///     row_id: Stable exchange identity used as the row element ID.
///     indent: Tree-provided indentation for the heading depth.
///     label: Localized shared venue-section caption.
///     logo: Prewarmed brand logo, absent for unidentified venues.
///     app: Application context providing palette and scaled geometry.
///
/// Returns:
///     Non-interactive hierarchy row with no selection, hover, disclosure, drag, or drop behavior.
fn exchange_row(
    row_id: SharedString,
    indent: Pixels,
    label: &str,
    logo: Option<Arc<RenderImage>>,
    app: &App,
) -> AnyElement {
    let p = MoonPalette::active(app);
    h_flex()
        .id(row_id)
        .w_full()
        .h(design::fit_h_px(app, 23.0, 14.0, 4.5))
        .pl(indent)
        .pr(design::ui_px(app, 6.0))
        .items_center()
        .gap(design::ui_px(app, 6.0))
        .when_some(logo, |row, logo| {
            row.child(
                img(logo)
                    .flex_none()
                    .w(design::ui_px(app, 13.0))
                    .h(design::ui_px(app, 13.0))
                    .rounded(design::ui_px(app, 2.0)),
            )
        })
        .child(
            div().flex_1().min_w_0().truncate().child(
                MoonText::new(label.to_string())
                    .mono(true)
                    .uppercase(false)
                    .color(p.text_soft)
                    .weight(600.0)
                    .render(),
            ),
        )
        .into_any_element()
}

enum ToggleTarget {
    Core(CoreId),
    Folder(CoreId, Vec<String>),
    /// The core's Deleted folder.
    Deleted(CoreId),
}

#[allow(clippy::too_many_arguments)]
/// Render a clickable core, folder, or Deleted heading in the strategy tree.
///
/// Actual folders also receive the context menu assembled inside this function. The disclosure
/// glyph remains passive because the enclosing row handles interaction.
///
/// Args:
///     view: Strategies view updated by row interactions.
///     row_id: The node's own tree id, used verbatim as this row's `ElementId`.
///     expanded: Whether the heading's children are visible.
///     selected: Whether to draw the selected-folder highlight.
///     indent: Leading indentation for the tree depth.
///     text: Heading label and count summary.
///     color: Heading text color.
///     weight: Heading font weight.
///     target: Core, folder, or Deleted collection toggled by the row.
///     app: Application context used for palette and sizing tokens.
///
/// Returns:
///     The complete interactive tree row.
fn core_folder_row(
    view: &Entity<StrategiesView>,
    row_id: SharedString,
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
    // In this core/folder row type, only actual folders receive the egui-style context menu for
    // rename, copy, paste, create, and delete; core and Deleted headings do not.
    let menu = match &target {
        ToggleTarget::Folder(c, path) => Some((*c, path.clone())),
        ToggleTarget::Core(_) | ToggleTarget::Deleted(_) => None,
    };
    let view_click = view.clone();
    let view_menu = view.clone();
    h_flex()
        // The node's own id, NEVER the rendered text. GPUI keeps `pending_mouse_down` in element
        // state looked up by `ElementId`, so an id derived from the caption ("core  3/10  (2)")
        // becomes a DIFFERENT element the moment a counter moves — and a repaint between press and
        // release, which a trading core produces constantly, silently discards the click. That was
        // the "clicking a folder sometimes does not expand it" defect.
        .id(row_id)
        .w_full()
        .h(design::fit_h_px(app, 23.0, 14.0, 4.5))
        .pl(indent)
        .pr(design::ui_px(app, 6.0))
        .items_center()
        .gap(design::ui_px(app, 5.0))
        .cursor_pointer()
        .rounded(design::ui_px(app, 3.0))
        .when(selected, |s| s.bg(moon_alpha(p.amber, 0.14)))
        .when(!selected, |s| {
            s.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)))
        })
        // Passive: the whole row carries the click that expands or collapses this node, so the
        // marker stays hitbox-free and cannot swallow it.
        // Chrome, so it tracks the UI slider rather than the row's own text scale.
        .child(
            MoonDisclosure::glyph(expanded)
                .size(design::DISCLOSURE_GLYPH_MARKER)
                .box_size(design::DISCLOSURE_BOX),
        )
        .child(
            MoonText::new(text)
                .mono(true)
                .uppercase(false)
                .color(color)
                .weight(weight)
                .render(),
        )
        .on_click(move |_e, window, app| {
            view_click.update(app, |this, cx| {
                window.focus(&this.focus, cx);
                match &target {
                    ToggleTarget::Core(c) => {
                        toggle(&mut this.expanded_cores, *c);
                        this.selected_folder = Some((*c, String::new()));
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
/// Render one strategy row already admitted by the effective workspace tree.
///
/// Args:
///     view: Owning Strategies view used by row callbacks.
///     core: Workspace-visible core containing the strategy.
///     id: Live strategy id within the core.
///     name: Displayed strategy name.
///     kind: Displayed strategy kind.
///     open_orders: Current open-order count.
///     server_checked: Checkbox state acknowledged by the core.
///     staged: Visible retained checkbox override, when one exists for this row.
///     highlighted: Whether filtering should emphasize the row.
///     is_short: Whether the strategy trades the short side.
///     indent: Tree indentation for the row.
///     app: Application context used for theme and design tokens.
///
/// Returns:
///     The rendered row; staging retained on hidden Classic cores never reaches this function.
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
                        if !strategy_core_is_visible(this.workspace_cores.as_deref(), key.0) {
                            return;
                        }
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
