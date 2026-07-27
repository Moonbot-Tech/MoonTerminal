//! Selection and expansion state transitions for the strategy tree.

use super::*;

impl StrategiesView {
    /// Apply a strategy click with selection modifiers.
    /// Shift selects the `order` range from the anchor, Ctrl/Cmd toggles one key, and an
    /// unmodified click replaces the selection.
    pub(super) fn apply_click(
        &mut self,
        key: Key,
        order: &[Key],
        shift: bool,
        command: bool,
    ) -> bool {
        let before_selected = self.selected;
        let before_anchor = self.anchor;
        let before_sel = self.sel.clone();
        if shift {
            if let Some(a) = self.anchor {
                let ia = order.iter().position(|k| *k == a);
                let ib = order.iter().position(|k| *k == key);
                if let (Some(ia), Some(ib)) = (ia, ib) {
                    let (lo, hi) = if ia <= ib { (ia, ib) } else { (ib, ia) };
                    self.sel = order[lo..=hi].iter().copied().collect();
                } else {
                    self.sel = std::iter::once(key).collect();
                }
            } else {
                self.sel = std::iter::once(key).collect();
                self.anchor = Some(key);
            }
        } else if command {
            if !self.sel.remove(&key) {
                self.sel.insert(key);
            }
            self.anchor = Some(key);
        } else {
            self.sel.clear();
            self.sel.insert(key);
            self.anchor = Some(key);
        }
        // The clicked strategy always becomes the primary schema/section source; keep the section.
        self.selected = Some(key);
        // A strategy click clears folder selection so Ctrl+C copies the strategy selection again.
        self.selected_folder = None;
        before_selected != self.selected || before_anchor != self.anchor || before_sel != self.sel
    }

    /// Make `key` the primary selection, replacing whatever was selected.
    ///
    /// Centralizing the assignment preserves the invariant used by `resolve_paste_target`: a
    /// strategy selection always retires the folder selection.
    pub(super) fn focus_strategy(&mut self, key: Key) {
        self.sel.clear();
        self.sel.insert(key);
        self.anchor = Some(key);
        self.selected = Some(key);
        self.selected_folder = None;
    }

    /// Resolve a pending create/paste/copy selection after the core echoes the named strategy.
    ///
    /// Expands the core and complete folder chain, then queues the resolved key for render because
    /// only render owns the tree item index required by `MoonTreeState::scroll_to_item`.
    ///
    /// Returns:
    ///     Whether the pending strategy was found and selected.
    pub(super) fn sync_pending_select(&mut self, cx: &App) -> bool {
        let Some((core, name)) = self.pending_select.clone() else {
            return false;
        };
        let found = {
            let store = self.backend.read(cx).session.store();
            store.core(core).and_then(|cd| {
                cd.strategies
                    .iter()
                    .find(|row| row.name == name)
                    .map(|row| ((core, row.id), row.folder_path.clone()))
            })
        };
        let Some((key, folder_path)) = found else {
            return false;
        };
        self.focus_strategy(key);
        self.expanded_cores.insert(core);
        self.expand_path(core, tree::ops::path_segments(&folder_path));
        // Expansion makes the row eligible for layout; render still needs this key to center the
        // corresponding item because only render owns the tree's item index.
        self.pending_scroll = Some(key);
        self.pending_select = None;
        self.clamp_selected_section(cx);
        true
    }

    /// Clear every filter that could hide a strategy the user explicitly asked to see.
    ///
    /// `set_value` does not emit Change, so the filter is updated explicitly alongside the input.
    fn clear_filters_for_reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.only_active = false;
        self.filter.kind = None;
        self.filter.dir = None;
        if !self.filter.search.trim().is_empty() {
            self.filter.search.clear();
            self.search
                .update(cx, |st, cx| st.set_value(String::new(), window, cx));
        }
    }

    /// Drain a `Backend::strategies_goto` request and reveal its strategy.
    /// Disables the active-only filter, resets other filters when they still hide the target,
    /// expands its core and folders, and selects it. Returns the key so render can scroll after
    /// calling `set_items`.
    ///
    /// A NAME request whose row has not echoed back yet is handed to `pending_select`, which
    /// finishes the job the moment the core reports it — a just-created strategy has no id here
    /// and would otherwise be dropped for not existing yet.
    pub(super) fn drain_goto(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Key> {
        let (core, target) = self.backend.read(cx).strategies_goto.clone()?;
        self.backend.update(cx, |b, _| b.strategies_goto = None);
        let row = {
            let store = self.backend.read(cx).session.store();
            store.core(core).and_then(|cd| {
                cd.strategies
                    .iter()
                    .find(|r| match &target {
                        RevealTarget::Id(id) => r.id == *id,
                        RevealTarget::Name(name) => r.name == *name,
                    })
                    .cloned()
            })
        };
        let Some(row) = row else {
            // Not there yet. Only a NAME request can be waiting on an echo; an id that resolves
            // to nothing is simply stale.
            if let RevealTarget::Name(name) = target {
                // The row is unknown, so `filter.matches` cannot be consulted — clear
                // everything that could hide it rather than reveal it into a filtered-out list.
                self.clear_filters_for_reveal(window, cx);
                self.pending_select = Some((core, name));
            }
            return None;
        };
        self.filter.only_active = false;
        // Reset kind, direction, and search only if they still hide the target.
        if !self.filter.matches(&row) {
            self.clear_filters_for_reveal(window, cx);
        }
        let key: Key = (core, row.id);
        // A direct request supersedes an unresolved name request; retaining both would let the
        // delayed echo steal the selection after this navigation completes.
        self.pending_select = None;
        self.expanded_cores.insert(core);
        self.expand_path(core, tree::ops::path_segments(&row.folder_path));
        self.focus_strategy(key);
        self.clamp_selected_section(cx);
        Some(key)
    }

    pub(super) fn clamp_selected_section(&mut self, cx: &App) -> bool {
        let store = self.backend.read(cx).session.store();
        let Some(sections) = selected_sections(self, store) else {
            return false;
        };
        if self.selected_section < sections.len() {
            return false;
        }
        self.selected_section = 0;
        true
    }

    // ── Actions for starting or stopping checked strategies ─────────────────

    /// Insert every cumulative path prefix into `expanded_folders`.
    /// Shared by Expand All and folder creation so a newly created folder is immediately visible.
    pub(super) fn expand_path<'a>(
        &mut self,
        core: CoreId,
        segments: impl Iterator<Item = &'a str>,
    ) {
        let mut acc = String::new();
        for part in segments {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
            self.expanded_folders.insert((core, acc.clone()));
        }
    }

    /// Expand every node when `collapsed` is true; otherwise collapse all nodes.
    pub(super) fn expand_collapse_toggle(
        &mut self,
        cores: &[(CoreId, String)],
        store: &CoreStore,
        collapsed: bool,
    ) {
        if !collapsed {
            self.expanded_cores.clear();
            self.expanded_folders.clear();
            return;
        }
        for (c, _) in cores {
            self.expanded_cores.insert(*c);
            let Some(cd) = store.core(*c) else { continue };
            let paths: Vec<String> = cd
                .strategies
                .iter()
                .map(|r| r.folder_path.clone())
                .collect();
            for path in paths {
                self.expand_path(*c, tree::ops::path_segments(&path));
            }
        }
    }

    // ── Panel 1: strategy tree ───────────────────────────────────────────────
}
