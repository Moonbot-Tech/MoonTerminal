//! Group-window dock mechanics: detach a panel into its own window; repin it by preferring
//! remembered split placement, then remembered tab placement, with canonical home-strip fallback;
//! reset a closed docked panel to its home tabs; and persist OS-window geometry. Factored out of
//! `shell.rs`; methods called from sibling shell modules are exposed as `pub(super)`.

use gpui::*;

use moon_ui::{DockArea, DockPlacement, DockSplitPlacement, PanelInfo, PanelState};

use moon_core::config::GroupLayout;
use moon_core::config::layout::DockSplitSlot;

use crate::Backend;
use crate::panels::registry::home_ordered_names;
use crate::window::detached;

use super::Shell;

/// Return the final fallback home index for a bottom-row panel.
///
/// The home-strip order is [`home_ordered_names`], derived from `panels::registry`. Remembered
/// split placement and persisted left-neighbor/tab-index placement take priority; if those cannot
/// place the panel, this index is clamped to the current tab count so a partially detached set
/// still preserves the
/// `Orders < Assets < Report < Alerts < News < CoreStatus < Log` relative order.
///
/// The order mirrors the default-layout push order in `shell/init.rs` because both derive from the
/// registry. A saved layout whose `DOCK_VERSION` still matches is restored verbatim. After a
/// version reset the default strip uses this canonical order, while an intentionally remembered
/// detached-panel placement still takes priority when that panel is repinned.
fn dock_home_priority(name: &str) -> usize {
    let order = home_ordered_names();
    order.iter().position(|n| *n == name).unwrap_or(order.len())
}

impl Shell {
    /// Detach the panels queued on `Backend`, through the same path the tab's double-click uses.
    ///
    /// Exists so something holding only a `Backend` can drive a detach — the UI event is otherwise
    /// the only way in. Drained beside the repins, on the same backend observation.
    pub(super) fn drain_panel_detach_requests(&mut self, cx: &mut Context<Self>) {
        let group = self.group.clone();
        let panels: Vec<String> = self.backend.update(cx, |b, _| {
            let mut mine = Vec::new();
            b.panel_detach_request.retain(|(g, p)| {
                if *g == group {
                    mine.push(p.clone());
                    false
                } else {
                    true
                }
            });
            mine
        });
        for panel in panels {
            self.defer_detach_panel(panel, cx);
        }
    }

    /// Restore every detached panel that queued a repin for this Classic group window.
    ///
    /// Auto suspends ordinary detached windows without consuming their specs, so a request already
    /// queued at the mode boundary waits until Classic owns the dock again.
    pub(super) fn drain_repin_requests(&mut self, cx: &mut Context<Self>) {
        if self.applied_workspace_mode == moon_core::config::WorkspaceMode::AutoTrading {
            return;
        }
        let group = self.group.clone();
        let repins: Vec<String> = self.backend.update(cx, |b, _| {
            let mut mine = Vec::new();
            b.repin_request.retain(|(g, p)| {
                if *g == group {
                    mine.push(p.clone());
                    false
                } else {
                    true
                }
            });
            mine
        });
        if repins.is_empty() {
            return;
        }
        let shell = cx.entity().downgrade();
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                for panel_name in repins {
                    let _ = shell.update(app, |this, cx| {
                        this.restore_panel_in_workspace(&panel_name, true, true, window, cx);
                    });
                }
            });
        });
    }

    /// Defer detaching one named panel while Classic owns the dock.
    ///
    /// Auto mode rejects the request before and after deferral so a queued action cannot create a
    /// window or mutate Classic detached persistence after a mode transition.
    pub(super) fn defer_detach_panel(&mut self, panel_name: String, cx: &mut Context<Self>) {
        if !crate::workspace::should_persist_normal_dock(
            self.backend.read(cx).workspace_mode(&self.group),
        ) {
            return;
        }
        let backend = self.backend.clone();
        let dock = self.dock.clone();
        let group = self.group.clone();
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                if !crate::workspace::should_persist_normal_dock(
                    backend.read(app).workspace_mode(&group),
                ) {
                    return;
                }
                // Assets detaches like any other panel: open a per-group window, remove its tab,
                // and repin it when that window closes. Its separate all-cores window opens from
                // the panel toolbar button rather than a double-click.
                if !detached::supports_panel(&panel_name) {
                    return;
                }
                if backend.read(app).is_detached(&group, &panel_name) {
                    return;
                }
                let spec = detached::DetachedSpec::with_saved_geom(
                    &backend,
                    app,
                    group.clone(),
                    panel_name.clone(),
                );
                // Capture placement before removal so restoration can return to the same location.
                // A split leaf records a split slot; a tab records its index. These stores are
                // mutually exclusive, so recording one clears the other; `None` changes neither.
                let key = format!("{group}:{panel_name}");
                if crate::diag::is_enabled() {
                    let outline = dock.update(app, |area, cx| tree_outline(&area.dump(cx).center));
                    dock_log(&format!("[dock] detach {key}: tree={outline}"));
                }
                match captured_slot(&dock, &panel_name, app) {
                    Some(DockSlot::Split {
                        siblings,
                        slot_panels,
                        index,
                        placement,
                        size,
                        sibling_size,
                    }) => {
                        dock_log(&format!(
                            "[dock] detach {key}: split index={index} placement={placement} \
                             siblings={siblings:?} slot_panels={slot_panels:?}"
                        ));
                        backend.update(app, |b, _| {
                            b.layout.dock_split_slot.insert(
                                key.clone(),
                                DockSplitSlot {
                                    siblings,
                                    slot_panels,
                                    index,
                                    placement,
                                    size,
                                    sibling_size,
                                },
                            );
                            b.layout.dock_tab_index.remove(&key);
                            b.layout_dirty = true;
                        });
                    }
                    Some(DockSlot::Tab { ix, left }) => {
                        dock_log(&format!(
                            "[dock] detach {key}: tab ix={ix} left={:?}",
                            left.as_deref().unwrap_or("<leftmost>")
                        ));
                        backend.update(app, |b, _| {
                            b.layout.dock_tab_index.insert(key.clone(), ix);
                            // An empty string means leftmost; otherwise store the left neighbor name.
                            b.layout
                                .dock_tab_left
                                .insert(key.clone(), left.unwrap_or_default());
                            b.layout.dock_split_slot.remove(&key);
                            b.layout_dirty = true;
                        });
                    }
                    None => {
                        dock_log(&format!(
                            "[dock] detach {key}: no slot captured (panel not found in dock)"
                        ));
                    }
                }
                let owner = window.window_handle();
                // `spawn` records the window handle on `Backend` itself, so every detach route
                // gets it and none has to remember.
                if let Err(err) = detached::spawn(app, &backend, &spec, Some(owner)) {
                    log::warn!(
                        "detach panel failed group={} panel={}: {err:#}",
                        group,
                        panel_name
                    );
                    return;
                }
                dock.update(app, |area, cx| {
                    area.remove_panel_by_name(&panel_name, window, cx);
                });
                backend.update(app, |b, _| {
                    b.detached.push(spec);
                    b.detached_dirty = true;
                });
            });
        });
    }

    /// Defer restoring a closed Classic dock panel to its normal home strip.
    pub(super) fn defer_restore_closed_panel(
        &mut self,
        panel_name: String,
        cx: &mut Context<Self>,
    ) {
        if !detached::supports_panel(&panel_name) {
            return;
        }
        let shell = cx.entity().downgrade();
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                let _ = shell.update(app, |this, cx| {
                    // Closing a docked Classic panel resets its placement to the normal home strip.
                    let key = format!("{}:{panel_name}", this.group);
                    this.backend.update(cx, |backend, _| {
                        if backend.layout.dock_split_slot.remove(&key).is_some() {
                            backend.layout_dirty = true;
                        }
                    });
                    this.restore_panel_in_workspace(&panel_name, false, false, window, cx);
                });
            });
        });
    }

    /// Restore one panel into the currently visible Classic topology.
    ///
    /// Args:
    ///     panel_name: Stable dock persistence name to rebuild and insert.
    ///     prefer_split: Whether remembered split placement takes priority over the home strip.
    ///     clear_detached: Whether the successful restore consumes a detached-window spec.
    ///     window: Owning group window required by DockArea mutation APIs.
    ///     cx: Shell context used to update dock and persistence state.
    ///
    /// Returns:
    ///     Nothing; a missing panel factory leaves state unchanged.
    fn restore_panel_in_workspace(
        &mut self,
        panel_name: &str,
        prefer_split: bool,
        clear_detached: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !restore_panel_to_home_tabs(
            &self.dock,
            &self.backend,
            &self.group,
            panel_name,
            prefer_split,
            window,
            cx,
        ) {
            return;
        }

        if clear_detached {
            let group = self.group.clone();
            self.backend.update(cx, |backend, _| {
                backend
                    .detached
                    .retain(|spec| !(spec.group == group && spec.panel == panel_name));
                backend.detached_dirty = true;
            });
        }
    }

    pub(super) fn persist_group_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
        let (bounds, maximized, fullscreen) = match window.window_bounds() {
            WindowBounds::Windowed(bounds) => (Some(bounds), false, false),
            WindowBounds::Maximized(bounds) => (Some(bounds), true, false),
            WindowBounds::Fullscreen(bounds) => (Some(bounds), false, true),
        };
        let Some(bounds) = bounds else {
            return;
        };
        // Identify the window's display by stable UUID. On macOS, x/y are relative to each screen
        // and cannot identify the display during restoration; see `GroupLayout::display_uuid`.
        let display_uuid = window
            .display(cx)
            .and_then(|d| d.uuid().ok())
            .map(|u| u.to_string());
        let layout = GroupLayout {
            x: f32::from(bounds.origin.x) as i32,
            y: f32::from(bounds.origin.y) as i32,
            w: f32::from(bounds.size.width) as u32,
            h: f32::from(bounds.size.height) as u32,
            maximized,
            fullscreen,
            collapsed: false,
            tab: 0,
            dock_h: 220.0,
            orders_primary: 0,
            orders_newest_first: true,
            orders_only_current: false,
            orders_kind: 0,
            display_uuid,
        };
        let group = self.group.clone();
        self.backend.update(cx, |backend, _| {
            let changed = backend
                .layout
                .groups
                .get(&group)
                .map(|old| {
                    old.x != layout.x
                        || old.y != layout.y
                        || old.w != layout.w
                        || old.h != layout.h
                        || old.maximized != layout.maximized
                        || old.fullscreen != layout.fullscreen
                        || old.display_uuid != layout.display_uuid
                })
                .unwrap_or(true);
            if changed {
                backend.layout.groups.insert(group, layout);
                backend.layout_dirty = true;
            }
        });
    }
}

/// Return left-to-right names from the first tab strip containing any [`home_ordered_names`] panel.
///
/// This identifies the home strip for stable restoration relative to the remembered left neighbor.
fn strip_names(node: &PanelState) -> Option<Vec<String>> {
    if let PanelInfo::Tabs { .. } = &node.info {
        let names: Vec<String> = node.children.iter().map(|c| c.panel_name.clone()).collect();
        if names
            .iter()
            .any(|n| home_ordered_names().contains(&n.as_str()))
        {
            return Some(names);
        }
    }
    node.children.iter().find_map(strip_names)
}

/// Append one dock-diagnostic line to `dock_diag.log` (in the working directory) and the app log,
/// but only when render diagnostics are enabled (`MOON_RENDER_DIAG`, same gate as `diag.rs`).
///
/// Gated because the debug binary is built for the `windows` subsystem (no console; `[profile.dev]`
/// disables debug assertions, so `not(debug_assertions)` holds even in debug): `[dock]` events reach
/// neither a redirected stderr nor a terminal — only the in-memory Log tab, which cannot be read
/// while a detached Log window is what is being reproduced. A per-line-flushed file is the only
/// channel that also survives a subsequent hang: the last line written localizes the freeze. Off by
/// default so ordinary detach/restore neither grows a file in the process CWD nor spams the Log tab.
fn dock_log(msg: &str) {
    use std::io::Write;
    if !crate::diag::is_enabled() {
        return;
    }
    log::info!("{msg}");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("dock_diag.log")
    {
        let _ = writeln!(f, "{msg}");
    }
}

/// Render a compact one-line outline of a dock subtree for diagnostics, e.g.
/// `Split(H)[Tabs[Orders,Log,Alerts], Panel(Report), Panel(Assets)]`.
///
/// Restoration bugs are position defects in a tree the static logs do not show, so a single
/// reproduction with the whole tree printed pins the exact node shape (a lone tab strip vs a
/// bare split leaf) that decides which restore path runs.
fn tree_outline(node: &PanelState) -> String {
    match &node.info {
        PanelInfo::Tabs { active_index } => {
            let kids: Vec<&str> = node
                .children
                .iter()
                .map(|c| c.panel_name.as_str())
                .collect();
            format!("Tabs@{active_index}[{}]", kids.join(","))
        }
        PanelInfo::Stack { axis, .. } => {
            let dir = if *axis == 0 { "H" } else { "V" };
            let kids: Vec<String> = node.children.iter().map(tree_outline).collect();
            format!("Split({dir})[{}]", kids.join(", "))
        }
        PanelInfo::Panel(_) if node.panel_name.is_empty() => "Panel(<empty>)".to_string(),
        PanelInfo::Panel(_) => format!("Panel({})", node.panel_name),
        PanelInfo::Tiles { .. } => {
            let kids: Vec<&str> = node
                .children
                .iter()
                .map(|c| c.panel_name.as_str())
                .collect();
            format!("Tiles[{}]", kids.join(","))
        }
    }
}

/// Build and restore one panel to its remembered normal-dock location.
///
/// Args:
///     dock: Live group DockArea currently showing the normal topology.
///     backend: Shared panel factories and placement persistence.
///     group: Owning group name.
///     panel_name: Stable panel persistence name.
///     prefer_split: Whether a remembered split slot takes priority over home tabs.
///     window: Owning group window required by DockArea mutation APIs.
///     app: Application context used to build and insert the panel.
///
/// Returns:
///     `true` after a panel was built and inserted, or `false` when no factory matched.
fn restore_panel_to_home_tabs(
    dock: &Entity<DockArea>,
    backend: &Entity<Backend>,
    group: &str,
    panel_name: &str,
    prefer_split: bool,
    window: &mut Window,
    app: &mut App,
) -> bool {
    let key = format!("{group}:{panel_name}");
    dock_log(&format!(
        "[dock] restore {key}: begin (prefer_split={prefer_split})"
    ));
    let Some(panel) = detached::build_panel(panel_name, group, backend, window, app) else {
        dock_log(&format!("[dock] restore {key}: build_panel returned none"));
        return false;
    };
    // Restoration priority:
    //   1) for `prefer_split`, restore the split slot beside a surviving sibling;
    //   2) restore into the home tab strip relative to the remembered left neighbor, falling back
    //      to the saved tab index;
    //   3) use canonical home priority.
    // Closing a docked panel passes `prefer_split = false` and therefore resets it to the tab strip.
    // Insert helpers clamp indices or locate siblings. A failed split insertion falls through with
    // the same panel; cloned handles go into attempts while the original remains for fallback.
    let split = prefer_split
        .then(|| backend.read(app).layout.dock_split_slot.get(&key).cloned())
        .flatten();
    let tab_ix = backend.read(app).layout.dock_tab_index.get(&key).copied();
    // Left neighbor at detach time: empty means leftmost, while `None` means it was not recorded.
    let tab_left = backend.read(app).layout.dock_tab_left.get(&key).cloned();
    // Dump the live home-strip order in a separate dock update, as `captured_slot` does. `dump`
    // traverses and reads every dock panel; running it inside the insertion update that mutates the
    // tree risks reentrancy or deadlock.
    let strip: Vec<String> = dock.update(app, |area, cx| {
        let state = area.dump(cx);
        strip_names(&state.center)
            .or_else(|| {
                state
                    .bottom_dock
                    .as_ref()
                    .and_then(|d| strip_names(&d.panel))
            })
            .or_else(|| state.left_dock.as_ref().and_then(|d| strip_names(&d.panel)))
            .or_else(|| {
                state
                    .right_dock
                    .as_ref()
                    .and_then(|d| strip_names(&d.panel))
            })
            .unwrap_or_default()
    });
    if crate::diag::is_enabled() {
        let before = dock.update(app, |area, cx| tree_outline(&area.dump(cx).center));
        dock_log(&format!(
            "[dock] restore {key}: strip={strip:?} tab_ix={tab_ix:?} left={:?} tree={before}",
            tab_left.as_deref()
        ));
    }
    dock.update(app, |area, cx| {
        area.remove_panel_by_name(panel_name, window, cx);
        if let Some(slot) = &split {
            let placement = match slot.placement {
                1 => DockSplitPlacement::Right,
                2 => DockSplitPlacement::Top,
                3 => DockSplitPlacement::Bottom,
                _ => DockSplitPlacement::Left,
            };
            // A zero size means flex layout, represented by no fixed slot size.
            let panel_size = (slot.size > 0.0).then_some(slot.size);
            let sibling_size = (slot.sibling_size > 0.0).then_some(slot.sibling_size);
            // Fork work-around for `insert_panel_beside_sibling` Case 1: one sibling slot holding one
            // panel means the panel shared a TWO-slot split with a lone neighbor, which COLLAPSES on
            // detach — the neighbor is absorbed into a same-orientation parent split. Case 1 then
            // re-inserts by the STALE `index` (the panel's position in the vanished inner split),
            // landing it at the wrong end of the parent row (e.g. Report ahead of the Orders strip
            // instead of beside Assets). Passing NO anchors makes the fork skip Case 1 and take Case 2,
            // which wraps the neighbor and places the panel on `placement` — rebuilding the pair in
            // place. See docs-internal/FORK_BUGS.md.
            //
            // Scope is deliberately `slot_panels.len() == 1`, not merely `siblings.len() == 1`: a lone
            // neighbor PANEL makes Case 2's `smallest_subtree_with_all` resolve to exactly that leaf, so
            // the wrap is precise. A multi-panel neighbor slot could have been dragged apart while this
            // panel was detached, and forcing Case 2 there would wrap their smallest common ancestor (up
            // to the whole row) — a wider blast radius than the Case-1 mis-index. Those keep the old
            // path. `siblings.len() == 1` counts NAMED neighbor slots and assumes real panels are named
            // (an unnamed split leaf could undercount), which holds for every panel this app docks.
            let slot_panels: Vec<&str> = slot.slot_panels.iter().map(|s| s.as_str()).collect();
            let two_slot_single_neighbor = slot.siblings.len() == 1 && slot_panels.len() == 1;
            let anchors: Vec<&str> = if two_slot_single_neighbor {
                Vec::new()
            } else {
                slot.siblings.iter().map(|s| s.as_str()).collect()
            };
            // Call with anchors (normal Case-1-first path) or when deliberately forcing the wrap for a
            // single-neighbor collapse. A malformed persisted entry (`siblings` empty via serde default)
            // still falls through to the tab restore below, as before this work-around.
            if (!anchors.is_empty() || two_slot_single_neighbor)
                && area.insert_panel_beside_sibling(
                    panel.clone(),
                    &anchors,
                    &slot_panels,
                    slot.index,
                    placement,
                    panel_size,
                    sibling_size,
                    window,
                    cx,
                )
            {
                dock_log(&format!(
                    "[dock] restore {key}: placed beside sibling (split)"
                ));
                return;
            }
        }
        // Compute the insertion index from the live strip and remembered left neighbor so changes
        // to the strip do not shift the intended location. A missing neighbor falls back to the raw
        // `tab_ix`, then canonical priority; an empty neighbor means index 0.
        let ix = match tab_left.as_deref() {
            Some("") => 0,
            Some(name) => strip
                .iter()
                .position(|n| n == name)
                .map(|p| p + 1)
                .or(tab_ix)
                .unwrap_or_else(|| dock_home_priority(panel_name)),
            None => tab_ix.unwrap_or_else(|| dock_home_priority(panel_name)),
        };
        dock_log(&format!("[dock] restore {key}: -> ix={ix}, inserting"));
        if !area.insert_panel_into_home_tabs(panel.clone(), ix, home_ordered_names(), window, cx) {
            dock_log(&format!(
                "[dock] restore {key}: home-tab insert failed, add_panel(Bottom) fallback"
            ));
            area.add_panel(panel, DockPlacement::Bottom, None, window, cx);
        }
        dock_log(&format!("[dock] restore {key}: inserted"));
    });
    if crate::diag::is_enabled() {
        let after = dock.update(app, |area, cx| tree_outline(&area.dump(cx).center));
        dock_log(&format!("[dock] restore {key}: done tree={after}"));
    }
    true
}

/// Remembered dock placement used when restoring a panel.
enum DockSlot {
    /// The panel was a tab at `ix`, to the right of `left`.
    ///
    /// Restoration prefers the neighbor so it remains stable when the strip shifts. `None` means
    /// there was no named left neighbor.
    Tab { ix: usize, left: Option<String> },
    /// The panel was a standalone leaf in a split.
    ///
    /// `siblings` are anchors for finding the split, `slot_panels` identifies the complete adjacent
    /// slot to wrap after collapse, `index` is the original split position, `placement` is its side,
    /// and `size`/`sibling_size` preserve pixel proportions; zero means flex sizing.
    Split {
        siblings: Vec<String>,
        slot_panels: Vec<String>,
        index: usize,
        placement: u8,
        size: f32,
        sibling_size: f32,
    },
}

/// Find `panel_name` placement by traversing a dock dump.
///
/// A standalone leaf directly in a split becomes [`DockSlot::Split`]; a tab becomes
/// [`DockSlot::Tab`]. All dock zones are searched in order and the first match wins.
fn captured_slot(dock: &Entity<DockArea>, panel_name: &str, app: &mut App) -> Option<DockSlot> {
    /// Return the first leaf-panel name in a subtree for use as a sibling anchor.
    fn first_panel_name(node: &PanelState) -> Option<String> {
        if matches!(node.info, PanelInfo::Panel(_)) && !node.panel_name.is_empty() {
            return Some(node.panel_name.clone());
        }
        node.children.iter().find_map(first_panel_name)
    }
    /// Collect every leaf-panel name in a subtree to identify the complete neighboring slot.
    fn collect_panel_names(node: &PanelState, out: &mut Vec<String>) {
        if matches!(node.info, PanelInfo::Panel(_)) && !node.panel_name.is_empty() {
            out.push(node.panel_name.clone());
        }
        for c in &node.children {
            collect_panel_names(c, out);
        }
    }
    fn walk(node: &PanelState, target: &str) -> Option<DockSlot> {
        match &node.info {
            PanelInfo::Tabs { .. } => {
                if let Some(ix) = node.children.iter().position(|c| c.panel_name == target) {
                    // Remember the named tab at ix - 1 for stable restoration, skipping empty names.
                    let left = ix.checked_sub(1).and_then(|j| {
                        node.children
                            .get(j)
                            .map(|c| c.panel_name.clone())
                            .filter(|n| !n.is_empty())
                    });
                    return Some(DockSlot::Tab { ix, left });
                }
            }
            PanelInfo::Stack { axis, sizes } => {
                // A standalone leaf directly in the split, rather than inside Tabs, is a split slot.
                for (i, child) in node.children.iter().enumerate() {
                    let is_leaf_target =
                        child.panel_name == target && matches!(child.info, PanelInfo::Panel(_));
                    if is_leaf_target {
                        let horizontal = *axis == 0;
                        // Use the adjacent slot as sibling and record the panel's side relative to it.
                        let (sib_ix, target_after) =
                            if i > 0 { (i - 1, true) } else { (i + 1, false) };
                        let placement = match (horizontal, target_after) {
                            (true, true) => 1,   // Right of sibling.
                            (true, false) => 0,  // Left of sibling.
                            (false, true) => 3,  // Below sibling.
                            (false, false) => 2, // Above sibling.
                        };
                        // Every other split child contributes an anchor for finding it on restore.
                        let siblings: Vec<String> = node
                            .children
                            .iter()
                            .enumerate()
                            .filter(|(j, _)| *j != i)
                            .filter_map(|(_, c)| first_panel_name(c))
                            .collect();
                        if siblings.is_empty() {
                            return None;
                        }
                        // Capture the complete adjacent slot, which may itself be a nested split.
                        let mut slot_panels = Vec::new();
                        if let Some(sib) = node.children.get(sib_ix) {
                            collect_panel_names(sib, &mut slot_panels);
                        }
                        // Preserve pixel slot sizes and use zero to represent flex sizing.
                        let size = sizes.get(i).copied().unwrap_or(0.0);
                        let sibling_size = sizes.get(sib_ix).copied().unwrap_or(0.0);
                        return Some(DockSlot::Split {
                            siblings,
                            slot_panels,
                            index: i,
                            placement,
                            size,
                            sibling_size,
                        });
                    }
                }
            }
            _ => {}
        }
        node.children.iter().find_map(|c| walk(c, target))
    }
    let state = dock.update(app, |area, cx| area.dump(cx));
    walk(&state.center, panel_name)
        .or_else(|| {
            state
                .bottom_dock
                .as_ref()
                .and_then(|d| walk(&d.panel, panel_name))
        })
        .or_else(|| {
            state
                .left_dock
                .as_ref()
                .and_then(|d| walk(&d.panel, panel_name))
        })
        .or_else(|| {
            state
                .right_dock
                .as_ref()
                .and_then(|d| walk(&d.panel, panel_name))
        })
}

#[cfg(test)]
mod tests;
