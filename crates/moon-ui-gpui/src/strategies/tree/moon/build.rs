//! MoonTree build helpers.

use super::*;

/// Builds the visible MoonTree forest and row data without exposing store borrows.
///
/// Collapsed cores contribute only their root row and totals; open cores are each scanned once for
/// visible rows and folder counts. The exchange filter excludes whole sections or cores before
/// row-level filtering; the display preference then wraps the retained cores in canonical
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
    cores: &moon_core::session::core_order::OrderedCores,
    venues: &HashMap<CoreId, CoreVenue>,
) -> MoonTreeBuild {
    let filter = view.filter.prepare();
    let searching = filter.searching();
    let mut items = Vec::new();
    let mut data: HashMap<SharedString, NodeData> = HashMap::new();
    let mut expanded: Vec<SharedString> = Vec::new();
    let mut flat: Vec<Key> = Vec::new();
    let mut nav: Vec<ops::NavNode> = Vec::new();

    if view.prefs.group_by_venue {
        let sections = moon_core::session::core_order::exchange_sections(
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
                    (&mut data, &mut flat, &mut nav, &mut expanded),
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
                (&mut data, &mut flat, &mut nav, &mut expanded),
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
        nav,
        searching,
    }
}

/// Build one visible core root with identical contents in grouped and flat tree modes.
/// Folder positions follow the pending full order immediately, before the core echoes it.
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
///     The core root, or `None` when data is absent or row filters leave nothing to display.
pub(super) fn build_core_root(
    view: &StrategiesView,
    store: &CoreStore,
    core: CoreId,
    core_name: &str,
    filter: &PreparedFilter,
    searching: bool,
    outputs: (
        &mut HashMap<SharedString, NodeData>,
        &mut Vec<Key>,
        &mut Vec<ops::NavNode>,
        &mut Vec<SharedString>,
    ),
) -> Option<MoonTreeItem> {
    let (data, flat, nav, expanded) = outputs;
    let cd = store.core(core)?;
    // Some(_) only when the CURRENT connection confirmed it: `store.rs:849-852` clears
    // `strategies_running_confirmed` on a reconnect but RETAINS the last value, so an
    // unconfirmed flag is the previous connection talking. The tree makes no claim on that.
    let engine = cd
        .strategies_running
        .filter(|_| cd.strategies_running_confirmed);
    // Nothing below a collapsed core can render, so it needs only the totals in its own caption.
    // Search and reveal paths force their required core/folder chain open before this build runs.
    // Direct field reads, not `state::core_is_open(...)`: the contract scanner
    // (`the_tree_cache_signature_covers_every_input_the_build_reads`, in
    // `tests/theme_contract/strategies.rs`) walks this function for `view.<field>` reads and
    // requires each one hashed in the tree signature, and an accessor would hide the second field.
    let core_open =
        searching || view.expanded_cores.contains(&core) || view.rail_expanded_core == Some(core);

    // One pass feeds both the visible set and every folder count.
    let mut counts = if core_open {
        FolderCounts::default()
    } else {
        FolderCounts::totals_only()
    };
    let mut matched: Vec<&StrategyRow> = Vec::new();
    let mut row_ranks = HashMap::new();
    let mut any_matched = false;
    for (at, row) in cd.strategies.iter().enumerate() {
        counts.add(row, filter, at);
        if filter.matches(row) {
            any_matched = true;
            if core_open {
                row_ranks.insert(row.id, at);
                matched.push(row);
            }
        }
    }
    // A core with no matching strategy is still worth a row when it holds folders that hold
    // none: on an account whose folders were prepared before its strategies, that is everything
    // there is to show. Asked of the folders that would actually be DRAWN — a core whose every
    // folder is occupied by strategies the filter removed has nothing to show and stays hidden.
    let empty_folders = empty_folder_paths(view, cd, core, searching);
    // Without row filters, even a completely empty core must remain available as a destination
    // for creating or copying its first strategy or folder.
    if filter.narrows() && !any_matched && empty_folders.is_empty() {
        return None;
    }
    // Only for a core whose rows are actually built: `matched` is filled solely when the core is
    // open, so resequencing it for a collapsed one is a whole-list walk nothing reads.
    if let Some(pending) = core_open.then(|| view.pending_order.get(&core)).flatten() {
        // A reorder this core has not echoed yet. Drawn instead of its own sequence because the
        // library keeps the confirmed order until the core answers, and the operator would
        // otherwise watch their own press do nothing for a whole round trip — see `tree::reorder`.
        //
        // Applied to the WHOLE list and filtered afterwards, not to the rows that survived the
        // filter. The rule places a strategy the sent sequence never named directly after the row
        // it follows, and "the row it follows" is a different row once a filter has removed its
        // neighbours — so ordering the filtered list would draw an arrangement the planner, which
        // reads the whole one, does not agree with.
        let mut all: Vec<&StrategyRow> = cd.strategies.iter().collect();
        moon_core::feed::strategy_order::resequence(&mut all, |row| pending.rank(row.id));
        // Folder headings must follow the sent order too, including when their first row is hidden.
        counts = FolderCounts::default();
        row_ranks.clear();
        for (at, row) in all.iter().enumerate() {
            counts.add(row, filter, at);
            row_ranks.insert(row.id, at);
        }
        matched = all.into_iter().filter(|row| filter.matches(row)).collect();
    }
    let (active, total) = counts.root();
    let open_orders_total = cd.orders.iter().filter(|order| !order.job_is_done).count();

    let cid = id_core(core);
    // Before the subtree, because this row is drawn above it. Every push below follows the same
    // rule, so `nav` ends up in exactly the order the operator sees.
    nav.push(ops::NavNode::Core(core));
    let mut children = Vec::new();
    if core_open {
        expanded.push(cid.clone());
        build_core_subtree(
            view,
            cd,
            core,
            filter,
            searching,
            engine,
            &counts,
            &row_ranks,
            &matched,
            &empty_folders,
            &mut children,
            data,
            flat,
            nav,
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
            selected: view.folder_sel.contains(&(core, String::new())),
            // Filter-aware coverage, including when this core is collapsed and `matched` is empty.
            checked: subtree_displayed_all_checked(
                &subtree_check_targets(&cd.strategies, &[], filter),
                &view.staged,
                core,
            ),
            engine,
        },
    );
    Some(
        MoonTreeItem::new(cid, core_name.to_string())
            .folder(true)
            .children(children),
    )
}

/// Folders of one core that hold no strategy, in the order the tree appends them.
///
/// Two sources, and which one a core uses is the core's own answer: one that keeps a folder tree
/// reports its empty folders itself, and the local marks are what a core that cannot keep them —
/// or has not confirmed one yet — leaves the window to draw.
///
/// Answered once per build and used twice: it decides both what to append and whether a core with
/// no matching strategy is worth a row at all.
///
/// Occupancy is derived from the strategies HERE rather than read from [`FolderCounts`], and that
/// is not duplication: a collapsed core's counts are totals-only and hold no folder at all, so
/// asking them would call every folder of every collapsed core empty. The walk costs nothing on
/// the ordinary core, which reports no folders and holds no marks and returns below immediately.
///
/// Args:
///     view: Window holding the local marks.
///     cd: The core's live data, including the tree it reports.
///     core: Core being built.
///     searching: Whether a text query is narrowing the tree.
///
/// Returns:
///     Segment paths of the folders to draw as empty; always empty while searching, since an empty
///     folder matches no query and cannot contain a match.
pub(super) fn empty_folder_paths(
    view: &StrategiesView,
    cd: &moon_core::session::store::CoreData,
    core: CoreId,
    searching: bool,
) -> Vec<Vec<String>> {
    if searching {
        return Vec::new();
    }
    let marks = view.ui_folder_paths(core);
    let reported: Vec<Vec<String>> = match cd.folders.supported {
        false => Vec::new(),
        true => cd
            .folders
            .paths
            .iter()
            // Paths MoonProto itself would refuse are skipped, and that is not a formality: for a
            // strategy in MoonBot's `"EMA / ORGANIC"` — one folder there — moonproto's own state
            // adds the split halves as parent folders, so the tree reports `"EMA "`. Drawn, that is
            // a folder which exists on no core and which no edit could ever name.
            .filter(|path| moon_core::feed::folder_tree::sendable([path.as_str()].into_iter()))
            .map(|path| strategy_path::split_path(path))
            .collect(),
    };
    if marks.is_empty() && reported.is_empty() {
        return Vec::new();
    }

    // Every folder the strategies occupy, folded as the core folds them when deciding whether two
    // spellings are one folder.
    let mut occupied: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in &cd.strategies {
        let mut key = String::new();
        for segment in strategy_path::path_segments(&row.folder_path) {
            if !key.is_empty() {
                key.push('/');
            }
            key.push_str(segment);
            occupied.insert(key.to_lowercase());
        }
    }

    let mut empty: Vec<Vec<String>> = marks
        .into_iter()
        .chain(reported)
        .filter(|parts| !parts.is_empty() && !occupied.contains(&parts.join("/").to_lowercase()))
        .collect();
    // One order over both sources: the core lists its folders in an order the protocol calls
    // meaningless, and the local marks come out of a set, so without this two frames drawing
    // identical data would place the same folders differently. Folded first so that a mark and a
    // reported path differing only in case land together — and are then deduped as the one folder
    // they are.
    empty.sort_by_cached_key(|parts| {
        let joined = parts.join("/");
        (joined.to_lowercase(), joined)
    });
    empty.dedup_by(|a, b| a.join("/").to_lowercase() == b.join("/").to_lowercase());
    empty
}

/// Build the folder and strategy rows of one open core, followed by its Deleted folder.
///
/// `row_ranks` keeps folders and loose strategies in the same displayed order while a pending
/// reorder overlays the core-confirmed sequence.
///
/// Args:
///     view: Strategies state that owns selections, folders, and deleted rows.
///     cd: Live snapshot for the open core.
///     core: Core whose rows are being built.
///     filter: Prepared predicate shared by the complete tree build.
///     searching: Whether a text query forces child folders open.
///     engine: Confirmed global-engine state copied onto folder headings.
///     counts: Filter-aware totals and folder ranks for the displayed rows.
///     row_ranks: Display rank for each strategy, including a pending reorder overlay.
///     matched: Visible live strategies in their displayed order.
///     empty_folders: Empty folders retained by the UI or confirmed by the core.
///     children: Output rows for the core subtree.
///     data: Output metadata keyed by tree id.
///     flat: Output strategy ids in visual order.
///     nav: Output node order for keyboard navigation.
///     expanded: Output ids that MoonTree should draw open.
///
/// Returns:
///     Nothing; all rendered tree data is appended to the supplied outputs.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_core_subtree(
    view: &StrategiesView,
    cd: &moon_core::session::store::CoreData,
    core: CoreId,
    filter: &PreparedFilter,
    searching: bool,
    engine: Option<bool>,
    counts: &FolderCounts,
    row_ranks: &HashMap<u64, usize>,
    matched: &[&StrategyRow],
    empty_folders: &[Vec<String>],
    children: &mut Vec<MoonTreeItem>,
    data: &mut HashMap<SharedString, NodeData>,
    flat: &mut Vec<Key>,
    nav: &mut Vec<ops::NavNode>,
    expanded: &mut Vec<SharedString>,
) {
    let mut order_counts: HashMap<u64, usize> = HashMap::new();
    for o in cd.orders.iter().filter(|o| !o.job_is_done) {
        *order_counts.entry(o.strat_id).or_insert(0) += 1;
    }
    // Every selected row in this core shares the same drag payload.
    let selected_ids: Rc<[u64]> = view.drag_ids_for_core(core);
    // Borrowed once per core so the per-folder probes below need no owned key.
    let folders = FolderSets {
        open: core_paths(&view.expanded_folders, core),
    };

    // Build the folder tree from visible strategies plus empty UI-only folders. `matched` holds
    // the core's own strategy order, which is what places the folders that hold strategies.
    let mut root = build_node(matched.iter().copied());
    for parts in empty_folders {
        ensure_folder(&mut root, parts);
    }

    // Folders the core keeps in its own tree, folded for the case-insensitive compare it uses. An
    // empty folder outside that set exists only in this window, which is what its row says. Asked
    // of `supported` and NOT of `editable`: a core whose tree cannot be edited still KEEPS the
    // folders it reports, and calling one of those local would tell the operator it disappears on
    // restart when it does not.
    let confirmed_folders: std::collections::HashSet<String> = match cd.folders.supported {
        false => std::collections::HashSet::new(),
        true => cd
            .folders
            .paths
            .iter()
            .map(|path| strategy_path::join_path(&strategy_path::split_path(path)).to_lowercase())
            .collect(),
    };
    let mut prefix: Vec<String> = Vec::new();
    convert_node(
        &root,
        core,
        counts,
        row_ranks,
        &confirmed_folders,
        &order_counts,
        &selected_ids,
        &folders,
        &mut prefix,
        view,
        &cd.strategies,
        filter,
        searching,
        engine,
        children,
        data,
        flat,
        nav,
        expanded,
    );

    // Append the Deleted folder after live strategies and filter its rows by name during search.
    let del: Vec<&moon_core::strat_db::stats::HeadRow> = view
        .deleted
        .get(&core)
        .map(|v| v.iter().filter(|h| filter.name_matches(&h.name)).collect())
        .unwrap_or_default();
    if !del.is_empty() {
        let did = id_del_folder(core);
        // The heading is drawn whenever the core has deleted rows; its children only when it is
        // open. `nav` follows the FOREST, not the data map, or the keyboard would step into rows
        // the tree is not showing.
        let deleted_open = searching || view.expanded_deleted.contains(&core);
        if deleted_open {
            expanded.push(did.clone());
        }
        nav.push(ops::NavNode::DeletedFolder(core));
        let mut dchildren = Vec::new();
        for h in &del {
            let sid_u = h.strategy_id as u64;
            let key: Key = (core, sid_u);
            if deleted_open {
                nav.push(ops::NavNode::DeletedStrategy(core, sid_u));
            }
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
