//! MoonTree shape helpers.

use super::*;

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

/// One core's folder-keyed expansion flags, borrowed for the length of that core's subtree build.
///
/// Keyed by the slash-joined path the tree already computes per folder, so a probe costs one hash
/// of a borrowed string and no allocation. Bulk-check display is derived per row from staged
/// overlays, not retained here.
pub(super) struct FolderSets<'a> {
    /// Paths whose children are currently rendered.
    pub(super) open: std::collections::HashSet<&'a str>,
}

/// Borrow one core's paths out of a folder-keyed window set.
pub(super) fn core_paths(
    set: &std::collections::HashSet<(CoreId, String)>,
    core: CoreId,
) -> std::collections::HashSet<&str> {
    set.iter()
        .filter(|(c, _)| *c == core)
        .map(|(_, path)| path.as_str())
        .collect()
}

/// One rendered child, ordered jointly with folders and strategies by its first wire position.
pub(super) enum OrderedSibling<'node, 'row> {
    /// A folder heading carries its whole rendered subtree at this position.
    Folder {
        path: String,
        name: &'node str,
        child: &'node super::super::super::logic::FolderNode<'row>,
    },
    /// A strategy directly inside this parent is a one-row sibling block.
    Strategy(&'row StrategyRow),
}

/// Merge child folders and loose strategies by the complete displayed order.
/// Folder ranks include filtered-out descendants; empty folders sort after positioned siblings.
/// The returned sequence drives rendering and keyboard navigation together.
///
/// Args:
///     node: Folder-tree node whose direct folders and strategies are merged.
///     parent: Canonical path preceding the direct children.
///     counts: Folder occupancy and ranks in the displayed order.
///     row_ranks: Display ranks for direct strategies, including a pending reorder overlay.
///
/// Returns:
///     Direct child folders and strategies in their shared displayed order.
pub(super) fn ordered_siblings<'node, 'row>(
    node: &'node super::super::super::logic::FolderNode<'row>,
    parent: &str,
    counts: &FolderCounts,
    row_ranks: &HashMap<u64, usize>,
) -> Vec<OrderedSibling<'node, 'row>> {
    let mut siblings: Vec<_> = node
        .children()
        .map(|(name, child)| {
            let path = if parent.is_empty() {
                name.to_string()
            } else {
                format!("{parent}/{name}")
            };
            (
                counts.order_of(&path).unwrap_or(usize::MAX),
                OrderedSibling::Folder { path, name, child },
            )
        })
        .collect();
    siblings.extend(node.strategies.iter().map(|row| {
        (
            row_ranks.get(&row.id).copied().unwrap_or(usize::MAX),
            OrderedSibling::Strategy(row),
        )
    }));
    siblings.sort_by_key(|(rank, _)| *rank);
    siblings.into_iter().map(|(_, sibling)| sibling).collect()
}

/// Converts one folder node and its subtree.
///
/// Folders and loose strategies share wire order, including a pending folder move.
/// Every node reached here is visible, so recursion stops at a closed folder because
/// `MoonTreeState` cannot render its descendants. `engine` is the owning core's confirmed
/// global-engine flag, copied onto folder headings and strategies so their activity matches
/// the core row.
/// `row_ranks` lets direct strategies join folder siblings at their pending displayed positions.
///
/// Args:
///     node: Visible folder-tree node to convert.
///     core: Owning core for output ids and metadata.
///     counts: Filter-aware folder totals and ranks.
///     row_ranks: Display rank for each strategy, including a pending reorder overlay.
///     confirmed_folders: Core-owned folder paths, folded for lookup.
///     order_counts: Open-order counts by strategy id.
///     selected_ids: Shared drag payload for selected strategies in the core.
///     folders: Open-folder paths for the core.
///     prefix: Current canonical path, extended during recursion.
///     view: Strategies state that owns selection and staged checkbox state.
///     strategies: Live strategies used for checkbox coverage.
///     filter: Prepared predicate shared by the complete tree build.
///     searching: Whether a text query forces child folders open.
///     engine: Confirmed global-engine state copied onto folder headings.
///     out: Output rows for this subtree.
///     data: Output metadata keyed by tree id.
///     flat: Output strategy ids in visual order.
///     nav: Output node order for keyboard navigation.
///     expanded: Output ids that MoonTree should draw open.
///
/// Returns:
///     Nothing; all rendered subtree data is appended to the supplied outputs.
#[allow(clippy::too_many_arguments)]
pub(super) fn convert_node(
    node: &super::super::super::logic::FolderNode,
    core: CoreId,
    counts: &FolderCounts,
    row_ranks: &HashMap<u64, usize>,
    confirmed_folders: &std::collections::HashSet<String>,
    order_counts: &HashMap<u64, usize>,
    selected_ids: &Rc<[u64]>,
    folders: &FolderSets<'_>,
    prefix: &mut Vec<String>,
    view: &StrategiesView,
    strategies: &[StrategyRow],
    filter: &PreparedFilter,
    searching: bool,
    engine: Option<bool>,
    out: &mut Vec<MoonTreeItem>,
    data: &mut HashMap<SharedString, NodeData>,
    flat: &mut Vec<Key>,
    nav: &mut Vec<ops::NavNode>,
    expanded: &mut Vec<SharedString>,
) {
    let parent = prefix.join("/");
    for sibling in ordered_siblings(node, &parent, counts, row_ranks) {
        match sibling {
            OrderedSibling::Folder { path, name, child } => {
                prefix.push(name.to_string());
                let fid = id_folder(core, &path);
                let fopen = searching || folders.open.contains(path.as_str());
                // Read before `path` is moved into the selection comparison below.
                let fchecked = subtree_displayed_all_checked(
                    &subtree_check_targets(strategies, prefix, filter),
                    &view.staged,
                    core,
                );
                let (active, total) = counts.for_path(&path);
                // Asked of the COUNTS, not of `total`, which the kind and direction filters narrow: a
                // folder whose strategies are all filtered away is not an empty folder, and drawing it as
                // one would take its caret away while its contents are one filter click from returning.
                let fill = match counts.knows(&path) {
                    true => FolderFill::Populated,
                    false => match confirmed_folders.contains(&path.to_lowercase()) {
                        true => FolderFill::EmptyOnCore,
                        false => FolderFill::EmptyLocal,
                    },
                };
                // The folder row itself, before whatever it contains. A CLOSED folder is still pushed —
                // it is drawn — while its children are not, which is what keeps the order navigable.
                nav.push(ops::NavNode::Folder(core, path.clone()));
                let mut fchildren = Vec::new();
                if fopen {
                    expanded.push(fid.clone());
                    convert_node(
                        child,
                        core,
                        counts,
                        row_ranks,
                        confirmed_folders,
                        order_counts,
                        selected_ids,
                        folders,
                        prefix,
                        view,
                        strategies,
                        filter,
                        searching,
                        engine,
                        &mut fchildren,
                        data,
                        flat,
                        nav,
                        expanded,
                    );
                }
                data.insert(
                    fid.clone(),
                    NodeData::Folder {
                        core,
                        path: prefix.clone(),
                        label: name.to_string(),
                        active,
                        total,
                        selected: view.folder_sel.contains(&(core, path)),
                        checked: fchecked,
                        fill,
                        engine,
                    },
                );
                out.push(
                    MoonTreeItem::new(fid, name.to_string())
                        .folder(true)
                        .children(fchildren),
                );
                prefix.pop();
            }
            OrderedSibling::Strategy(r) => {
                let key: Key = (core, r.id);
                let sid = id_strat(core, r.id);
                let staged = view.staged.get(&key).copied();
                let cut = view
                    .cut
                    .as_ref()
                    .is_some_and(|cut| cut.dims(key, &r.folder_path));
                let in_sel = view.sel.contains(&key);
                let highlighted = if view.sel.is_empty() {
                    view.selected == Some(key)
                } else {
                    in_sel
                };
                flat.push(key);
                nav.push(ops::NavNode::Strategy(core, r.id));
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
                        engine,
                        highlighted,
                        is_short: r.is_short,
                        cut,
                        drag_ids: in_sel.then(|| selected_ids.clone()),
                    },
                );
                out.push(MoonTreeItem::new(sid, r.name.clone()));
            }
        }
    }
}
