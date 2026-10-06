//! Auto dock topology policies, insertion, and panel re-homing.

use super::*;

/// Stable first-run order for the shared Auto topology, with pinned Charts leading operations.
pub(super) const AUTO_PANEL_ORDER: &[&str] = &[
    "ChartTabs",
    "Report",
    "Orders",
    "Assets",
    "CoreStatus",
    "Log",
    "Detects",
];

/// Auto-only top tabs whose real activation may become the group's restart preference.
const AUTO_WORKSPACE_TAB_NAMES: &[&str] = &[
    "ChartTabs",
    "Report",
    "Assets",
    "CoreStatus",
    "Log",
    "Detects",
];

/// Stable panel names retained exclusively for the Classic workspace.
const AUTO_CLASSIC_ONLY_PANEL_NAMES: &[&str] = &["News", "Alerts"];

/// Return the complete stable-name policy for panels unavailable in Auto.
///
/// Returns:
///     Classic-only panel names shared by dock extraction, detached exclusion, and tests.
pub(super) fn auto_classic_only_panel_names() -> &'static [&'static str] {
    AUTO_CLASSIC_ONLY_PANEL_NAMES
}

/// Return whether a stable panel name is eligible for Auto top-tab persistence.
///
/// Args:
///     panel_name: Stable dock panel name emitted by MoonUI.
///
/// Returns:
///     `true` for top operational surfaces; Orders and both Classic-only panels are excluded.
pub(super) fn auto_workspace_tab_is_eligible(panel_name: &str) -> bool {
    AUTO_WORKSPACE_TAB_NAMES.contains(&panel_name)
}

/// Resolve a saved Auto tab to an eligible stable name with a deterministic Report fallback.
///
/// Args:
///     saved: Raw persisted name, including possible stale or hand-edited values.
///
/// Returns:
///     The saved eligible name, or `Report` without rewriting the persisted source value.
pub(in crate::shell) fn resolved_auto_workspace_tab(saved: Option<&str>) -> &str {
    saved
        .filter(|name| auto_workspace_tab_is_eligible(name))
        .unwrap_or("Report")
}

/// Return the deterministic fallback after a requested Auto panel was absent from the live dock.
///
/// Args:
///     activated: Whether the requested saved or resolved panel was found and activated.
///
/// Returns:
///     `Report` only after a failed activation; successful activation needs no second request.
pub(super) fn auto_workspace_activation_fallback(activated: bool) -> Option<&'static str> {
    (!activated).then_some("Report")
}

/// Return an eligible activation only when it represents a user-visible Auto transition.
///
/// Args:
///     auto: Whether Auto workspace mode currently owns the dock.
///     applying_topology: Whether programmatic topology effects are still being delivered.
///     panel_name: Stable panel name carried by the activation event.
///
/// Returns:
///     The eligible name to persist, or `None` for Classic, programmatic, or ineligible events.
pub(in crate::shell) fn auto_workspace_tab_to_persist(
    auto: bool,
    applying_topology: bool,
    panel_name: &str,
) -> Option<&str> {
    (auto && !applying_topology && auto_workspace_tab_is_eligible(panel_name)).then_some(panel_name)
}

/// Return whether a dock-layout event may update the shared Auto topology.
///
/// Args:
///     auto: Whether Auto workspace mode currently owns the dock.
///     applying_topology: Whether programmatic topology effects are still being delivered.
///
/// Returns:
///     `true` only for a user-driven Auto layout mutation.
pub(in crate::shell) fn auto_workspace_topology_is_persistable(
    auto: bool,
    applying_topology: bool,
) -> bool {
    auto && !applying_topology
}

/// Return the insertion index that keeps `panel_name` in [`AUTO_PANEL_ORDER`] among `names`.
///
/// Names absent from the first-run order stay where they are; the requested panel is placed
/// before the first present successor, or at the end when none remains.
fn auto_panel_insert_index(names: &[String], panel_name: &str) -> usize {
    let Some(desired) = AUTO_PANEL_ORDER.iter().position(|name| *name == panel_name) else {
        return names.len();
    };
    names
        .iter()
        .position(|existing| {
            AUTO_PANEL_ORDER
                .iter()
                .position(|name| *name == existing.as_str())
                .is_some_and(|position| position > desired)
        })
        .unwrap_or(names.len())
}

/// Insert `panel_name` into the first non-empty Auto strip so a reveal can activate it.
///
/// Split children are walked in order so the upper operational tabs win over the Orders leaf.
/// Empty nodes are skipped so a leftover split slot is not turned into a singleton panel.
fn insert_auto_panel_name(node: &mut DockTopologyNode, panel_name: &str) -> bool {
    match node {
        DockTopologyNode::Empty => false,
        DockTopologyNode::Panel { name } => {
            let existing = name.clone();
            let mut names = vec![existing];
            let at = auto_panel_insert_index(&names, panel_name);
            names.insert(at, panel_name.to_string());
            *node = DockTopologyNode::Tabs { names };
            true
        }
        DockTopologyNode::Tabs { names } => {
            let at = auto_panel_insert_index(names, panel_name);
            names.insert(at, panel_name.to_string());
            true
        }
        DockTopologyNode::Tiles { .. } => false,
        DockTopologyNode::Split { items, .. } => items
            .iter_mut()
            .any(|item| insert_auto_panel_name(item, panel_name)),
    }
}

/// Name `panel_name` in a saved Auto topology that omitted it.
///
/// `apply_topology_by_name` discards unknown names, so a reveal still has to supply a live
/// instance separately. This only makes the requested name present, once, in first-run order.
pub(super) fn ensure_auto_topology_contains_panel(
    topology: &mut DockTopologyByName,
    panel_name: &str,
) {
    if topology.panel_names().iter().any(|name| name == panel_name) {
        return;
    }
    if insert_auto_panel_name(&mut topology.center, panel_name) {
        return;
    }
    let existing = std::mem::replace(&mut topology.center, DockTopologyNode::Empty);
    topology.center = match existing {
        DockTopologyNode::Empty => DockTopologyNode::Panel {
            name: panel_name.to_string(),
        },
        other => DockTopologyNode::Split {
            horizontal: false,
            items: vec![
                other,
                DockTopologyNode::Panel {
                    name: panel_name.to_string(),
                },
            ],
            sizes: vec![None, None],
        },
    };
}

/// Build the first-run Auto topology before a shared `auto_dock.json` exists.
///
/// Returns:
///     A vertical operations layout with flexible upper tabs containing Report and Log, plus a
///     taller fixed Orders surface below them. Charts stays first; activation is applied separately.
pub(super) fn default_auto_workspace_topology() -> DockTopologyByName {
    let primary_names = AUTO_PANEL_ORDER
        .iter()
        .copied()
        .filter(|name| *name != "Orders")
        .map(str::to_string)
        .collect();
    DockTopologyByName {
        center: DockTopologyNode::Split {
            horizontal: false,
            items: vec![
                DockTopologyNode::Tabs {
                    names: primary_names,
                },
                DockTopologyNode::Panel {
                    name: "Orders".to_string(),
                },
            ],
            // The upper slot stays flexible. Orders gains four visible table rows while retaining
            // the same logical-pixel semantics as the previous 260 px first-run preference.
            sizes: vec![None, Some(260.0 + 4.0 * design::TABLE_ROW_H)],
        },
        left: None,
        right: None,
        bottom: None,
    }
    .normalized()
}

/// Strip every occurrence of `panel_name` from a topology node.
///
/// Matching `Panel` leaves become `Empty`; `Tabs` / `Tiles` keep the other names; `Split`
/// children are walked in place. Collapse of emptied branches is left to [`DockTopologyByName::normalized`].
fn remove_panel_name(node: &mut DockTopologyNode, panel_name: &str) {
    match node {
        DockTopologyNode::Empty => {}
        DockTopologyNode::Panel { name } => {
            if name == panel_name {
                *node = DockTopologyNode::Empty;
            }
        }
        DockTopologyNode::Tabs { names } => {
            names.retain(|name| name != panel_name);
        }
        DockTopologyNode::Tiles { names, metas } => {
            let mut ix = 0;
            while ix < names.len() {
                if names[ix] == panel_name {
                    names.remove(ix);
                    if ix < metas.len() {
                        metas.remove(ix);
                    }
                } else {
                    ix += 1;
                }
            }
        }
        DockTopologyNode::Split { items, .. } => {
            for item in items.iter_mut() {
                remove_panel_name(item, panel_name);
            }
        }
    }
}

/// Insert `panel_name` into the Tabs or Panel node that already holds pinned `"ChartTabs"`.
///
/// [`insert_auto_panel_name`] walks a Split in order and returns on the first accepting child,
/// and a stray `Panel` leaf accepts by becoming `Tabs`. The topology being re-homed is by
/// definition not the default, so that first child is often the dragged-out leaf rather than
/// the home strip. `"ChartTabs"` is the pinned leading panel and the reliable marker of the
/// strip the close control must restore onto.
fn insert_auto_panel_into_chart_tabs_strip(node: &mut DockTopologyNode, panel_name: &str) -> bool {
    match node {
        DockTopologyNode::Empty => false,
        DockTopologyNode::Panel { name } if name == "ChartTabs" => {
            let existing = name.clone();
            let mut names = vec![existing];
            let at = auto_panel_insert_index(&names, panel_name);
            names.insert(at, panel_name.to_string());
            *node = DockTopologyNode::Tabs { names };
            true
        }
        DockTopologyNode::Panel { .. } => false,
        DockTopologyNode::Tabs { names } if names.iter().any(|name| name == "ChartTabs") => {
            let at = auto_panel_insert_index(names, panel_name);
            names.insert(at, panel_name.to_string());
            true
        }
        DockTopologyNode::Tabs { .. } => false,
        DockTopologyNode::Tiles { .. } => false,
        DockTopologyNode::Split { items, .. } => items
            .iter_mut()
            .any(|item| insert_auto_panel_into_chart_tabs_strip(item, panel_name)),
    }
}

/// Return `topology` with `panel_name` moved back to its Auto home (the ChartTabs strip, or
/// the Orders slot below it).
///
/// Auto's persisted authority is a name-only tree with no version gate, so rewriting that
/// tree is the only in-app recovery from a dragged-out panel. Pinned Charts and names
/// outside [`AUTO_PANEL_ORDER`] are returned unchanged.
///
/// Args:
///     topology: Shared Auto name-topology to rewrite.
///     panel_name: Stable panel name the user asked to close.
///
/// Returns:
///     A normalized topology with `panel_name` at its first-run home, or the input when the
///     name is not eligible to re-home.
pub(in crate::shell) fn rehome_auto_panel(
    topology: DockTopologyByName,
    panel_name: &str,
) -> DockTopologyByName {
    if panel_name == "ChartTabs" || !AUTO_PANEL_ORDER.contains(&panel_name) {
        return topology;
    }
    let mut next = topology;
    remove_panel_name(&mut next.center, panel_name);
    for side in [&mut next.left, &mut next.right, &mut next.bottom]
        .into_iter()
        .flatten()
    {
        remove_panel_name(&mut side.item, panel_name);
    }
    let mut next = next.normalized();
    if panel_name == "Orders" {
        let orders_size = match default_auto_workspace_topology().center {
            DockTopologyNode::Split { sizes, .. } => sizes.get(1).copied().flatten(),
            _ => None,
        };
        let surviving = std::mem::replace(&mut next.center, DockTopologyNode::Empty);
        next.center = match surviving {
            DockTopologyNode::Split {
                horizontal: false,
                mut items,
                mut sizes,
            } => {
                items.push(DockTopologyNode::Panel {
                    name: "Orders".to_string(),
                });
                sizes.push(orders_size);
                DockTopologyNode::Split {
                    horizontal: false,
                    items,
                    sizes,
                }
            }
            surviving => DockTopologyNode::Split {
                horizontal: false,
                items: vec![
                    surviving,
                    DockTopologyNode::Panel {
                        name: "Orders".to_string(),
                    },
                ],
                sizes: vec![None, orders_size],
            },
        };
    } else if !insert_auto_panel_into_chart_tabs_strip(&mut next.center, panel_name) {
        let mut inserted = false;
        for side in [&mut next.left, &mut next.right, &mut next.bottom]
            .into_iter()
            .flatten()
        {
            if insert_auto_panel_into_chart_tabs_strip(&mut side.item, panel_name) {
                inserted = true;
                break;
            }
        }
        if !inserted {
            // Degenerate: the pinned Charts node is gone, so first-non-empty is the only remaining home.
            ensure_auto_topology_contains_panel(&mut next, panel_name);
        }
    }
    next.normalized()
}

/// Return detached panel names that need temporary Auto-only instances.
///
/// The live Classic dock is the name authority. A stale `detached.json` record for a panel already
/// present there must not contribute a second `Rc`; independent debounce can leave exactly that
/// `docks.json` / `detached.json` disagreement until startup reconciliation finishes.
///
/// Args:
///     group: Group whose detached records are being suspended for Auto.
///     classic_panel_names: Stable names already owned by the live Classic dock.
///     detached: Persisted Classic detached-window specifications.
///
/// Returns:
///     Unique detached names absent from the live dock, in persisted order.
pub(super) fn auto_only_detached_panel_names(
    group: &str,
    classic_panel_names: &[String],
    detached: &[crate::window::detached::DetachedSpec],
) -> Vec<String> {
    let mut accounted = classic_panel_names.iter().cloned().collect::<HashSet<_>>();
    detached
        .iter()
        .filter(|spec| {
            spec.group == group
                && !auto_classic_only_panel_names().contains(&spec.panel.as_str())
                && accounted.insert(spec.panel.clone())
        })
        .map(|spec| spec.panel.clone())
        .collect()
}
